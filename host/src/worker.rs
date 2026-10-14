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
        let _ = ready.send(Ok(()));
        let (work_tx, work_rx) = mpsc::sync_channel::<Work>(WORK_QUEUE);
        let prover = {
            let sh = sh.clone();
            let gs = gs.clone();
            std::thread::Builder::new().name(format!("prover{dev}")).spawn(move || prover_loop(work_rx, sh, sinks, gs)).expect("spawn prover")
        };
        if let Err(e) = run(dev, m_dev, &sh, &gs, &work_tx) {
            eprintln!("!! GPU {dev} worker stopped: {e:#}");
        }
        gs.alive.store(false, Ordering::Relaxed);
        drop(work_tx); // let the prover drain its queue, then wait for it
        let _ = prover.join();
    })
}

fn run(dev: i32, m_dev: usize, sh: &Shared, gs: &GpuStat, work_tx: &SyncSender<Work>) -> Result<()> {
    let mut my_gen = 0u64;
    let mut cur: Option<Arc<Assignment>> = None;
    let mut loaded: Option<Arc<Job>> = None; // the Job (for this device's m) whose B side is in this GPU's memory
    // two-slot pipeline: attempt i+1 is prepared and enqueued before waiting for attempt i
    let mut pending: Option<Pending> = None;
    let mut slot = 0i32;
    let mut counter = (gs.dev as u64 + 1) << 40; // disjoint nonce ranges per GPU; never reset
    let mut hit_buf = vec![gpu::Hit::default(); MAX_HITS];
    while !sh.stop.load(Ordering::Relaxed) {
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
        let job = loaded.as_ref().expect("job loaded whenever an assignment is current");
        // prepare and enqueue the next attempt, then collect the previous one
        let row0 = job.nonce_row(counter);
        let a_seed = job.a_noise_seed(&row0);
        // the kernel derives P_A and A_L from a_seed itself: no host-side permutation (a_perm = 512 hashes per attempt)
        gpu::launch(slot, &a_seed, &[], &[], m_dev, &target_words(c.target))?;
        let next = Pending { slot, src: c.src, pj: c.pj.clone(), job: job.clone(), row0, a_seed, target: c.target, counter };
        counter += 1;
        slot ^= 1;
        if let Some(p) = pending.replace(next) {
            finish(p, &mut hit_buf, sh, gs, work_tx)?;
        }
    }
    if let Some(p) = pending.take() {
        finish(p, &mut hit_buf, sh, gs, work_tx)?;
    }
    Ok(())
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
    let proof = prove_hit(&p.job, &p.row0, h, &p.a_seed, p.target)?;
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
        "SUBMIT [{}] id {id} job {} tile=({}, {}) jackpot={jackpot} proof {} B{}",
        src_name(p.src),
        p.pj.job_id,
        h.row,
        h.col,
        payload.len(),
        if enc { " gzip" } else { "" }
    );
    Ok(line)
}
