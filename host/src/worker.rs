// FreeForGemPear — see LICENSE
//! One worker thread per GPU (the libpearlgpu state is per thread) plus one proof thread per GPU.
//! The coordinator in main.rs publishes an `Assignment` (job + target + which login it belongs to); every worker
//! picks it up, loads the job's B side into its GPU and mines it with the two-slot pipeline.
use crate::mining::*;
use crate::stratum::{PoolJob, Src, Stats, Submit};
use crate::{gpu, hex, hit_hash, prove_hit};
use anyhow::Result;
use base64::Engine;
use primitive_types::U256;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::collections::BTreeMap;
use std::sync::mpsc::{self, Sender, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::Instant;

const MAX_HITS: usize = 64;
/// Max queued attempts with hits per GPU; each one pins its Job (~100 MB for an old job), so keep it small.
const WORK_QUEUE: usize = 8;

/// What the GPUs mine right now.  `jobs` holds one prepared Job per distinct device `m` (SM group): a
/// homogeneous rig has one entry, a mixed-SM rig one per SM count.  Each worker picks the Job for its own m.
pub struct Assignment {
    pub src: Src,
    pub pj: PoolJob,
    pub jobs: Arc<BTreeMap<usize, Arc<Job>>>,
    pub target: U256,
}

/// State shared by the coordinator and all workers.
pub struct Shared {
    pub assign: Mutex<Option<Arc<Assignment>>>,
    /// Bumped whenever `assign` changes.
    pub gen_: AtomicU64,
    pub stop: Arc<AtomicBool>,
    pub next_id: AtomicU64,
    pub hits_seen: AtomicU64,
    pub dropped0: AtomicU64,
    pub bad: AtomicU64,
    pub queue_dropped: AtomicU64,
    pub stats: [Arc<Stats>; 2],
}

impl Shared {
    pub fn new(stop: Arc<AtomicBool>, stats: [Arc<Stats>; 2]) -> Shared {
        Shared {
            assign: Mutex::new(None),
            gen_: AtomicU64::new(0),
            stop,
            next_id: AtomicU64::new(100),
            hits_seen: AtomicU64::new(0),
            dropped0: AtomicU64::new(0),
            bad: AtomicU64::new(0),
            queue_dropped: AtomicU64::new(0),
            stats,
        }
    }

    pub fn publish(&self, a: Option<Arc<Assignment>>) {
        *self.assign.lock().unwrap() = a;
        self.gen_.fetch_add(1, Ordering::Release);
    }
}

/// Per-GPU counters read by the coordinator's log line.
pub struct GpuStat {
    pub dev: i32,
    pub name: String,
    pub attempts: AtomicU64,
    pub alive: AtomicBool,
    /// Undervolt tuner (uv.rs): asks the worker to stop feeding the GPU for a while / acknowledges it / counts hits that failed CPU verification.
    pub pause: AtomicBool,
    pub paused: AtomicBool,
    pub uv_bad: AtomicU64,
    /// Why the worker stopped (for the fault attribution in main.rs, faults.rs).
    pub err: Mutex<Option<String>>,
}

/// An attempt in flight on the GPU, with everything needed to turn its hits into shares.
struct Pending {
    slot: i32,
    src: Src,
    pj: PoolJob,
    job: Arc<Job>,
    row0: Vec<u8>,
    a_seed: [u8; 32],
    target: U256,
    counter: u64,
}

/// Hits of one attempt plus everything the prover needs (the job is held by Arc, so a hit of an older job
/// is still proved with that job's data and submitted under its own job_id and login).
struct Work {
    src: Src,
    pj: PoolJob,
    job: Arc<Job>,
    row0: Vec<u8>,
    a_seed: [u8; 32],
    target: U256,
    counter: u64,
    hits: Vec<gpu::Hit>,
}

pub fn src_name(s: Src) -> &'static str {
    match s {
        Src::User => "user",
        Src::Fee => "fee",
    }
}

/// Starts the worker thread of GPU `dev`. `ready` receives the device name once the GPU is initialised.
pub fn spawn(
    dev: i32,
    m_dev: usize,
    sh: Arc<Shared>,
    gs: Arc<GpuStat>,
    sinks: [Sender<Submit>; 2],
    ready: Sender<Result<()>>,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new().name(format!("gpu{dev}")).spawn(move || {
        if let Err(e) = gpu::init(dev) {
            let _ = ready.send(Err(e));
            return;
        }
        crate::mining::warn_m_align(dev, m_dev, gpu::row_align());
        crate::wintune::feed_thread();
        let _ = ready.send(Ok(()));
        let (work_tx, work_rx) = mpsc::sync_channel::<Work>(WORK_QUEUE);
        let prover = {
            let sh = sh.clone();
            let gs = gs.clone();
            std::thread::Builder::new().name(format!("prover{dev}")).spawn(move || prover_loop(work_rx, sh, sinks, gs)).expect("spawn prover")
        };
        if let Err(e) = run(dev, m_dev, &sh, &gs, &work_tx) {
            eprintln!("!! GPU {dev} worker stopped: {e:#}");
            *gs.err.lock().unwrap() = Some(format!("{e:#}"));
        }
        gs.alive.store(false, Ordering::Relaxed);
        drop(work_tx); // let the prover drain its queue, then wait for it
        let _ = prover.join();
    })
}

/// Seconds between the first launches of the mining GPUs (FFG_START_STAGGER_MS overrides; 0 = all at once).
fn stagger_ms() -> u64 {
    std::env::var("FFG_START_STAGGER_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(1500)
}

/// Before a GPU's first launch (1.7.3): the GPUs start mining one by one, STAGGER apart, so 4-8 cards do not jump to full
/// power in the same instant (Rig05: 4x CMP 50HX lost power right at the miner start).  Pearl and NOID workers call it.
pub fn stagger_start(dev: i32, stop: &std::sync::atomic::AtomicBool) {
    static ORDER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let k = ORDER.fetch_add(1, Ordering::SeqCst);
    let d = std::time::Duration::from_millis(k * stagger_ms());
    if d.is_zero() {
        return;
    }
    println!("GPU {dev}: starts mining in {:.1} s (the GPUs start one by one: no power surge of all cards at once)", d.as_secs_f64());
    let t = Instant::now();
    while t.elapsed() < d && !stop.load(Ordering::Relaxed) {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

fn run(dev: i32, m_dev: usize, sh: &Shared, gs: &GpuStat, work_tx: &SyncSender<Work>) -> Result<()> {
    let mut staggered = false;
    let mut my_gen = 0u64;
    let mut cur: Option<Arc<Assignment>> = None;
    let mut loaded: Option<Arc<Job>> = None; // the Job (for this device's m) whose B side is in this GPU's memory
    // two-slot pipeline: attempt i+1 is prepared and enqueued before waiting for attempt i
    let mut pending: Option<Pending> = None;
    let mut slot = 0i32;
    let mut counter = (gs.dev as u64 + 1) << 40; // disjoint nonce ranges per GPU; never reset
    let mut hit_buf = vec![gpu::Hit::default(); MAX_HITS];
    let mut fd = FeedDiag::new(dev);
    // test hook (faults.rs): a fault of this GPU after N s of mining (FFG_TEST_FAULT_ALL=1: of every GPU, as a real CUDA fault)
    let inject = crate::faults::test_fault_for(dev).map(|secs| Instant::now() + std::time::Duration::from_secs(secs));
    while !sh.stop.load(Ordering::Relaxed) {
        if inject.is_some_and(|t| Instant::now() >= t && gs.attempts.load(Ordering::Relaxed) > 0) {
            anyhow::bail!("pg_wait: injected fault (FFG_TEST_FAULT): an illegal memory access was encountered");
        }
        if gs.pause.load(Ordering::Acquire) {
            // the undervolt tuner wants the GPU for a check (its own process): finish what is in flight and wait
            if let Some(p) = pending.take() {
                finish(p, &mut hit_buf, sh, gs, work_tx)?;
            }
            gs.paused.store(true, Ordering::Release);
            while gs.pause.load(Ordering::Acquire) && !sh.stop.load(Ordering::Relaxed) {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            gs.paused.store(false, Ordering::Release);
            continue;
        }
        let g = sh.gen_.load(Ordering::Acquire);
        if g != my_gen {
            my_gen = g;
            let a = sh.assign.lock().unwrap().clone();
            // the Job this device mines = the one built for its own m (SM group)
            let newjob = a.as_ref().map(|n| n.jobs.get(&m_dev).expect("no prepared Job for this device's m").clone());
            let job_changed = match (&newjob, &loaded) {
                (Some(n), Some(l)) => !Arc::ptr_eq(n, l),
                (Some(_), None) => true,
                _ => false,
            };
            if a.is_none() || job_changed {
                // the attempt in flight must finish before the GPU gets the new B side
                // (pg_set_job overwrites the B' buffer the kernels read)
                if let Some(p) = pending.take() {
                    finish(p, &mut hit_buf, sh, gs, work_tx)?;
                }
            }
            if let (true, Some(n), Some(nj)) = (job_changed, &a, &newjob) {
                let t = Instant::now();
                gpu::set_job(&nj.br_t, &nj.q1, &nj.q2, N, k())?;
                loaded = Some(nj.clone());
                println!("GPU {dev}: [{}] job {} (cert {}) m={m_dev} loaded in {:?}", src_name(n.src), n.pj.job_id, n.pj.cert_version, t.elapsed());
            }
            cur = a;
        }
        let Some(c) = &cur else {
            std::thread::sleep(std::time::Duration::from_millis(20));
            continue;
        };
        if !staggered {
            staggered = true;
            stagger_start(dev, &sh.stop);
            continue; // the job may have changed meanwhile
        }
        let job = loaded.as_ref().expect("job loaded whenever an assignment is current");
        // prepare and enqueue the next attempt, then collect the previous one
        let t_prep = Instant::now();
        let row0 = job.nonce_row(counter);
        let a_seed = job.a_noise_seed(&row0);
        // the kernel derives P_A and A_L from a_seed itself: no host-side permutation (a_perm = 512 hashes per attempt)
        let t_launch = Instant::now();
        gpu::launch(slot, &a_seed, &[], &[], m_dev, &target_words(c.target))?;
        fd.launched(t_prep, t_launch);
        let next = Pending { slot, src: c.src, pj: c.pj.clone(), job: job.clone(), row0, a_seed, target: c.target, counter };
        counter += 1;
        slot ^= 1;
        if let Some(p) = pending.replace(next) {
            let t_wait = Instant::now();
            finish(p, &mut hit_buf, sh, gs, work_tx)?;
            fd.finished(t_wait);
        }
    }
    if let Some(p) = pending.take() {
        finish(p, &mut hit_buf, sh, gs, work_tx)?;
    }
    Ok(())
}

/// FFGP_FEED_DIAG=1: per 10 s window, how the GPU-feeding thread spends its time (prep = nonce row + A seed,
/// launch = enqueue, wait = collecting the previous attempt incl. its hits) and the largest gap between two launches.
/// A launch gap longer than one attempt's GPU time means the two-slot queue ran dry (GPU idle).
struct FeedDiag {
    on: bool,
    dev: i32,
    t0: Instant,
    last_launch: Option<Instant>,
    n: u32,
    prep: f64,
    launch: f64,
    wait: f64,
    gap_max: f64,
    gap_sum: f64,
    quick_waits: u32,
}

impl FeedDiag {
    fn new(dev: i32) -> FeedDiag {
        let on = std::env::var("FFGP_FEED_DIAG").map(|v| v != "0").unwrap_or(false);
        FeedDiag { on, dev, t0: Instant::now(), last_launch: None, n: 0, prep: 0.0, launch: 0.0, wait: 0.0, gap_max: 0.0, gap_sum: 0.0, quick_waits: 0 }
    }
    fn launched(&mut self, t_prep: Instant, t_launch: Instant) {
        if !self.on {
            return;
        }
        let now = Instant::now();
        self.prep += (t_launch - t_prep).as_secs_f64();
        self.launch += (now - t_launch).as_secs_f64();
        if let Some(l) = self.last_launch {
            let g = (t_launch - l).as_secs_f64();
            self.gap_sum += g;
            self.gap_max = self.gap_max.max(g);
        }
        self.last_launch = Some(t_launch);
        self.n += 1;
    }
    fn finished(&mut self, t_wait: Instant) {
        if !self.on {
            return;
        }
        let w = t_wait.elapsed().as_secs_f64();
        self.wait += w;
        if w < 0.0003 {
            self.quick_waits += 1;
        }
        if self.t0.elapsed().as_secs_f64() >= 10.0 && self.n > 1 {
            let n = self.n as f64;
            println!(
                "[feed {}] n {} | prep {:.3} launch {:.3} wait {:.3} ms/att | launch gap avg {:.3} max {:.3} ms | waits<0.3ms {}",
                self.dev, self.n, 1e3 * self.prep / n, 1e3 * self.launch / n, 1e3 * self.wait / n, 1e3 * self.gap_sum / (n - 1.0), 1e3 * self.gap_max, self.quick_waits
            );
            *self = FeedDiag { last_launch: self.last_launch, ..FeedDiag::new(self.dev) };
        }
    }
}

/// Waits for an attempt and queues its hits for the proof thread (never blocks on proving).
fn finish(p: Pending, buf: &mut [gpu::Hit], sh: &Shared, gs: &GpuStat, work_tx: &SyncSender<Work>) -> Result<()> {
    let (total, stored) = gpu::wait(p.slot, buf)?;
    gs.attempts.fetch_add(1, Ordering::Relaxed);
    sh.hits_seen.fetch_add(total as u64, Ordering::Relaxed);
    let mut hits = Vec::new();
    for h in &buf[..stored] {
        if h.row == 0 {
            sh.dropped0.fetch_add(1, Ordering::Relaxed); // tile holds the nonce row, computed as 64 on the GPU
        } else {
            hits.push(*h);
        }
    }
    if !hits.is_empty() {
        let w = Work { src: p.src, pj: p.pj, job: p.job, row0: p.row0, a_seed: p.a_seed, target: p.target, counter: p.counter, hits };
        match work_tx.try_send(w) {
            Ok(()) => {}
            Err(mpsc::TrySendError::Full(w)) => {
                sh.queue_dropped.fetch_add(1, Ordering::Relaxed);
                eprintln!("!! proof queue full ({WORK_QUEUE}): dropping hit(s) of job {} counter {}", w.pj.job_id, w.counter);
            }
            Err(mpsc::TrySendError::Disconnected(_)) => anyhow::bail!("proof worker died"),
        }
    }
    Ok(())
}

/// Proof thread: builds, CPU-verifies and hands over at most one share per queued attempt.
/// Runs off the GPU-feeding thread, so proof building (~160 ms) never stalls the GPU queue.
fn prover_loop(rx: mpsc::Receiver<Work>, sh: Arc<Shared>, sinks: [Sender<Submit>; 2], gs: Arc<GpuStat>) {
    while let Ok(p) = rx.recv() {
        for h in &p.hits {
            let id = sh.next_id.fetch_add(1, Ordering::Relaxed);
            match submit_hit(&p, h, id) {
                Ok(line) => {
                    sh.stats[p.src.idx()].submitted.fetch_add(1, Ordering::Relaxed);
                    let _ = sinks[p.src.idx()].send(Submit { id, job_id: p.pj.job_id.clone(), line });
                    break; // one share per attempt
                }
                Err(e) => {
                    sh.bad.fetch_add(1, Ordering::Relaxed);
                    gs.uv_bad.fetch_add(1, Ordering::Relaxed);
                    eprintln!("!! hit not submitted (job {}, counter {}): {e:#}", p.pj.job_id, p.counter);
                }
            }
        }
    }
}

/// Proves and verifies one hit and builds its mining.submit line.
fn submit_hit(p: &Work, h: &gpu::Hit, id: u64) -> Result<String> {
    let t_proof = Instant::now();
    let proof = prove_hit(&p.job, &p.row0, h, &p.a_seed, p.target)?;
    let proof_ms = t_proof.elapsed().as_millis();
    let raw = bincode::serialize(&proof)?;
    let jackpot = hex(&hit_hash(h));
    let (payload, enc) = if p.pj.gzip {
        let mut z = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(&raw)?;
        (z.finish()?, true)
    } else {
        (raw, false)
    };
    let mut params = serde_json::json!({
        "job_id": p.pj.job_id,
        "plain_proof": base64::engine::general_purpose::STANDARD.encode(&payload),
        "jackpot": jackpot,
        "adjustment_factor": adjustment_factor(),
    });
    if enc {
        params["proof_encoding"] = "gzip".into();
    }
    let line = serde_json::json!({"jsonrpc":"2.0","id":id,"method":"mining.submit","params":params}).to_string();
    println!(
        "SUBMIT [{}] id {id} job {} tile=({}, {}) jackpot={jackpot} proof {} B{} (proved in {proof_ms} ms)",
        src_name(p.src),
        p.pj.job_id,
        h.row,
        h.col,
        payload.len(),
        if enc { " gzip" } else { "" }
    );
    Ok(line)
}
