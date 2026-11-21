// FreeForGem — see LICENSE
//! Proxima (PROX) mining: `--algo prox` (or a Proxima pool URL).  Transaction-hash PoW of library upgrade 6 (slot 196000+):
//! h = BLAKE2b-256(message || nonce_be8), a share / solution needs k zero low bits in h[24..32] (big-endian).
//! GPU search in the closed-source PROX library (gpu.rs, ffgprox kernel A), every candidate re-hashed on the CPU (blake2b.rs)
//! before it is sent; work from a pool (pool.rs: suprnova / proximamine stratum, TCP or TLS) or a solo node (solo.rs: seeker
//! protocol v2 of `proxi node mine`).  1% developer fee (devfee.rs schedule), always on the proximamine pool (also when the
//! user mines solo or on another pool).
//!
//!   freeforgem --algo prox --pool stratum+ssl://pool.proximamine.com:3334 --wallet <PROX address> --worker rig1 --password <secret>
//!   freeforgem --algo prox --solo http://127.0.0.1:8100 --token <mine.seeker.token>
pub mod blake2b;
pub mod fee;
pub mod gpu;
pub mod innov;
pub mod pool;
pub mod solo;
pub mod tune;

use crate::clocks;
use crate::devfee::{self, Change, DevFee};
use crate::stratum::{Src, Stats};
use anyhow::{Result, bail, ensure};
use pool::{Event, PJob, Submit};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const ALGO_NAME: &str = "proxima";

pub struct Opts {
    pub pool: Option<String>,
    /// every --pool entry (primary first, then backups of the same protocol); `pool` is the first
    pub pools: Vec<String>,
    pub solo: Option<String>,
    pub token: String,
    pub seeker_name: String,
    pub wallet: String,
    pub worker: String,
    pub password: String,
    pub minutes: Option<f64>,
    pub devices: Option<Vec<i32>>,
    pub lock_cclock: Option<String>,
    pub lock_mclock: Option<String>,
    pub moffset: Option<String>,
    pub temp: crate::templimit::Opts,
    pub uv: crate::uv::Opts,
    pub no_mem_lock: bool, // --no-mem-lock: leave the memory clock alone (default: the lowest one that keeps the full core range)
}

/// A Proxima pool URL (`--algo` not given): a host name with "prox" in it (proximamine.com, proxi.suprnova.cc), the InnovLab
/// scheme innov+tls:// / innov+tcp://, or an InnovLab host on its PROX port 19701 (19601 is NOID there).
pub fn looks_like_prox_pool(url: &str) -> bool {
    let u = url.to_ascii_lowercase();
    if u.starts_with("innov") {
        return true;
    }
    let host = u.split_once("://").map_or(u.as_str(), |x| x.1);
    host.contains("prox") || (host.contains("innovlab") && host.split(['/', '?']).next().is_some_and(|h| h.ends_with(":19701")))
}

/// Pool URL: the NOID parser (TLS for stratum+ssl / ssl / tls and a bare HOST:PORT, plain TCP for stratum+tcp / tcp).
pub fn parse_pool(s: &str) -> Result<crate::noid::pool::PoolUrl> {
    crate::noid::pool::parse_url(s).map_err(anyhow::Error::msg)
}

/// Loads the library and checks the devices; the list of usable devices (`devices` or all).
fn open_devices(devices: &Option<Vec<i32>>) -> Result<Vec<i32>> {
    gpu::load()?;
    let n = gpu::device_count();
    ensure!(n > 0, "no NVIDIA GPU visible (PROX needs an NVIDIA GPU, GTX 10 or newer; check the driver and CUDA_VISIBLE_DEVICES)");
    let devs: Vec<i32> = match devices {
        Some(d) => d.clone(),
        None => (0..n as i32).collect(),
    };
    let mut ok = Vec::new();
    for &d in &devs {
        ensure!(d >= 0 && (d as usize) < n, "device {d} does not exist ({n} NVIDIA device(s) visible)");
        let cc = gpu::compute_capability(d);
        if cc < 61 {
            eprintln!("WARNING: GPU{d} ({}, sm_{cc}) is not supported by PROX (needs sm_61+): skipped", gpu::device_name(d));
            continue;
        }
        ok.push(d);
    }
    ensure!(!ok.is_empty(), "no GPU usable for PROX among {devs:?}");
    Ok(ok)
}

fn test_message(len: usize, seed: u32) -> Vec<u8> {
    let mut x = 0x9e37_79b9u32.wrapping_mul(seed);
    (0..len)
        .map(|_| {
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (x >> 24) as u8
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------------------------------
// self-test: CPU BLAKE2b vs the node's and the pool's vectors, then per GPU: for 16 message lengths (block borders, the mainnet
// ~369 and 600 bytes) 2^17 counters at k=9: the GPU's candidate set must equal the CPU's exactly (the counter -> nonce map of the
// library applied to every counter, each nonce hashed on the CPU), and every candidate keeps the pool's extranonce1
// ---------------------------------------------------------------------------------------------------------------------

pub fn self_test(devices: &Option<Vec<i32>>) -> Result<()> {
    println!("== PROX self-test ==");
    let t = Instant::now();
    for &(n, want) in blake2b::NODE_VECTORS {
        let m: Vec<u8> = (0..n).map(|i| i as u8).collect();
        ensure!(pool::hex(&blake2b::hash_nonce(&m, 0x0102030405060708)) == want, "CPU BLAKE2b differs from the node vector of length {n}");
    }
    for &(n, want, work, first) in blake2b::POOL_VECTORS {
        let p: Vec<u8> = (0..n).map(|i| ((i * 7 + 3) & 0xff) as u8).collect();
        let h = blake2b::hash_nonce(&p, 0x0102030405060708);
        ensure!(pool::hex(&h) == want && blake2b::work_bits(&h) == work, "CPU BLAKE2b differs from the pool vector of prefix length {n}");
        let e = (0u64..).find(|e| blake2b::work_bits(&blake2b::hash_nonce(&p, 0x0102_0000_0000_0000 | e)) >= 12).unwrap();
        ensure!(format!("{e:012x}") == first, "first extranonce2 with 12 zero bits differs for prefix length {n}");
    }
    println!(
        "[ok] CPU: {} node vectors + {} pool vectors (txid-blake2b, block borders, first extranonce2) in {:?}",
        blake2b::NODE_VECTORS.len(),
        blake2b::POOL_VECTORS.len(),
        t.elapsed()
    );
    let devs = open_devices(devices)?;
    let mut failed = Vec::new();
    for &d in &devs {
        match gpu_self_test(d) {
            Ok(()) => {}
            Err(e) => {
                println!("[FAIL] GPU{d}: {e:#}");
                failed.push(d);
            }
        }
    }
    ensure!(failed.is_empty(), "PROX self-test failed on GPU(s) {failed:?}");
    println!("PROX self-test passed on {} GPU(s)", devs.len());
    Ok(())
}

fn gpu_self_test(d: i32) -> Result<()> {
    let t = Instant::now();
    let lens = [112usize, 119, 120, 121, 124, 127, 128, 247, 248, 249, 255, 256, 369, 600, 632, 640];
    let mut buf = vec![0u64; 1 << 16];
    let mut total = 0usize;
    let mut variants = Vec::new();
    for &l in &lens {
        let msg = test_message(l, l as u32);
        let k = 9u32;
        let job = gpu::job_build(&msg, k, None)?;
        let count = 1u64 << 17;
        let start = 0xa5a5_0000_0000_0000u64 | (l as u64) << 20; // top 16 bits = a pool's extranonce1
        let mut got = Vec::new();
        gpu::search(d, &job, start, count, &mut buf, &mut got)?;
        got.sort_unstable();
        let mut want: Vec<u64> = (0..count)
            .map(|c| gpu::counter_to_nonce(&job, start + c))
            .filter(|&n| blake2b::work_bits(&blake2b::hash_nonce(&msg, n)) >= k)
            .collect();
        want.sort_unstable();
        ensure!(got.iter().all(|n| n >> 48 == 0xa5a5), "length {l}: a candidate lost the extranonce1 (top 16 bits)");
        ensure!(got == want, "length {l}: GPU found {} candidates, the CPU {} (sets differ)", got.len(), want.len());
        total += got.len();
        let v = gpu::variant_name(&job);
        if !variants.contains(&v) {
            variants.push(v);
        }
    }
    println!(
        "[ok] GPU{d} {}: {} message lengths x 2^17 nonces at k=9: candidate set = CPU exactly ({total} candidates), kernels {} ({:?})",
        gpu::device_name(d),
        lens.len(),
        variants.join(", "),
        t.elapsed()
    );
    Ok(())
}

/// `--hash-bench SECS [--hash-prefix-len N]`: kernel throughput through this host on a synthetic job (k=60, nothing found),
/// comparable with `ffgprox --hash-bench`; all GPUs at once.
pub fn hash_bench(devices: &Option<Vec<i32>>, secs: f64, plen: usize) -> Result<()> {
    let devs = open_devices(devices)?;
    let nv = crate::nvml::Nvml::load().ok();
    let msg = test_message(plen, 12345);
    println!("PROX hash-bench: {} GPU(s) {:?}, prefix_len {plen}, {secs} s (+1 s warm-up)", devs.len(), devs);
    let res: Vec<Result<(i32, f64, Option<f64>)>> = std::thread::scope(|s| {
        let hs: Vec<_> = devs
            .iter()
            .map(|&d| {
                let (msg, nv) = (&msg, nv.as_ref());
                s.spawn(move || -> Result<(i32, f64, Option<f64>)> {
                    let job = gpu::job_build(msg, 60, None)?;
                    let h = nv.and_then(|n| gpu::device_pci_bus_id(d).ok().and_then(|b| n.by_bus(&b).ok()));
                    let mut buf = vec![0u64; 1 << 12];
                    let mut out = Vec::new();
                    let count = 1u64 << 30; // >= 2^30 per call (kernel3)
                    let mut base = 0x5a5a_0000_0000_0000u64; // bits 40-47 zero, as in mining (the kernel's m[15] path)
                    let tw = Instant::now();
                    while tw.elapsed().as_secs_f64() < 1.0 {
                        gpu::search(d, &job, base, count, &mut buf, &mut out)?;
                        base += count;
                    }
                    let (t0, mut ts, mut total, mut pw, mut pn) = (Instant::now(), Instant::now(), 0u64, 0.0, 0);
                    while t0.elapsed().as_secs_f64() < secs {
                        gpu::search(d, &job, base, count, &mut buf, &mut out)?;
                        base += count;
                        total += count;
                        if ts.elapsed() >= Duration::from_secs(1) {
                            ts = Instant::now();
                            if let (Some(n), Some(h)) = (nv, h) {
                                if let Some(w) = n.power_w(h) {
                                    pw += w;
                                    pn += 1;
                                }
                            }
                        }
                    }
                    Ok((d, total as f64 / t0.elapsed().as_secs_f64(), (pn > 0).then(|| pw / pn as f64)))
                })
            })
            .collect();
        hs.into_iter().map(|h| h.join().unwrap_or_else(|_| Err(anyhow::anyhow!("bench thread panicked")))).collect()
    });
    let mut tot = 0.0;
    for r in res {
        let (d, hs, w) = r?;
        tot += hs;
        let pw = w.map_or(String::new(), |w| format!("  {w:.0} W  {:.4} GH/W", hs / 1e9 / w));
        println!("hash-bench GPU{d} {} prefix_len {plen}: {:.3} GH/s{pw}", gpu::device_name(d), hs / 1e9);
    }
    println!("hash-bench total {:.3} GH/s", tot / 1e9);
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------------
// mining
// ---------------------------------------------------------------------------------------------------------------------

struct Assign {
    src: Src,
    job: Arc<PJob>,
    jgen: u64, // job generation of its login (stale hits: job no longer current)
}

#[allow(dead_code)]
pub struct GpuStat {
    pub dev: i32,
    pub pos: usize,
    pub name: String,
    pub bus: String,
    pub label: Mutex<String>,
    pub nonces: AtomicU64,
    pub alive: AtomicBool,
    pub bad: Arc<AtomicU64>, // candidates that failed the CPU check (wrong GPU results)
    pub err: Mutex<Option<String>>,
    pub pause: Arc<AtomicBool>,  // the auto-uv re-check of this GPU asks the mining thread to pause
    pub paused: Arc<AtomicBool>, // the mining thread is paused (no call in flight)
}

struct Shared {
    stop: Arc<AtomicBool>,
    cur: Mutex<Option<Arc<Assign>>>,
    jgen: [AtomicU64; 2],
    subs: [Mutex<mpsc::Sender<Submit>>; 2],
    stats: [Arc<Stats>; 2],
    next_id: AtomicU64,
    hits: AtomicU64,
    stale: AtomicU64,
    solo: bool,
    beat: Arc<solo::Beat>,
    attempts: Arc<AtomicU64>, // solo: attempts not yet reported to the node
}

/// target time of one library call: a job change is picked up within about this long
const CALL_SECS: f64 = 0.15;
/// kernel3 (libproxgpu): a prox_search call below 2^30 nonces loses hash rate (the author's advice), so the size never goes below it
/// (about 0.2-0.4 s per call on a 3070 .. 4070 Ti; a job change is picked up at the next call, shares of the old job are dropped as stale)
const COUNT_MIN: u64 = 1 << 30;
const COUNT_MAX: u64 = 1 << 33;

fn worker(dev: i32, pos: usize, sh: Arc<Shared>, gs: Arc<GpuStat>) {
    let r = worker_loop(dev, pos, &sh, &gs);
    if let Err(e) = r {
        eprintln!("GPU{dev}: PROX worker stopped: {e:#}");
        *gs.err.lock().unwrap() = Some(format!("{e:#}"));
    }
    gs.alive.store(false, Ordering::Relaxed);
}

fn worker_loop(dev: i32, pos: usize, sh: &Shared, gs: &GpuStat) -> Result<()> {
    let mut buf = vec![0u64; 1 << 16];
    let mut out: Vec<u64> = Vec::new();
    let mut count: u64 = COUNT_MIN;
    let mut cur: Option<Arc<Assign>> = None;
    let mut job = gpu::ProxJob::default();
    let (mut base, mut ctr) = (0u64, 0u64);
    let mut staggered = false;
    let inject = crate::faults::test_fault_for(dev).map(|secs| Instant::now() + Duration::from_secs(secs));
    loop {
        if sh.stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        if inject.is_some_and(|t| Instant::now() >= t && gs.nonces.load(Ordering::Relaxed) > 0) {
            bail!("injected fault (FFG_TEST_FAULT): an illegal memory access was encountered");
        }
        if gs.pause.load(Ordering::SeqCst) {
            gs.paused.store(true, Ordering::SeqCst); // the auto-uv re-check of this GPU runs now
            std::thread::sleep(Duration::from_millis(20));
            continue;
        }
        gs.paused.store(false, Ordering::SeqCst);
        // --temp-limit / idle light load: 0 % = stopped
        let duty = crate::templimit::duty(dev);
        if duty == Some(0) {
            std::thread::sleep(Duration::from_millis(200));
            continue;
        }
        let newest = sh.cur.lock().unwrap().clone();
        let changed = match (&cur, &newest) {
            (None, None) => false,
            (Some(a), Some(b)) => !Arc::ptr_eq(a, b),
            _ => true,
        };
        if changed {
            cur = newest;
            if let Some(a) = &cur {
                job = gpu::job_build(&a.job.prefix, a.job.k, None)?;
                *gs.label.lock().unwrap() = gpu::variant_name(&job);
                ctr = 0;
                // stratum pool: the top 16 bits are the pool's extranonce1, the low 48 bits ours (layout below);
                // solo / InnovLab (the nonce is ours): a random start with bits 32-47 clear (GPU slot / counter, see below)
                base = if a.job.pool { (a.job.en1 as u64) << 48 } else { rand::random::<u64>() & !(0xffffu64 << 32) };
            }
        }
        let Some(a) = cur.clone() else {
            std::thread::sleep(Duration::from_millis(20));
            continue;
        };
        if a.job.expires.is_some_and(|e| Instant::now() >= e) {
            std::thread::sleep(Duration::from_millis(20));
            continue;
        }
        if !staggered {
            staggered = true;
            crate::worker::stagger_start(dev, &sh.stop);
            continue;
        }
        if a.job.pool && ctr + count > 1 << 40 {
            // the 40-bit counter of this GPU is spent (2^40 nonces = ~5 min on an RTX 5090): wait for the next job
            std::thread::sleep(Duration::from_millis(50));
            continue;
        }
        // nonce layout of ffgprox 0.1.2: counter bits 0-35 in place, the GPU slot in bits 36-39 (+ 44-47 past 16 GPUs), counter
        // bits 36-39 in bits 40-43: bits 40-47 (nonce byte 7 after the kernel's map) stay 0 for the first 2^36 nonces of a job,
        // which lets the kernel take its cheaper m[15] path on every GPU.  A call never crosses a 2^36 boundary.
        let lo = ctr & ((1u64 << 36) - 1);
        let n = count.min((1u64 << 36) - lo);
        let place = lo | ((pos as u64 & 15) << 36) | ((ctr >> 36) << 40) | (((pos as u64 >> 4) & 15) << 44);
        let start = base.wrapping_add(place);
        out.clear();
        let t = Instant::now();
        gpu::search(dev, &job, start, n, &mut buf, &mut out)?;
        let dt = t.elapsed().as_secs_f64();
        ctr += n;
        gs.nonces.fetch_add(n, Ordering::Relaxed);
        if sh.solo && a.src == Src::User {
            sh.attempts.fetch_add(n, Ordering::Relaxed);
        }
        for &n in &out {
            check_hit(sh, gs, &a, &job, n);
        }
        if n < count {
            continue; // a short call at a 2^36 boundary: keep the tuned size
        }
        if dt > 0.0 {
            let c = (count as f64 * (CALL_SECS / dt)).clamp(count as f64 * 0.5, count as f64 * 2.0) as u64;
            count = (c.clamp(COUNT_MIN, COUNT_MAX) >> 20) << 20;
        }
        if let Some(p) = duty {
            // light load: mine p % of the time
            std::thread::sleep(Duration::from_secs_f64(dt * (100 - p as u32) as f64 / p as f64));
        }
    }
}

fn check_hit(sh: &Shared, gs: &GpuStat, a: &Assign, job: &gpu::ProxJob, nonce: u64) {
    let j = &a.job;
    let h = blake2b::hash_nonce(&j.prefix, nonce);
    sh.hits.fetch_add(1, Ordering::Relaxed);
    let w = blake2b::work_bits(&h);
    if w < job.k.min(32) || (j.pool && (nonce >> 48) as u16 != j.en1) {
        gs.bad.fetch_add(1, Ordering::Relaxed);
        eprintln!("GPU{}: HIT FAILED THE CPU CHECK (wrong GPU result): job {} nonce {nonce:016x} hash {} work {w} < k — not submitted", gs.dev, j.job_id, pool::hex(&h));
        return;
    }
    if w < j.k {
        return; // a prefilter candidate (k > 32), not a share
    }
    // solo: the node's beat (it falls with every accepted solution); InnovLab: the job's share_beat
    let beat = if sh.solo && a.src == Src::User { sh.beat.get(&j.job_id).or(j.beat) } else { j.beat };
    {
        if let Some(b) = beat {
            if blake2b::work_key(&h) >= b {
                return; // valid but not below the best the node knows
            }
        }
    }
    if sh.jgen[a.src.idx()].load(Ordering::Relaxed) != a.jgen || j.expires.is_some_and(|e| Instant::now() >= e) {
        sh.stale.fetch_add(1, Ordering::Relaxed);
        return;
    }
    let id = sh.next_id.fetch_add(1, Ordering::Relaxed);
    sh.stats[a.src.idx()].submitted.fetch_add(1, Ordering::Relaxed);
    let _ = sh.subs[a.src.idx()].lock().unwrap().send(Submit { id, job_id: j.job_id.clone(), nonce, hash: h, gpu: gs.dev });
}

pub fn mine(o: Opts) -> Result<()> {
    ensure!(o.pool.is_some() != o.solo.is_some(), "--algo prox needs --pool stratum+ssl://HOST:PORT (a Proxima pool) or --solo http://HOST:PORT (a node), not both");
    // the user's pool(s) and their protocol: InnovLab (innov+tls:// or an innovlab host) or the suprnova-format stratum; a list has
    // one protocol (the wallet / password are the same on every entry)
    let entries: Vec<String> = if !o.pools.is_empty() { o.pools.clone() } else { o.pool.iter().cloned().collect() };
    ensure!(o.solo.is_none() || entries.len() <= 1, "--solo is one node: a --pool list does not apply");
    let mut user_pools: Vec<(crate::noid::pool::PoolUrl, fee::Proto)> = Vec::new();
    for p in &entries {
        ensure!(!o.wallet.is_empty(), "--wallet is required: your PROX address / holder id (the pool login is ADDRESS.WORKER)");
        user_pools.push(match innov::transport_url(p) {
            Some(t) => (parse_pool(&t)?, fee::Proto::Innov),
            None => (parse_pool(p)?, fee::Proto::Stratum),
        });
    }
    ensure!(user_pools.iter().all(|x| x.1 == user_pools[0].1), "--pool: the entries of one list must use the same protocol (all InnovLab or all stratum): the wallet and the password are the same on every entry");
    let user_pool: Option<(crate::noid::pool::PoolUrl, fee::Proto)> = user_pools.first().cloned();
    let user_url = user_pool.as_ref().map(|x| x.0.clone());
    let solo_node = match &o.solo {
        Some(s) => {
            let (host, port) = solo::parse_url(s).map_err(anyhow::Error::msg)?;
            ensure!(!o.token.is_empty(), "--solo needs --token (the node's mine.seeker.token)");
            Some(solo::Node { host, port, token: o.token.clone(), name: o.seeker_name.clone() })
        }
        None => None,
    };
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
    let devices = open_devices(&o.devices)?;
    let dir = crate::uv::state_dir(&o.uv);
    let buses: Vec<String> = devices.iter().map(|&d| gpu::device_pci_bus_id(d)).collect::<Result<_>>()?;
    o.temp.validate(devices.len())?;
    let target = match (&user_url, &solo_node) {
        (Some(u), _) => format!(
            "pool {u}{}{}",
            if user_pool.as_ref().is_some_and(|x| x.1 == fee::Proto::Innov) { " (InnovLab protocol)" } else { "" },
            if user_pools.len() > 1 { format!(" (primary; backup: {})", user_pools[1..].iter().map(|x| x.0.to_string()).collect::<Vec<_>>().join(", ")) } else { String::new() }
        ),
        (_, Some(n)) => format!("solo {n}"),
        _ => unreachable!(),
    };
    println!("{} {} | PROX ({ALGO_NAME}, txid-blake2b) | {target} | worker {} | {} GPU(s) {:?}", crate::PRODUCT, env!("CARGO_PKG_VERSION"), o.worker, devices.len(), devices);
    clocks::set_bus_resolver(gpu::device_pci_bus_id); // the clock code must not load pearlgpu for this coin
    // FFG 1.8.0 (recover.rs, FFGN 1.0.13): after a fault-restart of the whole process (faults::restart_self) the faulted GPU gets
    // its safe state (the driver's reset, a probe process, not above its crash - 2 steps) before this run sets clocks or opens it
    crate::recover::at_restart(&buses, &dir, o.uv.step, &stop, "recover");
    clocks::recover(&dir);
    let clock_opts = clocks::ClockOpts {
        core_mhz: match &o.lock_cclock { Some(s) => clocks::expand_u32(s, devices.len(), "--lock-cclock")?, None => vec![] },
        mem_mhz: match &o.lock_mclock { Some(s) => clocks::expand_u32(s, devices.len(), "--lock-mclock")?, None => vec![] },
        mem_off: match &o.moffset { Some(s) => clocks::expand_i32(s, devices.len(), "--moffset")?, None => vec![] },
        ..clocks::ClockOpts::policy(crate::coins::spec(crate::coins::Coin::Prox).mem)
    };
    let mut clock_opts = clock_opts;
    // ffgprox 0.1.1: the memory clock locked at the lowest value that keeps the full core range (BLAKE2b does not use the video
    // memory: ~+1 % hash rate at a power limit, several W less at a locked core); not with --lock-mclock / --moffset /
    // --no-mem-lock, and not on a GPU whose HiveOS OC profile locks the memory itself
    if clock_opts.mem_mhz.is_empty() && clock_opts.mem_off.is_empty() && !o.no_mem_lock {
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
                println!("clocks: PROX default memory lock {:?} MHz (the lowest that keeps the full core clock range; --no-mem-lock leaves it alone)", clock_opts.mem_mhz);
            }
        }
    }
    // --auto-lock (1.7.8): the saved / measured core lock per GPU (a sweep runs here, before this run touches any clock)
    let mut o = o;
    if crate::autolock::active() {
        let cards: Vec<crate::autolock::Card> = devices.iter().enumerate().map(|(p, &d)| crate::autolock::Card { dev: d, bus: buses[p].clone(), kernel: "prox-a".into() }).collect();
        match crate::autolock::resolve("prox", &cards, &mut clock_opts.core_mhz, &clock_opts.mem_mhz, true, &dir, &dir, &stop) {
            crate::autolock::Outcome::Stop => return Ok(()),
            crate::autolock::Outcome::Done(r) => return r.map_err(anyhow::Error::msg),
            crate::autolock::Outcome::Mine { swept } => {
                if swept {
                    (o.uv.reset, o.uv.reset_all) = (false, false); // the profiles of the chosen locks were just checked / tuned
                    o.minutes = crate::autolock::remaining_minutes(o.minutes); // --minutes counts from the start, the sweep included
                }
            }
        }
    }
    let _clock_guard = clocks::apply(&devices, &clock_opts, &dir);
    // core V/F offset: the owner's offsets recorded (marker) and put back on any exit; --auto-uv (default for PROX) tunes them
    // before mining (tune.rs), --uv-offset N sets N, --no-auto-uv leaves them as they are
    let flags: Vec<(Arc<AtomicU64>, Arc<AtomicBool>, Arc<AtomicBool>)> = devices.iter().map(|_| (Arc::new(AtomicU64::new(0)), Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)))).collect();
    let mut live: Option<Arc<tune::Live>> = None;
    let mut fixed: Option<(&'static crate::nvml::Nvml, tune::Fixed)> = None;
    let _off_guard = crate::noid::offset::Guard;
    if o.uv.enabled() {
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
            // 1.7.8: an OC apply (HiveOS nvidia-oc) that resets the fixed offset is answered with setting it again
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
                println!("auto-uv: a GPU without a core lock is tuned at its clock under load (profile \"~MHz\"); --lock-cclock (RTX 30: 1400-1500) is recommended");
            }
            live = Some(tune::prepare(nv, cards, &o.uv, &dir, &stop));
        } else {
            println!("auto-uv: NVML not available: offsets are left as they are");
        }
        if stop.load(Ordering::Relaxed) {
            crate::noid::offset::restore_all();
            return Ok(());
        }
    }
    let p = devfee::Params::from_env();

    let stats = [Arc::new(Stats::default()), Arc::new(Stats::default())];
    let (ev_tx, ev_rx) = mpsc::channel::<(Src, Event)>();
    let (user_tx, user_rx) = mpsc::channel::<Submit>();
    let hashrate = [Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0))];
    let beat = Arc::new(solo::Beat::default());
    let attempts = Arc::new(AtomicU64::new(0));
    let user_link = crate::failover::Link::new(user_pools.len().max(1));
    if let Some(u) = &user_url {
        let login = if o.wallet.contains('.') || o.worker.is_empty() { o.wallet.clone() } else { format!("{}.{}", o.wallet, o.worker) };
        let urls: Vec<crate::noid::pool::PoolUrl> = user_pools.iter().map(|x| x.0.clone()).collect();
        let mk = |name: &'static str, link: Arc<crate::failover::Link>| pool::Login {
            name,
            url: u.clone(),
            urls: urls.clone(),
            passes: vec![o.password.clone(); urls.len()],
            link,
            follow: None,
            login: login.clone(),
            password: o.password.clone(),
            enabled: Arc::new(AtomicBool::new(true)),
            stop: stop.clone(),
            refused: Arc::new(AtomicU64::new(0)),
            hashrate: hashrate[0].clone(),
        };
        let sess: pool::Session = if user_pool.as_ref().is_some_and(|x| x.1 == fee::Proto::Innov) { innov::session } else { pool::session_stratum };
        for x in &user_pools {
            if fee::is_suprnova(&x.0) {
                println!("WARNING: suprnova accepts only its own miner's user agent at authorize; FreeForGem sends its own (FreeForGem/<version>) and is likely refused there: use proximamine or InnovLab");
            }
        }
        if urls.len() > 1 {
            let fb = crate::failover::opts().failback_min;
            println!(
                "pool list: {} entries, the first is the primary; failover to the next entry when a pool is unusable, {}",
                urls.len(),
                if fb > 0.0 { format!("back to the primary after 2 good checks (checked every {fb} min while on a backup, --failback-minutes)") } else { "failback is off (--failback-minutes 0)".to_string() }
            );
        }
        pool::spawn_with(mk("user", user_link.clone()), Src::User, ev_tx.clone(), user_rx, stats[0].clone(), sess);
        {
            let probe_login = mk("probe", crate::failover::Link::new(1));
            let labels: Vec<String> = urls.iter().map(|u| format!("{}:{}", u.host, u.port)).collect();
            crate::failover::spawn_prober("user", user_link.clone(), stop.clone(), move |e| labels[e].clone(), move |e| pool::probe(&probe_login, e, crate::failover::opts().probe_timeout, sess));
        }
    } else if let Some(n) = solo_node {
        solo::spawn(n, ev_tx.clone(), user_rx, stats[0].clone(), beat.clone(), attempts.clone(), stop.clone());
    }
    drop(ev_tx);
    // dev fee: a second login on the user's pool, the dev account on proximamine as the fallback (fee.rs)
    // the same-pool fee login follows the user's entry: one target per entry; a list with a suprnova entry (its authorize is gated to
    // another miner's user agent) uses the dev account for the whole run, as a single suprnova pool does
    let same_targets: Option<Vec<fee::Target>> = if user_pools.is_empty() { None } else { user_pools.iter().map(|(u, k)| fee::same_pool_target(Some((u, *k)))).collect() };
    let dev_t = fee::dev_target()?;
    if devfee::FEE_PASS_PROX.is_empty() {
        eprintln!("WARNING: this is a DEV build without the PROX dev-fee secret: the proximamine fee login will be refused (not a release)");
    }
    let mut route = fee::Route {
        same: same_targets.map(|ts| fee::spawn_following(ts, Some(user_link.clone()), "fee", stats[1].clone(), stop.clone(), hashrate[1].clone())),
        dev: fee::spawn(dev_t, "fee-dev", stats[1].clone(), stop.clone(), hashrate[1].clone()),
        on_dev: false,
    };
    let fee_tx = route.active().tx.clone(); // the fee shares' channel (swapped on a fallback)
    println!(
        "devfee: {}% (one {}s slot per {} min cycle, random position) on {}{}",
        devfee::FEE_PERCENT,
        p.slot.as_secs(),
        p.cycle.as_secs() / 60,
        route.active().target.describe(),
        if route.same.is_some() { format!(" (fallback: {})", route.dev.target.describe()) } else if user_url.is_none() { " (solo: the dev account)".into() } else { " (suprnova: the dev account)".into() }
    );
    let sh = Arc::new(Shared {
        stop: stop.clone(),
        cur: Mutex::new(None),
        jgen: [AtomicU64::new(0), AtomicU64::new(0)],
        subs: [Mutex::new(user_tx), Mutex::new(fee_tx)],
        stats: stats.clone(),
        next_id: AtomicU64::new(10),
        hits: AtomicU64::new(0),
        stale: AtomicU64::new(0),
        solo: user_url.is_none(),
        beat,
        attempts,
    });
    // faults by GPU UUID (faults.rs, as for Pearl / NOID): which GPU failed (driver Xid), remembered for the next start; a GPU
    // that crashed repeatedly at its base offset is left out for an hour
    let watch = crate::faults::Watch::new(&devices.iter().enumerate().map(|(p, &d)| (d, buses[p].clone(), gpu::device_name(d))).collect::<Vec<_>>());
    let mut excluded = vec![false; devices.len()];
    let mut gstats = Vec::new();
    let mut handles = Vec::new();
    for (pos, &dev) in devices.iter().enumerate() {
        let gs = Arc::new(GpuStat {
            dev,
            pos,
            name: gpu::device_name(dev),
            bus: buses[pos].clone(),
            label: Mutex::new(String::new()),
            nonces: AtomicU64::new(0),
            alive: AtomicBool::new(true),
            bad: flags[pos].0.clone(),
            err: Mutex::new(None),
            pause: flags[pos].1.clone(),
            paused: flags[pos].2.clone(),
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
        handles.push(std::thread::Builder::new().name(format!("prox-gpu{dev}")).spawn(move || worker(dev, pos, sh2, gs2))?);
        println!("GPU {dev}: {} (bus {}, sm_{}, {} SMs), kernel A (ffgprox)", gs.name, crate::uv::short_bus(&gs.bus), gpu::compute_capability(dev), gpu::sm_count(dev));
        gstats.push(gs);
    }
    crate::templimit::start(&o.temp, &devices.iter().enumerate().map(|(pos, &d)| (d, buses[pos].clone(), gpu::device_name(d), pos)).collect::<Vec<_>>(), &stop);
    let nvml = crate::nvml::Nvml::load().ok();
    let power: Option<(&crate::nvml::Nvml, Vec<Option<crate::nvml::Dev>>)> = nvml.as_ref().map(|n| (n, buses.iter().map(|b| n.by_bus(b).ok()).collect()));

    let (start, deadline) = (Instant::now(), o.minutes.map(|m| Duration::from_secs_f64(m * 60.0)));
    let mut jobs: [Option<Arc<PJob>>; 2] = [None, None];
    let mut fee = DevFee::new(p, start);
    println!("devfee: next slot in {:.1} min", fee.next_slot_in(start).as_secs_f64() / 60.0);
    let mut published: Option<(Src, usize)> = None;
    let mut fee_conn = false;
    let mut last_n = vec![0u64; gstats.len()];
    let mut dead_seen = excluded.clone();
    let mut ok_cleared = false;
    if excluded.iter().all(|&e| e) {
        // every GPU is left out: wait (no restart loop for HiveOS) until the first one may mine again, then start over
        let w = watch.as_ref().expect("excluded GPUs come from the watch");
        let left = w.cards.iter().filter_map(|c| crate::faults::excluded(&dir, &c.uuid)).min().unwrap_or(60);
        println!("faults: every mining GPU is left out after repeated crashes (see above); waiting {} min, then the miner starts again", left / 60 + 1);
        let t = Instant::now();
        while !stop.load(Ordering::Relaxed) && t.elapsed() < Duration::from_secs(left + 5) {
            std::thread::sleep(Duration::from_secs(1));
        }
        if !stop.load(Ordering::Relaxed) {
            println!("faults: {}", crate::faults::restart_self("a GPU may mine again"));
        }
        crate::noid::offset::restore_all();
        return Ok(());
    }
    let mut win = Instant::now();
    let mut watchdog = crate::failover::Watch::default(); // accepted-shares watchdog of the user's session (not in solo: no link entry)
    let (mut mined, mut last_loop) = (Duration::ZERO, Instant::now());
    while !stop.load(Ordering::Relaxed) && deadline.map_or(true, |d| start.elapsed() < d) {
        let im = crate::idle::mode();
        if im != crate::idle::Mode::Pause {
            mined += last_loop.elapsed();
        }
        last_loop = Instant::now();
        if (0..gstats.len()).any(|i| !dead_seen[i] && !gstats[i].alive.load(Ordering::Relaxed)) {
            // which GPU failed (driver Xid events), remembered for the next start (faults.rs); a CUDA fault may stop every worker
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
            for (k, x) in &v.culprits {
                let w = watch.as_ref().unwrap();
                let c = &w.cards[*k];
                let r = crate::faults::record_crash(&dir, &c.uuid, &c.bus, &c.name, *x, w.offset(*k));
                // with --auto-uv a crash at a tuned offset makes the next start tune below it; at the base: after 2 crash starts the
                // GPU is left out for an hour
                let tunable = o.uv.auto && r.offset > 0;
                if !tunable && r.n >= crate::faults::LOWER_AFTER {
                    let mut r = r.clone();
                    r.excluded_until = crate::uv::now() + crate::faults::EXCLUDE_SECS;
                    crate::faults::put(&dir, &c.uuid, &r);
                    println!("faults: {}", crate::faults::excluded_line(&c.label(), &r, crate::faults::EXCLUDE_SECS));
                } else {
                    println!("faults: {} crash {} in a row at offset {} remembered{}", c.label(), r.n, crate::uv::signed(r.offset), if tunable { " (PROX auto-uv tunes below it at the next start)" } else { "" });
                }
            }
            if v.culprits.is_empty() && alive_n == 0 && dead.len() > 1 && crate::faults::unknown_crash(&dir) >= 3 {
                if let Some(w) = &watch {
                    for k in 0..w.cards.len() {
                        let c = &w.cards[k];
                        crate::faults::record_crash(&dir, &c.uuid, &c.bus, &c.name, 0, w.offset(k));
                    }
                    println!("faults: 3 crashes in a row without an Xid: every GPU steps back from its offset at the next start");
                }
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
            crate::noid::offset::restore_all();
            clocks::restore_all();
            println!("faults: {}", crate::faults::restart_self("every GPU worker stopped"));
            bail!("all GPU workers have stopped");
        }
        if let Some(l) = crate::status::summary_due() {
            println!("{l}");
        }
        let mut evs: Vec<(Src, Event)> = ev_rx.try_iter().collect();
        {
            let act = route.active() as *const fee::Client;
            for c in route.same.iter().chain(std::iter::once(&route.dev)) {
                let mine = std::ptr::eq(c, act);
                for e in c.ev.try_iter() {
                    if mine {
                        evs.push(e); // the inactive fee login's events are dropped (it is being closed)
                    }
                }
            }
        }
        for (src, ev) in evs {
            let tag = if src == Src::Fee { "fee" } else { "user" };
            match ev {
                Event::Job(j) => {
                    if src == Src::User || fee.active() {
                        println!(
                            "new job {} [{tag}] slot {} message {} B k={} (expires in {}s){}",
                            j.job_id,
                            j.slot,
                            j.prefix.len(),
                            j.k,
                            j.expires.map_or(0, |e| e.saturating_duration_since(Instant::now()).as_secs()),
                            if j.beat.is_some() { " (beat set)" } else { "" }
                        );
                    }
                    if j.clean {
                        sh.jgen[src.idx()].fetch_add(1, Ordering::Relaxed);
                    }
                    jobs[src.idx()] = Some(Arc::new(j));
                }
                Event::Pause(why) => {
                    if jobs[src.idx()].take().is_some() {
                        println!("{tag}: {why}: current job dropped, waiting for the next");
                    }
                    sh.jgen[src.idx()].fetch_add(1, Ordering::Relaxed);
                }
                Event::Disconnected => {
                    if src == Src::Fee {
                        jobs[1] = None;
                        sh.jgen[1].fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }
        for (i, j) in jobs.iter_mut().enumerate() {
            if j.as_ref().is_some_and(|j| j.expires.is_some_and(|e| Instant::now() > e)) {
                if i == 0 {
                    println!("job {} expired without a new one: idle until the next job", j.as_ref().unwrap().job_id);
                }
                *j = None;
                sh.jgen[i].fetch_add(1, Ordering::Relaxed);
            }
        }
        // developer fee schedule (devfee.rs, as for Pearl / NOID), on the mining time; the fee login: fee.rs (same pool, fallback)
        let now = start + mined;
        let want_conn = fee.conn_wanted(now);
        if fee_conn && !want_conn {
            jobs[1] = None;
        }
        fee_conn = want_conn;
        route.want(want_conn);
        // fallback to the dev account on proximamine: the same-pool fee login refused, or its shares rejected
        let why = if route.same.as_ref().is_some_and(|c| c.refused.load(Ordering::Relaxed) > 0) {
            Some("the fee login was refused".to_string())
        } else {
            let (a, r, st) = (stats[1].accepted.load(Ordering::Relaxed), stats[1].rejected.load(Ordering::Relaxed), stats[1].stale_rej.load(Ordering::Relaxed));
            (r - st.min(r) >= 3 && r - st.min(r) > a).then(|| format!("{} fee shares rejected ({a} accepted)", r - st.min(r)))
        };
        if let Some(w) = why {
            if route.fall_back(&w) {
                *sh.subs[1].lock().unwrap() = route.dev.tx.clone();
                jobs[1] = None;
                sh.jgen[1].fetch_add(1, Ordering::Relaxed);
                route.want(want_conn);
            }
        }
        let fee_ready = want_conn && jobs[1].is_some();
        match fee.tick(now, fee_ready) {
            Change::Started => println!("devfee: mining {}s for the developer ({}%) on {}", p.slot.as_secs(), devfee::FEE_PERCENT, route.active().target.describe()),
            Change::Finished => println!("devfee: back to user"),
            Change::Aborted => println!("devfee: fee connection lost mid-slot, back to user"),
            Change::Skipped => {
                println!("devfee: fee login on {} unavailable, skipping this slot (user mining continues)", route.active().target.describe());
                if route.fall_back("no fee job within the ready window") {
                    *sh.subs[1].lock().unwrap() = route.dev.tx.clone();
                    jobs[1] = None;
                    sh.jgen[1].fetch_add(1, Ordering::Relaxed);
                }
            }
            Change::Scheduled => println!("devfee: next slot in {:.1} min", fee.next_slot_in(now).as_secs_f64() / 60.0),
            Change::None => {}
        }
        let src = if fee.active() { Src::Fee } else { Src::User };
        let want = jobs[src.idx()].clone().filter(|_| im != crate::idle::Mode::Pause).map(|j| (src, Arc::as_ptr(&j) as usize, j));
        if want.as_ref().map(|w| (w.0, w.1)) != published {
            published = want.as_ref().map(|w| (w.0, w.1));
            *sh.cur.lock().unwrap() = want.map(|(s, _, j)| Arc::new(Assign { src: s, job: j, jgen: sh.jgen[s.idx()].load(Ordering::Relaxed) }));
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
                parts.push(format!("GPU{} {:.3}{}{}{}{}", g.dev, ghs, pw, if g.alive.load(Ordering::Relaxed) { "" } else { " DEAD" }, crate::status::tag(g.dev), crate::templimit::tag(g.dev)));
            }
            hashrate[0].store((tot * 1e9) as u64, Ordering::Relaxed);
            hashrate[1].store(if fee.active() { (tot * 1e9) as u64 } else { 0 }, Ordering::Relaxed);
            let pp = if all_w && tot_w > 0.0 { format!(", {:.0} W, {:.4} GH/W", tot_w, tot / tot_w) } else { String::new() };
            let bad: u64 = gstats.iter().map(|g| g.bad.load(Ordering::Relaxed)).sum();
            let (u, f) = (&stats[0], &stats[1]);
            let ld = |a: &AtomicU64| a.load(Ordering::Relaxed);
            {
                let hps = jobs[0].as_ref().map(|j| 2f64.powi(j.k as i32));
                let allowed = !fee.active() && im == crate::idle::Mode::Full && u.connected.load(Ordering::Relaxed) != 0;
                crate::failover::watch_tick(&mut watchdog, &user_link, "user", el.as_secs_f64(), allowed, tot * 1e9, hps, ld(&u.accepted), ld(&u.rejected), ld(&u.submitted));
            }
            println!(
                "[{:>4}s] {} | total {:.3} GH/s{} | hits {} (bad {}, stale {}) | user sub {} acc {} rej {} (stale {}, low-diff {}{}) | fee sub {} acc {} rej {}{}{}{}",
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
                ld(&u.low_diff),
                if sh.solo { format!(", not-best {}", ld(&u.skipped)) } else { String::new() },
                ld(&f.submitted),
                ld(&f.accepted),
                ld(&f.rejected),
                match (fee.active(), im) {
                    (_, crate::idle::Mode::Pause) => "  [PAUSED: PC in use]".to_string(),
                    (_, crate::idle::Mode::Light(p)) => format!("  [LIGHT {p} %: PC in use]{}", if fee.active() { " [DEVFEE]" } else { "" }),
                    (true, _) => "  [DEVFEE]".to_string(),
                    _ => String::new(),
                },
                format!("{}{}", if u.connected.load(Ordering::Relaxed) == 0 { "  [DISCONNECTED]" } else { "" }, crate::clocks::unheld_tag()),
                if jobs[0].is_none() && !fee.active() && im == crate::idle::Mode::Full { "  [NO JOB]" } else { "" }
            );
            win = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    stop.store(true, Ordering::SeqCst);
    crate::noid::offset::restore_all();
    clocks::restore_all();
    for h in handles {
        let _ = h.join();
    }
    std::thread::sleep(Duration::from_secs(3));
    let n: u64 = gstats.iter().map(|g| g.nonces.load(Ordering::Relaxed)).sum();
    let g = |s: &Stats, f: fn(&Stats) -> &AtomicU64| f(s).load(Ordering::Relaxed);
    println!(
        "done: {:.1} G nonces in {:?} ({:.3} GH/s average); hits {} (bad {}, stale {}); user: submitted {} accepted {} rejected {} | fee: submitted {} accepted {} rejected {}",
        n as f64 / 1e9,
        start.elapsed(),
        n as f64 / start.elapsed().as_secs_f64() / 1e9,
        sh.hits.load(Ordering::Relaxed),
        gstats.iter().map(|g| g.bad.load(Ordering::Relaxed)).sum::<u64>(),
        sh.stale.load(Ordering::Relaxed),
        g(&stats[0], |s| &s.submitted),
        g(&stats[0], |s| &s.accepted),
        g(&stats[0], |s| &s.rejected),
        g(&stats[1], |s| &s.submitted),
        g(&stats[1], |s| &s.accepted),
        g(&stats[1], |s| &s.rejected),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn pool_detection() {
        assert!(super::looks_like_prox_pool("stratum+ssl://pool.proximamine.com:3334"));
        assert!(super::looks_like_prox_pool("stratum+tcp://proxi.suprnova.cc:3355"));
        assert!(!super::looks_like_prox_pool("stratum+ssl://eu2.innovlab.cc:19601"));
        let u = super::parse_pool("stratum+ssl://pool.proximamine.com:3334").unwrap();
        assert!(u.tls && u.port == 3334);
        let u = super::parse_pool("stratum+tcp://127.0.0.1:13355").unwrap();
        assert!(!u.tls);
    }
}

