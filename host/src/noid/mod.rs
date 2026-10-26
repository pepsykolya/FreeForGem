// FreeForGem — see LICENSE
//! NOID (Parano1d) mining: `--algo noid` (or a NOID pool URL).  GPU search in the closed-source NOID library (gpu.rs), every hit
//! re-checked on the CPU with the official crate (cpu.rs) before it is submitted, pool client `parano1d-stratum-v1` over TLS
//! (pool.rs), 1% developer fee on the same pool (devfee.rs schedule), NOID-specific undervolt (tune.rs, offset.rs).
//!
//!   freeforgem --algo noid --pool stratum+ssl://eu2.innovlab.cc:19601 --wallet o1q... [--noid-mode eff|speed]
pub mod cpu;
pub mod gpu;
pub mod offset;
pub mod pool;
pub mod tune;

use crate::clocks;
use crate::devfee::{self, Change, DevFee};
use crate::nvml::Nvml;
use crate::stratum::{Src, Stats};
use anyhow::{Result, bail, ensure};
use pool::{Event, NJob, Submit};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const ALGO_NAME: &str = "parano1d";

pub struct Opts {
    pub pool: String,
    pub wallet: String,
    pub worker: String,
    pub password: String,
    pub minutes: Option<f64>,
    pub devices: Option<Vec<i32>>,
    pub mode: i32,
    pub uv: crate::uv::Opts,
    pub lock_cclock: Option<String>,
    pub lock_mclock: Option<String>,
    pub moffset: Option<String>,
}

pub fn mode_of(s: &str) -> Result<i32> {
    match s.to_ascii_lowercase().as_str() {
        "eff" | "efficiency" | "eco" => Ok(gpu::MODE_EFF),
        "speed" | "fast" | "max" => Ok(gpu::MODE_SPEED),
        x => bail!("--noid-mode {x:?}: eff or speed"),
    }
}
pub fn mode_name(m: i32) -> &'static str {
    if m == gpu::MODE_SPEED { "speed" } else { "eff" }
}

/// A NOID pool URL (`--algo` not given): the scheme stratum+ssl/tls with a NOID host, or a host name with "noid"/"parano1d".
pub fn looks_like_noid_pool(url: &str) -> bool {
    let u = url.to_ascii_lowercase();
    let host = u.split_once("://").map_or(u.as_str(), |x| x.1);
    host.contains("noid") || host.contains("parano1d") || host.contains("innovlab")
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Vec<u8> {
    crate::stratum::hex_decode(s).expect("hex")
}

/// Loads the library and checks the driver; the list of usable devices (`devices` or all) with their labels.
fn open_devices(devices: &Option<Vec<i32>>, mode: i32) -> Result<Vec<i32>> {
    gpu::load()?;
    let n = gpu::device_count();
    ensure!(n > 0, "no NVIDIA GPU visible (NOID needs an NVIDIA RTX 30/40/50 or newer and driver R580+; check CUDA_VISIBLE_DEVICES)");
    if let Some(v) = gpu::driver_version() {
        ensure!(v >= 13000, "NOID needs an NVIDIA driver R580 or newer (CUDA 13.0); this driver supports CUDA {}.{} - update the driver (HiveOS: nvidia-driver-update 580)", v / 1000, v % 1000 / 10);
    }
    let devs: Vec<i32> = match devices {
        Some(d) => d.clone(),
        None => (0..n as i32).collect(),
    };
    let mut ok = Vec::new();
    for &d in &devs {
        ensure!(d >= 0 && (d as usize) < n, "device {d} does not exist ({n} NVIDIA device(s) visible)");
        let l = gpu::kernel_label_for(d, mode);
        if l.is_empty() {
            let cc = gpu::compute_capability(d);
            eprintln!("WARNING: GPU{d} ({}, sm_{cc}) is not supported by NOID (needs sm_80+: RTX 30/40/50, A100/H100): skipped", gpu::device_name(d));
            continue;
        }
        ok.push(d);
    }
    ensure!(!ok.is_empty(), "no GPU usable for NOID among {devs:?}");
    Ok(ok)
}

// ---------------------------------------------------------------------------------------------------------------------
// self-test: official vectors (CPU), then per GPU and per mode: GPU digests vs the official crate, a full search launch's
// candidate set vs the CPU
// ---------------------------------------------------------------------------------------------------------------------

pub fn self_test(devices: &Option<Vec<i32>>) -> Result<()> {
    println!("== NOID self-test ==");
    let t = Instant::now();
    for (i, (f, d)) in cpu::VECTORS.iter().enumerate() {
        let raw: [u8; 256] = unhex(f).try_into().unwrap();
        ensure!(hex(&cpu::digest(&raw)) == *d, "official crate digest of vector {i} differs from the stored test vector");
        let n0 = u128::from_le_bytes(raw[160..176].try_into().unwrap());
        ensure!(hex(&cpu::digests(&raw, n0, 1)[0]) == *d, "batch digest of vector {i} differs");
    }
    println!("[ok] CPU: {} official test vectors (noid_poseidon2b, sponge and batch) in {:?}", cpu::VECTORS.len(), t.elapsed());
    let devs = open_devices(devices, gpu::MODE_EFF)?;
    let mut failed = Vec::new();
    for &d in &devs {
        for mode in [gpu::MODE_EFF, gpu::MODE_SPEED] {
            let r = std::thread::scope(|s| s.spawn(|| gpu_self_test(d, mode)).join());
            match r {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    println!("[FAIL] GPU{d} {}: {e:#}", mode_name(mode));
                    failed.push((d, mode_name(mode)));
                }
                Err(_) => {
                    println!("[FAIL] GPU{d} {}: the check thread panicked", mode_name(mode));
                    failed.push((d, mode_name(mode)));
                }
            }
        }
    }
    ensure!(failed.is_empty(), "NOID self-test FAILED on {failed:?}");
    println!("GPUs checked: {devs:?} (modes eff and speed)");
    println!("== NOID self-test PASSED ==");
    Ok(())
}

fn gpu_self_test(dev: i32, mode: i32) -> Result<()> {
    gpu::init(dev, mode)?;
    let t = Instant::now();
    println!("GPU{dev}: {} kernel {}", gpu::device_name(dev), gpu::kernel_label());
    let ff = [0xffu8; 32];
    let mut total = 0;
    for (i, (f, want)) in cpu::VECTORS.iter().enumerate() {
        let raw: [u8; 256] = unhex(f).try_into().unwrap();
        gpu::set_job(&raw, &ff)?;
        let lo = u32::from_le_bytes(raw[160..164].try_into().unwrap());
        ensure!(hex(&gpu::digest(lo, 1)?[0]) == *want, "GPU digest of official vector {i} is wrong");
        let start = 0x1000 * i as u32;
        let g = gpu::digest(start, 2048)?;
        let mut base = raw;
        base[160..164].copy_from_slice(&start.to_le_bytes());
        let c = cpu::digests(&raw, u128::from_le_bytes(base[160..176].try_into().unwrap()), 2048);
        let bad = g.iter().zip(&c).filter(|(a, b)| a != b).count();
        ensure!(bad == 0, "vector {i}: {bad} of 2048 GPU digests differ from the official crate");
        total += 2048;
    }
    println!("[ok] GPU{dev} {}: {} official vectors + {total} GPU digests == official crate", mode_name(mode), cpu::VECTORS.len());
    // one search launch with an easy candidate target (~1/256): the GPU's candidate set must equal the CPU's exactly
    let raw: [u8; 256] = unhex(cpu::VECTORS[5].0).try_into().unwrap();
    let (got, want, n) = search_check(&raw, 1, 1.0 / 256.0)?;
    ensure!(got == want, "search: GPU candidate set ({} nonces) differs from the CPU ({} nonces) on {n} nonces", got.len(), want.len());
    println!("[ok] GPU{dev} {}: search launch over {n} nonces: {} candidates, identical to the CPU set ({:?})", mode_name(mode), got.len(), t.elapsed());
    gpu::release();
    Ok(())
}

/// Field set with the nonce index bits (bytes 160..163) zero.
fn zero_low(raw: &[u8; 256]) -> [u8; 256] {
    let mut f = *raw;
    f[160..164].fill(0);
    f
}

/// Candidate threshold (hi64 of the digest) that a fraction `p` of nonces passes.
fn thresh(p: f64) -> u64 {
    ((p.clamp(0.0, 1.0) * 18446744073709551615.0) as u64).max(1)
}

fn target_hi(th: u64) -> [u8; 32] {
    let mut t = [0xffu8; 32];
    t[24..32].copy_from_slice(&th.to_le_bytes());
    t
}

/// The expected candidate set (sorted nonce indexes) of nonces 0 .. n of `raw` for threshold `th`, on the CPU.
pub fn cpu_candidates(raw: &[u8; 256], n: u64, th: u64) -> Vec<u32> {
    let f = zero_low(raw);
    let n0 = u128::from_le_bytes(f[160..176].try_into().unwrap());
    let d = cpu::digests(&f, n0, n as usize);
    d.iter().enumerate().filter(|(_, x)| cpu::hi64(x) <= th).map(|(i, _)| i as u32).collect()
}

/// One launch of `iters` over nonces 0..: (GPU set, CPU set, nonces), both sorted.
fn search_check(raw: &[u8; 256], iters: u32, p: f64) -> Result<(Vec<u32>, Vec<u32>, u64)> {
    let f = zero_low(raw);
    let th = thresh(p);
    gpu::set_job(&f, &target_hi(th))?;
    let n = gpu::grid_nonces() * iters as u64;
    gpu::launch(0, 0, iters)?;
    let mut buf = vec![0u32; gpu::max_hits()];
    let (cnt, k) = gpu::wait(0, &mut buf)?;
    ensure!(cnt <= buf.len(), "{cnt} candidates: more than the library stores ({})", buf.len());
    let mut got = buf[..k].to_vec();
    got.sort_unstable();
    Ok((got, cpu_candidates(&f, n, th), n))
}

// ---------------------------------------------------------------------------------------------------------------------
// tuning check (child process): agent M's stress check — full-grid launches on a fixed job, every launch's candidate set
// compared with the CPU set (thousands of byte-exact checks under full load; the self-test alone does not see the edge)
// ---------------------------------------------------------------------------------------------------------------------

pub struct CheckArgs {
    pub dev: i32,
    pub mode: i32,
    pub secs: f64,
}

pub const CHECK_ITERS: u32 = 8;

/// Prints `NOID_CHECK ok|FAIL launches=.. bad=.. mhs=.. hits=..` and returns the exit code (0 ok, 1 wrong results, 2 error).
pub fn check_child(a: &CheckArgs) -> i32 {
    crate::uv::die_with_parent();
    match check_inner(a) {
        Ok((launches, bad, mhs, hits, first)) => {
            println!(
                "NOID_CHECK {} launches={launches} bad={bad} mhs={mhs:.3} hits={hits}{}",
                if bad == 0 { "ok" } else { "FAIL" },
                first.map_or(String::new(), |f| format!(" first_bad_at={f:.1}s"))
            );
            if bad == 0 { 0 } else { 1 }
        }
        Err(e) => {
            println!("NOID_CHECK error {e:#}");
            2
        }
    }
}

fn check_inner(a: &CheckArgs) -> Result<(u64, u64, f64, usize, Option<f64>)> {
    gpu::load()?;
    gpu::init(a.dev, a.mode)?;
    let raw: [u8; 256] = unhex(cpu::VECTORS[3].0).try_into().unwrap();
    let f = zero_low(&raw);
    let n = gpu::grid_nonces() * CHECK_ITERS as u64;
    let th = thresh(1500.0 / n as f64);
    gpu::set_job(&f, &target_hi(th))?;
    let want = cpu_candidates(&f, n, th);
    ensure!(!want.is_empty() && want.len() < gpu::max_hits(), "bad expected candidate count {}", want.len());
    let mut buf = vec![0u32; gpu::max_hits()];
    let (mut launches, mut bad, mut first) = (0u64, 0u64, None);
    // the GPU load starts now (the CPU reference above can take long while other checks run): the parent measures the power
    // from here on only, so the MH/W of every check compares like with like (Rig01 beta2: MH/W "losses" of 15-33 % were noise)
    println!("NOID_LOAD");
    let t0 = Instant::now();
    gpu::launch(0, 0, CHECK_ITERS)?;
    let mut s = 0;
    loop {
        let more = t0.elapsed().as_secs_f64() < a.secs;
        if more {
            gpu::launch(s ^ 1, 0, CHECK_ITERS)?;
        }
        let (cnt, k) = gpu::wait(s, &mut buf)?;
        launches += 1;
        let got = &mut buf[..k];
        got.sort_unstable();
        if cnt != want.len() || got != &want[..] {
            if bad < 3 {
                eprintln!("check: launch {launches}: {cnt} candidates, expected {} (wrong GPU results)", want.len());
            }
            first.get_or_insert(t0.elapsed().as_secs_f64());
            bad += 1;
        }
        if !more {
            break;
        }
        s ^= 1;
    }
    let mhs = (launches * n) as f64 / t0.elapsed().as_secs_f64() / 1e6;
    Ok((launches, bad, mhs, want.len(), first))
}

// ---------------------------------------------------------------------------------------------------------------------
// mining
// ---------------------------------------------------------------------------------------------------------------------

struct Assign {
    src: Src,
    job: Arc<NJob>,
    jgen: u64, // job generation of its login (stale hits: job no longer current)
}

/// Per-GPU state shared with the main thread.
pub struct GpuStat {
    pub dev: i32,
    pub pos: usize,
    pub name: String,
    pub bus: String,
    pub label: Mutex<String>,
    pub nonces: AtomicU64,
    pub alive: AtomicBool,
    pub bad: AtomicU64,      // hits that failed the CPU check (wrong GPU results)
    pub bad_seen: AtomicU64, // handled by the main thread (offset step back)
    pub watch_runs: AtomicU64,
    pub pause: Arc<AtomicBool>,  // a quick check after an OC apply asks the mining thread of this GPU to pause
    pub paused: Arc<AtomicBool>, // the mining thread is paused (no launch in flight)
    pub err: Mutex<Option<String>>, // why the worker stopped (fault attribution, faults.rs)
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
    blocks: AtomicU64,
}

const LAUNCH_SECS: f64 = 0.15;

fn worker(dev: i32, pos: usize, mode: i32, sh: Arc<Shared>, gs: Arc<GpuStat>, ready: mpsc::Sender<Result<()>>) {
    if let Err(e) = gpu::init(dev, mode) {
        let _ = ready.send(Err(e));
        gs.alive.store(false, Ordering::Relaxed);
        return;
    }
    *gs.label.lock().unwrap() = gpu::kernel_label();
    let _ = ready.send(Ok(()));
    let r = worker_loop(pos, &sh, &gs);
    if let Err(e) = r {
        eprintln!("GPU{dev}: NOID worker stopped: {e:#}");
        *gs.err.lock().unwrap() = Some(format!("{e:#}"));
    }
    gs.alive.store(false, Ordering::Relaxed);
    gpu::release();
}

struct Flight {
    a: Arc<Assign>,
    hi: u32,
    thr: u64, // candidate threshold the kernel used (hi64 of the digest)
    t: Instant,
    nonces: u64,
}

/// Candidates are reported down to this density (about 1 in 4 M nonces, ~10/s on an RTX 3070) even when the pool's share
/// target is harder: every candidate is re-computed on the CPU, so wrong GPU results show up while mining, not only as lost
/// shares.  Only candidates below the share target are submitted.
const LOOSE_THR: u64 = u64::MAX >> 22;

fn kernel_target(share: &[u8; 32]) -> ([u8; 32], u64) {
    let thr = cpu::hi64(share).max(LOOSE_THR);
    (target_hi(thr), thr)
}

/// In-mining watchdog: every WATCH_SECS (FFGP_NOID_WATCH_SECS, 0 = off) a few launches of the tuning check's fixed job are
/// compared with their CPU candidate set (the silent edge shows up as wrong sets long before it costs a visible share).
fn watch_secs() -> u64 {
    std::env::var("FFGP_NOID_WATCH_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(120)
}
const WATCH_LAUNCHES: u32 = 6;

fn worker_loop(pos: usize, sh: &Shared, gs: &GpuStat) -> Result<()> {
    let grid = gpu::grid_nonces().max(1);
    let mut iters: u32 = ((40e6 * LAUNCH_SECS) / grid as f64).clamp(1.0, 4096.0) as u32;
    let mut cur: Option<Arc<Assign>> = None;
    let mut hi_ctr: u32 = 0;
    let mut base: u64 = 0;
    let mut hi: u32 = 0;
    let mut flight: [Option<Flight>; 2] = [None, None];
    let mut buf = vec![0u32; gpu::max_hits()];
    let mut slot = 0usize;
    let mut rate: f64 = 0.0;
    let (mut tgt, mut thr) = ([0u8; 32], 0u64);
    // watchdog reference: the tuning check's fixed job and its CPU candidate set (computed once)
    let wsecs = watch_secs();
    let wf = zero_low(&unhex(cpu::VECTORS[3].0).try_into().unwrap());
    let wn = grid * CHECK_ITERS as u64;
    let wth = thresh(1500.0 / wn as f64);
    let wwant = if wsecs > 0 { cpu_candidates(&wf, wn, wth) } else { Vec::new() };
    let mut wlast = Instant::now();
    // test hook (faults.rs): a fault of this GPU after N s of mining
    let inject = crate::faults::test_fault_for(gs.dev).map(|secs| Instant::now() + Duration::from_secs(secs));
    loop {
        if inject.is_some_and(|t| Instant::now() >= t && gs.nonces.load(Ordering::Relaxed) > 0) {
            bail!("injected fault (FFG_TEST_FAULT): an illegal memory access was encountered");
        }
        if wsecs > 0 && wlast.elapsed() >= Duration::from_secs(wsecs) && flight.iter().all(Option::is_none) && !gs.pause.load(Ordering::Relaxed) {
            wlast = Instant::now();
            gpu::set_job(&wf, &target_hi(wth))?;
            let mut wrong = 0;
            for _ in 0..WATCH_LAUNCHES {
                gpu::launch(0, 0, CHECK_ITERS)?;
                let (cnt, k) = gpu::wait(0, &mut buf)?;
                let got = &mut buf[..k];
                got.sort_unstable();
                if cnt != wwant.len() || got != &wwant[..] {
                    wrong += 1;
                }
            }
            gs.watch_runs.fetch_add(WATCH_LAUNCHES as u64, Ordering::Relaxed);
            if wrong > 0 {
                gs.bad.fetch_add(wrong, Ordering::Relaxed);
                eprintln!("GPU{}: WATCHDOG: {wrong} of {WATCH_LAUNCHES} check launches gave WRONG results (silent undervolt edge?)", gs.dev);
            }
            if let Some(a) = &cur {
                gpu::set_job(&job_fields(&a.job, hi), &tgt)?;
            }
        }
        let stop = sh.stop.load(Ordering::Relaxed);
        // collect the older launch of this slot
        if let Some(fl) = flight[slot].take() {
            let (cnt, k) = gpu::wait(slot as i32, &mut buf)?;
            let dt = fl.t.elapsed().as_secs_f64();
            gs.nonces.fetch_add(fl.nonces, Ordering::Relaxed);
            // two launches are in flight: one launch takes about dt / 2 here
            let r = fl.nonces as f64 / (dt / 2.0).max(1e-3);
            rate = if rate == 0.0 { r } else { 0.8 * rate + 0.2 * r };
            if cnt > k {
                eprintln!("GPU{}: {cnt} candidates in one launch, only {k} kept", gs.dev);
            }
            for &idx in &buf[..k] {
                check_hit(sh, gs, &fl, idx);
            }
        }
        if stop || gs.pause.load(Ordering::Relaxed) {
            if flight.iter().all(Option::is_none) {
                if stop {
                    return Ok(());
                }
                // paused for a check of this GPU (after an OC apply): nothing in flight until it is over
                gs.paused.store(true, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(20));
                continue;
            }
            slot ^= 1;
            continue;
        }
        if gs.paused.swap(false, Ordering::SeqCst) {
            wlast = Instant::now();
        }
        // newest assignment
        let newest = sh.cur.lock().unwrap().clone();
        let changed = match (&cur, &newest) {
            (None, None) => false,
            (Some(a), Some(b)) => !Arc::ptr_eq(a, b),
            _ => true,
        };
        if changed {
            cur = newest;
            if let Some(a) = &cur {
                hi_ctr = hi_ctr.wrapping_add(1);
                hi = ((pos as u32 & 0xff) << 24) | (hi_ctr & 0x00ff_ffff);
                base = 0;
                (tgt, thr) = kernel_target(&a.job.share_target);
                gpu::set_job(&job_fields(&a.job, hi), &tgt)?;
            }
        }
        if wsecs > 0 && wlast.elapsed() >= Duration::from_secs(wsecs) {
            // let both launches finish, the watchdog runs on an empty pipeline
            slot ^= 1;
            continue;
        }
        let Some(a) = cur.clone() else {
            if flight.iter().all(Option::is_none) {
                std::thread::sleep(Duration::from_millis(20));
            }
            slot ^= 1;
            continue;
        };
        if base >= 1u64 << 32 {
            hi_ctr = hi_ctr.wrapping_add(1);
            hi = ((pos as u32 & 0xff) << 24) | (hi_ctr & 0x00ff_ffff);
            base = 0;
            gpu::set_job(&job_fields(&a.job, hi), &tgt)?;
        }
        if rate > 0.0 {
            iters = ((rate * LAUNCH_SECS) / grid as f64).clamp(1.0, 4096.0) as u32;
        }
        let n = (grid * iters as u64).min((1u64 << 32) - base);
        let it = (n / grid).max(1) as u32;
        let n = grid * it as u64;
        gpu::launch(slot as i32, base as u32, it)?;
        flight[slot] = Some(Flight { a, hi, thr, t: Instant::now(), nonces: n });
        base += n;
        slot ^= 1;
    }
}

/// The 256 job bytes with the nonce field set: bytes 0..3 the kernel index (0 here), 4..own the GPU's word `hi`, the rest the
/// pool's prefix.
fn job_fields(j: &NJob, hi: u32) -> [u8; 256] {
    let mut f = j.fields;
    let o = 16 * j.nonce_field;
    f[o..o + 16].fill(0);
    f[o + 4..o + 8].copy_from_slice(&hi.to_le_bytes());
    f[o + j.own_bytes..o + 16].copy_from_slice(&j.prefix);
    f
}

fn check_hit(sh: &Shared, gs: &GpuStat, fl: &Flight, idx: u32) {
    let j = &fl.a.job;
    let mut f = job_fields(j, fl.hi);
    let o = 16 * j.nonce_field;
    f[o..o + 4].copy_from_slice(&idx.to_le_bytes());
    let d = cpu::digest(&f);
    sh.hits.fetch_add(1, Ordering::Relaxed);
    if cpu::hi64(&d) > fl.thr {
        gs.bad.fetch_add(1, Ordering::Relaxed);
        eprintln!(
            "GPU{}: HIT FAILED THE CPU CHECK (wrong GPU result): job {} nonce {} digest {} — not submitted",
            gs.dev,
            j.job_id,
            hex(&f[o..o + 16]),
            hex(&d)
        );
        return;
    }
    if !cpu::below(&d, &j.share_target) {
        return; // equal top 64 bits, above the target in the low bits: a candidate, not a share
    }
    if sh.jgen[fl.a.src.idx()].load(Ordering::Relaxed) != fl.a.jgen {
        sh.stale.fetch_add(1, Ordering::Relaxed);
        return;
    }
    if j.block_target.as_ref().is_some_and(|b| cpu::below(&d, b)) {
        sh.blocks.fetch_add(1, Ordering::Relaxed);
        println!("GPU{}: share meets the BLOCK target (height {}), digest {}", gs.dev, j.height, hex(&d));
    }
    let id = sh.next_id.fetch_add(1, Ordering::Relaxed);
    let nonce: [u8; 16] = f[o..o + 16].try_into().unwrap();
    let line = pool::submit_line(id, j, &nonce);
    sh.stats[fl.a.src.idx()].submitted.fetch_add(1, Ordering::Relaxed);
    let _ = sh.subs[fl.a.src.idx()].lock().unwrap().send(Submit { id, job_id: j.job_id.clone(), line });
}

#[derive(Default)]
struct SrcState {
    job: Option<Arc<NJob>>,
}

pub fn mine(o: Opts) -> Result<()> {
    let url = pool::parse_url(&o.pool).map_err(anyhow::Error::msg)?;
    ensure!(!o.wallet.is_empty(), "--wallet is required: your NOID address (o1...)");
    crate::wintune::process(false);
    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = stop.clone();
        ctrlc::set_handler(move || {
            if stop.swap(true, Ordering::SeqCst) {
                offset::restore_all();
                clocks::restore_all();
                tune::forget_trying_on_stop();
                eprintln!("second interrupt: exiting at once");
                std::process::exit(130);
            }
            eprintln!("shutting down (finishing in-flight work)...");
        })?;
    }
    crate::wintune::on_console_close(|| {
        offset::restore_all();
        clocks::restore_all();
        tune::forget_trying_on_stop();
    });
    let devices = open_devices(&o.devices, o.mode)?;
    o.uv.check_counts(devices.len())?;
    let dir = crate::uv::state_dir(&o.uv);
    let buses: Vec<String> = devices.iter().map(|&d| gpu::device_pci_bus_id(d)).collect::<Result<_>>()?;
    println!(
        "{} {} | NOID ({ALGO_NAME}) | pool {url} | worker {} | {} GPU(s) {:?} | mode {}",
        crate::PRODUCT,
        env!("CARGO_PKG_VERSION"),
        o.worker,
        devices.len(),
        devices,
        mode_name(o.mode)
    );
    // clocks (as for Pearl), then the core offsets: owner's values recorded first, restored on any exit
    clocks::recover(&dir);
    let clock_opts = clocks::ClockOpts {
        core_mhz: match &o.lock_cclock { Some(s) => clocks::expand_u32(s, devices.len(), "--lock-cclock")?, None => vec![] },
        mem_mhz: match &o.lock_mclock { Some(s) => clocks::expand_u32(s, devices.len(), "--lock-mclock")?, None => vec![] },
        mem_off: match &o.moffset { Some(s) => clocks::expand_i32(s, devices.len(), "--moffset")?, None => vec![] },
    };
    let _clock_guard = clocks::apply(&devices, &clock_opts, &dir);
    offset::recover(&dir);
    let nv = offset::begin(&buses, &dir);
    let _off_guard = offset::Guard;
    let cards: Vec<tune::CardRef> = devices
        .iter()
        .enumerate()
        .map(|(pos, &d)| tune::CardRef { dev: d, pos, bus: buses[pos].clone(), name: gpu::device_name(d), core_lock: clock_opts.core_mhz.get(pos).copied().flatten() })
        .collect();
    let live = tune::prepare(nv, &cards, &o.uv, o.mode, &dir, &stop)?;
    if stop.load(Ordering::Relaxed) {
        return Ok(());
    }
    println!("devfee: {}% (one {}s slot per {} min cycle, random position) on the same pool", devfee::FEE_PERCENT, devfee::Params::from_env().slot.as_secs(), devfee::Params::from_env().cycle.as_secs() / 60);

    let stats = [Arc::new(Stats::default()), Arc::new(Stats::default())];
    let (ev_tx, ev_rx) = mpsc::channel::<(Src, Event)>();
    let (user_tx, user_rx) = mpsc::channel::<Submit>();
    let (fee_tx, fee_rx) = mpsc::channel::<Submit>();
    let fee_enabled = Arc::new(AtomicBool::new(false));
    let hashrate = [Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0))];
    let login = if o.wallet.contains('.') || o.worker.is_empty() { o.wallet.clone() } else { format!("{}.{}", o.wallet, o.worker) };
    pool::spawn(
        pool::Login { name: "user", url: url.clone(), login, password: o.password.clone(), enabled: Arc::new(AtomicBool::new(true)), stop: stop.clone(), hashrate: hashrate[0].clone() },
        Src::User,
        ev_tx.clone(),
        user_rx,
        stats[0].clone(),
    );
    pool::spawn(
        pool::Login {
            name: "fee",
            url: url.clone(),
            login: format!("{}.{}", devfee::FEE_WALLET_NOID, devfee::FEE_WORKER),
            password: "x".into(),
            enabled: fee_enabled.clone(),
            stop: stop.clone(),
            hashrate: hashrate[1].clone(),
        },
        Src::Fee,
        ev_tx,
        fee_rx,
        stats[1].clone(),
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
        blocks: AtomicU64::new(0),
    });
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
            bad: AtomicU64::new(0),
            bad_seen: AtomicU64::new(0),
            watch_runs: AtomicU64::new(0),
            pause: Arc::new(AtomicBool::new(false)),
            paused: Arc::new(AtomicBool::new(false)),
            err: Mutex::new(None),
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
        live.attach(pos, gs.pause.clone(), gs.paused.clone());
        let (rtx, rrx) = mpsc::channel();
        let (sh2, gs2, mode) = (sh.clone(), gs.clone(), o.mode);
        handles.push(std::thread::Builder::new().name(format!("noid-gpu{dev}")).spawn(move || worker(dev, pos, mode, sh2, gs2, rtx))?);
        rrx.recv()??;
        println!("GPU {dev}: {} (bus {}), kernel {}, core offset {}", gs.name, crate::uv::short_bus(&gs.bus), gs.label.lock().unwrap(), offset::current(&gs.bus).map_or("?".into(), crate::uv::signed));
        gstats.push(gs);
    }
    let power: Option<(&Nvml, Vec<Option<crate::nvml::Dev>>)> = nv.map(|n| (n, buses.iter().map(|b| n.by_bus(b).ok()).collect()));

    let (start, deadline) = (Instant::now(), o.minutes.map(|m| Duration::from_secs_f64(m * 60.0)));
    let mut st = [SrcState::default(), SrcState::default()];
    let mut fee = DevFee::new(devfee::Params::from_env(), start);
    println!("devfee: next slot in {:.1} min", fee.next_slot_in(start).as_secs_f64() / 60.0);
    let mut published: Option<(Src, usize)> = None; // (src, job ptr)
    let mut last_n = vec![0u64; gstats.len()];
    let mut win = Instant::now();
    let mut last_bad = 0u64;
    if excluded.iter().all(|&e| e) {
        // every GPU is left out: wait here (no restart loop for HiveOS) until the first one may mine again, then start over
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
        return Ok(());
    }
    // mining runs: the NOID checks / tunings start now, each pausing only its own GPU
    tune::start_jobs(&live);
    let mut dead_seen = excluded.clone();
    let mut ok_cleared = false;
    while !stop.load(Ordering::Relaxed) && deadline.map_or(true, |d| start.elapsed() < d) {
        if (0..gstats.len()).any(|i| !dead_seen[i] && !gstats[i].alive.load(Ordering::Relaxed)) {
            // which GPU failed (driver Xid events), remembered for the next start (faults.rs); a CUDA fault stops every worker
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
            let v = crate::faults::attribute(watch.as_ref(), &dead, alive_n, injected);
            for (k, x) in &v.culprits {
                let w = watch.as_ref().unwrap();
                let c = &w.cards[*k];
                let r = crate::faults::record_crash(&dir, &c.uuid, &c.bus, &c.name, *x, w.offset(*k));
                // with --auto-uv a crash at a tuned NOID offset makes the next start tune below it; at the base (0 or the owner's
                // negative offset) there is nothing to lower: after 2 crash starts the GPU is left out for an hour
                let tunable = o.uv.auto && r.offset > 0;
                if !tunable && r.n >= crate::faults::LOWER_AFTER {
                    let mut r = r.clone();
                    r.excluded_until = crate::uv::now() + crate::faults::EXCLUDE_SECS;
                    crate::faults::put(&dir, &c.uuid, &r);
                    println!("faults: {}", crate::faults::excluded_line(&c.label(), &r, crate::faults::EXCLUDE_SECS));
                } else {
                    println!("faults: {} crash {} in a row at offset {} remembered{}", c.label(), r.n, crate::uv::signed(r.offset), if tunable { " (NOID auto-uv tunes below it at the next start)" } else { "" });
                }
            }
            if v.culprits.is_empty() && alive_n == 0 && dead.len() > 1 {
                // nobody to blame (no Xid): after 3 such crashes in a row every GPU steps back from its offset at the next start
                if crate::faults::unknown_crash(&dir) >= 3 {
                    if let Some(w) = &watch {
                        for k in 0..w.cards.len() {
                            let c = &w.cards[k];
                            crate::faults::record_crash(&dir, &c.uuid, &c.bus, &c.name, 0, w.offset(k));
                        }
                        println!("faults: 3 crashes in a row without an Xid: every GPU steps back from its offset at the next start");
                    }
                }
            }
            for l in &v.lines {
                println!("faults: {l}");
            }
            if alive_n == 0 {
                stop.store(true, Ordering::SeqCst);
                offset::restore_all();
                clocks::restore_all();
                println!("faults: {}", crate::faults::restart_self("every GPU worker stopped"));
                bail!("all GPU workers have stopped");
            }
        }
        if let Some(l) = crate::status::summary_due() {
            println!("{l}");
        }
        if !ok_cleared && start.elapsed() >= Duration::from_secs(crate::faults::OK_SECS) {
            ok_cleared = true;
            if let Some(w) = &watch {
                let ok: Vec<String> = w.cards.iter().filter(|c| gstats.iter().any(|g| g.dev == c.dev && g.alive.load(Ordering::Relaxed))).map(|c| c.uuid.clone()).collect();
                crate::faults::mined_ok(&dir, &ok);
            }
        }
        while let Ok((src, ev)) = ev_rx.try_recv() {
            let s = &mut st[src.idx()];
            match ev {
                Event::Job(j) => {
                    println!(
                        "new job {} [{}] height {} (share target {}.., expires in {}s)",
                        j.job_id,
                        if src == Src::Fee { "fee" } else { "user" },
                        j.height,
                        hex(&j.share_target[24..32].iter().rev().copied().collect::<Vec<u8>>()),
                        j.expires.map_or(0, |e| e.saturating_duration_since(Instant::now()).as_secs())
                    );
                    // a clean job voids the older ones (their results in flight are dropped); clean=false keeps them valid
                    let clean = j.clean;
                    s.job = Some(Arc::new(j));
                    if clean {
                        sh.jgen[src.idx()].fetch_add(1, Ordering::Relaxed);
                    }
                }
                Event::Pause(why) => {
                    if s.job.take().is_some() {
                        println!("pool pause [{}] ({why}): current job dropped, waiting for the next", if src == Src::Fee { "fee" } else { "user" });
                    }
                    sh.jgen[src.idx()].fetch_add(1, Ordering::Relaxed);
                }
                Event::Disconnected => {
                    if src == Src::Fee {
                        s.job = None;
                        sh.jgen[src.idx()].fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }
        for (i, s) in st.iter_mut().enumerate() {
            if s.job.as_ref().is_some_and(|j| j.expires.is_some_and(|e| Instant::now() > e)) {
                println!("job {} expired without a new one from the pool: idle until the next job", s.job.as_ref().unwrap().job_id);
                s.job = None;
                sh.jgen[i].fetch_add(1, Ordering::Relaxed);
            }
        }
        // developer fee schedule (the Pearl schedule, devfee.rs)
        let now = Instant::now();
        let want_conn = fee.conn_wanted(now);
        if fee_enabled.swap(want_conn, Ordering::Relaxed) && !want_conn {
            st[Src::Fee.idx()].job = None;
        }
        let fee_ready = want_conn && stats[1].connected.load(Ordering::Relaxed) == 1 && st[1].job.is_some();
        match fee.tick(now, fee_ready) {
            Change::Started => println!("devfee: mining {}s for the developer ({}%)", devfee::Params::from_env().slot.as_secs(), devfee::FEE_PERCENT),
            Change::Finished => println!("devfee: back to user"),
            Change::Aborted => println!("devfee: fee connection lost mid-slot, back to user"),
            Change::Skipped => println!("devfee: fee login unavailable, skipping this slot (user mining continues)"),
            Change::Scheduled => println!("devfee: next slot in {:.1} min", fee.next_slot_in(now).as_secs_f64() / 60.0),
            Change::None => {}
        }
        let src = if fee.active() { Src::Fee } else { Src::User };
        let want = st[src.idx()].job.clone().map(|j| (src, Arc::as_ptr(&j) as usize, j));
        if want.as_ref().map(|w| (w.0, w.1)) != published {
            published = want.as_ref().map(|w| (w.0, w.1));
            *sh.cur.lock().unwrap() = want.map(|(s, _, j)| Arc::new(Assign { src: s, job: j, jgen: sh.jgen[s.idx()].load(Ordering::Relaxed) }));
        }
        // wrong GPU results while mining: step the card's offset back (tuned cards), keep mining
        for g in &gstats {
            let b = g.bad.load(Ordering::Relaxed);
            if b > g.bad_seen.swap(b, Ordering::Relaxed) {
                tune::on_bad_hit(&live, g.pos, &g.bus, g.dev);
            }
        }
        // an OC apply (HiveOS) that reset an offset this run set: set again (tuned: after a quick check); 30 s per card
        tune::watch(&live);
        let el = win.elapsed();
        if el >= Duration::from_secs(10) {
            let mut parts = Vec::new();
            let (mut tot, mut tot_w, mut all_w) = (0.0, 0.0, true);
            for (i, g) in gstats.iter().enumerate() {
                let n = g.nonces.load(Ordering::Relaxed);
                let mhs = (n - last_n[i]) as f64 / el.as_secs_f64() / 1e6;
                last_n[i] = n;
                tot += mhs;
                let w = power.as_ref().and_then(|(nv, d)| d[i].and_then(|h| nv.power_w(h)));
                let pw = match w {
                    Some(w) if w > 0.0 => {
                        tot_w += w;
                        format!(" ({:.0} W, {:.3} MH/W)", w, mhs / w)
                    }
                    _ => {
                        all_w = false;
                        String::new()
                    }
                };
                parts.push(format!("GPU{} {:.3}{}{}{}", g.dev, mhs, pw, if g.alive.load(Ordering::Relaxed) { "" } else { " DEAD" }, crate::status::tag(g.dev)));
            }
            hashrate[0].store((tot * 1e6) as u64, Ordering::Relaxed);
            hashrate[1].store(if fee.active() { (tot * 1e6) as u64 } else { 0 }, Ordering::Relaxed);
            let pp = if all_w && tot_w > 0.0 { format!(", {:.0} W, {:.3} MH/W", tot_w, tot / tot_w) } else { String::new() };
            let bad: u64 = gstats.iter().map(|g| g.bad.load(Ordering::Relaxed)).sum();
            let (u, f) = (&stats[0], &stats[1]);
            println!(
                "[{:>4}s] {} | total {:.3} MH/s{} | hits {} (bad {}, stale {}) | user sub {} acc {} rej {} (stale {}) | fee sub {} acc {} rej {}{}{}{}",
                start.elapsed().as_secs(),
                parts.join("  "),
                tot,
                pp,
                sh.hits.load(Ordering::Relaxed),
                bad,
                sh.stale.load(Ordering::Relaxed) + u.stale_drop.load(Ordering::Relaxed) + f.stale_drop.load(Ordering::Relaxed),
                u.submitted.load(Ordering::Relaxed),
                u.accepted.load(Ordering::Relaxed),
                u.rejected.load(Ordering::Relaxed),
                u.stale_rej.load(Ordering::Relaxed),
                f.submitted.load(Ordering::Relaxed),
                f.accepted.load(Ordering::Relaxed),
                f.rejected.load(Ordering::Relaxed),
                if fee.active() { "  [DEVFEE]" } else { "" },
                if u.connected.load(Ordering::Relaxed) == 0 { "  [DISCONNECTED]" } else { "" },
                if st[0].job.is_none() && !fee.active() { "  [NO JOB]" } else { "" }
            );
            if bad > last_bad {
                last_bad = bad;
            }
            win = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    stop.store(true, Ordering::SeqCst);
    offset::restore_all();
    clocks::restore_all();
    for h in handles {
        let _ = h.join();
    }
    std::thread::sleep(Duration::from_secs(3));
    let n: u64 = gstats.iter().map(|g| g.nonces.load(Ordering::Relaxed)).sum();
    let g = |s: &Stats, f: fn(&Stats) -> &AtomicU64| f(s).load(Ordering::Relaxed);
    println!(
        "done: {:.3} G nonces in {:?}; hits {} (bad {}, stale {}); user: submitted {} accepted {} rejected {} | fee: submitted {} accepted {} rejected {}",
        n as f64 / 1e9,
        start.elapsed(),
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
