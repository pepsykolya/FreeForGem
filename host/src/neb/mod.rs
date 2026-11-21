// FFGN — see LICENSE
//! NEB (Neural Engine Byte) mining, `--algo neb` (the default and only product coin of FFGN).  Proof of work nebhash v2:
//! one attempt = one integer training step of the chain's model on 16 samples of a pinned dataset (spec/shares.md,
//! spec/nebhash-v2.md); ticket = BLAKE3(digest || key), compared as a 256-bit little-endian integer.
//!
//! Work sources (one of):
//!   * `--node 127.0.0.1:3441[,backup]`  solo, the node's work service (node.rs); the block pays the node's `--miner-lock`;
//!   * `--pool HOST:PORT[,backup]`       a neb-pool/1 pool (pool.rs) or an `ffgn --gate` (gate.rs, `--password TOKEN`).
//! Devices: the closed-source FFGN engine `nebgpu` (gpu.rs) or the CPU reference (`--backend cpu`).  EVERY nonce a device
//! reports is evaluated again with the C reference (refc.rs) before it is sent; models are checked on upload and one batch
//! in 64 is audited (exact device tickets of explicit keys vs the reference).
//! Developer fee 1 % (fee.rs): 1.0.3: always a fee session of its own (pool mode: the user's pool list, a second connection;
//! never a second login in the user's session: Suprnova credited the user's shares to the fee login); solo = a fee session to
//! the FFGN pool during the slot.
pub mod cli;
pub mod fee;
pub mod gate;
pub mod gpu;
pub mod iso;
pub mod node;
pub mod pool;
pub mod proto;
pub mod refc;
pub mod supr;
pub mod tune;
pub mod uvmine;

use crate::clocks;
use crate::stratum::{Src, Stats};
use anyhow::{Result, bail, ensure};
use proto::{admits, hex};
use refc::{DatasetFile, Model};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const ALGO_NAME: &str = "nebhash-v2";
/// 1.0.4: the averaging window of the rate (stats line, API, HiveOS): smooths the change of the block's dataset (slot)
const AVG_WINDOW: std::time::Duration = std::time::Duration::from_secs(300);

/// Where a job came from (what a find needs to be sent back).
#[derive(Clone, Debug)]
pub enum Origin {
    Node { id: u64 },
    Pool { job_id: String },
}

/// A job of a source, normalized.
pub struct NJob {
    pub src: Src,
    pub origin: Origin,
    pub job: proto::Job,
    pub slot: u8,
    pub model: Arc<Model>,
    /// pool target of the session (pool mode); None = report the share target (solo)
    pub pool_target: Option<[u8; 32]>,
    pub en1: Vec<u8>,
    pub en2_size: usize,
    /// gate: this session's nonce range (base, span)
    pub nonce_range: Option<(u32, u64)>,
    /// connection number of the source (a find of an older connection is dropped)
    pub conn: u64,
    pub received: Instant,
    /// the reference header (all-zero extranonce): its nonces are tried once per job, also across pauses, fee slots and
    /// re-published work (F-09); a random start so that two rigs of one node do not try the same nonces first (F-13)
    pub ref_next: AtomicU64,
    pub ref_base: u32,
}

impl NJob {
    /// a job's lifetime from its arrival (F-03: not the rig's clock against `expiry`): expiry - time_start, 60 s .. 2 h
    pub fn expired(&self) -> bool {
        let life = self.job.expiry.saturating_sub(self.job.time_start).clamp(60, 7200);
        self.received.elapsed() > Duration::from_secs(life as u64)
    }
}

static FATAL: std::sync::OnceLock<String> = std::sync::OnceLock::new();
/// 1.0.10: a GPU hung in an auto-uv step (tune.rs): the main loop restarts the miner, as when every worker stopped
static RESTART: std::sync::OnceLock<String> = std::sync::OnceLock::new();

pub fn request_restart(why: &str) {
    let _ = RESTART.set(why.to_string());
}

/// A source found the run cannot go on (e.g. the only pool is on another network): the miner stops with this message.
pub fn fatal(msg: &str) {
    let _ = FATAL.set(msg.to_string());
}

/// A random 32-bit start for the reference header's nonces.
pub fn random_base() -> u32 {
    rand::random::<u32>()
}

pub enum Ev {
    Job(Arc<NJob>),
    Down,
}

/// A checked nonce on its way to its source.
pub struct Find {
    pub job: Arc<NJob>,
    pub extranonce: Vec<u8>,
    pub time: u32,
    pub nonce: u32,
    pub ticket: [u8; 32],
    /// ticket <= share target (node: Share frame)
    pub share: bool,
    /// ticket <= block target
    pub block: bool,
    /// mined for the dev-fee login
    pub fee: bool,
}

/// What the devices mine now: one header (job + extranonce + time) and its nonce range.
pub struct Work {
    pub nj: Arc<NJob>,
    pub fee: bool,
    pub extranonce: Vec<u8>,
    pub time: u32,
    pub header: [u8; 80],
    /// report bound: the pool target (pool), the share target (solo), the block target (off the reference header of a gate)
    pub bound: [u8; 32],
    /// the header is the job's reference header (shares can go to a node; its progress lives in the job)
    pub reference: bool,
    base: u32,
    span: u64,
    next: AtomicU64,
    pub solved: AtomicBool,
}

impl Work {
    fn new(nj: Arc<NJob>, fee: bool, en2_counter: u64) -> Work {
        let gate = nj.nonce_range;
        // the reference header first: a node / a work-protocol pool (no extranonce2 of ours) or an ffgn gate's range
        let reference = nj.en2_size == 0 || (gate.is_some() && nj.ref_next.load(Ordering::Relaxed) < gate.map_or(0, |g| g.1));
        let mut en2 = vec![0u8; nj.en2_size];
        if !reference {
            for (i, b) in en2.iter_mut().enumerate().take(4) {
                *b = (en2_counter >> (8 * i)) as u8;
            }
            if let Some((base, _)) = gate {
                // off the gate's reference header: this session's range base makes the header its own (block-only work)
                for (i, b) in en2.iter_mut().enumerate().skip(4).take(4) {
                    *b = (base >> (8 * (i - 4))) as u8;
                }
                if en2.len() < 8 {
                    en2.iter_mut().for_each(|b| *b = b.wrapping_add(1));
                }
            } else {
                for (i, b) in en2.iter_mut().enumerate().skip(4).take(4) {
                    *b = (en2_counter >> (8 * i)) as u8;
                }
            }
            if fee {
                // the fee login's headers never equal the user's: top bit of the extranonce2
                if let Some(b) = en2.last_mut() {
                    *b |= 0x80;
                }
            } else if let Some(b) = en2.last_mut() {
                *b &= 0x7f;
            }
        }
        let mut extranonce = nj.en1.clone();
        extranonce.extend_from_slice(&en2);
        let time = nj.job.time_start;
        let header = nj.job.header(&extranonce, time, 0);
        let (bound, base, span) = match gate {
            Some((b, sp)) if reference => (nj.pool_target.unwrap_or(nj.job.share_target), b, sp),
            Some(_) => (nj.job.target, 0, 1u64 << 32), // only a solution can leave a gate off the reference header
            None if reference => (nj.pool_target.unwrap_or(nj.job.share_target), nj.ref_base, 1u64 << 32),
            None => (nj.pool_target.unwrap_or(nj.job.share_target), 0, 1u64 << 32),
        };
        Work { nj, fee, extranonce, time, header, bound, reference, base, span, next: AtomicU64::new(0), solved: AtomicBool::new(false) }
    }

    fn counter(&self) -> &AtomicU64 {
        if self.reference { &self.nj.ref_next } else { &self.next }
    }

    /// up to `n` untried nonces (start, count) of this header
    pub fn take(&self, n: u32) -> Option<(u32, u32)> {
        if self.solved.load(Ordering::Relaxed) {
            return None;
        }
        let s = self.counter().fetch_add(n as u64, Ordering::Relaxed);
        if s >= self.span {
            return None;
        }
        let c = (self.span - s).min(n as u64) as u32;
        Some((self.base.wrapping_add(s as u32), c))
    }

    pub fn exhausted(&self) -> bool {
        self.counter().load(Ordering::Relaxed) >= self.span
    }
}

pub struct Opts {
    pub node: Vec<String>,
    pub pools: Vec<String>,
    pub wallet: String,
    pub worker: String,
    pub password: String,
    pub dataset: Option<String>,
    pub backend: Option<gpu::Backend>,
    pub cpu_threads: usize,
    pub minutes: Option<f64>,
    pub blocks: Option<u64>,
    pub devices: Option<Vec<i32>>,
    pub lock_cclock: Option<String>,
    pub lock_mclock: Option<String>,
    pub table_mclock: Option<String>,
    pub moffset: Option<String>,
    pub temp: crate::templimit::Opts,
    pub uv: crate::uv::Opts,
    pub allow_other_dataset: bool,
    pub api: Option<String>,
    /// --keep-hive-mem: keep a HiveOS memory lock below the maximum (D-007)
    pub keep_hive_mem: bool,
    /// 1.0.11: --single-process: every GPU's engine in the miner process (as before; a CUDA fault of one GPU stops all of them)
    pub single_process: bool,
}

pub fn parse_backend(s: Option<&str>) -> Result<Option<gpu::Backend>> {
    Ok(match s.map(|x| x.to_ascii_lowercase()) {
        None => None,
        Some(x) if x == "auto" => None,
        Some(x) if x == "cuda" || x == "nvidia" || x == "gpu" => Some(gpu::Backend::Cuda),
        Some(x) if x == "cpu" => Some(gpu::Backend::Cpu),
        Some(x) => bail!("--backend {x}: cuda or cpu"),
    })
}

/// 1.8.0 (coins.rs): a pool URL that is NEB by a positive sign only (never a guess): the NEB work protocol (`work://`,
/// `neb+work://`, `aria://`, Aria's host), `neb://`, or a host whose first label is `neb` (neb.suprnova.cc, neb.ariabrain.com).
/// Any other NEB pool (or an ffgn gate) needs `--algo neb` / `--coin neb`.
pub fn looks_like_neb_pool(url: &str) -> bool {
    let t = url.trim().to_ascii_lowercase();
    if ["work://", "neb+work://", "aria://", "neb://"].iter().any(|p| t.starts_with(p)) || t.contains("ariabrain.com") {
        return true;
    }
    let rest = t.split_once("://").map_or(t.as_str(), |(_, r)| r);
    let host = rest.split(['/', ':']).next().unwrap_or("");
    host.split('.').next() == Some("neb") && host.contains('.')
}

/// The dataset file: `--dataset FILE`, else `neb-datasets.bin` next to the binary, in the working directory, or in the
/// HiveOS miner folder.
pub fn find_dataset(given: Option<&str>) -> Result<std::path::PathBuf> {
    if let Some(g) = given {
        return Ok(g.into());
    }
    let mut c: Vec<std::path::PathBuf> = Vec::new();
    if let Some(d) = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())) {
        c.push(d.join("neb-datasets.bin"));
    }
    c.push("neb-datasets.bin".into());
    c.push("/hive/miners/custom/freeforgem/neb-datasets.bin".into());
    c.push("/hive/miners/custom/ffgn-hive/neb-datasets.bin".into()); // FFGN's HiveOS folder (a rig moved from ffgn-hive)
    for p in &c {
        if p.is_file() {
            return Ok(p.clone());
        }
    }
    bail!(
        "neb-datasets.bin not found (looked in {}); download the dataset file of NEB 1.0.1 (main network: {} bytes, BLAKE3 {}…, {}) and give it with --dataset FILE",
        c.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", "),
        refc::MAINNET_SIZE,
        &refc::MAINNET_BLAKE3[..16],
        refc::MAINNET_URL
    )
}

pub fn load_dataset(given: Option<&str>, allow_other: bool) -> Result<(Arc<DatasetFile>, Arc<Vec<u8>>)> {
    let p = find_dataset(given)?;
    let t = Instant::now();
    let f = refc::load_datasets(&p, allow_other).map_err(anyhow::Error::msg)?;
    let raw = f.bytes.clone();
    println!(
        "dataset: {} ({} network, {} dataset(s): {}; BLAKE3 {}… checked in {:.1} s)",
        p.display(),
        f.network,
        f.sets.len(),
        f.sets.iter().enumerate().map(|(i, d)| format!("{i}={} x{}", d.arch.name(), d.n)).collect::<Vec<_>>().join(" "),
        &hex(&f.hash)[..16],
        t.elapsed().as_secs_f64()
    );
    Ok((Arc::new(f), raw))
}

fn open_devices(devices: &Option<Vec<i32>>, backend: Option<gpu::Backend>, cpu_threads: usize) -> Result<Vec<i32>> {
    gpu::load(backend, cpu_threads)?;
    let n = gpu::device_count();
    ensure!(n > 0, "no GPU visible to the NEB GPU library (check the driver; NVIDIA RTX 20 / GTX 16 or newer)");
    let devs: Vec<i32> = match devices {
        Some(d) => d.clone(),
        None => (0..n as i32).collect(),
    };
    let mut ok = Vec::new();
    for &d in &devs {
        if d < 0 || (d as usize) >= n {
            // D-014: one wrong entry of --devices must not stop the others
            eprintln!("WARNING: --devices {d}: no such GPU ({n} visible): skipped, the others mine");
            continue;
        }
        let i = gpu::info(d)?;
        if i.supported == 0 {
            eprintln!("WARNING: GPU{d} ({}, sm_{}{}) is not supported by the NEB engine (compute capability below what the engine supports): skipped", gpu::device_name(d), i.cc_major, i.cc_minor);
            continue;
        }
        ok.push(d);
    }
    ensure!(!ok.is_empty(), "no usable GPU among {devs:?}");
    Ok(ok)
}

// ---------------------------------------------------------------------------------------------------------------------
// self-test and bench
// ---------------------------------------------------------------------------------------------------------------------

pub fn self_test(o: &Opts) -> Result<()> {
    println!("== NEB self-test ==");
    refc::self_check().map_err(anyhow::Error::msg)?;
    println!("[ok] CPU: the built-in C reference reproduces the NEB vectors (KEP1, RET2)");
    let devs = open_devices(&o.devices, o.backend, o.cpu_threads)?;
    let (file, raw) = load_dataset(o.dataset.as_deref(), o.allow_other_dataset)?;
    let mut failed = Vec::new();
    for &d in &devs {
        let r = (|| -> Result<String> {
            let mut s = gpu::open(d, &file, &raw)?;
            let rep = s.selftest()?;
            // real dataset: every slot, a synthetic model of its network, hits vs the reference on 256 nonces
            let mut sc = refc::Scratch::default();
            for (ds, set) in file.sets.iter().enumerate() {
                if set.arch == refc::Arch::V1 {
                    continue;
                }
                let (_, m) = refc::synthetic(set.arch).map_err(anyhow::Error::msg)?;
                s.set_model(ds, &m)?;
                let h = [ds as u8 ^ 0x5a; 80];
                let mut bound = [0xffu8; 32];
                bound[31] = 0x40;
                let n = if gpu::backend() == gpu::Backend::Cpu { 16 } else { 256 };
                let out = s.scan(ds, &h, 1000, n, &[0u8; 32], &bound)?;
                ensure!(!out.compute_error, "compute error on dataset {ds}");
                let mut got: Vec<u32> = out.hits.iter().map(|x| x.0).collect();
                got.sort_unstable();
                let want: Vec<u32> = (1000..1000 + n).filter(|&k| admits(&bound, &refc::ticket_of(&mut sc, &m, set, &h, k).unwrap())).collect();
                ensure!(got == want, "dataset {ds} ({}): device hits {got:?} != reference {want:?}", set.arch.name());
            }
            Ok(rep)
        })();
        match r {
            Ok(rep) => println!("[ok] GPU{d} {}: {rep}; real dataset: device hits = reference", gpu::device_name(d)),
            Err(e) => {
                println!("[FAIL] GPU{d}: {e:#}");
                failed.push(d);
            }
        }
    }
    ensure!(failed.is_empty(), "NEB self-test failed on GPU(s) {failed:?}");
    println!("NEB self-test passed on {} device(s)", devs.len());
    Ok(())
}

pub fn hash_bench(o: &Opts, secs: f64) -> Result<()> {
    let devs = open_devices(&o.devices, o.backend, o.cpu_threads)?;
    let nv = crate::nvml::Nvml::load().ok();
    println!("NEB hash-bench: {} device(s) {:?}, {secs} s per network ({})", devs.len(), devs, gpu::build_info());
    let empty = Arc::new(DatasetFile { hash: [0; 32], bytes: Arc::new(Vec::new()), network: "none", sets: vec![] });
    for arch in [1u32, 2] {
        let res: Vec<Result<(i32, f64, Option<f64>)>> = std::thread::scope(|s| {
            let hs: Vec<_> = devs
                .iter()
                .map(|&d| {
                    let (nv, empty) = (nv.as_ref(), empty.clone());
                    s.spawn(move || -> Result<(i32, f64, Option<f64>)> {
                        let mut sc = gpu::open(d, &empty, &[])?;
                        let h = nv.and_then(|n| gpu::device_pci_bus_id(d).ok().and_then(|b| n.by_bus(&b).ok()));
                        let w = Arc::new(Mutex::new((0.0, 0u32)));
                        let done = Arc::new(AtomicBool::new(false));
                        let pw = {
                            let (w, done) = (w.clone(), done.clone());
                            nv.zip(h).map(|(n, h)| {
                                let n: &'static crate::nvml::Nvml = unsafe { &*(n as *const crate::nvml::Nvml) };
                                std::thread::spawn(move || {
                                    while !done.load(Ordering::Relaxed) {
                                        std::thread::sleep(Duration::from_millis(500));
                                        if let Some(p) = n.power_w(h) {
                                            let mut g = w.lock().unwrap();
                                            g.0 += p;
                                            g.1 += 1;
                                        }
                                    }
                                })
                            })
                        };
                        let r = sc.bench(-1, arch, (secs * 1000.0) as u32);
                        if arch == 1 {
                            if let Some(k) = sc.kep1_kernel() {
                                println!("hash-bench KEP1 GPU{d}: kernel {k}");
                            }
                        }
                        done.store(true, Ordering::Relaxed);
                        if let Some(t) = pw {
                            let _ = t.join();
                        }
                        let g = w.lock().unwrap();
                        Ok((d, r?, (g.1 > 0).then(|| g.0 / g.1 as f64)))
                    })
                })
                .collect();
            hs.into_iter().map(|h| h.join().unwrap_or_else(|_| Err(anyhow::anyhow!("bench thread panicked")))).collect()
        });
        let mut tot = 0.0;
        for r in res {
            let (d, hs, w) = r?;
            tot += hs;
            let pw = w.map_or(String::new(), |w| format!("  {w:.0} W  {:.1} att/W", hs / w));
            println!("hash-bench {} GPU{d} {}: {:.0} attempts/s{pw}", if arch == 1 { "KEP1" } else { "RET2" }, gpu::device_name(d), hs);
        }
        println!("hash-bench {} total {:.0} attempts/s", if arch == 1 { "KEP1" } else { "RET2" }, tot);
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------------
// mining
// ---------------------------------------------------------------------------------------------------------------------

pub struct GpuStat {
    pub dev: i32,
    pub name: String,
    pub bus: String,
    /// attempts EVALUATED (1.0.9: from the engine's nebgpu_eval_stats when it has it; the hashrate)
    pub attempts: Arc<AtomicU64>,
    /// 1.0.10 (auto-uv on the mining load): results of this GPU that passed the CPU check (hits and audits), and what it mines
    /// now (dataset slot + model + path, hashed; 0 = nothing yet): the tuner pairs power samples only under the same load
    pub checked: Arc<AtomicU64>,
    pub load_key: Arc<AtomicU64>,
    /// 1.0.9: nonces scanned (with the engine's cheap filter / digest table: more than evaluated; not a hashrate)
    pub screened: AtomicU64,
    pub alive: Arc<AtomicBool>,
    pub bad: Arc<AtomicU64>,
    pub compute_err: AtomicU64,
    pub err: Mutex<Option<String>>,
    pub pause: Arc<AtomicBool>,
    pub paused: Arc<AtomicBool>,
    pub kernel: Mutex<String>,
    /// 1.0.5: the KEP1 attempt kernel chosen by the engine (1.0.6: chosen again at every new model; logged when it changes)
    pub kep1: Mutex<Option<String>>,
    /// 1.0.5: position of the GPU in --devices order (the model jitter)
    pub pos: usize,
    /// 1.0.5: the GPU's idle time on the last model change (ms: from the end of the last scan of the old model to the first scan
    /// of the new one) and the moment it went back to work (unix ms)
    pub gap_ms: AtomicU64,
    pub back_at: AtomicU64,
    /// cancels a running scan of this GPU's worker when the work changes (1.0.11: the engine context in this process, or a
    /// cancel frame to the GPU's engine process), None = none
    pub ctx: Mutex<Option<gpu::Canceller>>,
    /// 1.0.11 (iso.rs): crashes of this GPU's own engine process (a CUDA fault / hang of this GPU): its worker started a new
    /// one; the tuner steps a tuned offset 2 steps below the crash (tune::tick), the main loop records the crash (faults.rs)
    pub crashes: Arc<AtomicU64>,
    /// 1.0.11: engine contexts / processes opened for this GPU (1 + its restarts)
    pub opens: AtomicU64,
    /// why this GPU does not mine the current work (None = it mines)
    pub idle_reason: Mutex<Option<String>>,
}

struct Shared {
    stop: Arc<AtomicBool>,
    cur: Mutex<Option<Arc<Work>>>,
    finds: Mutex<mpsc::Sender<Find>>,
    file: Arc<DatasetFile>,
    raw: Arc<Vec<u8>>,
    hits: AtomicU64,
    shares: AtomicU64,
    blocks: AtomicU64,
    stale: AtomicU64,
    audits: AtomicU64,
    easy_warned: AtomicU64,
    verbose_mem: AtomicU64, // 1.0.11: --table-mclock switch lines logged (the first few only)
    /// the extranonce2 counter of the published headers (1.0.9: shared, a worker that used up a header publishes the next)
    en2: AtomicU64,
    /// 1.0.11 (iso.rs): one engine process per GPU; how a worker opens its device (gpu::open or iso::open; tests: a fake)
    iso: bool,
    open: Opener,
    /// 1.0.11: crashes of a GPU's engine process, for the main loop (the crash record, after the driver's Xid had a moment)
    crashes: Mutex<Vec<IsoCrash>>,
    /// the wait before a failed worker opens its device again
    retry: Duration,
}

type Opener = Arc<dyn Fn(i32, &Arc<DatasetFile>, &[u8]) -> Result<Box<dyn gpu::Scanner>> + Send + Sync>;

/// 1.0.11: a crash of one GPU's engine process (only this GPU restarts).
struct IsoCrash {
    dev: i32,
    pos: usize,
    err: String,
    at: Instant,
}

/// 1.0.11: the wait before a worker opens its device again: 5 s in one process (as before); 3 s for a new engine process
/// of its own (the driver has freed the dead process' context by then)
fn retry_wait(iso: bool) -> Duration {
    Duration::from_secs(if iso { 3 } else { 5 })
}

/// target time of one device call
/// (the engine's advice: scans of >= 0.5 s; a new job cancels a running scan: nebgpu_cancel)
const CALL_SECS: f64 = 0.5;
/// 1.0.9: the call size by the engine's path of the slot (engine 0.6.0 nebgpu_slot_path), as (target seconds, least, most
/// nonces). A call is sized by time; the bounds hold it after the path, and with it the nonce rate, changes. The cancel
/// latency does not depend on the call: the engine stops within one launch (<= ~20 ms of GPU time; nebtest cancel). The
/// hits of a call are checked and sent when it returns, so a fast path aims at 0.1 s (a share's delay, stale risk) and
/// still makes calls of >= 2^26 nonces on the table (~4-5 G nonces/s screened): smaller calls cost the per-call work
/// (canary, syncs, the host loop): RTX 3070, Kepler: 2^24 nonces/call -7 %, 2^25 -3 % against 2^28 (Z-bench3070).
/// Plain: as before (0.5 s, at most 2^24).
fn call_size(p: Option<gpu::Path>) -> (f64, u32, u32) {
    match p {
        Some(gpu::Path::Table) => (0.1, 1 << 26, 1 << 31),
        Some(gpu::Path::Cheap) => (0.1, 1 << 23, 1 << 28),
        _ => (CALL_SECS, 1024, 1 << 24),
    }
}
const MODEL_PROBES: usize = 2;
const AUDIT_BATCHES: u64 = 64;
/// hits of one batch checked on the CPU at most (F-14)
const MAX_CHECKS: usize = 64;

/// errors in a row after which a GPU is given up (its context is reopened before each retry)
const WORKER_RETRIES: u32 = 3;

fn worker(dev: i32, sh: Arc<Shared>, gs: Arc<GpuStat>) {
    // F-16: whatever ends this thread (a panic included), the GPU is reported as stopped
    struct Dead<'a>(&'a GpuStat);
    impl Drop for Dead<'_> {
        fn drop(&mut self) {
            if std::thread::panicking() {
                *self.0.err.lock().unwrap() = Some("the worker thread panicked".into());
            }
            self.0.alive.store(false, Ordering::Relaxed);
        }
    }
    let _dead = Dead(&gs);
    let mut fails = 0u32;
    loop {
        let t = Instant::now();
        match worker_loop(dev, &sh, &gs) {
            Ok(()) => return,
            Err(e) => {
                if t.elapsed() > Duration::from_secs(600) {
                    fails = 0; // it worked for a while: a new series
                }
                fails += 1;
                let text = format!("{e:#}");
                // 1.0.11 (iso.rs): a CUDA fault / crash / hang in this GPU's own engine process ends only that process: this GPU
                // restarts alone (a new engine process), the other GPUs keep mining. It is counted as a crash of this GPU (not as
                // a wrong result): the tuner and the fault record act on it
                let crash = sh.iso && crate::faults::sticky(&text) && !sh.stop.load(Ordering::Relaxed);
                if crash {
                    gs.crashes.fetch_add(1, Ordering::Relaxed);
                    sh.crashes.lock().unwrap().push(IsoCrash { dev, pos: gs.pos, err: text.clone(), at: Instant::now() });
                }
                // F-07: an engine error (a wrong model probe, a CUDA error) is retried on a fresh context: the engine asks to
                // reload the model after a compute error; a card that keeps failing is given up and goes to faults.rs
                // (one process: an injected fault stands for a fault that ends CUDA for the process; no retry)
                if fails > WORKER_RETRIES || sh.stop.load(Ordering::Relaxed) || (!sh.iso && text.contains("injected fault")) {
                    eprintln!("GPU{dev}: NEB worker stopped: {text}");
                    *gs.err.lock().unwrap() = Some(text);
                    return;
                }
                let wait = sh.retry;
                if crash {
                    eprintln!("GPU{dev}: {text} — only this GPU restarts: a new engine process in {:.1} s (try {fails} of {WORKER_RETRIES}); the other GPUs keep mining", wait.as_secs_f64());
                } else {
                    eprintln!("GPU{dev}: {text} — the GPU context is opened again in {:.0} s (try {fails} of {WORKER_RETRIES})", wait.as_secs_f64());
                    gs.bad.fetch_add(1, Ordering::Relaxed);
                }
                let t = Instant::now();
                while t.elapsed() < wait {
                    if sh.stop.load(Ordering::Relaxed) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        }
    }
}

/// FFGN 1.0.5: every GPU stops making attempts in the same few seconds while there is work and none is paused: one line
/// with the time, for the diagnosis of a rig (hive3070SS: 12 GPUs dropped from ~120 W to ~40 W at once mid-job, then
/// "a job but no attempts" and Xid 79 "GPU fallen off the bus": a PCIe bus or power event, not the pool or the miner).
fn stall_watch(gs: &[Arc<GpuStat>], sh: &Shared, t0: Instant) {
    let n = gs.len();
    let mut last: Vec<u64> = gs.iter().map(|g| g.attempts.load(Ordering::Relaxed)).collect();
    let mut moved: Vec<Instant> = vec![Instant::now(); n];
    let mut ever = vec![false; n];
    let mut reported = false;
    // 1.0.10 (CRASH-12gpu): the last moment each worker reported an engine error (its `bad` / `compute_err` count moved)
    let errs = |g: &GpuStat| g.bad.load(Ordering::Relaxed) + g.compute_err.load(Ordering::Relaxed) + g.crashes.load(Ordering::Relaxed);
    let mut last_err: Vec<u64> = gs.iter().map(|g| errs(g)).collect();
    let mut err_at: Vec<Option<Instant>> = vec![None; n];
    while !sh.stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(250));
        let now = Instant::now();
        for (i, g) in gs.iter().enumerate() {
            let a = g.attempts.load(Ordering::Relaxed);
            if a != last[i] {
                last[i] = a;
                moved[i] = now;
                ever[i] = true;
            }
            let e = errs(g);
            if e != last_err[i] {
                last_err[i] = e;
                err_at[i] = Some(now);
            }
        }
        let busy = sh.cur.lock().unwrap().is_some() && gs.iter().all(|g| !g.pause.load(Ordering::Relaxed) && g.alive.load(Ordering::Relaxed));
        let stalled = |i: usize| now.duration_since(moved[i]) >= Duration::from_secs(4);
        if !busy || !ever.iter().all(|&e| e) {
            // after a pause every GPU starts over (no stall is counted across it)
            if !busy {
                moved.iter_mut().for_each(|m| *m = now);
            }
            continue;
        }
        if (0..n).all(stalled) {
            let first = moved.iter().min().copied().unwrap_or(now);
            let lastm = moved.iter().max().copied().unwrap_or(now);
            if !reported && lastm.duration_since(first) <= Duration::from_secs(3) {
                reported = true;
                tune::RIG_TROUBLE.fetch_add(1, Ordering::Relaxed); // --uv-parallel goes down to 1
                let at = |m: Instant| m.duration_since(t0).as_secs_f64();
                let errored = err_at.iter().filter(|t| t.is_some_and(|t| now.duration_since(t) <= Duration::from_secs(STALL_ERR_SECS))).count();
                eprintln!(
                    "ERROR: all {n} GPUs stalled at once at {} (uptime {:.1} s; the last attempts {:.1}..{:.1} s, within {} ms): {}",
                    { let s = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()) % 86400; format!("{:02}:{:02}:{:02} UTC", s / 3600, s / 60 % 60, s % 60) },
                    at(now),
                    at(first),
                    at(lastm),
                    lastm.duration_since(first).as_millis(),
                    stall_cause(errored, n, sh.iso)
                );
            }
        } else if (0..n).all(|i| !stalled(i)) {
            reported = false;
        }
    }
}

/// 1.0.11: the driver's Xid of a fault comes a moment after the error of the engine process: a crash is recorded this long after it.
const ISO_XID_WAIT: Duration = Duration::from_millis(1500);

/// 1.0.11 (iso.rs): the crashes of single GPUs' engine processes, from the main loop: which GPU (it is the GPU of the process;
/// the Xid by NVML when there is one), the crash record (gpu-faults.json, with the offset of the crash: the tuner's fault
/// offset, else the card's offset now), one line each.  The auto-uv answer (2 steps below a tuned offset) is tune::tick's.
fn iso_crashes(sh: &Shared, gs: &[Arc<GpuStat>], watch: Option<&crate::faults::Watch>, live: Option<&Arc<tune::Live>>, dir: &std::path::Path) {
    let due: Vec<IsoCrash> = {
        let mut q = sh.crashes.lock().unwrap();
        if q.is_empty() {
            return;
        }
        let (d, keep): (Vec<IsoCrash>, Vec<IsoCrash>) = q.drain(..).partition(|c| c.at.elapsed() >= ISO_XID_WAIT);
        *q = keep;
        d
    };
    for c in due {
        if sh.stop.load(Ordering::Relaxed) {
            return;
        }
        let others = gs.iter().filter(|g| g.pos != c.pos && g.alive.load(Ordering::Relaxed)).count();
        let injected = crate::faults::test_fault().is_some_and(|f| f.0 == c.dev);
        let Some((w, k)) = watch.and_then(|w| w.by_dev(c.dev).map(|k| (w, k))) else {
            println!("faults: {}", crate::faults::iso_line(&format!("GPU{}", c.dev), None, &c.err, others, None));
            continue;
        };
        let card = &w.cards[k];
        let xid = w.take_for(k).first().copied().or(injected.then_some(13));
        let off = live.and_then(|l| tune::fault_offset(l, c.dev)).or_else(|| w.offset(k));
        // another tool's change of the card at the same moment (HiveOS nvidia-oc): not counted (as faults::attribute)
        let foreign = crate::faults::foreign_offset(live.and_then(|l| tune::expected_offset(l, c.dev)), w.offset(k)) || crate::faults::foreign_near(&card.bus);
        let rec = (!foreign).then(|| crate::faults::record_crash(dir, &card.uuid, &card.bus, &card.name, xid.unwrap_or(0), off));
        println!("faults: {}", crate::faults::iso_line(&card.label(), xid, &c.err, others, rec.as_ref()));
    }
}

/// 1.0.10 (CRASH-12gpu): an engine error of a worker within this many seconds of an all-GPU stall explains the stall.
const STALL_ERR_SECS: u64 = 15;

/// 1.0.10 (CRASH-12gpu): the cause named by the all-GPU stall line. When workers reported engine errors around the stall
/// (12x RTX 2060 SUPER rig, 21.11.2026: one GPU's Xid 31, then "an illegal memory access" on all 12 workers), the stall is
/// the CUDA fault of ONE GPU ending CUDA for every GPU of the process (one process drives all GPUs), not a bus or power
/// event of the rig: the faulting GPU is the one with the Xid (the `faults:` lines), the others are not at fault.
fn stall_cause(errored: usize, n: usize, iso: bool) -> String {
    if errored > 0 && iso {
        // 1.0.11: every GPU has an engine process of its own: one GPU's CUDA fault cannot stop the others
        format!(
            "{errored} of {n} GPU engine processes reported an error at the same time although each GPU runs in a process of its own: a rig-wide event (PCIe bus, power, the driver), not the CUDA fault of one GPU"
        )
    } else if errored > 0 {
        format!(
            "{errored} of {n} GPU workers reported an engine error at the same time: a CUDA fault on one GPU ends CUDA for every GPU of the process; the failing GPU is the one the driver names (Xid, see the faults: lines), not every GPU, and not a PCIe bus / power event by itself"
        )
    } else {
        "likely a PCIe bus / power event (risers, PSU, cables), not the pool".to_string()
    }
}

/// 1.0.5: one line per model change once every GPU is back at work: the idle time of each GPU (ms) and how far apart they went
/// back to full load (the soft ramp spreads it on rigs of 4+ GPUs; a few ms = a synchronous power step of all cards)
fn model_gap_line(gs: &[Arc<GpuStat>]) {
    static SEEN: Mutex<Vec<u64>> = Mutex::new(Vec::new());
    let back: Vec<u64> = gs.iter().map(|g| g.back_at.load(Ordering::Relaxed)).collect();
    let mut seen = SEEN.lock().unwrap();
    if seen.len() != back.len() {
        *seen = vec![0; back.len()];
    }
    if back.iter().zip(seen.iter()).all(|(b, s)| *b > *s) {
        let (lo, hi) = (back.iter().min().copied().unwrap_or(0), back.iter().max().copied().unwrap_or(0));
        if hi - lo < 60_000 {
            println!(
                "model change: GPU idle {} ms; back at work within {} ms of each other",
                gs.iter().map(|g| format!("GPU{} {}", g.dev, g.gap_ms.load(Ordering::Relaxed))).collect::<Vec<_>>().join(", "),
                hi - lo
            );
        }
        *seen = back;
    }
}

fn key_of(header: &[u8; 80], nonce: u32) -> [u8; 32] {
    let mut h = *header;
    h[76..80].copy_from_slice(&nonce.to_le_bytes());
    *blake3::hash(&h).as_bytes()
}

/// the engine's KEP1 kernel text without its timings ("mixed per slot: 0 dp4a (auto: ...); 1 warp-mma (...)" ->
/// "mixed per slot: 0 dp4a; 1 warp-mma"): the log line is printed when this changes
fn kep1_names(s: &str) -> String {
    let mut o = String::new();
    let mut depth = 0u32;
    for c in s.chars() {
        match c {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            _ if depth == 0 => o.push(c),
            _ => {}
        }
    }
    // 1.0.9: engine 0.6.0 appends " | <path>" to each slot's kernel text; the path has its own log line (worker_loop)
    let o = o.split_whitespace().collect::<Vec<_>>().join(" ").replace(" ;", ";");
    o.split(';').map(|p| p.split(" |").next().unwrap_or("").trim()).collect::<Vec<_>>().join("; ")
}

fn worker_loop(dev: i32, sh: &Shared, gs: &GpuStat) -> Result<()> {
    let pos = gs.pos;
    let mut last_scan_end = Instant::now();
    let mut gap_from: Option<Instant> = None;
    let mut probe: Option<(usize, std::thread::JoinHandle<bool>)> = None; // 1.0.5: the CPU check of a new model, in the background // 1.0.5: a model change: the idle time is measured up to the next scan
    let mut idle_since: Option<Instant> = None; // 1.0.5: since when this GPU has had nothing to run (the soft ramp after it)
    let mut s = (sh.open)(dev, &sh.file, &sh.raw)?;
    if gs.opens.fetch_add(1, Ordering::Relaxed) > 0 && sh.iso {
        println!("GPU{dev}: a new engine process is open: back to mining (restart {})", gs.opens.load(Ordering::Relaxed) - 1);
    }
    *gs.kernel.lock().unwrap() = s.kernel();
    *gs.ctx.lock().unwrap() = s.canceller();
    // the context is closed when `s` drops (after this guard): nobody may cancel it after that (F-17)
    struct Clear<'a>(&'a Mutex<Option<gpu::Canceller>>);
    impl Drop for Clear<'_> {
        fn drop(&mut self) {
            *self.0.lock().unwrap() = None;
        }
    }
    let _clear = Clear(&gs.ctx);
    let mut sc = refc::Scratch::default();
    let mut loaded: Vec<Option<[u8; 32]>> = vec![None; sh.file.sets.len().max(16)];
    // 1.0.9: nonces per call per dataset slot (the slots can mine on different paths, i.e. at very different nonce rates)
    let mut counts: Vec<u32> = vec![if gpu::backend() == gpu::Backend::Cpu { 4 } else { 1 << 14 }; sh.file.sets.len().max(16)];
    let mut paths: Vec<Option<gpu::Path>> = vec![None; sh.file.sets.len().max(16)];
    let mut path_text: Vec<String> = vec![String::new(); sh.file.sets.len().max(16)];
    let mut last_eval = s.evaluated();
    let mut batches = 0u64;
    let mut staggered = false;
    let mut last_idle_log = Instant::now();
    let mut mapped_note: std::collections::HashSet<(u8, usize)> = std::collections::HashSet::new(); // logged once per pair
    let mut rng = 0x2545_f491_4f6c_dd1du64 ^ (dev as u64) << 32 ^ crate::uv::now();
    let mut inject = crate::faults::test_fault_for(dev).map(|secs| Instant::now() + Duration::from_secs(secs));
    loop {
        if sh.stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        if inject.is_some_and(|t| Instant::now() >= t && gs.attempts.load(Ordering::Relaxed) > 0) {
            inject = None;
            // 1.0.11: FFG_TEST_FAULT_KIND=kernel|exit|hang: a real fault of the device (a kernel storing to an unmapped
            // address, the engine process dying / hanging); its error comes from the next engine call
            let kind = std::env::var("FFG_TEST_FAULT_KIND").unwrap_or_default();
            if kind.is_empty() || !s.inject_fault(&kind)? {
                bail!("injected fault (FFG_TEST_FAULT): an illegal memory access was encountered");
            }
            continue;
        }
        if gs.pause.load(Ordering::SeqCst) {
            gs.paused.store(true, Ordering::SeqCst);
            idle_since.get_or_insert_with(Instant::now);
            std::thread::sleep(Duration::from_millis(20));
            continue;
        }
        gs.paused.store(false, Ordering::SeqCst);
        let duty = crate::templimit::duty(dev);
        if duty == Some(0) {
            idle_since.get_or_insert_with(Instant::now);
            std::thread::sleep(Duration::from_millis(200));
            continue;
        }
        let Some(w) = sh.cur.lock().unwrap().clone() else {
            idle_since.get_or_insert_with(Instant::now);
            std::thread::sleep(Duration::from_millis(20));
            continue;
        };
        // 1.0.2: the job's slot must hold a dataset of the job's network (the network follows the dataset, not the slot number:
        // NEB 1.0.1 main network = KEP1 in both slots); a job this file cannot mine is reported loudly, never waited on in silence
        let ds = match refc::resolve_dataset(&sh.file.sets, w.nj.slot, w.nj.model.arch) {
            Ok(i) => {
                if gs.idle_reason.lock().unwrap().take().is_some() {
                    eprintln!("GPU{dev}: mining again");
                }
                i
            }
            Err(e) => {
                let mut r = gs.idle_reason.lock().unwrap();
                if r.is_none() || last_idle_log.elapsed() > Duration::from_secs(30) {
                    eprintln!("ERROR GPU{dev}: NOT MINING: {e} (dataset file: {}; give --dataset with the dataset file of the job's network)", sh.file.network);
                    last_idle_log = Instant::now();
                }
                *r = Some(e);
                drop(r);
                std::thread::sleep(Duration::from_millis(500));
                continue;
            }
        };
        if ds != w.nj.slot as usize && mapped_note.insert((w.nj.slot, ds)) {
            println!("GPU{dev}: the job's dataset {} ({}) is dataset {ds} of this file (same data)", w.nj.slot, w.nj.model.arch.name());
        }
        let set = &sh.file.sets[ds];
        if !staggered {
            staggered = true;
            // the first start only: a worker that opens its device again (1.0.11: a new engine process) does not wait for
            // the place of a new GPU in the start order (on a 12-GPU rig that was up to 18 s more per restart)
            if gs.opens.load(Ordering::Relaxed) <= 1 {
                crate::worker::stagger_start(dev, &sh.stop);
            }
            continue;
        }
        if loaded[ds] != Some(w.nj.model.hash) {
            if gap_from.is_none() && loaded[ds].is_some() {
                gap_from = Some(last_scan_end);
            }
            if loaded[ds].is_some() {
                // 1.0.5: a new block's model is not built on every GPU in the same millisecond (rigs of 6+ GPUs)
                let j = crate::worker::model_jitter(pos);
                if !j.is_zero() {
                    std::thread::sleep(j);
                }
            }
            // 1.0.11: the table build (and the full-path trial of a first model) runs at the full memory clock
            if clocks::table_mem(dev, false).is_some() && sh.verbose_mem.fetch_add(1, Ordering::Relaxed) < 2 {
                println!("GPU{dev}: full memory clock for the new model (--table-mclock)");
            }
            let t = Instant::now();
            s.set_model(ds, &w.nj.model)?;
            loaded[ds] = Some(w.nj.model.hash);
            // the model on the device: exact tickets of a few keys vs the reference
            let keys: Vec<[u8; 32]> = (0..MODEL_PROBES as u32).map(|i| key_of(&w.header, i.wrapping_mul(0x9e37_79b9))).collect();
            if let Some(dt) = s.eval_keys(ds, &keys)? {
                // 1.0.5: the CPU side of the check runs beside the mining (the GPU does not wait ~0.1 s on each new block); its
                // verdict is read before the next batch, and every hit is checked on the CPU before it is sent anyway
                let (m, set2) = (w.nj.model.clone(), sh.file.sets[ds].clone());
                let h = std::thread::spawn(move || {
                    let mut sc = refc::Scratch::default();
                    keys.iter().zip(&dt).all(|(k, t2)| refc::ticket(&mut sc, &m, &set2, k).is_ok_and(|r| &r == t2))
                });
                probe = Some((ds, h));
            }
            if t.elapsed() > Duration::from_secs(2) {
                println!("GPU{dev}: model upload took {:.1} s", t.elapsed().as_secs_f64());
            }
        }
        // 1.0.9: the slot's path (engine 0.6.0): the call size follows it (a cheap / table slot screens 10-100x more nonces/s)
        if let Some((p, txt)) = s.path(ds) {
            if paths[ds] != Some(p) {
                let (_, lo, hi) = call_size(Some(p));
                counts[ds] = counts[ds].clamp(lo, hi);
                paths[ds] = Some(p);
            }
            if path_text[ds] != txt {
                let name = |t: &str| kep1_names(t); // 1.0.11: without the parentheses, so the L=3 choice of the table path shows too
                // the engine's interim texts while it measures a new model ("plain" without details, "... trial running")
                if name(&path_text[ds]) != name(&txt) && txt != "plain" && !txt.contains("trial running") {
                    println!("GPU{dev}: dataset {ds} path {txt}");
                }
                path_text[ds] = txt;
            }
        }
        // 1.0.11 (LOWP111): --table-mclock: the low memory clock only while this slot runs the pure table path (an L=3 /
        // fallback part reads row data from DRAM and wants the full memory clock)
        let pure_table = paths[ds] == Some(gpu::Path::Table) && !path_text[ds].contains("L=3 on") && !path_text[ds].contains("L=3 trial");
        if let Some(low) = clocks::table_mem(dev, pure_table) {
            if sh.verbose_mem.fetch_add(1, Ordering::Relaxed) < 2 {
                println!("GPU{dev}: memory clock {} (--table-mclock)", if low { "low: pure table path" } else { "full: not the pure table path" });
            }
        }
        gs.load_key.store(load_key(ds, &w.nj.model.hash, &path_text[ds]), Ordering::Relaxed);
        let count = counts[ds];
        let Some((start, n)) = w.take(count) else {
            // 1.0.9: the header's 2^32 nonces are used up (a GPU on the digest table screens them in ~1 s): the next header
            // (a new extranonce2) is published here at once instead of on the main loop's next tick
            if w.exhausted() && w.nj.en2_size > 0 && !w.solved.load(Ordering::Relaxed) {
                let mut cur = sh.cur.lock().unwrap();
                if cur.as_ref().is_some_and(|c| Arc::ptr_eq(c, &w)) {
                    let en2 = sh.en2.fetch_add(1, Ordering::Relaxed) + 1;
                    *cur = Some(Arc::new(Work::new(w.nj.clone(), w.fee, en2)));
                }
                continue;
            }
            idle_since.get_or_insert_with(Instant::now);
            std::thread::sleep(Duration::from_millis(10));
            continue;
        };
        if idle_since.take().is_some_and(|t| t.elapsed() >= Duration::from_secs(2)) {
            crate::worker::ramp_gate(&sh.stop); // 1.0.5: back to full load one GPU after another
        }
        if probe.as_ref().is_some_and(|p| p.1.is_finished()) {
            let (pds, h) = probe.take().unwrap();
            if !h.join().unwrap_or(false) {
                gs.bad.fetch_add(1, Ordering::Relaxed);
                loaded[pds] = None;
                bail!("the device disagrees with the C reference on a new model ({} slot {pds}): unstable clocks / undervolt?", w.nj.model.arch.name());
            }
        }
        if let Some(g) = gap_from.take() {
            gs.gap_ms.store(g.elapsed().as_millis() as u64, Ordering::Relaxed);
            gs.back_at.store(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64), Ordering::Relaxed);
        }
        let t = Instant::now();
        let out = s.scan(ds, &w.header, start, n, &w.nj.job.target, &w.bound)?;
        last_scan_end = Instant::now();
        let dt = t.elapsed().as_secs_f64();
        let done = out.cancelled_done.unwrap_or(n);
        gs.screened.fetch_add(done as u64, Ordering::Relaxed);
        // 1.0.9: the hashrate counts EVALUATED attempts (engine 0.6.0); an older engine evaluates every scanned nonce
        let ev = s.evaluated();
        match (ev, last_eval) {
            (Some(e), Some(l)) => gs.attempts.fetch_add(e.saturating_sub(l), Ordering::Relaxed),
            _ => gs.attempts.fetch_add(done as u64, Ordering::Relaxed),
        };
        if ev.is_some() {
            last_eval = ev;
        }
        if w.nj.model.arch == refc::Arch::Kep1 {
            // 1.0.6: engine 0.3.0 times both KEP1 kernels on the first launches of every new model; the API shows the latest
            // choice, the log a line only when the kernel itself changes (not its timings)
            if let Some(k) = s.kep1_kernel() {
                let mut g = gs.kep1.lock().unwrap();
                if g.as_deref() != Some(k.as_str()) {
                    if g.as_deref().map(kep1_names) != Some(kep1_names(&k)) {
                        println!("GPU{dev}: KEP1 kernel {k}");
                    }
                    *g = Some(k);
                }
            }
        }
        if out.compute_error {
            gs.compute_err.fetch_add(1, Ordering::Relaxed);
            // F-06: the auto-uv tuner watches `bad`: an engine canary failure steps the offset back like a wrong nonce
            gs.bad.fetch_add(1, Ordering::Relaxed);
            loaded[ds] = None; // the engine asks for a fresh table
            eprintln!("GPU{dev}: the engine's canary check FAILED (wrong results at these clocks): batch discarded, model reloaded");
            continue;
        }
        batches += 1;
        // F-14: a pool target that is far too easy (difficulty ~1) makes every nonce a hit: at most MAX_CHECKS of a batch are
        // checked (7 ms each on the CPU) and sent, the rest dropped with a warning
        let take = out.hits.len().min(MAX_CHECKS);
        if out.hits.len() > MAX_CHECKS && sh.easy_warned.fetch_add(1, Ordering::Relaxed) % 100 == 0 {
            eprintln!(
                "GPU{dev}: {} hits in one batch: the pool's target is very easy (about {:.1} attempts per share); only {MAX_CHECKS} per batch are checked and sent",
                out.hits.len(),
                proto::expected(&w.bound)
            );
        }
        for (nonce, _dev_ticket) in &out.hits[..take] {
            check_hit(sh, gs, &mut sc, &w, set, *nonce);
        }
        if batches % AUDIT_BATCHES == 0 {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            let nonce = start.wrapping_add((rng % n.max(1) as u64) as u32);
            let k = key_of(&w.header, nonce);
            if let Some(dt) = s.eval_keys(ds, &[k])? {
                let r = refc::ticket(&mut sc, &w.nj.model, set, &k).map_err(anyhow::Error::msg)?;
                sh.audits.fetch_add(1, Ordering::Relaxed);
                if dt[0] == r {
                    gs.checked.fetch_add(1, Ordering::Relaxed);
                }
                if dt[0] != r {
                    gs.bad.fetch_add(1, Ordering::Relaxed);
                    loaded[ds] = None;
                    eprintln!("GPU{dev}: AUDIT FAILED: the device ticket of nonce {nonce} differs from the C reference (unstable clocks?); model reloaded");
                }
            }
        }
        if dt > 0.0 && done == n {
            // 1.0.9: from the nonces of THIS call (n < count at the end of a header), at most x2 / x0.5 per call
            let (secs, lo, hi) = call_size(paths[ds]);
            let c = (n as f64 * (secs / dt)).clamp(count as f64 * 0.5, count as f64 * 2.0);
            let lo = if gpu::backend() == gpu::Backend::Cpu { 1.0 } else { lo as f64 };
            counts[ds] = c.clamp(lo, hi as f64) as u32;
        }
        if let Some(p) = duty {
            std::thread::sleep(Duration::from_secs_f64(dt * (100 - p as u32) as f64 / p as f64));
        }
    }
}

fn check_hit(sh: &Shared, gs: &GpuStat, sc: &mut refc::Scratch, w: &Arc<Work>, set: &refc::Dataset, nonce: u32) {
    sh.hits.fetch_add(1, Ordering::Relaxed);
    let t = match refc::ticket_of(sc, &w.nj.model, set, &w.header, nonce) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("GPU{}: reference check failed: {e}", gs.dev);
            return;
        }
    };
    if !admits(&w.bound, &t) {
        gs.bad.fetch_add(1, Ordering::Relaxed);
        eprintln!("GPU{}: NONCE FAILED THE CPU CHECK (wrong GPU result): height {} nonce {nonce} ticket {}… — not sent", gs.dev, w.nj.job.height, &hex(&t)[48..]);
        return;
    }
    gs.checked.fetch_add(1, Ordering::Relaxed);
    // off the reference header (an ffgn gate's block-only work) a ticket is a solution or nothing: a Share frame of the node
    // can only name a nonce of the reference header
    let share = admits(&w.nj.job.share_target, &t) && (w.reference || w.nj.nonce_range.is_none());
    let block = admits(&w.nj.job.target, &t);
    if share {
        sh.shares.fetch_add(1, Ordering::Relaxed);
    }
    if block {
        // solo: one Submit per job (the node answers it and sends the next job)
        if matches!(w.nj.origin, Origin::Node { .. }) && w.solved.swap(true, Ordering::SeqCst) {
            return;
        }
        sh.blocks.fetch_add(1, Ordering::Relaxed);
        println!("GPU{}: BLOCK FOUND height {} nonce {nonce} ticket …{} — submitting", gs.dev, w.nj.job.height, &hex(&t)[40..]);
    }
    let _ = sh.finds.lock().unwrap().send(Find { job: w.nj.clone(), extranonce: w.extranonce.clone(), time: w.time, nonce, ticket: t, share, block, fee: w.fee });
}

/// 1.0.10: what a GPU mines now, for the tuner (auto-uv pairs power samples only under the same load): FNV-1a of the dataset
/// slot, the model hash and the engine's path text of the slot; never 0.
fn load_key(ds: usize, model: &[u8; 32], path: &str) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in (ds as u64).to_le_bytes().iter().chain(model.iter()).chain(path.as_bytes()) {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h.max(1)
}

fn now_unix() -> u32 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()) as u32
}

pub fn mine(o: Opts) -> Result<()> {
    let solo = !o.node.is_empty();
    ensure!(solo != !o.pools.is_empty(), "give a work source: --node 127.0.0.1:3441 (solo, a NEB node) or --pool HOST:PORT (a neb-pool/1 pool or an ffgn gate), not both");
    if !solo {
        ensure!(!o.wallet.is_empty(), "--wallet neb1... is required with --pool (the pool pays that address)");
    }
    let (file, raw) = load_dataset(o.dataset.as_deref(), o.allow_other_dataset)?;
    crate::wintune::process(false);
    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = stop.clone();
        ctrlc::set_handler(move || {
            if stop.swap(true, Ordering::SeqCst) {
                crate::offset::restore_all();
                clocks::restore_all();
                eprintln!("second interrupt: exiting at once");
                crate::conout::exit(130);
            }
            eprintln!("shutting down (finishing in-flight work)...");
        })?;
    }
    crate::wintune::on_console_close(|| {
        crate::offset::restore_all();
        clocks::restore_all();
    });
    let devices = open_devices(&o.devices, o.backend, o.cpu_threads)?;
    let cuda = gpu::backend() == gpu::Backend::Cuda;
    // 1.0.11 (iso.rs): one engine process per GPU, unless --single-process
    let iso = iso::wanted(o.single_process, cuda);
    iso::set_enabled(iso);
    let dir = crate::uv::state_dir(&o.uv);
    let buses: Vec<String> = devices.iter().map(|&d| gpu::device_pci_bus_id(d)).collect::<Result<_>>()?;
    o.temp.validate(devices.len())?;
    let fee_cfg = fee::config(solo);
    println!(
        "{} {} | NEB ({ALGO_NAME}) | {} | {} | {} device(s) {:?}",
        crate::PRODUCT,
        env!("CARGO_PKG_VERSION"),
        if solo { format!("solo node {}", o.node.join(",")) } else { format!("pool {}", o.pools.join(",")) },
        gpu::build_info(),
        devices.len(),
        devices
    );
    if iso {
        println!("engine: one process per GPU (a CUDA fault on one GPU restarts only that GPU, the others keep mining; --single-process: every GPU in this process)");
    }
    if cuda {
        clocks::set_bus_resolver(gpu::device_pci_bus_id);
        clocks::recover(&dir);
        // D-006: an offset left by a run that was killed is put back at every start, also one without offset options
        crate::offset::recover(&dir);
    }
    let mut clock_opts = clocks::ClockOpts {
        core_mhz: match &o.lock_cclock { Some(s) => clocks::expand_u32(s, devices.len(), "--lock-cclock")?, None => vec![] },
        mem_mhz: match &o.lock_mclock { Some(s) => clocks::expand_u32(s, devices.len(), "--lock-mclock")?, None => vec![] },
        mem_off: match &o.moffset { Some(s) => clocks::expand_i32(s, devices.len(), "--moffset")?, None => vec![] },
        table_mhz: match &o.table_mclock { Some(s) => clocks::expand_u32(s, devices.len(), "--table-mclock")?, None => vec![] },
        // D-007 / the 1.8.0 memory policy of NEB (coins.rs): memory bound, a low HiveOS memory lock (Pearl's 810) is replaced by
        // the maximum while mining unless --keep-hive-mem (then the profile's lock, as for Pearl)
        ..clocks::ClockOpts::policy(if o.keep_hive_mem { clocks::MemPolicy::HiveProfile } else { crate::coins::spec(crate::coins::Coin::Neb).mem })
    };
    let mut o = o;
    if cuda && crate::autolock::active() {
        let cards: Vec<crate::autolock::Card> = devices.iter().enumerate().map(|(p, &d)| crate::autolock::Card { dev: d, bus: buses[p].clone(), kernel: "neb-v2".into() }).collect();
        match crate::autolock::resolve("neb", &cards, &mut clock_opts.core_mhz, &clock_opts.mem_mhz, true, &dir, &dir, &stop) {
            crate::autolock::Outcome::Stop => return Ok(()),
            crate::autolock::Outcome::Done(r) => return r.map_err(anyhow::Error::msg),
            crate::autolock::Outcome::Mine { swept } => {
                if swept {
                    (o.uv.reset, o.uv.reset_all) = (false, false);
                }
            }
        }
    }
    let _clock_guard = if cuda { clocks::apply(&devices, &clock_opts, &dir) } else { None };
    // per GPU: bad, pause, paused, (1.0.10) evaluated attempts, CPU-checked results, load key, alive: shared by the worker and the tuner
    let flags: Vec<tune::Flags> = devices.iter().map(|_| tune::Flags::default()).collect();
    let _off_guard = crate::offset::Guard;
    let mut live: Option<Arc<tune::Live>> = None;
    let mut fixed: Option<(&'static crate::nvml::Nvml, tune::Fixed)> = None;
    if o.uv.enabled() && cuda {
        crate::offset::recover(&dir);
        if o.uv.fixed.is_set() {
            let off_nv = crate::offset::begin(&buses, &dir);
            let mut set = Vec::new();
            for (pos, b) in buses.iter().enumerate() {
                if let Some(v) = o.uv.fixed.get(pos) {
                    match crate::offset::set(b, v) {
                        Ok(()) => {
                            println!("GPU{}: core offset {} (--uv-offset)", devices[pos], crate::uv::signed(v));
                            set.push((devices[pos], b.clone(), v));
                        }
                        Err(e) => println!("GPU{}: --uv-offset {}: {e:#}", devices[pos], crate::uv::signed(v)),
                    }
                }
            }
            if let Some(nv) = off_nv {
                fixed = tune::fixed_prepare(nv, &set);
            }
        } else if let Some(nv) = crate::offset::begin(&buses, &dir) {
            let hive = crate::hiveoc::load();
            let cards: Vec<tune::CardRef> = devices
                .iter()
                .enumerate()
                .map(|(pos, &d)| tune::CardRef {
                    dev: d,
                    bus: buses[pos].clone(),
                    name: gpu::device_name(d),
                    core_lock: clock_opts.core_mhz.get(pos).copied().flatten().or_else(|| hive.as_ref().and_then(|h| nv.by_bus(&buses[pos]).ok().and_then(|x| nv.index(x)).and_then(|i| h.core_lock(i as usize)))),
                    pause: flags[pos].pause.clone(),
                    paused: flags[pos].paused.clone(),
                    bad: flags[pos].bad.clone(),
                    crashes: flags[pos].crashes.clone(),
                    attempts: flags[pos].attempts.clone(),
                    checked: flags[pos].checked.clone(),
                    load_key: flags[pos].load_key.clone(),
                    alive: flags[pos].alive.clone(),
                })
                .collect();
            if cards.iter().any(|c| c.core_lock.is_none()) {
                println!("auto-uv: a GPU without a core lock is tuned at its clock under load (profile \"~MHz\"); --lock-cclock is recommended");
            }
            live = Some(tune::prepare(nv, cards, &o.uv, &dir, &stop));
        } else {
            println!("auto-uv: NVML not available: offsets are left as they are");
        }
        if stop.load(Ordering::Relaxed) {
            crate::offset::restore_all();
            return Ok(());
        }
    } else if o.uv.enabled() {
        println!("auto-uv: the CPU backend has no GPU offsets to tune");
    }

    // sources (stopped after the workers: the nonces found last are still sent)
    let src_stop = Arc::new(AtomicBool::new(false));
    let stats = Arc::new(Stats::default());
    let fee_stats = Arc::new(Stats::default());
    let (ev_tx, ev_rx) = mpsc::channel::<(Src, Ev)>();
    let (user_tx, user_rx) = mpsc::channel::<Find>();
    let user_link = crate::failover::Link::new(if solo { o.node.len() } else { o.pools.len() });
    let ns = Arc::new(node::NodeStats::default());
    let fee_ok = Arc::new(AtomicBool::new(false));
    let fee_enabled = Arc::new(AtomicBool::new(false));
    let user_kinds: Arc<Mutex<Vec<pool::Kind>>> = Arc::new(Mutex::new(Vec::new()));
    let user_gate = Arc::new(AtomicBool::new(false));
    if solo {
        for a in &o.node {
            if !a.starts_with("127.") && !a.starts_with("localhost") && !a.starts_with("[::1]") {
                println!("note: --node {a} is not on this machine: the work service of a NEB node listens on 127.0.0.1 only (use an SSH tunnel, or `ffgn --gate` beside the node and --pool here)");
            }
        }
        node::spawn(
            node::NodeLogin { addrs: o.node.clone(), addr: o.node[0].clone(), link: user_link.clone(), stop: src_stop.clone(), enabled: Arc::new(AtomicBool::new(true)), name: format!("{}/{}", crate::PRODUCT.to_ascii_lowercase(), env!("CARGO_PKG_VERSION")), src: Src::User, pool: false, label: String::new(), ready: Arc::new(AtomicBool::new(false)), conn_seq: Arc::new(AtomicU64::new(0)), held: Arc::new(Mutex::new(Vec::new())) },
            ev_tx.clone(),
            user_rx,
            stats.clone(),
            ns.clone(),
        );
    } else {
        let urls: Vec<_> = o.pools.iter().map(|u| pool::parse_url(u).map_err(anyhow::Error::msg)).collect::<Result<_>>()?;
        *user_kinds.lock().unwrap() = o.pools.iter().map(|u| pool::kind_of(u)).collect();
        let login = if o.worker.is_empty() || o.wallet.contains('.') { o.wallet.clone() } else { format!("{}.{}", o.wallet, o.worker) };
        let l = pool::Login {
            name: "user",
            entry: 0,
            url: urls[0].clone(),
            urls: urls.clone(),
            link: user_link.clone(),
            follow: None,
            src: Src::User,
            login,
            password: o.password.clone(),
            // 1.0.3: never a second mining.authorize (the fee login) in the user's session: Suprnova (Miningcore) credited every
            // later share of the session to the last login, i.e. the user's shares went to the dev-fee address after the first fee
            // slot. The fee always mines on a connection of its own (the `fee` / `fee-pool` sessions below).
            fee_login: None,
            enabled: Arc::new(AtomicBool::new(true)),
            stop: src_stop.clone(),
            dataset_hash: file.hash,
            fee_ok: fee_ok.clone(),
            conn_seq: Arc::new(AtomicU64::new(0)),
            fee_stats: fee_stats.clone(),
            kinds: user_kinds.clone(),
            ns: ns.clone(),
            ready: Arc::new(AtomicBool::new(false)),
            gate: user_gate.clone(),
        };
        if urls.len() > 1 {
            let probe_login = pool::Login {
                fee_login: None,
                enabled: Arc::new(AtomicBool::new(true)),
                stop: src_stop.clone(),
                fee_ok: Arc::new(AtomicBool::new(false)),
                link: crate::failover::Link::new(1),
                conn_seq: Arc::new(AtomicU64::new(1 << 40)),
                url: l.url.clone(),
                urls: l.urls.clone(),
                login: l.login.clone(),
                password: l.password.clone(),
                dataset_hash: l.dataset_hash,
                name: "probe",
                follow: None,
                src: Src::User,
                fee_stats: Arc::new(Stats::default()),
                kinds: Arc::new(Mutex::new(user_kinds.lock().unwrap().clone())), // F-04: the prober never writes the user's list
                ns: Arc::new(node::NodeStats::default()),
                ready: Arc::new(AtomicBool::new(false)),
                entry: 0,
                gate: Arc::new(AtomicBool::new(false)),
            };
            let labels: Vec<String> = urls.iter().map(|u| format!("{}:{}", u.host, u.port)).collect();
            crate::failover::spawn_prober("user", user_link.clone(), src_stop.clone(), move |e| labels[e].clone(), move |e| pool::probe(&probe_login, e, crate::failover::opts().probe_timeout));
        }
        pool::spawn(l, ev_tx.clone(), user_rx, stats.clone());
    }
    // the fee login on connections of its own: [0] follows the user's pool (Aria: one login per connection; the fallback when
    // the user's pool refuses a second login in its session), [1] the public fee pools (solo, or the user mines through an ffgn
    // gate, whose node pays the gate owner's lock) (fee.rs)
    let sep_ok = [Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false))];
    let fee_enabled2 = [fee_enabled.clone(), Arc::new(AtomicBool::new(false))];
    let mut fee_txs: Vec<mpsc::Sender<Find>> = Vec::new();
    if let Some(fc) = &fee_cfg {
        for which in 0..2 {
            let (list, kinds, follow) = if which == 0 {
                if solo {
                    continue;
                }
                (o.pools.clone(), user_kinds.clone(), Some(user_link.clone()))
            } else {
                (fc.endpoints.clone(), Arc::new(Mutex::new(fc.endpoints.iter().map(|u| pool::kind_of(u)).collect::<Vec<_>>())), None)
            };
            let urls: Vec<_> = list.iter().filter_map(|u| pool::parse_url(u).ok()).collect();
            if urls.is_empty() || urls.len() != list.len() {
                continue;
            }
            let (tx, rx) = mpsc::channel::<Find>();
            fee_txs.push(tx);
            pool::spawn(
                pool::Login {
                    name: if which == 0 { "fee" } else { "fee-pool" },
                    entry: 0,
                    url: urls[0].clone(),
                    urls: urls.clone(),
                    link: crate::failover::Link::new(urls.len()),
                    follow,
                    src: Src::Fee,
                    login: fc.login.clone(),
                    password: fc.password.clone(),
                    fee_login: None,
                    enabled: fee_enabled2[which].clone(),
                    stop: src_stop.clone(),
                    dataset_hash: file.hash,
                    fee_ok: sep_ok[which].clone(),
                    conn_seq: Arc::new(AtomicU64::new((1 << 32) * (which as u64 + 1))),
                    fee_stats: fee_stats.clone(),
                    kinds,
                    ns: Arc::new(node::NodeStats::default()),
                    ready: Arc::new(AtomicBool::new(false)),
                    gate: Arc::new(AtomicBool::new(false)),
                },
                ev_tx.clone(),
                rx,
                fee_stats.clone(),
            );
        }
    }
    let fee_tx: Option<()> = (!fee_txs.is_empty()).then_some(());
    if solo && o.node.len() > 1 {
        let addrs = o.node.clone();
        let labels = addrs.clone();
        crate::failover::spawn_prober("node", user_link.clone(), src_stop.clone(), move |e| labels[e].clone(), move |e| node::probe(&addrs[e], Duration::from_secs(20)));
    }
    drop(ev_tx);
    let mut fallback = solo;
    let mut separate = solo;
    let mut devfee = fee_cfg.as_ref().map(|_| crate::devfee::DevFee::new(crate::devfee::Params::from_env(), Instant::now()));
    match &fee_cfg {
        Some(f) => println!("devfee: {} % ({} s in every {} min){}", crate::devfee::FEE_PERCENT, crate::devfee::Params::from_env().slot.as_secs(), crate::devfee::Params::from_env().cycle.as_secs() / 60, if solo { format!(", solo: mined for the fee address on {}", f.endpoints.join(",")) } else { ", pool: the fee login on the same pool".into() }),
        None => println!("devfee: NOT ACTIVE in this build (development build without the fee address — never release it)"),
    }

    let (find_tx, find_rx) = mpsc::channel::<Find>();
    let sh = Arc::new(Shared {
        stop: stop.clone(),
        cur: Mutex::new(None),
        finds: Mutex::new(find_tx),
        file: file.clone(),
        raw: raw.clone(),
        hits: AtomicU64::new(0),
        shares: AtomicU64::new(0),
        blocks: AtomicU64::new(0),
        stale: AtomicU64::new(0),
        audits: AtomicU64::new(0),
        easy_warned: AtomicU64::new(0),
        verbose_mem: AtomicU64::new(0),
        en2: AtomicU64::new(0),
        iso,
        open: if iso { Arc::new(iso::open) } else { Arc::new(gpu::open) },
        crashes: Mutex::new(Vec::new()),
        retry: retry_wait(iso),
    });
    let watch = if cuda { crate::faults::Watch::new(&devices.iter().enumerate().map(|(p, &d)| (d, buses[p].clone(), gpu::device_name(d))).collect::<Vec<_>>()) } else { None };
    let mut gstats = Vec::new();
    let mut handles = Vec::new();
    crate::worker::GPUS.store(devices.len(), Ordering::Relaxed);
    if crate::worker::ramp_ms() > 0 {
        println!("soft ramp: {} GPUs come back to full load {} ms apart after a pause; a new block's model {} ms apart per GPU", devices.len(), crate::worker::ramp_ms(), crate::worker::model_jitter(1).as_millis());
    }
    for (pos, &dev) in devices.iter().enumerate() {
        let gs = Arc::new(GpuStat {
            dev,
            name: gpu::device_name(dev),
            bus: buses[pos].clone(),
            attempts: flags[pos].attempts.clone(),
            checked: flags[pos].checked.clone(),
            load_key: flags[pos].load_key.clone(),
            screened: AtomicU64::new(0),
            alive: flags[pos].alive.clone(),
            bad: flags[pos].bad.clone(),
            compute_err: AtomicU64::new(0),
            err: Mutex::new(None),
            pause: flags[pos].pause.clone(),
            paused: flags[pos].paused.clone(),
            kernel: Mutex::new(String::new()),
            kep1: Mutex::new(None),
            pos,
            gap_ms: AtomicU64::new(0),
            back_at: AtomicU64::new(0),
            ctx: Mutex::new(None),
            crashes: flags[pos].crashes.clone(),
            opens: AtomicU64::new(0),
            idle_reason: Mutex::new(None),
        });
        let (sh2, gs2) = (sh.clone(), gs.clone());
        handles.push(std::thread::Builder::new().name(format!("neb-gpu{dev}")).spawn(move || worker(dev, sh2, gs2))?);
        if cuda {
            let i = gpu::info(dev)?;
            println!("GPU {dev}: {} (bus {}, sm_{}{}, {} SMs{})", gs.name, crate::uv::short_bus(&gs.bus), i.cc_major, i.cc_minor, i.sm_count, if i.tensor_int8 != 0 { ", int8 tensor cores" } else { "" });
        }
        gstats.push(gs);
    }
    if cuda {
        crate::templimit::start(&o.temp, &devices.iter().enumerate().map(|(pos, &d)| (d, buses[pos].clone(), gpu::device_name(d), pos)).collect::<Vec<_>>(), &stop);
    }
    if gstats.len() >= 2 {
        let (g2, sh2, t0) = (gstats.clone(), sh.clone(), Instant::now());
        let _ = std::thread::Builder::new().name("neb-stall".into()).spawn(move || stall_watch(&g2, &sh2, t0));
    }
    let nvml = if cuda { crate::nvml::Nvml::load().ok() } else { None };
    let power: Option<(&crate::nvml::Nvml, Vec<Option<crate::nvml::Dev>>)> = nvml.as_ref().map(|n| (n, buses.iter().map(|b| n.by_bus(b).ok()).collect()));
    let api = o.api.as_ref().map(|a| api::start(a));

    let (start, deadline) = (Instant::now(), o.minutes.map(|m| Duration::from_secs_f64(m * 60.0)));
    let mut ujob: Option<Arc<NJob>> = None;
    let mut fjob: Option<Arc<NJob>> = None;
    let mut published: Option<(usize, bool)> = None;
    let mut last_n = vec![0u64; gstats.len()];
    let mut last_sc = vec![0u64; gstats.len()];
    let mut dead_seen = vec![false; gstats.len()];
    let mut ok_cleared = false;
    let mut win = Instant::now();
    let mut last_parent: Option<[u8; 32]> = None;
    let mut rate = 0.0f64;
    let mut watchdog = crate::failover::Watch::default();
    let mut skew_warned = false;
    let mut last_nm_log = Instant::now() - Duration::from_secs(60);
    let not_mining_api: Mutex<Option<String>> = Mutex::new(None);
    let mut mem_warned = false;
    // 1.0.4: the rate over the last AVG_WINDOW (per GPU and total): the stats line, the API and HiveOS show it next to the
    // 10 s rate, so a change of the block's dataset (slot 0 Kepler / slot 1 TESS) does not look like a regression
    let mut hist: std::collections::VecDeque<(Instant, Vec<u64>)> = std::collections::VecDeque::new();
    while !stop.load(Ordering::Relaxed) && deadline.map_or(true, |d| start.elapsed() < d) {
        if o.blocks.is_some_and(|b| stats.accepted.load(Ordering::Relaxed) >= b) {
            println!("--blocks {}: reached, stopping", o.blocks.unwrap());
            break;
        }
        if let Some(m) = FATAL.get() {
            stop.store(true, Ordering::SeqCst);
            crate::offset::restore_all();
            clocks::restore_all();
            bail!("{m}");
        }
        let im = crate::idle::mode();
        if iso {
            iso_crashes(&sh, &gstats, watch.as_ref(), live.as_ref(), &dir);
        }
        if (0..gstats.len()).any(|i| !dead_seen[i] && !gstats[i].alive.load(Ordering::Relaxed)) {
            std::thread::sleep(Duration::from_millis(1500));
            let mut dead = Vec::new();
            for (i, g) in gstats.iter().enumerate() {
                if !dead_seen[i] && !g.alive.load(Ordering::Relaxed) {
                    dead_seen[i] = true;
                    dead.push((g.dev, g.err.lock().unwrap().clone().unwrap_or_else(|| "stopped".into())));
                }
            }
            let alive_n = gstats.iter().filter(|g| g.alive.load(Ordering::Relaxed)).count();
            let injected = crate::faults::test_fault().map(|f| f.0).filter(|d| dead.iter().any(|x| x.0 == *d));
            let v = crate::faults::attribute(watch.as_ref(), &dead, alive_n, injected, &|d| live.as_ref().and_then(|l| tune::expected_offset(l, d)));
            for (k, x) in v.culprits.iter().filter(|_| !iso) {
                let w = watch.as_ref().unwrap();
                let c = &w.cards[*k];
                // 1.0.10: a tuning step that faulted a moment ago owns the crash (the fault handling had put the last good offset back)
                let off = live.as_ref().and_then(|l| tune::fault_offset(l, c.dev)).or_else(|| w.offset(*k));
                let r = crate::faults::record_crash(&dir, &c.uuid, &c.bus, &c.name, *x, off);
                println!("faults: {} crash {} in a row at offset {} remembered", c.label(), r.n, crate::uv::signed(r.offset));
            }
            for l in &v.lines {
                println!("faults: {l}");
            }
        }
        if !ok_cleared && start.elapsed() >= Duration::from_secs(crate::faults::OK_SECS) {
            ok_cleared = true;
            if let Some(w) = &watch {
                let ok: Vec<String> = w.cards.iter().filter(|c| gstats.iter().any(|g| g.dev == c.dev && g.alive.load(Ordering::Relaxed))).map(|c| c.uuid.clone()).collect();
                crate::faults::mined_ok(&dir, &ok);
            }
        }
        if gstats.iter().all(|g| !g.alive.load(Ordering::Relaxed)) {
            stop.store(true, Ordering::SeqCst);
            crate::offset::restore_all();
            clocks::restore_all();
            println!("faults: {}", crate::faults::restart_self("every GPU worker stopped"));
            bail!("all GPU workers have stopped");
        }
        if let Some(why) = RESTART.get() {
            stop.store(true, Ordering::SeqCst);
            crate::offset::restore_all();
            clocks::restore_all();
            println!("faults: {}", crate::faults::restart_self(why));
            bail!("{why}");
        }
        if let Some(l) = crate::status::summary_due() {
            println!("{l}");
        }
        for (src, ev) in ev_rx.try_iter() {
            match (src, ev) {
                (Src::User, Ev::Job(j)) => {
                    if ujob.is_none() && last_parent == Some(j.job.parent) {
                        println!("job again (height {}): mining resumed", j.job.height);
                    }
                    if last_parent != Some(j.job.parent) {
                        last_parent = Some(j.job.parent);
                        // for --auto-lock-goal profit (profit.rs): the network data of the job
                        println!("auto-lock: network neb height {} work {:.6e}", j.job.height, proto::expected(&j.job.target));
                        println!(
                            "new job: height {} slot {} ({}) model {}… target {}… (~{:.2e} attempts per block{})",
                            j.job.height,
                            j.slot,
                            j.model.arch.name(),
                            &hex(&j.model.hash)[..12],
                            &hex(&{
                                let mut t = j.job.target;
                                t.reverse();
                                t
                            })[..16],
                            proto::expected(&j.job.target),
                            j.pool_target.map_or(String::new(), |t| format!(", pool share ~{:.2e}", proto::expected(&t)))
                        );
                    }
                    ujob = Some(j);
                }
                (Src::Fee, Ev::Job(j)) => fjob = Some(j),
                (Src::User, Ev::Down) => {
                    if ujob.take().is_some() {
                        println!("{}: disconnected: mining paused", if solo { "node" } else { "pool" });
                    }
                }
                (Src::Fee, Ev::Down) => fjob = None,
            }
        }
        // expired jobs are not mined (spec/node.md 7.1 expiry)
        // F-03: the lifetime of a job is counted from its arrival, not by the rig's clock (a clock minutes off must not stop
        // the mining); a clock far off is reported once
        if ujob.as_ref().is_some_and(|j| j.expired()) {
            println!("job of height {} expired without a new one: mining paused", ujob.as_ref().unwrap().job.height);
            ujob = None;
        }
        if fjob.as_ref().is_some_and(|j| j.expired()) {
            fjob = None;
        }
        if !skew_warned {
            if let Some(j) = &ujob {
                let skew = now_unix() as i64 - j.job.time_start as i64;
                if skew.abs() > 120 {
                    skew_warned = true;
                    println!("WARNING: this computer's clock differs from the {}'s time by {skew} s: set the time (NTP); mining goes on", if solo { "node" } else { "pool" });
                }
            }
        }
        if let Some(l) = &live {
            tune::tick(l);
        }
        if let Some((nv, f)) = &fixed {
            tune::fixed_tick(nv, f);
        }
        // dev fee
        let mut fee_active = false;
        if let Some(df) = devfee.as_mut() {
            let n = Instant::now();
            // the fee login in the user's own session: a JSON pool that accepted it (neb-pool/1, Suprnova); else its own connection
            let entry_work = !solo && user_link.entry().is_some_and(|e| matches!(user_kinds.lock().unwrap().get(e), Some(pool::Kind::Work | pool::Kind::TryWork)));
            // through an ffgn gate the rig mines solo for the gate's node: the fee goes to the public fee pools, as in solo
            let public = solo || user_gate.load(Ordering::Relaxed);
            // 1.0.3: always a connection of its own (see fee_login above); `fallback` / `entry_work` are kept for the log only
            let _ = (fallback, entry_work);
            separate = true;
            let k = if public { 1 } else { 0 };
            fee_enabled2[k].store(separate && df.conn_wanted(n), Ordering::Relaxed);
            fee_enabled2[1 - k].store(false, Ordering::Relaxed);
            let ready = if separate { fjob.is_some() && sep_ok[k].load(Ordering::Relaxed) } else { ujob.is_some() && fee_ok.load(Ordering::Relaxed) };
            let ch = df.tick(n, ready);
            if !separate && fee_tx.is_some() && matches!(ch, crate::devfee::Change::Skipped | crate::devfee::Change::Aborted) {
                fallback = true;
                println!("devfee: the fee login in the user's pool session is not usable: the next slots use a connection of its own (fallback) for the rest of the run");
            }
            match ch {
                crate::devfee::Change::Started => println!("devfee: slot started ({} s)", crate::devfee::Params::from_env().slot.as_secs()),
                crate::devfee::Change::Finished => println!("devfee: slot finished, back to the user"),
                crate::devfee::Change::Skipped => println!("devfee: slot skipped (the fee {} was not usable)", if separate { "connection" } else { "login" }),
                crate::devfee::Change::Aborted => println!("devfee: slot aborted (the fee login stopped working)"),
                _ => {}
            }
            fee_active = df.active();
        }
        // route checked nonces to their source
        for f in find_rx.try_iter() {
            route(f, &user_tx, &fee_txs);
        }
        let paused = im == crate::idle::Mode::Pause;
        let want: Option<(Arc<NJob>, bool)> = if paused {
            None
        } else if fee_active {
            if separate { fjob.clone().map(|j| (j, false)) } else { ujob.clone().map(|j| (j, true)) }
        } else {
            ujob.clone().map(|j| (j, false))
        };
        let key = want.as_ref().map(|(j, f)| (Arc::as_ptr(j) as usize, *f));
        let exhausted = sh.cur.lock().unwrap().as_ref().is_some_and(|w| w.exhausted());
        if key != published || (exhausted && want.as_ref().is_some_and(|(j, _)| j.en2_size > 0)) {
            // a new extranonce2 for every new header, never reset: a pool may resend a job with a new id but the same content
            // (Suprnova), and the same header again would give the same shares ("duplicate share")
            let en2 = sh.en2.fetch_add(1, Ordering::Relaxed) + 1;
            let new_job = key != published;
            published = key;
            *sh.cur.lock().unwrap() = want.map(|(j, f)| Arc::new(Work::new(j, f, en2)));
            if new_job {
                // a running scan of the old work returns at once (its nonces up to there are still checked and sent)
                for g in &gstats {
                    // F-17: under the lock the worker cannot close the context meanwhile
                    if let Some(c) = g.ctx.lock().unwrap().as_ref() {
                        c();
                    }
                }
            }
        }
        let el = win.elapsed();
        if el >= Duration::from_secs(10) {
            let mut parts = Vec::new();
            let (mut tot, mut tot_w, mut all_w, mut tot_sc) = (0.0, 0.0, true, 0.0);
            for (i, g) in gstats.iter().enumerate() {
                let n = g.attempts.load(Ordering::Relaxed);
                let r = (n - last_n[i]) as f64 / el.as_secs_f64();
                last_n[i] = n;
                tot += r;
                let sc = g.screened.load(Ordering::Relaxed);
                tot_sc += (sc - last_sc[i]) as f64 / el.as_secs_f64();
                last_sc[i] = sc;
                let w = power.as_ref().and_then(|(nv, d)| d[i].and_then(|h| nv.power_w(h)));
                let pw = match w {
                    Some(w) if w > 0.0 => {
                        tot_w += w;
                        format!(" ({:.0} W, {:.4} MA/W)", w, r / 1e6 / w)
                    }
                    _ => {
                        all_w = false;
                        String::new()
                    }
                };
                parts.push(format!("GPU{} {:.3}{}{}{}{}", g.dev, r / 1e6, pw, if g.alive.load(Ordering::Relaxed) { "" } else { " DEAD" }, crate::status::tag(g.dev), crate::templimit::tag(g.dev)));
            }
            rate = tot;
            let now_n: Vec<u64> = last_n.clone();
            hist.push_back((Instant::now(), now_n.clone()));
            while hist.len() > 2 && hist.front().is_some_and(|(t, _)| t.elapsed() > AVG_WINDOW) {
                hist.pop_front();
            }
            // the average over what the window holds (from the first 10 s on), None in the first window
            let avg: Option<(Vec<f64>, f64)> = match hist.front() {
                Some((t0, n0)) if hist.len() >= 2 && t0.elapsed() >= Duration::from_secs(5) => {
                    let secs = t0.elapsed().as_secs_f64();
                    let v: Vec<f64> = now_n.iter().zip(n0).map(|(a, b)| a.saturating_sub(*b) as f64 / secs).collect();
                    let t = v.iter().sum();
                    Some((v, t))
                }
                _ => None,
            };
            if !mem_warned && start.elapsed() > Duration::from_secs(60) && tot > 0.0 {
                mem_warned = true;
                if let Some((nv, d)) = power.as_ref() {
                    for (i, g) in gstats.iter().enumerate() {
                        let Some(h) = d[i] else { continue };
                        if let (Some(cur), Some(max)) = (nv.mem_clock_mhz(h), nv.max_mem_clock_mhz(h)) {
                            // 1.0.4: the OC profile's memory lock stayed (the driver refused ffgn's lock at the maximum): loud, on every line
                            if let Some(m) = clocks::hive_mem_kept(&g.bus).filter(|&m| clocks::mem_at_lock(cur, m, max)) {
                                clocks::mark_mem_stuck(&g.bus, g.dev, m);
                            } else if (cur as f64) < max as f64 * 0.6 && !clocks::table_low(g.dev) {
                                // 1.0.11: not when the low clock is our own --table-mclock lock of the pure table path (live
                                // GPU1 Shahtyor: the warning fired 60 s after the start on the 810 MHz table lock)
                                println!("WARNING GPU{} (bus {}): the memory runs at {cur} MHz of {max} MHz: NEB is memory bound and mines several times slower; remove the memory underclock (OC profile / Afterburner) or give --lock-mclock {max}", g.dev, g.bus);
                            }
                        }
                    }
                }
            }
            let pp = if all_w && tot_w > 0.0 { format!(", {:.0} W, {:.4} MA/W", tot_w, tot / 1e6 / tot_w) } else { String::new() };
            // 1.0.9: the nonces screened by the engine's cheap filter / digest table (not a hashrate: MA/s above counts the
            // evaluated attempts, each with the same chance of a share)
            let pp = if tot_sc > tot * 1.01 { format!("{pp} (screened {:.3} G/s)", tot_sc / 1e9) } else { pp };
            // 1.0.11: a crash of a GPU's engine process counts as before (one process: the retry of a worker counted in `bad`)
            let bad: u64 = gstats.iter().map(|g| g.bad.load(Ordering::Relaxed) + g.compute_err.load(Ordering::Relaxed) + g.crashes.load(Ordering::Relaxed)).sum();
            let ld = |a: &AtomicU64| a.load(Ordering::Relaxed);
            let src_part = if solo {
                format!("shares {} | blocks sub {} acc {} rej {} | fee sub {} acc {} rej {}", ld(&ns.shares_sent), ld(&stats.submitted), ld(&stats.accepted), ld(&stats.rejected), ld(&fee_stats.submitted), ld(&fee_stats.accepted), ld(&fee_stats.rejected))
            } else {
                format!("user sub {} acc {} rej {} (stale {}) | fee sub {} acc {} rej {}", ld(&stats.submitted), ld(&stats.accepted), ld(&stats.rejected), ld(&stats.stale_rej), ld(&fee_stats.submitted), ld(&fee_stats.accepted), ld(&fee_stats.rejected))
            };
            if !solo {
                // accepted-shares watchdog of the user's pool session (--noshare-minutes / --reject-minutes, failover.rs)
                let hps = ujob.as_ref().and_then(|j| j.pool_target).map(|t| proto::expected(&t));
                let allowed = !fee_active && im == crate::idle::Mode::Full && stats.connected.load(Ordering::Relaxed) != 0;
                crate::failover::watch_tick(&mut watchdog, &user_link, "user", el.as_secs_f64(), allowed, tot, hps, ld(&stats.accepted), ld(&stats.rejected), ld(&stats.submitted));
            }
            // 1.0.1: a job and no attempts = a loud error (in the log, the stats line, HiveOS and the API), never silence
            model_gap_line(&gstats);
            let reasons: Vec<String> = gstats.iter().filter_map(|g| g.idle_reason.lock().unwrap().clone().map(|r| format!("GPU{}: {r}", g.dev))).collect();
            let not_mining: Option<String> = if ujob.is_some() && tot <= 0.0 && start.elapsed() > Duration::from_secs(45) && im != crate::idle::Mode::Pause {
                Some(reasons.first().cloned().unwrap_or_else(|| "a job but no attempts (GPU busy / failed?)".into()))
            } else {
                None
            };
            if let Some(r) = &not_mining {
                if last_nm_log.elapsed() > Duration::from_secs(30) {
                    last_nm_log = Instant::now();
                    eprintln!("ERROR: NOT MINING: {r}");
                }
            }
            *not_mining_api.lock().unwrap() = not_mining.clone();
            let eta = ujob.as_ref().filter(|_| tot > 0.0).map(|j| proto::expected(&j.job.target) / tot / 60.0);
            // 1.0.3: a core lock that does not hold stays visible on every stats line (HiveOS h-stats shows it next to the version)
            let unheld: Vec<String> = gstats.iter().filter(|g| crate::clocks::not_held(&g.bus).is_some()).map(|g| format!("GPU{}", g.dev)).collect();
            // 1.0.4: the OC profile's memory lock that stayed (one warning per lock value, the GPUs listed)
            let mut stuck: Vec<(u32, Vec<String>)> = Vec::new();
            for g in &gstats {
                if let Some(m) = clocks::mem_stuck(&g.bus) {
                    match stuck.iter_mut().find(|x| x.0 == m) {
                        Some(x) => x.1.push(format!("GPU{}", g.dev)),
                        None => stuck.push((m, vec![format!("GPU{}", g.dev)])),
                    }
                }
            }
            let mem_part: String = stuck.iter().map(|(m, gs)| format!("  [WARNING {}: {}]", gs.join(","), clocks::mem_stuck_message(*m))).collect();
            let slot = ujob.as_ref().map(|j| (j.slot, refc::slot_name(sh.file.network, j.slot, j.model.arch)));
            let avg_part = match &avg {
                Some((v, t)) => format!(" | avg {} min {:.3} MA/s ({})", AVG_WINDOW.as_secs() / 60, t / 1e6, v.iter().map(|x| format!("{:.3}", x / 1e6)).collect::<Vec<_>>().join(", ")),
                None => String::new(),
            };
            println!(
                "[{:>4}s] {} | total {:.3} MA/s{}{}{} | found {} (bad {}) | {}{}{}{}{}{}",
                start.elapsed().as_secs(),
                parts.join("  "),
                tot / 1e6,
                pp,
                avg_part,
                slot.map_or(String::new(), |(n, name)| format!(" | slot {n} {name}")),
                ld(&sh.hits),
                bad,
                src_part,
                eta.map_or(String::new(), |m| format!(" | ~{} per block at this rate", fmt_minutes(m))),
                if stats.connected.load(Ordering::Relaxed) == 0 { if solo { "  [NODE DOWN]" } else { "  [POOL DOWN]" } } else { "" },
                match im {
                    crate::idle::Mode::Pause => "  [PAUSED: PC in use]".to_string(),
                    crate::idle::Mode::Light(p) => format!("  [LIGHT {p} %: PC in use]"),
                    _ if fee_active => "  [DEVFEE]".to_string(),
                    _ if ujob.is_none() => "  [NO JOB]".to_string(),
                    _ if not_mining.is_some() => format!("  [NOT MINING: {}]", not_mining.as_deref().unwrap_or("")),
                    _ => String::new(),
                },
                if unheld.is_empty() { String::new() } else { format!("  [LOCK NOT HELD: {}]", unheld.join(",")) }
                    + &tune::tags().iter().map(|w| format!("  [AUTO-UV {w}]")).collect::<String>(),
                mem_part
            );
            if let Some(a) = &api {
                api::publish(
                    a,
                    api::Snapshot {
                        rate,
                        rate_avg: avg.as_ref().map(|a| a.1),
                        avg_secs: AVG_WINDOW.as_secs(),
                        slot: slot.map(|(n, name)| (n, name.to_string())),
                        not_mining: not_mining_api.lock().unwrap().clone(),
                        uptime: start.elapsed().as_secs(),
                        solo,
                        source: if solo { o.node.join(",") } else { o.pools.join(",") },
                        height: ujob.as_ref().map(|j| j.job.height),
                        shares: if solo { ld(&ns.shares_sent) } else { ld(&stats.accepted) },
                        rejected: ld(&stats.rejected),
                        // D-019: in pool mode the session counters are shares; blocks = solutions this miner found
                        blocks_sub: if solo { ld(&stats.submitted) } else { ld(&sh.blocks) },
                        blocks_acc: if solo { ld(&stats.accepted) } else { 0 },
                        bad,
                        gpus: gstats.iter().enumerate().map(|(i, g)| api::Gpu { dev: g.dev, name: g.name.clone(), bus: g.bus.clone(), attempts: g.attempts.load(Ordering::Relaxed), screened: g.screened.load(Ordering::Relaxed), alive: g.alive.load(Ordering::Relaxed), bad: g.bad.load(Ordering::Relaxed) + g.crashes.load(Ordering::Relaxed), power: power.as_ref().and_then(|(nv, d)| d[i].and_then(|h| nv.power_w(h))), lock_not_held: crate::clocks::not_held(&g.bus), rate_avg: avg.as_ref().and_then(|a| a.0.get(i).copied()), mem_stuck: clocks::mem_stuck(&g.bus), kep1_kernel: g.kep1.lock().unwrap().clone(), model_gap_ms: Some(g.gap_ms.load(Ordering::Relaxed)).filter(|&x| x > 0) }).collect(),
                    },
                );
            }
            win = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    stop.store(true, Ordering::SeqCst);
    crate::offset::restore_all();
    clocks::restore_all();
    for h in handles {
        let _ = h.join();
    }
    // the last nonces of the workers go out before the sessions close
    let mut last = 0;
    for f in find_rx.try_iter() {
        last += 1;
        route(f, &user_tx, &fee_txs);
    }
    std::thread::sleep(Duration::from_millis(if last > 0 { 1500 } else { 300 }));
    src_stop.store(true, Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(300));
    let n: u64 = gstats.iter().map(|g| g.attempts.load(Ordering::Relaxed)).sum();
    println!(
        "done: {} attempts in {:?} ({} average); found {} (bad {}), audits {}; {}",
        n,
        start.elapsed(),
        fmt_rate(n as f64 / start.elapsed().as_secs_f64()),
        sh.hits.load(Ordering::Relaxed),
        gstats.iter().map(|g| g.bad.load(Ordering::Relaxed) + g.crashes.load(Ordering::Relaxed)).sum::<u64>(),
        sh.audits.load(Ordering::Relaxed),
        if solo {
            format!("shares sent {}, blocks submitted {} accepted {} rejected {}", ns.shares_sent.load(Ordering::Relaxed), stats.submitted.load(Ordering::Relaxed), stats.accepted.load(Ordering::Relaxed), stats.rejected.load(Ordering::Relaxed))
        } else {
            format!("pool shares submitted {} accepted {} rejected {}; dev fee {} / {}", stats.submitted.load(Ordering::Relaxed), stats.accepted.load(Ordering::Relaxed), stats.rejected.load(Ordering::Relaxed), fee_stats.submitted.load(Ordering::Relaxed), fee_stats.accepted.load(Ordering::Relaxed))
        }
    );
    let _ = sh.stale.load(Ordering::Relaxed);
    Ok(())
}

/// `--gpu-state`: offset, clocks and power of every GPU (NVML), nothing changed.
pub fn gpu_state(o: &Opts) -> Result<()> {
    gpu::load(o.backend, 1)?;
    let nv = crate::nvml::Nvml::load()?;
    for dev in 0..gpu::device_count() as i32 {
        let Ok(bus) = gpu::device_pci_bus_id(dev) else { continue };
        let Ok(h) = nv.by_bus(&bus) else { continue };
        let off = nv.probe_offset(h).map(|(_, cur, _, _)| crate::uv::signed(cur)).unwrap_or_else(|e| format!("? ({e:#})"));
        println!("gpu-state: GPU{dev} bus {bus} offset {off} core {} MHz mem {} MHz {:.0} W", nv.clock_mhz(h).unwrap_or(0), nv.mem_clock_mhz(h).unwrap_or(0), nv.power_w(h).unwrap_or(0.0));
    }
    Ok(())
}

/// `--pool-check`: the failback check (connect + subscribe + authorize + a usable job) of every `--pool` entry, nothing submitted.
pub fn pool_check(o: &Opts) -> Result<()> {
    ensure!(!o.pools.is_empty(), "--pool-check needs --pool");
    let (file, _) = load_dataset(o.dataset.as_deref(), o.allow_other_dataset)?;
    let urls: Vec<_> = o.pools.iter().map(|u| pool::parse_url(u).map_err(anyhow::Error::msg)).collect::<Result<_>>()?;
    let login = if o.worker.is_empty() || o.wallet.contains('.') { o.wallet.clone() } else { format!("{}.{}", o.wallet, o.worker) };
    let l = pool::Login {
        name: "check",
        url: urls[0].clone(),
        urls: urls.clone(),
        link: crate::failover::Link::new(1),
        follow: None,
        src: Src::User,
        login,
        password: o.password.clone(),
        fee_login: None,
        enabled: Arc::new(AtomicBool::new(true)),
        stop: Arc::new(AtomicBool::new(false)),
        dataset_hash: file.hash,
        fee_ok: Arc::new(AtomicBool::new(false)),
        conn_seq: Arc::new(AtomicU64::new(0)),
        fee_stats: Arc::new(Stats::default()),
        kinds: Arc::new(Mutex::new(o.pools.iter().map(|u| pool::kind_of(u)).collect())),
        ns: Arc::new(node::NodeStats::default()),
        ready: Arc::new(AtomicBool::new(false)),
        entry: 0,
        gate: Arc::new(AtomicBool::new(false)),
    };
    let mut bad = 0;
    for (i, u) in urls.iter().enumerate() {
        // 1.0.3: the job must be of this dataset file's network (Aria answered with test network jobs on 18.11.2026: "OK" before)
        let check = |j: &NJob| refc::resolve_dataset(&file.sets, j.slot, j.model.arch).map(|_| ()).map_err(|e| format!("WRONG NETWORK: job height {} slot {} ({}): {e}", j.job.height, j.slot, j.model.arch.name()));
        match pool::probe_job(&l, i, crate::failover::opts().probe_timeout, check) {
            Ok(()) => println!("pool-check: {} of {}: {}:{} OK (subscribe, authorize, a usable job of the {} network)", i + 1, urls.len(), u.host, u.port, file.network),
            Err(e) => {
                bad += 1;
                println!("pool-check: {} of {}: {}:{} FAILED: {e}", i + 1, urls.len(), u.host, u.port);
            }
        }
    }
    ensure!(bad == 0, "{bad} of {} pool(s) failed the check", urls.len());
    Ok(())
}

/// A checked nonce to the session of its job (fee finds to every fee session: each keeps those of its own connection).
fn route(f: Find, user: &mpsc::Sender<Find>, fee: &[mpsc::Sender<Find>]) {
    match f.job.src {
        Src::User => {
            let _ = user.send(f);
        }
        Src::Fee => {
            let f = Arc::new(f);
            for t in fee {
                let _ = t.send(Find { job: f.job.clone(), extranonce: f.extranonce.clone(), time: f.time, nonce: f.nonce, ticket: f.ticket, share: f.share, block: f.block, fee: f.fee });
            }
        }
    }
}

pub fn fmt_rate(r: f64) -> String {
    if r >= 1e6 {
        format!("{:.3} MA/s", r / 1e6)
    } else if r >= 1e3 {
        format!("{:.2} kA/s", r / 1e3)
    } else {
        format!("{:.1} A/s", r)
    }
}

fn fmt_minutes(m: f64) -> String {
    if m >= 60.0 * 48.0 {
        format!("{:.1} d", m / 1440.0)
    } else if m >= 120.0 {
        format!("{:.1} h", m / 60.0)
    } else {
        format!("{m:.0} min")
    }
}

pub mod api;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neb_pool_by_positive_signs_only() {
        for u in ["stratum+tcp://neb.suprnova.cc:7804", "neb.suprnova.cc:7805", "work://neb.ariabrain.com:3370", "neb.ariabrain.com:3371", "aria://1.2.3.4:3370", "neb://10.0.0.5:3442"] {
            assert!(looks_like_neb_pool(u), "{u}");
        }
        for u in ["pearl-eu2.luckypool.io:3360", "stratum+ssl://pool.proximamine.com:3334", "stratum+tcp://nebula.example:1", "10.0.0.5:3442", "cmfd+tls://1.2.3.4:5?pin=ab", "stratum+tcp://prl.suprnova.cc:7000"] {
            assert!(!looks_like_neb_pool(u), "{u}");
        }
    }

    #[test]
    fn stall_cause_names_a_cuda_fault_when_workers_errored() {
        // CRASH-12gpu: 12 workers with an engine error around the stall = one GPU's CUDA fault, not a bus / power event
        let c = super::stall_cause(12, 12, false);
        assert!(c.contains("CUDA fault on one GPU") && c.contains("12 of 12") && !c.starts_with("likely"));
        assert!(super::stall_cause(0, 12, false).starts_with("likely a PCIe bus / power event"));
    }
    #[test]
    fn kep1_names_ignore_the_path() {
        // 1.0.9: engine 0.6.0 appends the slot's path; a new k / table alone is not a kernel change
        let a = "warp-mma (auto: warp-mma 37.21, dp4a 16.28 M/s) | cheap k=6 (keep 50.0%; speed k6 40.94 plain 41.89 M/s)";
        let b = "warp-mma (auto: warp-mma 36.90, dp4a 16.20 M/s) | digest table L<=2 (P 1.56%, 592 MB, built 922 ms)";
        assert_eq!(kep1_names(a), "warp-mma");
        assert_eq!(kep1_names(a), kep1_names(b));
        let m = "mixed per slot: 0 dp4a (x) | cheap k=4 (y); 1 warp-mma (z) | plain (w)";
        assert_eq!(kep1_names(m), "mixed per slot: 0 dp4a; 1 warp-mma");
    }

    #[test]
    fn call_size_follows_the_path() {
        assert_eq!(call_size(None), (CALL_SECS, 1024, 1 << 24));
        assert_eq!(call_size(Some(gpu::Path::Plain)), call_size(None));
        let (t, lo, hi) = call_size(Some(gpu::Path::Table));
        assert!(t <= 0.1 && lo >= 1 << 26 && hi > lo);
        assert!(call_size(Some(gpu::Path::Cheap)).2 < hi);
    }

    use super::*;

    /// 1.0.11 (iso.rs): a device whose engine process dies after a few scans (the real Remote path: the "process" ends,
    /// its socket closes, the call fails with the crash text)
    struct Dying {
        inner: Box<dyn gpu::Scanner>,
        left: Option<u32>,
    }

    impl gpu::Scanner for Dying {
        fn kernel(&self) -> String {
            self.inner.kernel()
        }
        fn set_model(&mut self, ds: usize, m: &Model) -> Result<()> {
            self.inner.set_model(ds, m)
        }
        fn scan(&mut self, ds: usize, header: &[u8; 80], start: u32, count: u32, target: &[u8; 32], bound: &[u8; 32]) -> Result<gpu::ScanOut> {
            if let Some(l) = self.left.as_mut() {
                if *l == 0 {
                    self.left = None;
                    let _ = self.inner.inject_fault("exit");
                } else {
                    *l -= 1;
                }
            }
            self.inner.scan(ds, header, start, count, target, bound)
        }
        fn eval_keys(&mut self, ds: usize, keys: &[[u8; 32]]) -> Result<Option<Vec<[u8; 32]>>> {
            self.inner.eval_keys(ds, keys)
        }
        fn selftest(&mut self) -> Result<String> {
            self.inner.selftest()
        }
        fn bench(&mut self, ds: i32, arch: u32, ms: u32) -> Result<f64> {
            self.inner.bench(ds, arch, ms)
        }
        fn canceller(&self) -> Option<gpu::Canceller> {
            self.inner.canceller()
        }
    }

    fn test_gpu(dev: i32, pos: usize) -> Arc<GpuStat> {
        Arc::new(GpuStat {
            dev,
            name: "CPU reference thread".into(),
            bus: format!("cpu:{dev}"),
            attempts: Default::default(),
            checked: Default::default(),
            load_key: Default::default(),
            screened: AtomicU64::new(0),
            alive: Arc::new(AtomicBool::new(true)),
            bad: Default::default(),
            compute_err: AtomicU64::new(0),
            err: Mutex::new(None),
            pause: Default::default(),
            paused: Default::default(),
            kernel: Mutex::new(String::new()),
            kep1: Mutex::new(None),
            pos,
            gap_ms: AtomicU64::new(0),
            back_at: AtomicU64::new(0),
            ctx: Mutex::new(None),
            crashes: Default::default(),
            opens: AtomicU64::new(0),
            idle_reason: Mutex::new(None),
        })
    }

    #[test]
    fn a_dying_engine_process_restarts_only_its_gpu_and_every_share_is_sent_once() {
        gpu::load(Some(gpu::Backend::Cpu), 1).unwrap();
        let (set, m) = refc::synthetic(refc::Arch::Kep1).unwrap();
        let file = Arc::new(DatasetFile { hash: [0; 32], bytes: Arc::new(Vec::new()), network: "test", sets: vec![Arc::new(set)] });
        let opened = Arc::new(Mutex::new(Vec::<i32>::new()));
        let op2 = opened.clone();
        // GPU0's first engine process dies after 3 scans; its second one and GPU1's never
        let open: Opener = Arc::new(move |dev, f: &Arc<DatasetFile>, _raw: &[u8]| {
            let first = {
                let mut o = op2.lock().unwrap();
                o.push(dev);
                o.iter().filter(|&&d| d == dev).count() == 1
            };
            let inner = iso::open_thread(dev, f)?;
            Ok(Box::new(Dying { inner, left: (dev == 0 && first).then_some(3) }) as Box<dyn gpu::Scanner>)
        });
        let (find_tx, find_rx) = mpsc::channel::<Find>();
        let sh = Arc::new(Shared {
            stop: Arc::new(AtomicBool::new(false)),
            cur: Mutex::new(None),
            finds: Mutex::new(find_tx),
            file: file.clone(),
            raw: Arc::new(Vec::new()),
            hits: AtomicU64::new(0),
            shares: AtomicU64::new(0),
            blocks: AtomicU64::new(0),
            stale: AtomicU64::new(0),
            audits: AtomicU64::new(0),
            easy_warned: AtomicU64::new(0),
            verbose_mem: AtomicU64::new(0),
            en2: AtomicU64::new(0),
            iso: true,
            open,
            crashes: Mutex::new(Vec::new()),
            retry: Duration::from_millis(200),
        });
        let mut share = [0xffu8; 32];
        share[31] = 0x7f; // about half of the nonces are shares
        let job = proto::Job {
            id: 1,
            clean: true,
            version: 1,
            parent: [3; 32],
            time_start: now_unix(),
            time_window: 600,
            bits: 0,
            target: [0; 32],
            share_target: share,
            coinbase_prefix: vec![1, 2, 3],
            coinbase_suffix: vec![4, 5],
            extranonce_size: 4,
            branch: vec![],
            tx_count: 1,
            shares_root: [0; 32],
            height: 7,
            expiry: now_unix() + 600,
        };
        let nj = Arc::new(NJob { src: Src::User, origin: Origin::Pool { job_id: "1".into() }, job, slot: 0, model: Arc::new(m), pool_target: Some(share), en1: vec![], en2_size: 4, nonce_range: None, conn: 1, received: Instant::now(), ref_next: AtomicU64::new(0), ref_base: 0 });
        *sh.cur.lock().unwrap() = Some(Arc::new(Work::new(nj, false, 1)));
        let gs = [test_gpu(0, 0), test_gpu(1, 1)];
        let hs: Vec<_> = gs
            .iter()
            .map(|g| {
                let (sh, g) = (sh.clone(), g.clone());
                std::thread::spawn(move || worker(g.dev, sh, g))
            })
            .collect();
        let t = Instant::now();
        let att = |g: &GpuStat| g.attempts.load(Ordering::Relaxed);
        // GPU0 crashes, comes back in a new engine process and mines again; GPU1 mines all the time
        let mut gpu1_at_crash = None;
        let mut gpu0_back = None;
        while t.elapsed() < Duration::from_secs(60) {
            if gpu1_at_crash.is_none() && gs[0].crashes.load(Ordering::Relaxed) > 0 {
                gpu1_at_crash = Some(att(&gs[1]));
            }
            if gpu0_back.is_none() && gs[0].opens.load(Ordering::Relaxed) >= 2 {
                gpu0_back = Some(att(&gs[0]));
            }
            if gpu0_back.is_some_and(|a| att(&gs[0]) > a + 20) && gpu1_at_crash.is_some_and(|a| att(&gs[1]) > a + 20) {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        sh.stop.store(true, Ordering::SeqCst);
        for h in hs {
            h.join().unwrap();
        }
        assert_eq!(gs[0].crashes.load(Ordering::Relaxed), 1, "one crash of GPU0's engine process");
        assert_eq!(gs[1].crashes.load(Ordering::Relaxed), 0);
        assert_eq!(gs[0].opens.load(Ordering::Relaxed), 2, "GPU0 restarted once");
        assert_eq!(gs[1].opens.load(Ordering::Relaxed), 1, "GPU1 never restarted");
        assert_eq!(opened.lock().unwrap().iter().filter(|&&d| d == 1).count(), 1);
        assert!(gpu1_at_crash.is_some_and(|a| att(&gs[1]) > a + 20), "GPU1 mined on during GPU0's restart");
        assert!(gpu0_back.is_some_and(|a| att(&gs[0]) > a + 20), "GPU0 mines again in its new engine process");
        assert!(gs.iter().all(|g| g.err.lock().unwrap().is_none()), "no worker gave up");
        // a crash is not a wrong result; every result passed the CPU check
        assert_eq!(gs[0].bad.load(Ordering::Relaxed) + gs[1].bad.load(Ordering::Relaxed), 0);
        {
            let q = sh.crashes.lock().unwrap();
            assert_eq!(q.len(), 1);
            assert!(q[0].dev == 0 && q[0].pos == 0 && q[0].err.contains("engine process ended"), "{}", q[0].err);
        }
        // accounting: every checked share went out once, no nonce twice (the restart did not mine a nonce again)
        let finds: Vec<Find> = find_rx.try_iter().collect();
        assert!(finds.len() > 10, "{}", finds.len());
        assert_eq!(finds.len() as u64, sh.shares.load(Ordering::Relaxed));
        assert_eq!(gs[0].checked.load(Ordering::Relaxed) + gs[1].checked.load(Ordering::Relaxed), finds.len() as u64);
        let mut seen = std::collections::HashSet::new();
        for f in &finds {
            assert!(f.share && !f.fee && !f.block);
            assert!(admits(&share, &f.ticket));
            assert!(seen.insert((f.extranonce.clone(), f.nonce)), "nonce {} sent twice", f.nonce);
        }
    }

    #[test]
    fn the_stall_line_in_one_process_per_gpu_is_not_one_gpus_fault() {
        let c = stall_cause(12, 12, true);
        assert!(c.contains("process of its own") && c.contains("rig-wide") && !c.contains("ends CUDA for every GPU"), "{c}");
        assert!(stall_cause(0, 12, true).starts_with("likely a PCIe bus / power event"));
    }

    #[test]
    fn kep1_line_only_when_the_kernel_changes() {
        let a = "warp-mma per slot: 0 warp-mma (auto: warp-mma 19.33, dp4a 16.30 M/s); 1 dp4a (auto: warp-mma 1.00, dp4a 2.00 M/s)";
        let b = "warp-mma per slot: 0 warp-mma (auto: warp-mma 19.10, dp4a 16.41 M/s); 1 dp4a (auto: warp-mma 1.10, dp4a 2.10 M/s)";
        let c = "warp-mma per slot: 0 warp-mma (auto: warp-mma 19.10, dp4a 16.41 M/s); 1 warp-mma (auto: warp-mma 26.0, dp4a 17.1 M/s)";
        assert_eq!(kep1_names(a), "warp-mma per slot: 0 warp-mma; 1 dp4a");
        assert_eq!(kep1_names(a), kep1_names(b));
        assert_ne!(kep1_names(b), kep1_names(c));
        assert_eq!(kep1_names("dp4a (CC 7.5: no int8 mma)"), "dp4a");
    }

    fn njob(en2_size: usize, range: Option<(u32, u64)>) -> Arc<NJob> {
        let packed = vec![0u8; refc::Arch::Kep1.packed_bytes()];
        let model = Arc::new(Model::from_packed(&packed, &refc::weights_hash(&packed)).unwrap());
        Arc::new(NJob {
            src: Src::User,
            origin: Origin::Node { id: 1 },
            job: proto::Job {
                id: 1,
                clean: true,
                version: 1,
                parent: [1; 32],
                time_start: 1000,
                time_window: 3600,
                bits: 0xd580_0000,
                target: [0xff; 32],
                share_target: [0xff; 32],
                coinbase_prefix: vec![1],
                coinbase_suffix: vec![2],
                extranonce_size: 16,
                branch: vec![],
                tx_count: 1,
                shares_root: [0; 32],
                height: 5,
                expiry: 1300,
            },
            slot: 0,
            model,
            pool_target: None,
            en1: vec![],
            en2_size,
            nonce_range: range,
            conn: 1,
            received: Instant::now(),
            ref_next: AtomicU64::new(0),
            ref_base: random_base(),
        })
    }

    #[test]
    fn f09_the_reference_header_keeps_its_progress_across_new_work() {
        let j = njob(0, None);
        let w1 = Work::new(j.clone(), false, 1);
        let (a, _) = w1.take(1000).unwrap();
        let w2 = Work::new(j.clone(), false, 2); // re-published after a pause / a fee slot
        let (b, _) = w2.take(1000).unwrap();
        assert_eq!(b, a.wrapping_add(1000), "no nonce of the reference header is tried twice");
        assert_eq!(w1.header, w2.header);
    }

    #[test]
    fn f13_two_jobs_start_at_different_nonces() {
        let starts: std::collections::HashSet<u32> = (0..8).map(|_| Work::new(njob(0, None), false, 1).take(1).unwrap().0).collect();
        assert!(starts.len() > 1);
    }

    #[test]
    fn f02_a_gate_range_is_followed_by_block_only_work() {
        let j = njob(16, Some((0x4000_0000, 1 << 10)));
        let w1 = Work::new(j.clone(), false, 1);
        assert!(w1.reference && w1.extranonce.iter().all(|&b| b == 0));
        assert_eq!(w1.take(1 << 20), Some((0x4000_0000, 1 << 10)));
        assert!(w1.exhausted());
        let w2 = Work::new(j.clone(), false, 2);
        assert!(!w2.reference);
        assert!(w2.extranonce.iter().any(|&b| b != 0));
        assert_eq!(w2.bound, j.job.target, "off the reference header only a solution counts");
        assert_ne!(w2.header, w1.header);
    }

    #[test]
    fn f03_a_job_expires_by_its_age_not_by_the_clock() {
        let mut j = Arc::try_unwrap(njob(0, None)).ok().unwrap();
        assert!(!j.expired());
        j.received = Instant::now().checked_sub(Duration::from_secs(301)).unwrap();
        assert!(j.expired(), "expiry - time_start = 300 s after its arrival");
    }
}
