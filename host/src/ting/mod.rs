// FFGT — see LICENSE
//! Ting / Daler (DLR) mining (`--algo ting`, the default of FFGT).  Proof of work: one SHA-256 over the tagged 294-byte header
//! (pow.rs), BlockId big-endian <= target, 16-byte little-endian nonce at offset 278.  GPU search in the closed-source tinggpu
//! library (gpu.rs: CUDA for NVIDIA, OpenCL for AMD), every candidate re-hashed on the CPU before it is submitted.
//! Work source (stage 1): a local Ting node through its control channel (node.rs: `template` / `solution`), solo.  The blocks
//! pay the node's payout account (`ting miner payout ACCOUNT`, set on the node, not here).
//!
//!   ffgt --node 127.0.0.1:18440 --node-dir /var/lib/ting [--devices 0,1] [--lock-cclock 1500] [--auto-uv]
pub mod gpu;
pub mod api;
pub mod cli;
pub mod iso;
pub mod gate;
pub mod node;
pub mod pool;
pub mod pow;
pub mod tune;

use crate::clocks;
use crate::stratum::{Src, Stats};
use anyhow::{Result, bail, ensure};
use crate::devfee;
use node::{Event, Job, Submit};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const ALGO_NAME: &str = "ting-sha256";

/// A Ting stratum pool by its host name (crypto-connection.net); an ffgt-gate on a LAN address needs `--algo ting`.
pub fn looks_like_ting_pool(url: &str) -> bool {
    let u = url.to_ascii_lowercase();
    u.contains("crypto-connection") || u.starts_with("ting://") || u.starts_with("ting+tcp://")
}

/// Module layout identifier: stays in the binary, never read for any purpose.
#[used]
static LAYOUT_TAG: [u8; 25] = *b"tgm-69f30df1e24bac61-bace";

static GROUPING: AtomicBool = AtomicBool::new(true);

/// `--no-group`: no template grouping (one template / job per search, as 0.2.1; for comparisons)
pub fn set_grouping(on: bool) {
    GROUPING.store(on, Ordering::Relaxed);
}

/// Template grouping on (default) unless --no-group or FFGT_NO_GROUP=1.
pub fn grouping() -> bool {
    GROUPING.load(Ordering::Relaxed) && std::env::var("FFGT_NO_GROUP").map_or(true, |v| v.is_empty() || v == "0")
}

/// The largest group the worker of `dev` searches (1 without grouping or with a library that has no group kernels).
pub fn group_max(dev: i32) -> usize {
    if grouping() { gpu::group_size(dev, node::GROUP_KEEP) } else { 1 }
}

pub struct Opts {
    pub node: Option<String>,
    pub node_dir: Option<String>,
    pub node_token_file: Option<String>,
    pub node_remote: bool,
    pub backend: Option<gpu::Backend>,
    pub minutes: Option<f64>,
    pub blocks: Option<u64>, // stop after this many accepted blocks (tests)
    pub devices: Option<Vec<i32>>,
    pub lock_cclock: Option<String>,
    pub lock_mclock: Option<String>,
    pub moffset: Option<String>,
    pub no_mem_lock: bool, // --no-mem-lock: leave the memory clock alone (default 0.2.1: the lowest one that keeps the full core range)
    pub temp: crate::templimit::Opts,
    pub uv: crate::uv::Opts,
    /// pool mode: the --pool entries (empty = solo on the node); login ACCOUNT.worker, password
    pub pools: Vec<String>,
    pub account: String,
    pub worker: String,
    pub password: String,
    pub failback_min: f64,
    /// --single-process: every GPU's engine in this process (default: one engine process per GPU, iso.rs)
    pub single_process: bool,
}

pub fn parse_backend(s: Option<&str>) -> Result<Option<gpu::Backend>> {
    Ok(match s.map(|x| x.to_ascii_lowercase()) {
        None => None,
        Some(x) if x == "cuda" || x == "nvidia" => Some(gpu::Backend::Cuda),
        Some(x) if x == "opencl" || x == "ocl" || x == "amd" => Some(gpu::Backend::OpenCl),
        Some(x) => bail!("--backend {x}: cuda or opencl"),
    })
}

/// Loads the library and checks the devices; the list of usable devices (`devices` or all).
fn open_devices(devices: &Option<Vec<i32>>, backend: Option<gpu::Backend>) -> Result<Vec<i32>> {
    gpu::load(backend)?;
    let n = gpu::device_count();
    ensure!(n > 0, "no GPU visible to the {:?} library (check the driver; NVIDIA needs GTX 10 or newer)", gpu::backend());
    let devs: Vec<i32> = match devices {
        Some(d) => d.clone(),
        None => (0..n as i32).collect(),
    };
    let mut ok = Vec::new();
    for &d in &devs {
        ensure!(d >= 0 && (d as usize) < n, "device {d} does not exist ({n} device(s) visible)");
        if gpu::backend() == gpu::Backend::Cuda {
            let cc = gpu::compute_capability(d);
            if cc < 61 {
                eprintln!("WARNING: GPU{d} ({}, sm_{cc}) is not supported (needs sm_61+): skipped", gpu::device_name(d));
                continue;
            }
        }
        ok.push(d);
    }
    ensure!(!ok.is_empty(), "no usable GPU among {devs:?}");
    Ok(ok)
}

fn test_header(seed: u32) -> [u8; pow::HEADER_LEN] {
    let mut x = 0x9e37_79b9u32.wrapping_mul(seed | 1);
    let mut h = [0u8; pow::HEADER_LEN];
    for b in h.iter_mut() {
        x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        *b = (x >> 24) as u8;
    }
    h
}

// ---------------------------------------------------------------------------------------------------------------------
// self-test: CPU vs the conformance vectors (pow.rs tests run in cargo test; here the first valid nonce of the vector), the
// library's job build vs this host's, then per GPU: for 12 headers / nonce bases 2^20 nonces at a 1-in-512 target word: the
// GPU's candidate set equals the CPU's exactly, ranges at both ends of the 32-bit g space included
// ---------------------------------------------------------------------------------------------------------------------

const VECTOR_HEADER: &str = "01008578787146be1ef0aacc797754001290272915e89cd9e5784fe1368e4c2131bc0d000000000000003f225a70abe46ef29f09504d0b69ce5e3d550e4d86bb253dc6cd163b5217989d0cf45365000000003fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff8c2beadecc1824b6e7f88cd39fff6d8ffdc9e8a41e93254d7f7109ba7d218f49de3aa3b4c7df30035eb3ddc2d357d2a54521c27a4742438a9ab4561cb516ee6b572d8e192ec138fe522c0aefb5fc2f4a42e2f6c7acdbe887071e5ca50eea4e83000000000000000000000000000000002e0100000000000000000000000000000000000051976778d26056c36803109ea49beaec0c618c49fc39c63fb1f10e51bf56592c04000000000000000000000000000000";

pub fn self_test(devices: &Option<Vec<i32>>, backend: Option<gpu::Backend>) -> Result<()> {
    println!("== Ting self-test ==");
    let vh: [u8; pow::HEADER_LEN] = pow::unhex(VECTOR_HEADER).unwrap().try_into().unwrap();
    let mut vt = [0xffu8; 32];
    vt[0] = 0x3f;
    let first = (0u8..64).find(|&n| {
        let mut nonce = [0u8; 16];
        nonce[0] = n;
        pow::meets(&pow::block_id(&vh, &nonce), &vt)
    });
    ensure!(first == Some(4), "CPU: the conformance vector's first valid nonce is {first:?}, not 4");
    println!("[ok] CPU: conformance vector v3-headers/an-empty-block: nonce 0 above the target, first valid nonce 4");
    let devs = open_devices(devices, backend)?;
    for s in 0..4 {
        let h = test_header(s + 100);
        let mut t = [0xffu8; 32];
        t[0] = s as u8;
        ensure!(gpu::job_build(&h, &t) == pow::job_words(&h, &t), "the library's job build differs from this host's (header {s})");
    }
    println!("[ok] library {:?}: job build (midstate, last block, target) = this host's", gpu::backend());
    let mut failed = Vec::new();
    for &d in &devs {
        if let Err(e) = gpu_self_test(d) {
            println!("[FAIL] GPU{d}: {e:#}");
            failed.push(d);
        }
    }
    ensure!(failed.is_empty(), "Ting self-test failed on GPU(s) {failed:?}");
    println!("Ting self-test passed on {} GPU(s)", devs.len());
    Ok(())
}

fn gpu_self_test(d: i32) -> Result<()> {
    let t = Instant::now();
    let mut buf = vec![0u32; 4096];
    let mut total = 0usize;
    for s in 0..12u32 {
        let word = if s % 2 == 0 { 11 } else { 10 };
        let h = test_header(s);
        let mut t9 = [0xffu8; 32];
        t9[..4].copy_from_slice(&0x007f_ffffu32.to_be_bytes());
        let job = gpu::job_build(&h, &t9);
        let mut base = [0u8; 16];
        base.iter_mut().enumerate().for_each(|(i, b)| *b = (s as u8).wrapping_mul(31).wrapping_add(i as u8 * 7));
        let count = 1u32 << 20;
        let g0 = match s % 3 {
            0 => 0,
            1 => 0u32.wrapping_sub(count),
            _ => s << 24,
        };
        let mut got = Vec::new();
        gpu::search_w(d, &job, &base, word, g0, count, &mut buf, &mut got)?;
        got.sort_unstable();
        let want: Vec<u32> = (0..count)
            .map(|k| g0.wrapping_add(k))
            .filter(|&g| u32::from_be_bytes(pow::block_id(&h, &pow::nonce_of_w(&base, word, g))[..4].try_into().unwrap()) <= 0x007f_ffff)
            .collect();
        ensure!(got == want, "header {s} (word {word}): GPU found {} candidates, the CPU {} (sets differ)", got.len(), want.len());
        total += got.len();
    }
    println!("[ok] GPU{d} {}: 12 headers x 2^20 nonces (kernels t1 and t1p): candidate set = CPU exactly ({total} candidates) ({:?})", gpu::device_name(d), t.elapsed());
    // 0.2.2: groups (templates sharing the last block): every member's candidates = the CPU's, words 11 and 10
    let gsz = gpu::group_size(d, 8);
    if gsz > 1 {
        let t = Instant::now();
        let mut pairs = vec![0u32; 2 * 4096];
        let mut gtotal = 0usize;
        for (s, word) in [(40u32, 11), (41, 10)] {
            let h0 = test_header(s);
            let mut t9 = [0xffu8; 32];
            t9[..4].copy_from_slice(&0x007f_ffffu32.to_be_bytes());
            let hs: Vec<[u8; pow::HEADER_LEN]> = (0..gsz)
                .map(|k| {
                    let mut x = h0;
                    if k > 0 {
                        x[(k * 29) % node::SHARED_TAIL] ^= 0xa5;
                    }
                    x
                })
                .collect();
            let jobs: Vec<gpu::TgJob> = hs.iter().map(|h| gpu::job_build(h, &t9)).collect();
            let base = [s as u8; 16];
            let count = 1u32 << 17;
            let g0 = 0u32.wrapping_sub(count);
            let mut got = Vec::new();
            gpu::search_group(d, &jobs, &base, word, g0, count, &mut pairs, &mut got)?;
            for (k, h) in hs.iter().enumerate() {
                let mut mine: Vec<u32> = got.iter().filter(|x| x.0 == k).map(|x| x.1).collect();
                mine.sort_unstable();
                let want: Vec<u32> = (0..count)
                    .map(|i| g0.wrapping_add(i))
                    .filter(|&g| u32::from_be_bytes(pow::block_id(h, &pow::nonce_of_w(&base, word, g))[..4].try_into().unwrap()) <= 0x007f_ffff)
                    .collect();
                ensure!(mine == want, "group of {gsz} (word {word}) member {k}: GPU found {} candidates, the CPU {} (sets differ)", mine.len(), want.len());
                gtotal += mine.len();
            }
            ensure!(got.iter().all(|x| x.0 < gsz), "group of {gsz}: a job index out of range");
        }
        println!("[ok] GPU{d}: template groups of {gsz} (words 11 and 10, 2^17 nonces): every member = CPU exactly ({gtotal} candidates) ({:?})", t.elapsed());
    }
    Ok(())
}

/// `--hash-bench SECS`: kernel throughput through this host on a target nothing meets; all GPUs at once, power from NVML.
pub fn hash_bench(devices: &Option<Vec<i32>>, backend: Option<gpu::Backend>, secs: f64) -> Result<()> {
    let devs = open_devices(devices, backend)?;
    let nv = crate::nvml::Nvml::load().ok();
    println!("Ting hash-bench: {} GPU(s) {:?}, {secs} s (+1 s warm-up)", devs.len(), devs);
    let res: Vec<Result<(i32, f64, Option<f64>, Option<u32>)>> = std::thread::scope(|s| {
        let hs: Vec<_> = devs
            .iter()
            .map(|&d| {
                let nv = nv.as_ref();
                s.spawn(move || -> Result<(i32, f64, Option<f64>, Option<u32>)> {
                    let h = test_header(7);
                    let mut t = [0u8; 32];
                    t[31] = 1;
                    // 0.2.2: a group of group_max(d) jobs sharing the last block, as mining (--no-group: one)
                    let gsz = group_max(d);
                    let jobs: Vec<gpu::TgJob> = (0..gsz)
                        .map(|k| {
                            let mut x = h;
                            x[10 + k] ^= 0x33;
                            if k == 0 {
                                x = h;
                            }
                            gpu::job_build(&x, &t)
                        })
                        .collect();
                    let nvh = nv.and_then(|n| gpu::device_pci_bus_id(d).ok().and_then(|b| n.by_bus(&b).ok()));
                    let mut buf = vec![0u32; 128];
                    let mut out = Vec::new();
                    let count = ((1u32 << 30) / gsz as u32) & !0xffff;
                    // FFGT_BENCH_WORD=10: the pool kernel (t1p) instead of t1
                    let word: i32 = std::env::var("FFGT_BENCH_WORD").ok().and_then(|v| v.parse().ok()).unwrap_or(11);
                    let mut base = [0x5au8; 16];
                    let tw = Instant::now();
                    while tw.elapsed().as_secs_f64() < 1.0 {
                        gpu::search_group(d, &jobs, &base, word, 0, count, &mut buf, &mut out)?;
                        base[0] = base[0].wrapping_add(1);
                    }
                    let (t0, mut ts, mut total, mut pw, mut pn, mut clk, mut cn) = (Instant::now(), Instant::now(), 0u64, 0.0, 0, 0u64, 0u64);
                    while t0.elapsed().as_secs_f64() < secs {
                        gpu::search_group(d, &jobs, &base, word, 0, count, &mut buf, &mut out)?;
                        base[0] = base[0].wrapping_add(1);
                        total += count as u64 * gsz as u64;
                        if ts.elapsed() >= Duration::from_secs(1) {
                            ts = Instant::now();
                            if let (Some(n), Some(h)) = (nv, nvh) {
                                if let Some(w) = n.power_w(h) {
                                    pw += w;
                                    pn += 1;
                                }
                                if let Some(c) = n.clock_mhz(h) {
                                    clk += c as u64;
                                    cn += 1;
                                }
                            }
                        }
                    }
                    Ok((d, total as f64 / t0.elapsed().as_secs_f64(), (pn > 0).then(|| pw / pn as f64), (cn > 0).then(|| (clk / cn) as u32)))
                })
            })
            .collect();
        hs.into_iter().map(|h| h.join().unwrap_or_else(|_| Err(anyhow::anyhow!("bench thread panicked")))).collect()
    });
    let mut tot = 0.0;
    for r in res {
        let (d, hs, w, clk) = r?;
        tot += hs;
        let pw = w.map_or(String::new(), |w| format!("  {w:.0} W  {:.4} GH/W", hs / 1e9 / w));
        let c = clk.map_or(String::new(), |c| format!("  {c} MHz"));
        println!("hash-bench GPU{d} {} ({:?} kernel {}, group {}): {:.3} GH/s{pw}{c}", gpu::device_name(d), gpu::backend(), gpu::kernel_name(d), group_max(d), hs / 1e9);
    }
    println!("hash-bench total {:.3} GH/s", tot / 1e9);
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------------
// mining
// ---------------------------------------------------------------------------------------------------------------------

#[allow(dead_code)]
pub struct GpuStat {
    pub dev: i32,
    pub pos: usize,
    pub name: String,
    pub bus: String,
    pub nonces: AtomicU64,
    pub alive: AtomicBool,
    pub bad: Arc<AtomicU64>, // candidates that failed the CPU check (wrong GPU results)
    pub err: Mutex<Option<String>>,
    pub pause: Arc<AtomicBool>,
    pub paused: Arc<AtomicBool>,
    pub last_call: AtomicU64, // unix ms of the last finished library call (stall watch)
    pub group: AtomicU64,     // templates in the last search (0.2.2 grouping; hashes = nonces x group)
    pub crashes: AtomicU64,   // 1.8.0 iso: engine processes of this GPU that died or hung (counted with `bad` in the stats)
    pub opens: AtomicU64,     // 1.8.0 iso: engine opens of this GPU (1 + restarts)
}

struct Shared {
    stop: Arc<AtomicBool>,
    cur: Mutex<Option<Job>>,
    subs: [Mutex<mpsc::Sender<Submit>>; 2],
    stats: [Arc<Stats>; 2],
    gens: [Arc<AtomicU64>; 2], // job generation per source (node tip / pool clean job): older candidates are stale
    hits: AtomicU64,
    stale: AtomicU64,
    blocks_seen: AtomicU64, // pool shares that also met the network target
    /// 1.8.0: one engine process per GPU (iso.rs); the opener of a GPU's engine (tests pass the thread engine)
    iso: bool,
    open: fn(i32) -> Result<Box<dyn iso::Engine>>,
    /// engine crashes of iso mode, for the main loop's fault record (after ISO_XID_WAIT, so the driver's Xid can arrive)
    crashes: Mutex<Vec<IsoCrash>>,
    /// FFG 1.8.0 (recover.rs, FFGN 1.0.13): an NVIDIA GPU's crashed engine gets its safe state (the driver's reset, a probe
    /// process, a fresh NVML handle, its clocks, 2 steps below a tuned offset) BEFORE its new engine process (tests: false)
    recover: bool,
}

struct IsoCrash {
    dev: i32,
    pos: usize,
    err: String,
    at: Instant,
}

/// The time the main loop waits after an engine crash before it reads the Xid of that card.
const ISO_XID_WAIT: Duration = Duration::from_millis(1500);
/// iso: engine failures of one GPU within RETRY_WINDOW before it is given up (back at the next miner start)
const RETRY_MAX: usize = 3;
const RETRY_WINDOW: Duration = Duration::from_secs(600);
const RETRY_AFTER: Duration = Duration::from_secs(3);
/// FFG 1.8.0 (FFGN 1.0.13): a GPU FAULTY at its safe offset pauses this long, then a new engine (the other GPUs mine on)
const FAULTY_PAUSE: Duration = Duration::from_secs(600);
/// FFG 1.8.0 (FFGN 1.0.13): an engine that could not open a device that is gone this many times in a row: DEAD
const GONE_OPENS: u32 = 2;

fn open_local(dev: i32) -> Result<Box<dyn iso::Engine>> {
    Ok(Box::new(iso::Local::new(dev)))
}

/// target time of one library call: a new tip / job is picked up within about this long
const CALL_SECS: f64 = 0.25;
const COUNT_MIN: u32 = 1 << 22;
/// a library call that has not returned after this long = a stalled GPU (no Xid): the miner restarts itself
const STALL_SECS: u64 = 60;

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

fn worker(dev: i32, pos: usize, sh: Arc<Shared>, gs: Arc<GpuStat>) {
    // FFG 1.8.0 (recover.rs, FFGN 1.0.13): only the faults at a SAFE offset count (a crash of a tuned offset is a tuning failure:
    // its offset goes down before the new engine); more than RETRY_MAX of them within RETRY_WINDOW = FAULTY (a pause, then a new
    // engine: not lost); DEAD only for a device that cannot be opened (recover::device_gone) or cannot be reached (Lost)
    let mut budget = crate::recover::Budget::new(RETRY_MAX as u32, RETRY_WINDOW);
    let mut gone = 0u32;
    let beat = || gs.last_call.store(now_ms(), Ordering::Relaxed);
    loop {
        let opens0 = gs.opens.load(Ordering::Relaxed);
        let r = worker_loop(dev, pos, &sh, &gs);
        let Err(e) = r else { break };
        let msg = format!("{e:#}");
        let sticky = crate::faults::sticky(&msg) || crate::recover::device_gone(&msg);
        // 1.8.0 iso: an engine process that died / hung, or a sticky CUDA error inside it, ends only that process: a new one
        // once it is safe; the other GPUs keep mining.  Single process: as before (the worker stops, the main loop decides).
        if !sh.iso || sh.stop.load(Ordering::Relaxed) || !sticky {
            eprintln!("GPU{dev}: Ting worker stopped: {msg}");
            *gs.err.lock().unwrap() = Some(msg);
            break;
        }
        if sh.recover {
            // no NVML call for this GPU from any thread while the driver resets it (nvml.rs quiet period)
            crate::nvml::gpu_fault(&gs.bus, &msg);
        }
        gs.crashes.fetch_add(1, Ordering::Relaxed);
        sh.crashes.lock().unwrap().push(IsoCrash { dev, pos, err: msg.clone(), at: Instant::now() });
        let opened = gs.opens.load(Ordering::Relaxed) > opens0;
        if !opened && crate::recover::device_gone(&msg) {
            gone += 1;
            if gone >= GONE_OPENS {
                eprintln!("GPU{dev}: Ting worker stopped: the device cannot be opened ({msg})");
                *gs.err.lock().unwrap() = Some(format!("the device cannot be opened: {msg}"));
                break;
            }
        } else if opened {
            gone = 0;
        }
        eprintln!("GPU{dev}: Ting engine failed ({msg}): only this GPU restarts: a new engine process once it is safe (the driver's reset, a fresh NVML handle, its clocks and a safe offset first); the other GPUs keep mining");
        let lowered = if sh.recover {
            let plan = tune::recovery_plan(&gs.bus);
            let hw = crate::recover::CoinGpu { bus: gs.bus.clone(), stop: &sh.stop, set: crate::noid::offset::set, read: crate::noid::offset::current, fault_at: None, beat: Some(&beat) };
            match crate::recover::recover_after_fault(&hw, &plan, &crate::recover::Opts::default(), &mut |l| println!("GPU{dev}: {l}")) {
                crate::recover::SafeState::Stopped => break,
                crate::recover::SafeState::Lost(why) => {
                    eprintln!("GPU{dev}: Ting worker stopped: {why}");
                    *gs.err.lock().unwrap() = Some(format!("the device cannot be reached: {why}"));
                    break;
                }
                crate::recover::SafeState::Ready { offset, lowered, waited, .. } => {
                    if let (true, Some(o), Some(c)) = (lowered, offset, plan.crashed) {
                        tune::after_recovery(&gs.bus, c, o);
                    }
                    println!(
                        "GPU{dev}: safe to start a new engine process after {:.0} s: {}",
                        waited.as_secs_f64(),
                        match (offset, plan.crashed) {
                            (Some(o), Some(c)) if lowered => format!("offset {c:+} -> {o:+} (2 steps below the crash, never below its base), set and read back"),
                            (Some(o), _) => format!("offset {o:+} (its safe one), read back"),
                            _ => "its offset is not changed by this run".into(),
                        }
                    );
                    lowered
                }
            }
        } else {
            let t = Instant::now();
            while t.elapsed() < RETRY_AFTER && !sh.stop.load(Ordering::Relaxed) {
                beat();
                std::thread::sleep(Duration::from_millis(50));
            }
            false
        };
        if sh.stop.load(Ordering::Relaxed) {
            break;
        }
        match budget.note(Instant::now(), lowered) {
            crate::recover::Verdict::Retry { counted: false, .. } => println!("GPU{dev}: the fault was above its safe offset (a tuning failure): it does not count against this GPU"),
            crate::recover::Verdict::Retry { counted: true, n } => println!("GPU{dev}: fault {n} of {RETRY_MAX} allowed within {} min at its safe offset", RETRY_WINDOW.as_secs() / 60),
            crate::recover::Verdict::Exhausted { n } => {
                eprintln!(
                    "WARNING GPU{dev} (bus {}): FAULTY: {n} faults within {} min at its safe offset (the device itself opens: not lost, no reboot needed): this GPU pauses {} min, then a new engine; check its clocks / power / riser",
                    crate::uv::short_bus(&gs.bus),
                    RETRY_WINDOW.as_secs() / 60,
                    FAULTY_PAUSE.as_secs() / 60
                );
                let t = Instant::now();
                while t.elapsed() < FAULTY_PAUSE && !sh.stop.load(Ordering::Relaxed) {
                    beat();
                    std::thread::sleep(Duration::from_millis(200));
                }
                if sh.stop.load(Ordering::Relaxed) {
                    break;
                }
                budget.clear();
            }
        }
    }
    gs.alive.store(false, Ordering::Relaxed);
}

/// The kernel's word: 10 when a pool fixes nonce bytes 12..15 (t1p), else 11 (t1).
fn word_of(j: &Job) -> i32 {
    if j.tpl.en1.is_some() { 10 } else { 11 }
}

/// The library's job build (as FFGT mines); the host's identical one (self-test: "job build = this host's") without a library.
fn build_job(h: &[u8; pow::HEADER_LEN], t: &[u8; 32]) -> gpu::TgJob {
    if gpu::loaded() { gpu::job_build(h, t) } else { pow::job_words(h, t) }
}

fn worker_loop(dev: i32, pos: usize, sh: &Shared, gs: &GpuStat) -> Result<()> {
    gs.last_call.store(now_ms(), Ordering::Relaxed);
    let mut eng = (sh.open)(dev)?;
    let first_open = gs.opens.fetch_add(1, Ordering::Relaxed) == 0;
    let mut out: Vec<(usize, u32)> = Vec::new();
    let mut count: u32 = COUNT_MIN;
    let mut cur: Option<Job> = None;
    let gmax = if grouping() { eng.group_max().min(node::GROUP_KEEP) } else { 1 };
    // the candidate budget of one call: the engine returns up to iso::MAX_PAIRS (job, g) pairs
    let buf_pairs = iso::MAX_PAIRS;
    // the group searched: the newest job first, then its older compatible ones (node.rs Ring), their library jobs
    let mut members: Vec<Job> = Vec::new();
    let mut tjobs: Vec<gpu::TgJob> = Vec::new();
    let mut base = [0u8; 16];
    let mut word = 11;
    let mut g: u64 = 0;
    let mut max_count: u32 = u32::MAX;
    // the start stagger only at the first open (a restarted engine does not wait for a new place in the start order)
    let mut staggered = !first_open;
    // the test fault fires once per run (the first engine of this GPU); FFG_TEST_FAULT_KIND=kernel|exit|hang = a real fault
    let inject = crate::faults::test_fault_for(dev).filter(|_| first_open).map(|secs| Instant::now() + Duration::from_secs(secs));
    let mut injected = false;
    // test hook: FFGT_TEST_STALL=<dev> hangs that worker (first run of a self-restart chain only)
    let stall_test = std::env::var("FFGT_TEST_STALL").ok().and_then(|v| v.parse::<i32>().ok()) == Some(dev) && std::env::var("FFG_SELF_RESTARTS").unwrap_or_default().is_empty();
    loop {
        gs.last_call.store(now_ms(), Ordering::Relaxed);
        if sh.stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        if !injected && inject.is_some_and(|t| Instant::now() >= t && gs.nonces.load(Ordering::Relaxed) > 0) {
            injected = true;
            let kind = std::env::var("FFG_TEST_FAULT_KIND").unwrap_or_default();
            if kind.is_empty() || !eng.inject_fault(&kind)? {
                bail!("injected fault (FFG_TEST_FAULT): an illegal memory access was encountered");
            }
        }
        if gs.pause.load(Ordering::SeqCst) {
            gs.paused.store(true, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(20));
            continue;
        }
        gs.paused.store(false, Ordering::SeqCst);
        let duty = crate::templimit::duty(dev);
        if duty == Some(0) {
            std::thread::sleep(Duration::from_millis(200));
            continue;
        }
        let newest = sh.cur.lock().unwrap().clone();
        let changed = match (&cur, &newest) {
            (None, None) => false,
            (Some(a), Some(b)) => !Arc::ptr_eq(&a.tpl, &b.tpl),
            _ => true,
        };
        if changed {
            cur = newest;
            if let Some(j) = &cur {
                // the kernel compares the first word of the share target (solo: the block target)
                let job = build_job(&j.tpl.header, &j.tpl.share);
                members.clear();
                tjobs.clear();
                members.push(j.alone());
                tjobs.push(job);
                for m in j.group.iter() {
                    if members.len() >= gmax {
                        break;
                    }
                    let t = build_job(&m.tpl.header, &m.tpl.share);
                    // the library refuses a group whose last block / target differ: checked here as well
                    if t.tail == job.tail && t.target == job.target && !m.solved.load(Ordering::Relaxed) {
                        members.push(m.alone());
                        tjobs.push(t);
                    }
                }
                let n = members.len().min(gmax).max(1);
                members.truncate(n);
                tjobs.truncate(n);
                gs.group.store(n as u64, Ordering::Relaxed);
                word = word_of(j);
                base = rand::random();
                match j.tpl.en1 {
                    // pool: byte 0 = this GPU's slot, bytes 1..5 and 10..11 ours (counter), 6..9 = g, 12..15 = extranonce1
                    Some(e1) => {
                        base[0] = pos as u8;
                        base[12..16].copy_from_slice(&e1);
                    }
                    // solo: bytes 10..13 = g, this GPU's slot in byte 14, the rest random
                    None => base[14] = pos as u8,
                }
                g = 0;
                // candidates per call: the group's jobs all count (the buffer holds 4096 pairs)
                let p = (job.target[0] as f64 + 1.0) / 4294967296.0 * tjobs.len() as f64;
                max_count = ((buf_pairs as f64 / 4.0) / p).clamp(256.0, u32::MAX as f64) as u32;
            }
        }
        let Some(j) = cur.clone().filter(|j| !j.solved.load(Ordering::Relaxed)) else {
            std::thread::sleep(Duration::from_millis(20));
            continue;
        };
        if !staggered {
            staggered = true;
            crate::worker::stagger_start(dev, &sh.stop);
            continue;
        }
        if g >= 1 << 32 {
            // a used-up base: the next one (pool: bytes 1..5; solo: bytes 0..7)
            if j.tpl.en1.is_some() {
                let mut c = [0u8; 8];
                c[..5].copy_from_slice(&base[1..6]);
                let c = u64::from_le_bytes(c).wrapping_add(1);
                base[1..6].copy_from_slice(&c.to_le_bytes()[..5]);
            } else {
                let c = u64::from_le_bytes(base[..8].try_into().unwrap()).wrapping_add(1);
                base[..8].copy_from_slice(&c.to_le_bytes());
            }
            g = 0;
        }
        let n = (count.min(max_count) as u64).min((1u64 << 32) - g) as u32;
        out.clear();
        if stall_test && gs.nonces.load(Ordering::Relaxed) > 0 {
            println!("GPU{dev}: FFGT_TEST_STALL: this worker hangs now (as a GPU call that never returns)");
            loop {
                std::thread::sleep(Duration::from_secs(1));
            }
        }
        let t = Instant::now();
        eng.search_group(&tjobs, &base, word, g as u32, n, &mut out)?;
        let dt = t.elapsed().as_secs_f64();
        g += n as u64;
        // hashes: every job of the group is tried at each nonce
        gs.nonces.fetch_add(n as u64 * tjobs.len() as u64, Ordering::Relaxed);
        for &(k, c) in &out {
            match members.get(k) {
                Some(m) => check_hit(sh, gs, m, &base, word, c),
                None => {
                    gs.bad.fetch_add(1, Ordering::Relaxed);
                    eprintln!("GPU{dev}: the library returned job index {k} of a group of {}: ignored", members.len());
                }
            }
        }
        if dt > 0.0 && n == count.min(max_count) {
            let c = (count as f64 * (CALL_SECS / dt)).clamp(count as f64 * 0.5, count as f64 * 2.0);
            count = (c.clamp(COUNT_MIN as f64, (1u64 << 32) as f64 - 1.0) as u32 >> 16).max(1) << 16;
        }
        if let Some(p) = duty {
            std::thread::sleep(Duration::from_secs_f64(dt * (100 - p as u32) as f64 / p as f64));
        }
    }
}

fn check_hit(sh: &Shared, gs: &GpuStat, j: &Job, base: &[u8; 16], word: i32, g: u32) {
    let nonce = pow::nonce_of_w(base, word, g);
    let id = pow::block_id(&j.tpl.header, &nonce);
    sh.hits.fetch_add(1, Ordering::Relaxed);
    if id[..4] > j.tpl.share[..4] {
        gs.bad.fetch_add(1, Ordering::Relaxed);
        eprintln!("GPU{}: CANDIDATE FAILED THE CPU CHECK (wrong GPU result): job {} g {g:08x} id {} — not submitted", gs.dev, j.tpl.job_id, pow::hex(&id));
        return;
    }
    if !pow::meets(&id, &j.tpl.share) {
        return;
    }
    let si = j.src.idx();
    if j.tgen != sh.gens[si].load(Ordering::SeqCst) {
        sh.stale.fetch_add(1, Ordering::Relaxed);
        return;
    }
    let pool = j.tpl.en1.is_some();
    if !pool {
        // solo: one block per template
        if j.solved.swap(true, Ordering::SeqCst) {
            return;
        }
        println!("GPU{}: BLOCK FOUND height {} id {} — submitting", gs.dev, j.tpl.height, pow::hex(&id));
    } else if pow::meets(&id, &j.tpl.target) {
        sh.blocks_seen.fetch_add(1, Ordering::Relaxed);
    }
    let _ = sh.subs[si].lock().unwrap().send(Submit { job: j.clone(), nonce, id, gpu: gs.dev });
}

/// Dev fee routing: the pool slot (a second login with the developer's account on the fee pool) or, solo without the fee
/// pool, the node's producer data switched to the developer's account for the slot (crash-safe marker).
enum NodeFee {
    Off,
    On { until: Instant, original: String },
}

/// The Ting account of a wallet string: Ting ID shares an address as `<64 hex>&net=<chain>` (sometimes with a `ting:` prefix);
/// the pool and the gate want the 64 hex digits only (0.2.0 sent the tail and the pool closed the connection).
pub fn clean_account(w: &str) -> String {
    let s = w.trim();
    let s = s.strip_prefix("ting:").unwrap_or(s);
    let s = s.split(['&', '?', '/']).next().unwrap_or("").trim();
    if s != w.trim() {
        println!("note: wallet {:?} -> Ting account {s} (the part after '&' / '?' is not part of the account)", w.trim());
    }
    s.to_string()
}

pub fn mine(o: Opts) -> Result<()> {
    std::hint::black_box(&LAYOUT_TAG);
    let solo = o.pools.is_empty();
    let nd = if solo { Some(node::resolve(o.node.as_deref(), o.node_dir.as_deref(), o.node_token_file.as_deref(), o.node_remote)?) } else { None };
    let user_urls: Vec<pool::Url> = o.pools.iter().map(|p| pool::parse_url(p).map_err(anyhow::Error::msg)).collect::<Result<_>>()?;
    if !solo {
        if !(o.account.len() == 64 && o.account.bytes().all(|b| b.is_ascii_hexdigit())) {
            println!("note: --wallet {:?} is not a Ting account (64 hex digits): fine for an ffgt-gate, a pool will refuse the login", o.account);
        }
    }
    let dir = crate::uv::state_dir(&o.uv);
    if let Some(nd) = &nd {
        // the node first: no clock is touched before it answers (and says where the blocks pay)
        let (tip, height, syncing, status) = node::tip(nd).map_err(|e| anyhow::anyhow!("the Ting node does not answer on {}: {e:#}", nd.addr))?;
        node::recover_marker(&dir, nd).map_err(|e| anyhow::anyhow!("devfee: cannot put the node's producer data back after an interrupted fee slot: {e:#} (see {})", node::marker_path(&dir).display()))?;
        let status = if dir.join("ting-fee-producer.json").exists() { status } else { node::tip(nd).map(|x| x.3).unwrap_or(status) };
        let payout = node::payout(&status);
        println!(
            "node: {} '{}' {} height {height} tip {}…{}; blocks pay {}",
            nd.addr,
            status["name"].as_str().unwrap_or("?"),
            status["network"].as_str().unwrap_or("?"),
            &tip[..tip.len().min(16)],
            if syncing { " (catching up)" } else { "" },
            match (&payout, status["producer_hex"].as_str()) {
                (Some(p), _) => format!("account {p}"),
                (None, Some(p)) if !p.is_empty() => format!("producer data {p} (not a payout account: check `ting miner status`)"),
                _ => "NOTHING".into(),
            }
        );
        ensure!(
            status["producer_hex"].as_str().is_some_and(|p| !p.is_empty()),
            "the node has no payout account: its blocks would issue no DLR. Set it on the node first: ting --data-dir DIR miner payout <account> (64 hex digits, `ting wallet account`)"
        );
        ensure!(payout.as_deref() != Some(devfee::FEE_ACCOUNT_TING), "the node pays the FFGT developer account: set your own (`ting miner payout <account>`)");
    }
    crate::wintune::process(false);
    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = stop.clone();
        ctrlc::set_handler(move || {
            if stop.swap(true, Ordering::SeqCst) {
                crate::noid::offset::restore_all();
                clocks::restore_all();
                eprintln!("second interrupt: exiting at once");
                crate::conout::exit(130);
            }
            eprintln!("shutting down (finishing in-flight work)...");
        })?;
    }
    crate::wintune::on_console_close(|| {
        crate::noid::offset::restore_all();
        clocks::restore_all();
    });
    let devices = open_devices(&o.devices, o.backend)?;
    let buses: Vec<String> = devices.iter().map(|&d| gpu::device_pci_bus_id(d)).collect::<Result<_>>()?;
    o.temp.validate(devices.len())?;
    let target = match &nd {
        Some(n) => format!("solo {n}"),
        None => format!(
            "pool {}{} as {}.{}",
            user_urls[0],
            if user_urls.len() > 1 { format!(" (backup: {})", user_urls[1..].iter().map(|u| u.to_string()).collect::<Vec<_>>().join(", ")) } else { String::new() },
            &o.account[..o.account.len().min(8)],
            o.worker
        ),
    };
    println!("{} {} | Ting/DLR ({ALGO_NAME}) | {target} | {:?} | {} GPU(s) {:?}", crate::PRODUCT, env!("CARGO_PKG_VERSION"), gpu::backend(), devices.len(), devices);
    let nvidia = gpu::backend() == gpu::Backend::Cuda;
    let iso_on = iso::wanted(o.single_process, nvidia);
    if nvidia {
        clocks::set_bus_resolver(gpu::device_pci_bus_id);
        // FFG 1.8.0 (recover.rs, FFGN 1.0.13): after a fault-restart of the whole process (faults::restart_self) the faulted GPU gets
        // its safe state (the driver's reset, a probe process, not above its crash - 2 steps) before this run sets clocks or opens it
        crate::recover::at_restart(&buses, &dir, o.uv.step, &stop, "recover");
        clocks::recover(&dir);
    }
    let mut clock_opts = clocks::ClockOpts {
        core_mhz: match &o.lock_cclock { Some(s) => clocks::expand_u32(s, devices.len(), "--lock-cclock")?, None => vec![] },
        mem_mhz: match &o.lock_mclock { Some(s) => clocks::expand_u32(s, devices.len(), "--lock-mclock")?, None => vec![] },
        mem_off: match &o.moffset { Some(s) => clocks::expand_i32(s, devices.len(), "--moffset")?, None => vec![] },
        // 1.8.0: the coin's memory policy (coins.rs: HiveProfile, the OC profile's memory lock is kept); the default low lock below
        ..clocks::ClockOpts::policy(crate::coins::spec(crate::coins::Coin::Ting).mem)
    };
    // 0.2.1: the memory clock locked at the lowest value that keeps the full core range (Ting's SHA-256 does not use the video
    // memory: RTX 3070 at a 1800 lock 128.9 W -> 115.5 W at 810 MHz, the same 5.14 GH/s); not with --lock-mclock / --moffset /
    // --no-mem-lock, and not on a GPU whose HiveOS OC profile locks the memory itself
    if nvidia && clock_opts.mem_mhz.is_empty() && clock_opts.mem_off.is_empty() && !o.no_mem_lock {
        if let Ok(nv) = crate::nvml::Nvml::load() {
            let hive = crate::hiveoc::load();
            clock_opts.mem_mhz = buses
                .iter()
                .map(|b| {
                    let d = nv.by_bus(b).ok()?;
                    if hive.as_ref().is_some_and(|h| nv.index(d).and_then(|i| h.mem_lock(i as usize)).is_some()) {
                        return None;
                    }
                    match nv.low_mem_clock(d) {
                        Ok(m) => Some(m),
                        Err(e) => {
                            println!("clocks: GPU bus {}: memory clock not lowered ({e:#})", crate::uv::short_bus(b));
                            None
                        }
                    }
                })
                .collect();
            if clock_opts.mem_mhz.iter().any(Option::is_some) {
                println!("clocks: default memory lock {:?} MHz (the lowest that keeps the full core clock range: SHA-256 does not use the video memory; --no-mem-lock leaves it alone)", clock_opts.mem_mhz);
            } else {
                clock_opts.mem_mhz.clear();
            }
        }
    }
    let mut o = o;
    if nvidia && crate::autolock::active() {
        let cards: Vec<crate::autolock::Card> = devices.iter().enumerate().map(|(p, &d)| crate::autolock::Card { dev: d, bus: buses[p].clone(), kernel: "ting-t1".into() }).collect();
        match crate::autolock::resolve("ting", &cards, &mut clock_opts.core_mhz, &clock_opts.mem_mhz, true, &dir, &dir, &stop) {
            crate::autolock::Outcome::Stop => return Ok(()),
            crate::autolock::Outcome::Done(r) => return r.map_err(anyhow::Error::msg),
            crate::autolock::Outcome::Mine { swept } => {
                if swept {
                    (o.uv.reset, o.uv.reset_all) = (false, false);
                }
            }
        }
    }
    let _clock_guard = if nvidia { clocks::apply(&devices, &clock_opts, &dir) } else { None };
    let flags: Vec<(Arc<AtomicU64>, Arc<AtomicBool>, Arc<AtomicBool>)> = devices.iter().map(|_| (Arc::new(AtomicU64::new(0)), Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)))).collect();
    let mut live: Option<Arc<tune::Live>> = None;
    let mut fixed: Option<(&'static crate::nvml::Nvml, tune::Fixed)> = None;
    let _off_guard = crate::noid::offset::Guard;
    if o.uv.enabled() && nvidia {
        crate::noid::offset::recover(&dir);
        let off_nv = crate::noid::offset::begin(&buses, &dir);
        if o.uv.fixed.is_set() {
            let mut set = Vec::new();
            for (pos, b) in buses.iter().enumerate() {
                if let Some(v) = o.uv.fixed.get(pos) {
                    match crate::noid::offset::set(b, v) {
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
        } else if let Some(nv) = off_nv {
            let hive = crate::hiveoc::load();
            let cards: Vec<tune::CardRef> = devices
                .iter()
                .enumerate()
                .map(|(pos, &d)| tune::CardRef {
                    dev: d,
                    bus: buses[pos].clone(),
                    name: gpu::device_name(d),
                    core_lock: clock_opts
                        .core_mhz
                        .get(pos)
                        .copied()
                        .flatten()
                        .or_else(|| hive.as_ref().and_then(|h| nv.by_bus(&buses[pos]).ok().and_then(|x| nv.index(x)).and_then(|i| h.core_lock(i as usize)))),
                    pause: flags[pos].1.clone(),
                    paused: flags[pos].2.clone(),
                    bad: flags[pos].0.clone(),
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
            crate::noid::offset::restore_all();
            return Ok(());
        }
    } else if o.uv.enabled() {
        println!("auto-uv: OpenCL backend: the NVIDIA offset tuner does not apply (AMD undervolt: not yet)");
    }

    // ---- work sources: the user's (node or pool) and the developer fee's (always a pool login) ----
    let stats = [Arc::new(Stats::default()), Arc::new(Stats::default())];
    let (ev_tx, ev_rx) = mpsc::channel::<(Src, Event)>();
    let (user_tx, user_rx) = mpsc::channel::<Submit>();
    let (fee_tx, fee_rx) = mpsc::channel::<Submit>();
    let nsh = Arc::new(node::Shared { tip_gen: Arc::new(AtomicU64::new(0)), blocks: Mutex::new(Vec::new()), kick: AtomicBool::new(false), keep: std::sync::atomic::AtomicUsize::new(node::GROUP_KEEP) });
    let user_gen: Arc<AtomicU64> = if solo { nsh.tip_gen.clone() } else { Arc::new(AtomicU64::new(0)) };
    let user_entry = Arc::new(AtomicU64::new(0));
    if let Some(nd) = &nd {
        node::spawn(nd.clone(), ev_tx.clone(), user_rx, stats[0].clone(), nsh.clone(), stop.clone());
    } else {
        pool::spawn(
            pool::Login {
                name: "user",
                src: Src::User,
                urls: user_urls.clone(),
                login: format!("{}.{}", o.account.to_ascii_lowercase(), o.worker),
                pass: o.password.clone(),
                enabled: Arc::new(AtomicBool::new(true)),
                stop: stop.clone(),
                failback_min: o.failback_min,
                jgen: user_gen.clone(),
                entry: user_entry.clone(),
            },
            ev_tx.clone(),
            user_rx,
            stats[0].clone(),
        );
    }
    let p = devfee::Params::from_env();
    let fee_enabled = Arc::new(AtomicBool::new(false));
    let fee_gen = Arc::new(AtomicU64::new(0));
    let fee_url = pool::parse_url(&std::env::var("FFGT_FEE_POOL").unwrap_or_else(|_| devfee::FEE_POOL_TING.into())).map_err(anyhow::Error::msg)?;
    pool::spawn(
        pool::Login {
            name: "fee",
            src: Src::Fee,
            urls: vec![fee_url.clone()],
            login: format!("{}.{}", devfee::FEE_ACCOUNT_TING, devfee::FEE_WORKER_TING),
            pass: "x".into(),
            enabled: fee_enabled.clone(),
            stop: stop.clone(),
            failback_min: 0.0,
            jgen: fee_gen.clone(),
            entry: Arc::new(AtomicU64::new(0)),
        },
        ev_tx.clone(),
        fee_rx,
        stats[1].clone(),
    );
    drop(ev_tx);
    println!(
        "devfee: {}% (one {}s slot per {} min cycle, random position) on {fee_url} as the FFGT developer account{}",
        devfee::FEE_PERCENT,
        p.slot.as_secs(),
        p.cycle.as_secs() / 60,
        if solo { "; without that pool: the node's blocks pay the developer for the slot (crash-safe marker)" } else { "" }
    );
    let sh = Arc::new(Shared {
        stop: stop.clone(),
        cur: Mutex::new(None),
        subs: [Mutex::new(user_tx), Mutex::new(fee_tx)],
        stats: stats.clone(),
        gens: [user_gen.clone(), fee_gen.clone()],
        hits: AtomicU64::new(0),
        stale: AtomicU64::new(0),
        blocks_seen: AtomicU64::new(0),
        iso: iso_on,
        open: if iso_on { iso::open } else { open_local },
        crashes: Mutex::new(Vec::new()),
        recover: iso_on && nvidia && crate::offset::nvml().is_some(),
    });
    if iso_on {
        println!("engine: one process per GPU (a CUDA fault on one GPU restarts only that GPU; --single-process = all GPUs in this process)");
    }
    let watch = crate::faults::Watch::new(&devices.iter().enumerate().map(|(p, &d)| (d, buses[p].clone(), gpu::device_name(d))).collect::<Vec<_>>());
    let mut gstats = Vec::new();
    let mut handles = Vec::new();
    let mut excluded = vec![false; devices.len()];
    for (pos, &dev) in devices.iter().enumerate() {
        let gs = Arc::new(GpuStat {
            dev,
            pos,
            name: gpu::device_name(dev),
            bus: buses[pos].clone(),
            nonces: AtomicU64::new(0),
            alive: AtomicBool::new(true),
            bad: flags[pos].0.clone(),
            err: Mutex::new(None),
            pause: flags[pos].1.clone(),
            paused: flags[pos].2.clone(),
            last_call: AtomicU64::new(now_ms()),
            group: AtomicU64::new(0),
            crashes: AtomicU64::new(0),
            opens: AtomicU64::new(0),
        });
        if let Some(c) = watch.as_ref().and_then(|w| w.by_dev(dev).map(|k| &w.cards[k])) {
            if let (Some(left), Some(r)) = (crate::faults::excluded(&dir, &c.uuid), crate::faults::get(&dir, &c.uuid)) {
                println!("faults: {}", crate::faults::excluded_line(&c.label(), &r, left));
                gs.alive.store(false, Ordering::Relaxed);
                excluded[pos] = true;
                gstats.push(gs);
                continue;
            }
        }
        let (sh2, gs2) = (sh.clone(), gs.clone());
        handles.push(std::thread::Builder::new().name(format!("ting-gpu{dev}")).spawn(move || worker(dev, pos, sh2, gs2))?);
        let gm = group_max(dev);
        println!(
            "GPU {dev}: {} (bus {}, {}, {} {}), kernel {}, {}",
            gs.name,
            crate::uv::short_bus(&gs.bus),
            if nvidia { format!("sm_{}", gpu::compute_capability(dev)) } else { "OpenCL".into() },
            gpu::sm_count(dev),
            if nvidia { "SMs" } else { "CUs" },
            if solo { "t1" } else { "t1p" },
            if gm > 1 {
                format!("group up to {gm} (templates of one tip searched together)")
            } else if !grouping() {
                "group 1 (--no-group)".to_string()
            } else {
                "group 1 (this library has no group kernels)".to_string()
            }
        );
        gstats.push(gs);
    }
    crate::templimit::start(&o.temp, &devices.iter().enumerate().map(|(pos, &d)| (d, buses[pos].clone(), gpu::device_name(d), pos)).collect::<Vec<_>>(), &stop);
    let nvml = crate::nvml::Nvml::load().ok();
    let power: Option<(&crate::nvml::Nvml, Vec<Option<crate::nvml::Dev>>)> = nvml.as_ref().map(|n| (n, buses.iter().map(|b| n.by_bus(b).ok()).collect()));
    api::publish_static(&gstats, &target);

    let (start, deadline) = (Instant::now(), o.minutes.map(|m| Duration::from_secs_f64(m * 60.0)));
    let mut jobs: [Option<Job>; 2] = [None, None];
    let mut fee = devfee::DevFee::new(p, start);
    println!("devfee: next slot in {:.1} min", fee.next_slot_in(start).as_secs_f64() / 60.0);
    let mut node_fee = NodeFee::Off;
    let mut published: Option<(Src, usize)> = None;
    let mut last_n = vec![0u64; gstats.len()];
    let mut dead_seen = excluded.clone();
    let mut ok_cleared = false;
    let mut win = Instant::now();
    let mut watchdog = crate::failover::Watch::default();
    let user_link = crate::failover::Link::new(user_urls.len().max(1));
    let (mut mined, mut last_loop) = (Duration::ZERO, Instant::now());
    let mut fee_since_ready: Option<Instant> = None;
    let mut pids_shown = false;
    while !stop.load(Ordering::Relaxed) && deadline.map_or(true, |d| start.elapsed() < d) {
        if o.blocks.is_some_and(|b| stats[0].accepted.load(Ordering::Relaxed) >= b) && solo {
            println!("--blocks {}: reached, stopping", o.blocks.unwrap());
            break;
        }
        let im = crate::idle::mode();
        if im != crate::idle::Mode::Pause {
            mined += last_loop.elapsed();
        }
        last_loop = Instant::now();
        // ---- GPU faults (Xid) and stalls ----
        let nowm = now_ms();
        for g in &gstats {
            if g.alive.load(Ordering::Relaxed) && !g.paused.load(Ordering::Relaxed) && nowm.saturating_sub(g.last_call.load(Ordering::Relaxed)) > STALL_SECS * 1000 {
                if sh.iso {
                    // its engine process is stopped: the worker's call fails and only this GPU restarts
                    println!("faults: GPU{} {}: no GPU call returned for {} s without a driver error (stalled): its engine process is stopped, only this GPU restarts", g.dev, g.name, STALL_SECS);
                    iso::kill(g.dev);
                    g.last_call.store(nowm, Ordering::Relaxed);
                    continue;
                }
                println!("faults: GPU{} {}: no GPU call returned for {} s without a driver error (stalled): the miner restarts", g.dev, g.name, STALL_SECS);
                stop.store(true, Ordering::SeqCst);
                restore_node_fee(&nd, &dir, &mut node_fee);
                crate::noid::offset::restore_all();
                clocks::restore_all();
                println!("faults: {}", crate::faults::restart_self("a GPU stalled"));
                bail!("GPU{} stalled", g.dev);
            }
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
            // iso: every engine crash was recorded when it happened (iso_crashes); a GPU given up is not recorded again
            for (k, x) in v.culprits.iter().filter(|_| !sh.iso) {
                let w = watch.as_ref().unwrap();
                let c = &w.cards[*k];
                let r = crate::faults::record_crash(&dir, &c.uuid, &c.bus, &c.name, *x, w.offset(*k));
                let tunable = o.uv.auto && r.offset > 0;
                if !tunable && r.n >= crate::faults::LOWER_AFTER {
                    let mut r = r.clone();
                    r.excluded_until = crate::uv::now() + crate::faults::EXCLUDE_SECS;
                    crate::faults::put(&dir, &c.uuid, &r);
                    println!("faults: {}", crate::faults::excluded_line(&c.label(), &r, crate::faults::EXCLUDE_SECS));
                } else {
                    println!("faults: {} crash {} in a row at offset {} remembered{}", c.label(), r.n, crate::uv::signed(r.offset), if tunable { " (auto-uv tunes below it at the next start)" } else { "" });
                }
            }
            for l in &v.lines {
                println!("faults: {l}");
            }
        }
        if sh.iso {
            iso_crashes(&sh, &gstats, watch.as_ref(), live.as_ref(), &dir);
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
            restore_node_fee(&nd, &dir, &mut node_fee);
            crate::noid::offset::restore_all();
            clocks::restore_all();
            println!("faults: {}", crate::faults::restart_self("every GPU worker stopped"));
            bail!("all GPU workers have stopped");
        }
        if let Some(l) = crate::status::summary_due() {
            println!("{l}");
        }
        // ---- events ----
        for (src, ev) in ev_rx.try_iter() {
            let i = src.idx();
            match ev {
                Event::Job(j) => {
                    let t = &j.tpl;
                    let same = jobs[i].as_ref().is_some_and(|o| o.tgen == j.tgen);
                    if !same && (src == Src::User || fee.active()) {
                        if solo && src == Src::User {
                            println!("new block template: height {} parent {}… target {}… ({} tx; expected {:.2e} hashes per block)", t.height, &t.parent[..t.parent.len().min(16)], &pow::hex(&t.target)[..20], t.txs, pow::expected_hashes(&t.target));
                        } else {
                            println!("new job {} [{}] height {} d={} (network target {}…)", t.job_id, if src == Src::Fee { "fee" } else { "user" }, t.height, t.diff, &pow::hex(&t.target)[..20]);
                        }
                    }
                    jobs[i] = Some(j);
                }
                Event::Pause(why) => {
                    if jobs[i].take().is_some() {
                        println!("{}: {why}: mining paused", if src == Src::Fee { "fee" } else { "user" });
                    }
                }
                Event::Disconnected => {
                    jobs[i] = None;
                }
            }
        }
        // ---- developer fee (devfee.rs schedule on mining time) ----
        let now = start + mined;
        let want_conn = fee.conn_wanted(now);
        fee_enabled.store(want_conn, Ordering::Relaxed);
        if !want_conn {
            jobs[1] = None;
        }
        let fee_ready = want_conn && jobs[1].is_some();
        if fee_ready && fee_since_ready.is_none() {
            fee_since_ready = Some(Instant::now());
        }
        match fee.tick(now, fee_ready) {
            devfee::Change::Started => println!("devfee: mining {}s for the developer ({}%) on {fee_url}", p.slot.as_secs(), devfee::FEE_PERCENT),
            devfee::Change::Finished => println!("devfee: back to user"),
            devfee::Change::Aborted => println!("devfee: fee connection lost mid-slot, back to user"),
            devfee::Change::Skipped if fee.retry_pending() => {
                println!("devfee: fee login on {fee_url} not ready: the slot is tried again in a few minutes (user mining continues)");
            }
            devfee::Change::Skipped => {
                // solo without the fee pool: the node's blocks pay the developer for one slot (marker first)
                match (&nd, &node_fee) {
                    (Some(n), NodeFee::Off) => match start_node_fee(n, &dir, p.slot) {
                        Ok(nf) => {
                            nsh.kick.store(true, Ordering::SeqCst);
                            node_fee = nf;
                        }
                        Err(e) => println!("devfee: fee pool unavailable and the node fee slot failed ({e:#}): slot skipped (user mining continues)"),
                    },
                    _ => println!("devfee: fee login on {fee_url} unavailable, skipping this slot (user mining continues)"),
                }
            }
            devfee::Change::Scheduled => println!("devfee: next slot in {:.1} min", fee.next_slot_in(now).as_secs_f64() / 60.0),
            devfee::Change::None => {}
        }
        if let NodeFee::On { until, .. } = &node_fee {
            if Instant::now() >= *until {
                restore_node_fee(&nd, &dir, &mut node_fee);
                nsh.kick.store(true, Ordering::SeqCst);
            }
        }
        let src = if fee.active() { Src::Fee } else { Src::User };
        let want = jobs[src.idx()].clone().filter(|_| im != crate::idle::Mode::Pause).map(|j| (src, Arc::as_ptr(&j.tpl) as usize, j));
        if want.as_ref().map(|w| (w.0, w.1)) != published {
            published = want.as_ref().map(|w| (w.0, w.1));
            *sh.cur.lock().unwrap() = want.map(|w| w.2);
        }
        if let Some(l) = &live {
            tune::tick(l);
        }
        if let Some((nv, f)) = &fixed {
            tune::fixed_tick(nv, f);
        }
        let el = win.elapsed();
        if el >= Duration::from_secs(10) {
            let mut parts = Vec::new();
            let (mut tot, mut tot_w, mut all_w) = (0.0, 0.0, true);
            let mut per = Vec::new();
            for (i, g) in gstats.iter().enumerate() {
                let n = g.nonces.load(Ordering::Relaxed);
                let ghs = (n - last_n[i]) as f64 / el.as_secs_f64() / 1e9;
                last_n[i] = n;
                tot += ghs;
                let w = power.as_ref().and_then(|(nv, d)| d[i].and_then(|h| nv.power_w(h)));
                let pw = match w {
                    Some(w) if w > 0.0 => {
                        tot_w += w;
                        format!(" ({:.0} W, {:.4} GH/W)", w, ghs / w)
                    }
                    _ => {
                        all_w = false;
                        String::new()
                    }
                };
                per.push(api::GpuLine { dev: g.dev, name: g.name.clone(), bus: g.bus.clone(), ghs, w, alive: g.alive.load(Ordering::Relaxed), bad: g.bad.load(Ordering::Relaxed) + g.crashes.load(Ordering::Relaxed) });
                parts.push(format!("GPU{} {:.3}{} group {}{}{}{}", g.dev, ghs, pw, g.group.load(Ordering::Relaxed), if g.alive.load(Ordering::Relaxed) { "" } else { " DEAD" }, crate::status::tag(g.dev), crate::templimit::tag(g.dev)));
            }
            let pp = if all_w && tot_w > 0.0 { format!(", {:.0} W, {:.4} GH/W", tot_w, tot / tot_w) } else { String::new() };
            let bad: u64 = gstats.iter().map(|g| g.bad.load(Ordering::Relaxed) + g.crashes.load(Ordering::Relaxed)).sum();
            let ld = |a: &AtomicU64| a.load(Ordering::Relaxed);
            let (u, f) = (&stats[0], &stats[1]);
            if !solo {
                let hps = jobs[0].as_ref().map(|j| j.tpl.diff * 4294967296.0);
                let allowed = !fee.active() && im == crate::idle::Mode::Full && u.connected.load(Ordering::Relaxed) != 0;
                crate::failover::watch_tick(&mut watchdog, &user_link, "user", el.as_secs_f64(), allowed, tot * 1e9, hps, ld(&u.accepted), ld(&u.rejected), ld(&u.submitted));
            }
            let eta = if solo { jobs[0].as_ref().filter(|_| tot > 0.0).map(|j| pow::expected_hashes(&j.tpl.target) / (tot * 1e9) / 60.0) } else { None };
            let line = format!(
                "[{:>4}s] {} | total {:.3} GH/s{} | candidates {} (bad {}, stale {}) | user sub {} acc {} rej {} (stale {}){} | fee sub {} acc {} rej {}{}{}{}{}{}",
                start.elapsed().as_secs(),
                parts.join("  "),
                tot,
                pp,
                ld(&sh.hits),
                bad,
                ld(&sh.stale) + ld(&u.stale_drop) + ld(&f.stale_drop),
                ld(&u.submitted),
                ld(&u.accepted),
                ld(&u.rejected),
                ld(&u.stale_rej),
                if solo { " blocks".to_string() } else { format!(" shares, pool {}", user_urls[user_entry.load(Ordering::Relaxed) as usize % user_urls.len()]) },
                ld(&f.submitted),
                ld(&f.accepted),
                ld(&f.rejected),
                eta.map_or(String::new(), |m| format!(" | ~{m:.0} min per block at this rate")),
                if u.connected.load(Ordering::Relaxed) == 0 { if solo { "  [NODE DOWN]" } else { "  [DISCONNECTED]" } } else { "" },
                match (fee.active(), &node_fee, im) {
                    (_, _, crate::idle::Mode::Pause) => "  [PAUSED: PC in use]".to_string(),
                    (_, _, crate::idle::Mode::Light(p)) => format!("  [LIGHT {p} %: PC in use]"),
                    (true, _, _) => "  [DEVFEE]".to_string(),
                    (_, NodeFee::On { .. }, _) => "  [DEVFEE: node]".to_string(),
                    _ => String::new(),
                },
                if jobs[0].is_none() && !fee.active() && im == crate::idle::Mode::Full { "  [NO JOB]" } else { "" },
                clocks::unheld_tag()
            );
            println!("{line}");
            if sh.iso && !pids_shown && start.elapsed() >= Duration::from_secs(30) {
                // the engine processes (their RAM is what a 4 GB rig of 12 GPUs must check)
                pids_shown = true;
                println!("engine processes: {}", iso::pids().iter().map(|(d, p)| format!("GPU{d} pid {p}")).collect::<Vec<_>>().join(", "));
            }
            api::publish(api::Snapshot {
                uptime: start.elapsed().as_secs(),
                total_ghs: tot,
                total_w: if all_w { Some(tot_w) } else { None },
                gpus: per,
                user: (ld(&u.submitted), ld(&u.accepted), ld(&u.rejected)),
                fee: (ld(&f.submitted), ld(&f.accepted), ld(&f.rejected)),
                candidates: ld(&sh.hits),
                bad,
                devfee: fee.active() || matches!(node_fee, NodeFee::On { .. }),
                solo,
                height: jobs[0].as_ref().map_or(0, |j| j.tpl.height),
            });
            win = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    stop.store(true, Ordering::SeqCst);
    restore_node_fee(&nd, &dir, &mut node_fee);
    crate::noid::offset::restore_all();
    clocks::restore_all();
    for h in handles {
        let _ = h.join();
    }
    std::thread::sleep(Duration::from_secs(2));
    let n: u64 = gstats.iter().map(|g| g.nonces.load(Ordering::Relaxed)).sum();
    let blocks = nsh.blocks.lock().unwrap().clone();
    let g = |s: &Stats, f: fn(&Stats) -> &AtomicU64| f(s).load(Ordering::Relaxed);
    println!(
        "done: {:.1} G nonces in {:?} ({:.3} GH/s average); candidates {} (bad {}, stale {}); user: submitted {} accepted {} rejected {} | fee: submitted {} accepted {} rejected {}{}{}",
        n as f64 / 1e9,
        start.elapsed(),
        n as f64 / start.elapsed().as_secs_f64() / 1e9,
        sh.hits.load(Ordering::Relaxed),
        gstats.iter().map(|g| g.bad.load(Ordering::Relaxed) + g.crashes.load(Ordering::Relaxed)).sum::<u64>(),
        sh.stale.load(Ordering::Relaxed),
        g(&stats[0], |s| &s.submitted),
        g(&stats[0], |s| &s.accepted),
        g(&stats[0], |s| &s.rejected),
        g(&stats[1], |s| &s.submitted),
        g(&stats[1], |s| &s.accepted),
        g(&stats[1], |s| &s.rejected),
        if blocks.is_empty() { String::new() } else { format!("; blocks: {}", blocks.iter().map(|(h, id)| format!("{h} {id}")).collect::<Vec<_>>().join(", ")) },
        if sh.blocks_seen.load(Ordering::Relaxed) > 0 { format!("; pool shares that met the network target: {}", sh.blocks_seen.load(Ordering::Relaxed)) } else { String::new() }
    );
    Ok(())
}

/// iso: the fault record of each engine crash, ISO_XID_WAIT after it (the driver's Xid of THAT card); a crash at a tuned
/// offset makes the tuner go 2 steps below it at once (rule 2), the GPU's new engine mines there.
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
        let tuned = live.and_then(|l| tune::expected_offset(l, c.dev));
        let off = tuned.or_else(|| w.offset(k));
        let foreign = crate::faults::foreign_offset(tuned, w.offset(k)) || crate::faults::foreign_near(&card.bus);
        let rec = (!foreign).then(|| crate::faults::record_crash(dir, &card.uuid, &card.bus, &card.name, xid.unwrap_or(0), off));
        println!("faults: {}", crate::faults::iso_line(&card.label(), xid, &c.err, others, rec.as_ref()));
        // FFG 1.8.0 (recover.rs): with the recovery the worker sets the 2 steps below BEFORE the new engine (tune::after_recovery)
        if !foreign && !sh.recover {
            if let Some(l) = live {
                tune::crashed_while_mining(l, c.dev);
            }
        }
    }
}

/// Switches the node's producer data to the developer's account for one slot: marker first, then the switch, read back.
fn start_node_fee(n: &node::Node, dir: &std::path::Path, slot: Duration) -> Result<NodeFee> {
    let (_, _, _, st) = node::tip(n)?;
    let original = st["producer_hex"].as_str().unwrap_or("").to_string();
    ensure!(!original.is_empty(), "the node has no producer data");
    node::write_marker(dir, n, &original)?;
    if let Err(e) = node::set_producer(n, &node::producer_for(devfee::FEE_ACCOUNT_TING)) {
        let _ = node::set_producer(n, &original);
        let _ = std::fs::remove_file(node::marker_path(dir));
        return Err(e);
    }
    println!("devfee: fee pool unavailable: the node's blocks pay the FFGT developer for {} s (marker {}); then back to {original}", slot.as_secs(), node::marker_path(dir).display());
    Ok(NodeFee::On { until: Instant::now() + slot, original })
}

fn restore_node_fee(nd: &Option<node::Node>, dir: &std::path::Path, nf: &mut NodeFee) {
    let NodeFee::On { original, .. } = &*nf else { return };
    let Some(n) = nd else { return };
    for attempt in 1..=10 {
        match node::set_producer(n, original) {
            Ok(()) => {
                let _ = std::fs::remove_file(node::marker_path(dir));
                println!("devfee: node fee slot over: producer data back to {original} (read back), marker removed");
                *nf = NodeFee::Off;
                return;
            }
            Err(e) => {
                println!("devfee: cannot put the producer data back yet (attempt {attempt}/10): {e:#}");
                std::thread::sleep(Duration::from_secs(2));
            }
        }
    }
    println!("devfee: WARNING the node still pays the developer: the marker {} stays; the next start puts it back (or: ting miner payout <your account>)", node::marker_path(dir).display());
    *nf = NodeFee::Off;
}

#[cfg(test)]
mod tests {
    #[test]
    fn coin_detection() {
        assert!(super::looks_like_ting_pool("crypto-connection.net:4073"));
        assert!(super::looks_like_ting_pool("stratum+tcp://crypto-connection.net:4073"));
        assert!(super::looks_like_ting_pool("ting://192.168.1.10:4173"));
        assert!(!super::looks_like_ting_pool("pearl-eu2.luckypool.io:3360"));
        assert!(!super::looks_like_ting_pool("stratum+ssl://pool.proximamine.com:3334"));
    }

    #[test]
    fn account_tail_is_cut() {
        let a = "00".repeat(32);
        assert_eq!(super::clean_account(&format!("{a}&net=mainnet")), a);
        assert_eq!(super::clean_account(&format!("ting:{a}")), a);
        assert_eq!(super::clean_account(&a), a);
    }

    // ---- 1.8.0 per-GPU isolation: a dying engine process restarts only its GPU (the NEB test of ISO.md, for Ting) ----
    use super::*;
    use std::sync::atomic::AtomicU32;

    /// A thread engine (iso::open_thread, CPU reference behind a real loopback socket) whose first engine of GPU 0 dies
    /// (the `exit` fault: the engine "process" ends) after 3 searches.
    struct Dying {
        inner: Box<dyn iso::Engine>,
        left: Option<u32>,
    }
    impl iso::Engine for Dying {
        fn group_max(&self) -> usize {
            self.inner.group_max()
        }
        fn search_group(&mut self, jobs: &[gpu::TgJob], base: &[u8; 16], word: i32, g0: u32, count: u32, out: &mut Vec<(usize, u32)>) -> Result<()> {
            match self.left {
                Some(0) => {
                    self.left = None;
                    let _ = self.inner.inject_fault("exit");
                }
                Some(n) => self.left = Some(n - 1),
                None => {}
            }
            self.inner.search_group(jobs, base, word, g0, count, out)
        }
        fn inject_fault(&mut self, kind: &str) -> Result<bool> {
            self.inner.inject_fault(kind)
        }
    }
    static OPENS0: AtomicU32 = AtomicU32::new(0);
    fn dying_open(dev: i32) -> Result<Box<dyn iso::Engine>> {
        let first0 = dev == 0 && OPENS0.fetch_add(1, Ordering::SeqCst) == 0;
        Ok(Box::new(Dying { inner: iso::open_thread(dev)?, left: first0.then_some(3) }))
    }

    fn pool_job() -> Job {
        let mut share = [0xffu8; 32];
        share[..4].copy_from_slice(&0x0fff_ffffu32.to_be_bytes()); // 1 in 16
        let tpl = node::Template {
            id: "t".into(),
            job_id: "j1".into(),
            share,
            diff: 1.0,
            en1: Some([1, 2, 3, 4]),
            height: 7,
            parent: "00".repeat(32),
            header: [0x42; pow::HEADER_LEN],
            target: [0; 32],
            txs: 0,
            got: Instant::now(),
        };
        Job::new(Src::User, tpl, 0)
    }

    fn run(iso_on: bool, open: fn(i32) -> Result<Box<dyn iso::Engine>>, secs: f64) -> (Vec<Arc<GpuStat>>, Vec<Submit>, Arc<Shared>) {
        unsafe { std::env::set_var("FFG_START_STAGGER_MS", "0") };
        let (utx, urx) = mpsc::channel::<Submit>();
        let (ftx, _frx) = mpsc::channel::<Submit>();
        let sh = Arc::new(Shared {
            stop: Arc::new(AtomicBool::new(false)),
            cur: Mutex::new(Some(pool_job())),
            subs: [Mutex::new(utx), Mutex::new(ftx)],
            stats: [Arc::new(Stats::default()), Arc::new(Stats::default())],
            gens: [Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0))],
            hits: AtomicU64::new(0),
            stale: AtomicU64::new(0),
            blocks_seen: AtomicU64::new(0),
            iso: iso_on,
            open,
            crashes: Mutex::new(Vec::new()),
            recover: false,
        });
        let gs: Vec<Arc<GpuStat>> = (0..2)
            .map(|d| {
                Arc::new(GpuStat {
                    dev: d,
                    pos: d as usize,
                    name: format!("test{d}"),
                    bus: format!("0000:0{d}:00.0"),
                    nonces: AtomicU64::new(0),
                    alive: AtomicBool::new(true),
                    bad: Arc::new(AtomicU64::new(0)),
                    err: Mutex::new(None),
                    pause: Arc::new(AtomicBool::new(false)),
                    paused: Arc::new(AtomicBool::new(false)),
                    last_call: AtomicU64::new(now_ms()),
                    group: AtomicU64::new(0),
                    crashes: AtomicU64::new(0),
                    opens: AtomicU64::new(0),
                })
            })
            .collect();
        let hs: Vec<_> = gs.iter().map(|g| { let (s2, g2) = (sh.clone(), g.clone()); std::thread::spawn(move || worker(g2.dev, g2.pos, s2, g2)) }).collect();
        let t = Instant::now();
        while t.elapsed().as_secs_f64() < secs {
            std::thread::sleep(Duration::from_millis(20));
        }
        sh.stop.store(true, Ordering::SeqCst);
        for h in hs {
            h.join().unwrap();
        }
        (gs, urx.try_iter().collect(), sh)
    }

    #[test]
    fn a_dying_engine_process_restarts_only_its_gpu_and_every_share_is_sent_once() {
        let (gs, subs, sh) = run(true, dying_open, 6.0);
        // GPU0: one crash, a second engine, and it mined again after it; GPU1: never stopped
        assert_eq!((gs[0].crashes.load(Ordering::Relaxed), gs[0].opens.load(Ordering::Relaxed)), (1, 2));
        assert_eq!((gs[1].crashes.load(Ordering::Relaxed), gs[1].opens.load(Ordering::Relaxed)), (0, 1));
        assert!(gs.iter().all(|g| g.err.lock().unwrap().is_none()), "no worker gave up");
        assert_eq!(gs.iter().map(|g| g.bad.load(Ordering::Relaxed)).sum::<u64>(), 0);
        let q = sh.crashes.lock().unwrap();
        assert_eq!(q.len(), 1);
        assert_eq!((q[0].dev, q[0].pos), (0, 0));
        assert!(crate::faults::sticky(&q[0].err), "{}", q[0].err);
        let crash_at = q[0].at;
        drop(q);
        // every share passed the CPU check (check_hit), none twice; both GPUs submitted, GPU0 also after its restart
        let mut seen = std::collections::HashSet::new();
        for s in &subs {
            assert!(pow::meets(&pow::block_id(&s.job.tpl.header, &s.nonce), &s.job.tpl.share));
            assert!(seen.insert(s.nonce), "a nonce submitted twice");
        }
        assert!(subs.iter().any(|s| s.gpu == 1));
        assert!(subs.iter().any(|s| s.gpu == 0), "GPU0 submitted");
        assert!(gs[0].nonces.load(Ordering::Relaxed) > 0 && crash_at.elapsed() > RETRY_AFTER);
        assert_eq!(sh.hits.load(Ordering::Relaxed) as usize, subs.len(), "every candidate was a share (pool mode, no stale)");
    }

    static OPENS_SP: AtomicU32 = AtomicU32::new(0);
    fn dying_open_sp(dev: i32) -> Result<Box<dyn iso::Engine>> {
        let first0 = dev == 0 && OPENS_SP.fetch_add(1, Ordering::SeqCst) == 0;
        Ok(Box::new(Dying { inner: iso::open_thread(dev)?, left: first0.then_some(3) }))
    }

    #[test]
    fn the_same_fault_in_single_process_mode_stops_the_worker() {
        let (gs, _subs, sh) = run(false, dying_open_sp, 2.0);
        assert_eq!(gs[0].opens.load(Ordering::Relaxed), 1, "no restart without isolation");
        assert!(gs[0].err.lock().unwrap().as_deref().is_some_and(|e| e.contains("engine process ended")));
        assert!(!gs[0].alive.load(Ordering::Relaxed));
        assert!(sh.crashes.lock().unwrap().is_empty());
        assert!(gs[1].err.lock().unwrap().is_none());
    }
}
