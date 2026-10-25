// FreeForGemPear — see LICENSE
//! FreeForGem (formerly FreeForGemPear): Pearl (PRL) GPU miner host: pool client + job/commitment logic around libpearlgpu;
//! Common Foundry (CMFD) through the FreeForgeMiner engine (cmfd.rs); NOID (Parano1d) around libnoidgpu (noid/).
//!
//!   freeforgempear --pool host:port --wallet ADDR [--worker NAME] [--devices 0,1] [--minutes N]
//!   freeforgempear --self-test [--devices 0]
//!   freeforgempear --pool cmfd+tls://IP:PORT?pin=HEX --wallet cmfd1...   (Common Foundry, see cmfd.rs)

// Console output must never stop the miner.  The std println!/eprintln! panic when stdout/stderr is gone: HiveOS
// `miner stop` sends SIGINT to the whole pipeline of h-run.sh and the `tee` behind the miner can exit first; on Windows
// Ctrl+C ends the PowerShell pipeline of start.ps1 that reads the miner's output.  A panic in the shutdown path killed
// the miner after the first GPU's undervolt was put back and left the others changed (1.5.x, 1.6.0).  These macros shadow
// the std ones in the whole crate (every module below) and drop write errors; SIGPIPE is ignored by the Rust runtime on
// unix, so a write to a closed pipe only returns EPIPE.
macro_rules! println {
    () => {{
        use std::io::Write as _;
        let _ = writeln!(std::io::stdout());
    }};
    ($($t:tt)*) => {{
        use std::io::Write as _;
        let _ = writeln!(std::io::stdout(), $($t)*);
    }};
}
macro_rules! print {
    ($($t:tt)*) => {{
        use std::io::Write as _;
        let _ = write!(std::io::stdout(), $($t)*);
    }};
}
macro_rules! eprintln {
    () => {{
        use std::io::Write as _;
        let _ = writeln!(std::io::stderr());
    }};
    ($($t:tt)*) => {{
        use std::io::Write as _;
        let _ = writeln!(std::io::stderr(), $($t)*);
    }};
}

mod amd_uv;
mod clocks;
mod cmfd;
mod cmfd_offset;
mod devfee;
mod gpu;
mod mining;
mod noid;
mod nvml;
mod stratum;
mod uv;
mod wintune;
mod worker;

use anyhow::{Result, bail, ensure};
use devfee::{Change, DevFee};
use mining::*;
use primitive_types::U256;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::collections::BTreeMap;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};
use stratum::{Event, Login, PoolJob, Src, Stats, Submit};
use worker::{Assignment, GpuStat, Shared};
use zk_pow::api::sanity_checks::penalized_target_bound;
use zk_pow::ffi::plain_proof::PlainProof;

/// Product name (the multi-coin miner: Pearl, CMFD, NOID).  The Pearl-only line stays FreeForGemPear.
pub const PRODUCT: &str = "FreeForGem";

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn hit_hash(h: &gpu::Hit) -> [u8; 32] {
    let mut out = [0u8; 32];
    for (i, w) in h.jackpot.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
    }
    out
}

fn test_header(seed: u8) -> Vec<u8> {
    (0..76u32).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)).collect()
}

#[derive(Clone, Copy, PartialEq)]
enum TargetMode {
    Base,    // pool sends the per-unit base target, scaled by the adjustment factor
    Jackpot, // pool already sends the final jackpot threshold
}

fn final_target(job: &PoolJob, pool_diff: Option<f64>, mode: TargetMode) -> Result<U256> {
    let base = match (&job.target, job.difficulty.or(pool_diff)) {
        (Some(t), _) => U256::from_str_radix(t, 16)?,
        (None, Some(d)) if d > 0.0 => U256::MAX / U256::from(d.max(1.0) as u128),
        _ => bail!("job carries neither target nor difficulty"),
    };
    Ok(match mode {
        TargetMode::Jackpot => base,
        TargetMode::Base => penalized_target_bound(base, &mining_config()).unwrap_or(U256::MAX),
    })
}

/// Everything that must hold before a hit may be submitted. Returns (proof, jackpot hash).
fn prove_hit(job: &Job, row0: &[u8], hit: &gpu::Hit, a_seed: &[u8; 32], target: U256) -> Result<PlainProof> {
    let proof = job.build_proof(row0, hit.row as usize, hit.col as usize);
    ensure!(proof.a.proof.root == job.hash_a(row0), "A root in proof != incremental hash");
    let (cpu, _bs, as_) = job.cpu_jackpot(&proof)?;
    let gpu_hash = hit_hash(hit);
    if as_ != *a_seed || cpu != gpu_hash {
        bail!(
            "GPU/CPU JACKPOT MISMATCH tile=({}, {}) gpu={} cpu={} a_seed_host={} a_seed_verifier={}",
            hit.row,
            hit.col,
            hex(&gpu_hash),
            hex(&cpu),
            hex(a_seed),
            hex(&as_)
        );
    }
    ensure!(U256::from_little_endian(&cpu) <= target, "jackpot above target");
    job.verify(&proof)?;
    Ok(proof)
}

fn set_job_gpu(job: &Job) -> Result<()> {
    gpu::set_job(&job.br_t, &job.q1, &job.q2, N, k())
}

/// Pearl self-test: the CPU reference checks once, then the GPU checks on every device of `devices` (default: all visible),
/// one device after another, each in its own thread (the library state is per thread).
fn self_test(devices: Option<&[i32]>) -> Result<()> {
    println!("== self-test ==");
    let t0 = Instant::now();
    let job = Job::new(&test_header(7), 3, m())?;
    println!("[ok] public_params_sanity_check for m={} n={N} k={} rank={RANK}; job built in {:?}", m(), k(), t0.elapsed());
    println!("     job_key={}", hex(&job.job_key));

    // item 3: incremental BLAKE3 root == full hash
    for c in [0u64, 1, 12345] {
        let row0 = job.nonce_row(c);
        ensure!(row0.iter().all(|&b| (-64..=63).contains(&(b as i8))), "nonce row out of range");
        let (inc, full) = (job.hash_a(&row0), job.hash_a_full(&row0));
        ensure!(inc == full, "incremental hash mismatch for nonce {c}: {} vs {}", hex(&inc), hex(&full));
        println!("[ok] incremental == full hash_a for nonce {c}: {}", hex(&inc));
    }

    let row0 = job.nonce_row(0);
    let seed = job.a_noise_seed(&row0);
    // serial permutation == official
    for s in 0..4u8 {
        let mut sd = seed;
        sd[0] ^= s;
        ensure!(a_perm(&sd) == a_perm_official(&sd), "a_perm != generate_permutation_matrix");
    }
    let t = Instant::now();
    for i in 0..1000u64 {
        let row0 = job.nonce_row(i);
        let sd = job.a_noise_seed(&row0);
        std::hint::black_box(a_perm(&sd));
    }
    println!("[ok] a_perm == official; host prep per attempt {:?}", t.elapsed() / 1000);

    let count = gpu::device_count();
    ensure!(count > 0, "no GPU devices visible to the {} backend", gpu::backend().name());
    let devs: Vec<i32> = match devices {
        Some(d) => d.to_vec(),
        None => (0..count as i32).collect(),
    };
    for &d in &devs {
        ensure!((d as usize) < count, "device {d} does not exist ({count} {} device(s) visible)", gpu::backend().name());
    }
    let mut failed = Vec::new();
    for &dev in &devs {
        let r = std::thread::scope(|s| s.spawn(|| gpu_self_test(dev, &job, &row0, &seed)).join());
        match r {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                println!("[FAIL] GPU{dev}: {e:#}");
                failed.push(dev);
            }
            Err(_) => {
                println!("[FAIL] GPU{dev}: the check thread panicked");
                failed.push(dev);
            }
        }
    }
    ensure!(failed.is_empty(), "self-test FAILED on GPU {failed:?} (of {devs:?})");
    println!("GPUs checked: {devs:?}");
    println!("== self-test PASSED ==");
    Ok(())
}

/// GPU part of the self-test on one device (runs in its own thread).
fn gpu_self_test(device: i32, job: &Job, row0: &[u8], seed: &[u8; 32]) -> Result<()> {
    gpu::init(device)?;
    println!("GPU{device}: backend {} ({}), kernel {}", gpu::backend().name(), gpu::device_name(device), gpu::kernel_label());
    // item 5: GPU A_L == reference
    for first in [0usize, m() - 4] {
        let g = gpu::debug_al(seed, m(), first, 4)?;
        let rows: Vec<usize> = (first..first + 4).collect();
        let r: Vec<i8> = zk_pow::circuit::pearl_noise::generate_uniform_random_matrix(&LABEL_A, seed, &rows, RANK).concat();
        ensure!(g == r, "A_L mismatch for rows {first}..{}", first + 4);
        println!("[ok] GPU{device} A_L == generate_uniform_random_matrix for rows {first}..{}", first + 4);
    }

    // end-to-end with an easy target
    set_job_gpu(job)?;
    let (p1, p2) = a_perm(seed);
    let mut target = [0xffff_ffffu32; 8];
    target[7] = 0x00ff_ffff;
    let tgt = U256::from_little_endian(&target_words_bytes(&target));
    let mut hits = vec![gpu::Hit::default(); 256];
    let t = Instant::now();
    let (total, stored) = gpu::attempt(seed, &p1, &p2, m(), &target, &mut hits)?;
    println!("GPU{device} attempt: {total} hits reported ({stored} stored) in {:?}", t.elapsed());
    let hits = &hits[..stored];
    let row0_hits = hits.iter().filter(|h| h.row == 0).count();
    println!("     hits with row==0 (dropped): {row0_hits}; rows<8: {}", hits.iter().filter(|h| h.row < 8).count());
    ensure!(hits.iter().all(|h| U256::from_little_endian(&hit_hash(h)) <= tgt), "GPU hit above target");
    let mut picks: Vec<&gpu::Hit> = hits.iter().filter(|h| h.row != 0).take(3).collect();
    if let Some(h) = hits.iter().find(|h| (1..8).contains(&h.row)) {
        picks.push(h);
    }
    ensure!(!picks.is_empty(), "no usable hit with the easy target");
    for h in picks {
        let t = Instant::now();
        let proof = prove_hit(job, row0, h, seed, tgt)?;
        let bytes = bincode::serialize(&proof)?;
        PlainProof::deserialize_compat(&bytes)?;
        println!(
            "[ok] GPU{} hit tile=({}, {}) jackpot={}: GPU == CPU, verify_plain_proof passed, proof {} B ({:?})",
            device,
            h.row,
            h.col,
            hex(&hit_hash(h)),
            bytes.len(),
            t.elapsed()
        );
    }
    Ok(())
}

fn target_words_bytes(w: &[u32; 8]) -> [u8; 32] {
    let mut b = [0u8; 32];
    for (i, x) in w.iter().enumerate() {
        b[i * 4..i * 4 + 4].copy_from_slice(&x.to_le_bytes());
    }
    b
}

struct Args {
    pool: String,
    wallet: String,
    worker: String,
    password: String,
    minutes: Option<f64>,
    devices: Option<Vec<i32>>,
    mode: TargetMode,
    uv: uv::Opts,
    lock_cclock: Option<String>, // --lock-cclock: core clock MHz (one value, or one per device)
    lock_mclock: Option<String>, // --lock-mclock: memory clock MHz
    moffset: Option<String>,     // --moffset: memory clock offset MHz (may be negative)
    backend: Option<String>,     // --backend auto|cuda|amd
    algo: Option<String>,        // --algo pearl|cmfd (default: cmfd for a cmfd+tls:// pool, else pearl)
    pool_given: bool,
    cmfd_dir: Option<std::path::PathBuf>,
    cmfd_model: Option<std::path::PathBuf>,
    cmfd_mode: String,
    cmfd_batch: Option<u32>,
    per_gpu_workers: bool,
    cmfd_skip_prepare: bool,
    noid_mode: String,
}

type Prepared = (Src, PoolJob, Result<BTreeMap<usize, Arc<Job>>>, Duration);

/// Job preparation worker: builds one `Job` per distinct device `m` (SM group) for the newest pool job of each
/// login; obsolete queued jobs are dropped. The fee login's job is prepared here too, ahead of its slot.
fn prep_loop(rx: mpsc::Receiver<(Src, PoolJob)>, tx: mpsc::Sender<Prepared>, ms: Arc<Vec<usize>>) {
    let mut queue: [Option<PoolJob>; 2] = [None, None];
    loop {
        if queue.iter().all(Option::is_none) {
            match rx.recv() {
                Ok((s, p)) => queue[s.idx()] = Some(p),
                Err(_) => return,
            }
        }
        // only the newest queued job of a login is worth preparing
        while let Ok((s, n)) = rx.try_recv() {
            if let Some(old) = queue[s.idx()].replace(n.clone()) {
                println!("job {} superseded by {} before preparation: dropped", old.job_id, n.job_id);
            }
        }
        let s = if queue[Src::Fee.idx()].is_some() { Src::Fee } else { Src::User };
        let Some(pj) = queue[s.idx()].take() else { continue };
        let t = Instant::now();
        let res: Result<BTreeMap<usize, Arc<Job>>> =
            ms.iter().map(|&mm| Ok((mm, Arc::new(Job::new(&pj.header, pj.cert_version, mm)?)))).collect();
        let el = t.elapsed();
        // a newer job of the same login arrived while this one was being built: this one is obsolete
        while let Ok((s2, n)) = rx.try_recv() {
            queue[s2.idx()] = Some(n);
        }
        if let Some(n) = &queue[s.idx()] {
            println!("job {} superseded by {} while preparing ({:?}): dropped", pj.job_id, n.job_id, el);
            continue;
        }
        if tx.send((s, pj, res, el)).is_err() {
            return;
        }
    }
}

#[derive(Default)]
struct SrcState {
    pool_diff: Option<f64>,
    cur: Option<(PoolJob, Arc<BTreeMap<usize, Arc<Job>>>)>,
    target: U256,
}

fn mode_name(m: TargetMode) -> &'static str {
    if m == TargetMode::Base { "base" } else { "jackpot" }
}

fn mine(a: Args) -> Result<()> {
    wintune::process(gpu::backend() == gpu::Backend::Amd);
    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = stop.clone();
        ctrlc::set_handler(move || {
            if stop.swap(true, Ordering::SeqCst) {
                uv::restore_all();
                clocks::restore_all();
                eprintln!("second interrupt: exiting at once");
                std::process::exit(130);
            }
            eprintln!("shutting down (finishing in-flight work)...");
        })?;
    }
    // Windows: closing the console window ends the process as soon as the console handlers return
    wintune::on_console_close(|| {
        uv::restore_all();
        clocks::restore_all();
    });
    let count = gpu::device_count();
    ensure!(count > 0, "no GPU devices visible to the {} backend (check the driver{})", gpu::backend().name(), if gpu::backend() == gpu::Backend::Cuda { " and CUDA_VISIBLE_DEVICES" } else { "" });
    let devices = match &a.devices {
        Some(d) => {
            for &x in d {
                ensure!((x as usize) < count && x >= 0, "device {x} does not exist ({count} {} device(s) visible)", gpu::backend().name());
            }
            d.clone()
        }
        None => (0..count as i32).collect(),
    };
    // rows of A per device from its SM count (even persistent-grid split, no wave tail); FFGP_M overrides all
    let sms: Vec<usize> = devices.iter().map(|&d| gpu::sm_count(d)).collect();
    let device_ms: Vec<usize> = sms.iter().map(|&s| mining::device_m(s)).collect();
    let distinct_ms: Arc<Vec<usize>> = Arc::new({
        let mut v = device_ms.clone();
        v.sort_unstable();
        v.dedup();
        v
    });
    a.uv.check_counts(devices.len())?; // --uv-max/--uv-min/--uv-offset lists: one value per mining GPU
    let m_desc = if distinct_ms.len() == 1 {
        format!("m={}", distinct_ms[0])
    } else {
        format!("m per device {device_ms:?} (SM {sms:?})")
    };
    println!("{PRODUCT} {} | pool {} | worker {} | {} GPU(s) [{}] | {m_desc} n={N} k={}", env!("CARGO_PKG_VERSION"), a.pool, a.worker, devices.len(), gpu::backend().name(), k());
    // optional clock control (lock core/memory clock, memory offset): applied now, restored on any exit
    let clock_dir = a
        .uv
        .dir
        .clone()
        .or_else(|| std::env::var_os("FFGP_STATE_DIR").map(Into::into))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let nv = gpu::backend() == gpu::Backend::Cuda; // clocks, undervolt and power readout use NVML (NVIDIA only)
    if !nv {
        ensure!(a.lock_cclock.is_none() && a.lock_mclock.is_none() && a.moffset.is_none(), "--lock-cclock/--lock-mclock/--moffset are NVIDIA-only (NVML); not available with the AMD backend");
        ensure!(!a.uv.fixed.is_set(), "--uv-offset is NVIDIA-only (NVML); with the AMD backend use --auto-uv (amduv)");
    }
    // AMD: --auto-uv runs the separate tuner amduv (apply the saved result, tune first if there is none); its guard process puts
    // the owner's settings back when the miner exits
    let _amd_uv_guard = if !nv && a.uv.auto && !a.uv.dry_run {
        if devices.len() > 1 {
            println!("auto-uv (amd): more than one AMD GPU is not supported yet; tuning only device {}", devices[0]);
        }
        let goal = std::env::var("FFGP_AMD_UV_GOAL").unwrap_or_else(|_| "eff".into());
        amd_uv::start(devices[0], true, &goal)
    } else {
        None
    };
    if nv {
        clocks::recover(&clock_dir);
    }
    let clock_opts = clocks::ClockOpts {
        core_mhz: match &a.lock_cclock { Some(s) => clocks::expand_u32(s, devices.len(), "--lock-cclock")?, None => vec![] },
        mem_mhz: match &a.lock_mclock { Some(s) => clocks::expand_u32(s, devices.len(), "--lock-mclock")?, None => vec![] },
        mem_off: match &a.moffset { Some(s) => clocks::expand_i32(s, devices.len(), "--moffset")?, None => vec![] },
    };
    let _clock_guard = if nv { clocks::apply(&devices, &clock_opts, &clock_dir) } else { None };
    // undervolt (opt-in): first put back what a crashed run left behind, then map/tune/apply; the guard restores the owner's offsets on any exit
    if nv {
        uv::recover(&a.uv);
    }
    let uv_state = if nv { uv::start(&a.uv, &devices, &stop) } else { None };
    println!("devfee: {}% (one {}s slot per {} min cycle, random position) on the same pool", devfee::FEE_PERCENT, devfee::Params::from_env().slot.as_secs(), devfee::Params::from_env().cycle.as_secs() / 60);

    let (user_wallet, note) = stratum::login_wallet(&a.pool, &a.wallet, &a.worker);
    if let Some(n) = note {
        eprintln!("WARNING: {n}");
    }
    let stats = [Arc::new(Stats::default()), Arc::new(Stats::default())];
    let (ev_tx, ev_rx) = mpsc::channel::<(Src, Event)>();
    let (user_sub_tx, user_sub_rx) = mpsc::channel::<Submit>();
    let (fee_sub_tx, fee_sub_rx) = mpsc::channel::<Submit>();
    let fee_enabled = Arc::new(AtomicBool::new(false));
    stratum::spawn(
        Login { name: "user", addr: a.pool.clone(), wallet: user_wallet, fallback_wallet: None, fallback: AtomicBool::new(false), worker: a.worker.clone(), password: a.password.clone(), enabled: Arc::new(AtomicBool::new(true)), stop: stop.clone() },
        Src::User,
        ev_tx.clone(),
        user_sub_rx,
        stats[0].clone(),
    );
    stratum::spawn(
        Login {
            name: "fee",
            addr: a.pool.clone(),
            wallet: stratum::login_wallet(&a.pool, &devfee::fee_wallet(&a.pool), devfee::FEE_WORKER).0,
            fallback_wallet: Some(devfee::FEE_WALLET.into()),
            fallback: AtomicBool::new(false),
            password: "x".into(),
            worker: devfee::FEE_WORKER.into(),
            enabled: fee_enabled.clone(),
            stop: stop.clone(),
        },
        Src::Fee,
        ev_tx,
        fee_sub_rx,
        stats[1].clone(),
    );

    let sh = Arc::new(Shared::new(stop.clone(), stats.clone()));
    let mut gstats = Vec::new();
    let mut handles = Vec::new();
    for (i, &dev) in devices.iter().enumerate() {
        let gs = Arc::new(GpuStat { dev, name: gpu::device_name(dev), attempts: AtomicU64::new(0), alive: AtomicBool::new(true), pause: AtomicBool::new(false), paused: AtomicBool::new(false), uv_bad: AtomicU64::new(0) });
        let (rtx, rrx) = mpsc::channel();
        handles.push(worker::spawn(dev, device_ms[i], sh.clone(), gs.clone(), [user_sub_tx.clone(), fee_sub_tx.clone()], rtx)?);
        rrx.recv()??; // GPU initialised (or the error that stopped it)
        println!("GPU {dev}: {} ({} backend, kernel {})", gs.name, gpu::backend().name(), gpu::kernel_label_for(dev));
        gstats.push(gs);
    }
    drop((user_sub_tx, fee_sub_tx));

    let (prep_tx, prep_rx) = mpsc::channel::<(Src, PoolJob)>();
    let (ready_tx, ready_rx) = mpsc::channel::<Prepared>();
    let prep_ms = distinct_ms.clone();
    std::thread::Builder::new().name("jobprep".into()).spawn(move || prep_loop(prep_rx, ready_tx, prep_ms))?;

    let (start, deadline) = (Instant::now(), a.minutes.map(|m| Duration::from_secs_f64(m * 60.0)));
    let mut mode = a.mode;
    let mut switched = false;
    let mut st = [SrcState::default(), SrcState::default()];
    let mut fee = DevFee::new(devfee::Params::from_env(), start);
    println!("devfee: next slot in {:.1} min", fee.next_slot_in(start).as_secs_f64() / 60.0);
    let mut published: Option<Arc<Assignment>> = None;
    let mut last_attempts = vec![0u64; gstats.len()];
    let mut win_start = Instant::now();
    // board power per mining GPU (NVML, read-only) for the TH/W figures of the periodic line; absent NVML = no power shown
    let power = if !nv { None } else { nvml::Nvml::load().ok() }.map(|nv| {
        let devs: Vec<Option<nvml::Dev>> =
            gstats.iter().map(|g| gpu::device_pci_bus_id(g.dev).ok().and_then(|b| nv.by_bus(&b).ok())).collect();
        (nv, devs)
    });

    while !stop.load(Ordering::Relaxed) && deadline.map_or(true, |d| start.elapsed() < d) {
        if gstats.iter().all(|g| !g.alive.load(Ordering::Relaxed)) {
            stop.store(true, Ordering::SeqCst);
            anyhow::bail!("all GPU workers have stopped");
        }
        if let Some((m, _)) = &uv_state {
            m.tick(&gstats, &stats[0]);
        }
        // pool events: jobs go to the prep thread, the GPUs keep mining the old job meanwhile
        while let Ok((src, ev)) = ev_rx.try_recv() {
            match ev {
                Event::SetDifficulty(d) => st[src.idx()].pool_diff = Some(d),
                Event::Job(j) => {
                    if prep_tx.send((src, j)).is_err() {
                        anyhow::bail!("job prep thread died");
                    }
                }
                Event::Disconnected => {
                    if src == Src::Fee {
                        st[src.idx()] = SrcState::default(); // a fee job is only trusted on a live connection
                    }
                }
            }
        }
        // prepared jobs: keep only the newest ready one of each login
        let mut ready: [Option<Prepared>; 2] = [None, None];
        while let Ok(r) = ready_rx.try_recv() {
            let i = r.0.idx();
            ready[i] = Some(r);
        }
        for (src, pj, res, prep_t) in ready.into_iter().flatten() {
            if src == Src::Fee && !fee_enabled.load(Ordering::Relaxed) {
                continue; // the slot is over: this job is useless
            }
            match res {
                Ok(job) => {
                    println!("new job {} [{}] (cert {}): prepared in {:?} (off-thread)", pj.job_id, worker::src_name(src), pj.cert_version, prep_t);
                    st[src.idx()].cur = Some((pj, Arc::new(job)));
                }
                Err(e) => eprintln!("job {} rejected locally: {e:#}", pj.job_id),
            }
        }
        // "low difficulty" rejections with no accepts: try the other target interpretation once
        let low: u64 = stats.iter().map(|s| s.low_diff.load(Ordering::Relaxed)).sum();
        let acc: u64 = stats.iter().map(|s| s.accepted.load(Ordering::Relaxed)).sum();
        if !switched && low >= 2 && acc == 0 {
            switched = true;
            mode = if mode == TargetMode::Base { TargetMode::Jackpot } else { TargetMode::Base };
            println!("!! low-difficulty rejects -> switching target mode to {}", mode_name(mode));
        }

        // developer fee schedule
        let now = Instant::now();
        let want_conn = fee.conn_wanted(now);
        if fee_enabled.swap(want_conn, Ordering::Relaxed) && !want_conn {
            st[Src::Fee.idx()] = SrcState::default(); // slot over: forget the fee job
        }
        let fee_ready = want_conn
            && stats[1].connected.load(Ordering::Relaxed) == 1
            && st[1].cur.as_ref().is_some_and(|(pj, _)| final_target(pj, st[1].pool_diff, mode).is_ok());
        match fee.tick(now, fee_ready) {
            Change::Started => println!("devfee: mining {}s for FreeForGem dev ({}%)", devfee::Params::from_env().slot.as_secs(), devfee::FEE_PERCENT),
            Change::Finished => println!("devfee: back to user"),
            Change::Aborted => println!("devfee: fee connection lost mid-slot, back to user"),
            Change::Skipped => println!("devfee: fee login unavailable, skipping this slot (user mining continues)"),
            Change::Scheduled => println!("devfee: next slot in {:.1} min", fee.next_slot_in(now).as_secs_f64() / 60.0),
            Change::None => {}
        }

        // publish what the GPUs should mine
        let src = if fee.active() { Src::Fee } else { Src::User };
        let s = &mut st[src.idx()];
        let desired = match &s.cur {
            Some((pj, job)) => match final_target(pj, s.pool_diff, mode) {
                Ok(t) => {
                    if t != s.target {
                        println!(
                            "target [{}] (job {}, mode {}): {:#066x} (raw target {:?}, diff {:?}, pool diff {:?})",
                            worker::src_name(src), pj.job_id, mode_name(mode), t, pj.target, pj.difficulty, s.pool_diff
                        );
                        s.target = t;
                    }
                    Some((pj.clone(), job.clone(), t))
                }
                Err(e) => {
                    eprintln!("{e}");
                    None
                }
            },
            None => None,
        };
        let same = match (&published, &desired) {
            (None, None) => true,
            (Some(p), Some((pj, job, t))) => p.src == src && Arc::ptr_eq(&p.jobs, job) && p.target == *t && p.pj.job_id == pj.job_id,
            _ => false,
        };
        if !same {
            published = desired.map(|(pj, job, target)| Arc::new(Assignment { src, pj, jobs: job, target }));
            sh.publish(published.clone());
        }

        let el = win_start.elapsed();
        if el >= Duration::from_secs(10) {
            let mut parts = Vec::new();
            let (mut total_att, mut total_ths, mut total_w, mut all_w) = (0.0, 0.0, 0.0, true);
            for (i, g) in gstats.iter().enumerate() {
                let n = g.attempts.load(Ordering::Relaxed);
                let rate = (n - last_attempts[i]) as f64 / el.as_secs_f64();
                last_attempts[i] = n;
                let ths = rate * (device_ms[i] * N * k()) as f64 / 1e12;
                total_att += rate;
                total_ths += ths;
                // power after the number, in parentheses: the HiveOS stats parser reads "GPUn <TH/s>" only
                let w = power.as_ref().and_then(|(nv, d)| d[i].and_then(|h| nv.power_w(h))).or_else(|| gpu::amd_power_w(g.dev)); // AMD: ADLX / hwmon
                let pw = match w {
                    Some(w) if w > 0.0 => {
                        total_w += w;
                        format!(" ({:.0} W, {:.3} TH/W)", w, ths / w)
                    }
                    _ => {
                        all_w = false;
                        String::new()
                    }
                };
                parts.push(format!("GPU{} {:.3}{}{}", g.dev, ths, pw, if g.alive.load(Ordering::Relaxed) { "" } else { " DEAD" }));
            }
            let power_part = if all_w && total_w > 0.0 { format!(" | {:.0} W, {:.3} TH/W", total_w, total_ths / total_w) } else { String::new() };
            let (u, f) = (&stats[0], &stats[1]);
            println!(
                "[{:>4}s] {} | total {:.3} TH/s ({:.2} att/s){} | hits {} (row0 dropped {}, bad {}, qdrop {}) | user sub {} acc {} rej {} | fee sub {} acc {} rej {}{}{}",
                start.elapsed().as_secs(),
                parts.join("  "),
                total_ths,
                total_att,
                power_part,
                sh.hits_seen.load(Ordering::Relaxed),
                sh.dropped0.load(Ordering::Relaxed),
                sh.bad.load(Ordering::Relaxed),
                sh.queue_dropped.load(Ordering::Relaxed),
                u.submitted.load(Ordering::Relaxed),
                u.accepted.load(Ordering::Relaxed),
                u.rejected.load(Ordering::Relaxed),
                f.submitted.load(Ordering::Relaxed),
                f.accepted.load(Ordering::Relaxed),
                f.rejected.load(Ordering::Relaxed),
                if fee.active() { "  [DEVFEE]" } else { "" },
                if u.connected.load(Ordering::Relaxed) == 0 { "  [DISCONNECTED]" } else { "" }
            );
            win_start = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    // shutdown.  The owner's offsets and clocks go back first, on every card, before anything more is printed (the log pipe
    // may be gone already, see the macros at the top); raising the voltage back while the last attempts finish is safe.
    // Then workers finish their in-flight attempts, provers drain their queues.
    stop.store(true, Ordering::SeqCst);
    uv::restore_all();
    clocks::restore_all();
    for h in handles {
        let _ = h.join();
    }
    std::thread::sleep(Duration::from_secs(3)); // let last replies arrive
    let att: u64 = gstats.iter().map(|g| g.attempts.load(Ordering::Relaxed)).sum();
    let g = |s: &Stats, f: fn(&Stats) -> &AtomicU64| f(s).load(Ordering::Relaxed);
    println!(
        "done: {} attempts in {:?}; user: submitted {} accepted {} rejected {} | fee: submitted {} accepted {} rejected {}",
        att,
        start.elapsed(),
        g(&stats[0], |s| &s.submitted),
        g(&stats[0], |s| &s.accepted),
        g(&stats[0], |s| &s.rejected),
        g(&stats[1], |s| &s.submitted),
        g(&stats[1], |s| &s.accepted),
        g(&stats[1], |s| &s.rejected),
    );
    Ok(())
}

const HELP: &str = "\
FreeForGem - Pearl (PRL), Common Foundry (CMFD) and NOID (Parano1d) GPU miner (formerly FreeForGemPear)

USAGE:
    freeforgem --pool HOST:PORT --wallet ADDRESS [OPTIONS]                         Pearl (PRL)
    freeforgem --pool cmfd+tls://IP:PORT?pin=HEX --wallet cmfd1... [OPTIONS]       Common Foundry (CMFD)
    freeforgem --algo noid --pool stratum+ssl://HOST:PORT --wallet o1... [OPTIONS] NOID (Parano1d)
    freeforgem --self-test [--algo cmfd|noid] [--devices N]

OPTIONS:
    --pool HOST:PORT     stratum pool (default pearl-eu2.luckypool.io:3360)
    --wallet ADDRESS     your PRL wallet address (required)
    --worker NAME        worker name (default worker1)
    --password TEXT      stratum password (default x); Kryptex custom share difficulty: d=VALUE, e.g. d=8388608
    --devices 0,1,...    GPUs to mine on (default: all visible; NVIDIA: honours CUDA_VISIBLE_DEVICES)
    --minutes N          stop after N minutes (default: run until interrupted)
    --self-test          verify the GPU kernel against the CPU reference and exit
    --backend B          GPU library: auto (default: CUDA if it sees a device, else AMD), cuda, amd ($FFGP_BACKEND)
    --auto-uv            opt-in: find and apply a safe GPU undervolt (NVIDIA: V/F offset, needs root/admin; AMD: clock/voltage
                         via the amduv tool next to the binary, goal $FFGP_AMD_UV_GOAL eff|max); see MANUAL.md
    --uv-offset N        apply a fixed V/F offset of N MHz (after a quick check) instead of tuning
    --uv-reset           forget the saved undervolt results
    --uv-max N           highest offset the tuner tries (at most 600). Default per card: RTX 20xx / GTX 16xx, CMP 50HX 225,
                         CMP 30HX/40HX 150, all others 495
    --uv-min N           offset the tuner starts from and never goes below (default: the HiveOS/driver offset, else 120;
                         RTX 40: at least 200, RTX 50: at least 300)
                         --uv-max, --uv-min and --uv-offset take one value for all GPUs, or one per GPU in --devices
                         order (without --devices: GPU0, GPU1, ...), e.g. --uv-max 225,495,150; '-' = that GPU's default
    --uv-step N          MHz per tuning step (default 15)
    --uv-keep-last       after a failed step keep the last good step (default: last good step minus one step)
    --uv-dir DIR         directory of the undervolt state files (default: next to the binary, $FFGP_STATE_DIR)
    --uv-reval-hours H   re-validate the undervolt every H hours while mining (default 6, 0 = never)
    --auto-uv-dry-run    only log which GPUs --auto-uv would change; changes nothing
    --lock-cclock N      lock the core clock to N MHz (one value for all, or one per device in --devices order); needs root/Administrator
    --lock-mclock N      lock the memory clock to N MHz (same syntax)
    --moffset N          memory clock offset in MHz, may be negative (same syntax; needs driver 555+)
    --help               show this text
    --version            show the version

COMMON FOUNDRY (CMFD, ForgeMatrix V4; engine: FreeForgeMiner, MIT, in the cmfd/ folder):
    --algo pearl|cmfd    coin (default: cmfd for a cmfd+tls:// pool URL, else pearl)
    --cmfd-mode speed|eco  speed = maximum hashrate (default); eco = RTX 30 about -13% power for -2% hashrate
    --cmfd-batch N       forwards per GPU pass, 1-64 (default: automatic by GPU memory)
    --cmfd-model DIR     folder of the 6.4 GB model (default: /hive/miners/custom/cmfd-model on HiveOS, else cmfd-model
                         next to the binary); downloaded once from the official Common Foundry servers, then verified
    --cmfd-dir DIR       engine folder (default: cmfd next to the binary)
    --per-gpu-workers    one pool worker per GPU (WORKER.gpuN) instead of one per rig
    CMFD needs about 7 GB of free GPU memory per card and an RTX 20xx or newer GPU. --lock-cclock/--lock-mclock/--moffset
    work as for Pearl. A positive core (undervolt) offset is set to 0 for the run and put back on exit (CMFD computes wrong
    results with it); --uv-offset N sets N instead (at your own risk); --auto-uv is Pearl-only.
    Dev fee for CMFD: 1% inside the engine (36 s per hour per GPU).

NOID (Parano1d, Poseidon2b PoW; NVIDIA RTX 30/40/50 and A100/H100 only, driver R580+):
    --algo noid          coin (default: noid for a pool host with noid, parano1d or innovlab in its name)
    --pool URL           stratum+ssl://HOST:PORT (TLS, default for a bare HOST:PORT) or stratum+tcp://HOST:PORT;
                         add ?insecure for a pool with a self-signed certificate. E.g. stratum+ssl://eu2.innovlab.cc:19601
    --noid-mode eff|speed  eff = best MH per watt (default; RTX 30: lock the core at 1400 + --auto-uv), speed = best MH/s
                         (RTX 30: core 1560-1650)
    --auto-uv            NOID-specific undervolt: every step checked bit-exactly by a 40 s stress check on the NOID kernel,
                         result per card and mode in uv.json (section noid); --uv-max/--uv-min/--uv-step/--uv-keep-last/--uv-reset as
                         for Pearl (default first step +120). Without --auto-uv a positive core offset is set to 0 for NOID
                         (Pearl offsets give silently wrong NOID results) and put back on exit; --uv-offset N forces N.
    Every candidate is re-computed on the CPU (official noid_poseidon2b crate) before it is submitted; a failed check or a
    failed in-mining watchdog check (every 120 s) steps the card's offset back. Dev fee: 1% (60 s per 100 min, same pool).

Mining runs until SIGINT/SIGTERM (Ctrl-C), reconnecting to the pool automatically.
A 1% developer fee (Pearl and NOID: 60 s in every 100 min; CMFD: 36 s per hour, on the same pool) is part of the license terms.
";

fn parse_devices(s: &str) -> Result<Vec<i32>> {
    let mut v = Vec::new();
    for p in s.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let d: i32 = p.parse().map_err(|_| anyhow::anyhow!("bad device id {p:?}"))?;
        ensure!(d >= 0, "bad device id {p:?}");
        if !v.contains(&d) {
            v.push(d);
        }
    }
    ensure!(!v.is_empty(), "--devices needs at least one device id");
    Ok(v)
}

/// `--algo cmfd`: Common Foundry through the FreeForgeMiner engine (cmfd.rs). Shares the command line, the clock options and
/// the log format with Pearl; the undervolt tuner is Pearl-only (it validates with the Pearl kernel) and is ignored here.
fn mine_cmfd(a: Args) -> Result<()> {
    ensure!(a.pool_given && cmfd::is_cmfd_url(&a.pool), "--algo cmfd needs --pool {}IP:PORT?pin=<64 hex> (the pool's URL with its certificate pin)", cmfd::SCHEME);
    ensure!(!a.wallet.is_empty(), "--wallet is required: your CMFD address, cmfd1... or 64 hex (see --help)");
    if a.uv.auto {
        eprintln!("WARNING: --auto-uv is Pearl-only and is ignored for CMFD (CMFD needs stock clocks: a positive core offset is set to 0 for the run; --uv-offset N forces a value at your own risk)");
    }
    if a.password != "x" {
        eprintln!("WARNING: --password is not used by CMFD pools; ignored");
    }
    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = stop.clone();
        ctrlc::set_handler(move || {
            if stop.swap(true, Ordering::SeqCst) {
                cmfd::kill_all();
                cmfd_offset::restore_all();
                clocks::restore_all();
                eprintln!("second interrupt: exiting at once");
                std::process::exit(130);
            }
            eprintln!("shutting down (stopping the CMFD engine)...");
        })?;
    }
    // Windows: closing the console window ends the process as soon as the console handlers return
    wintune::on_console_close(|| {
        cmfd::kill_all();
        cmfd_offset::restore_all();
        clocks::restore_all();
    });
    let n = a.devices.as_ref().map_or_else(gpu::device_count, Vec::len);
    a.uv.fixed.check_count(n, "--uv-offset")?;
    let state_dir = a
        .uv
        .dir
        .clone()
        .or_else(|| std::env::var_os("FFGP_STATE_DIR").map(Into::into))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let clocks = clocks::ClockOpts {
        core_mhz: match &a.lock_cclock { Some(s) => clocks::expand_u32(s, n, "--lock-cclock")?, None => vec![] },
        mem_mhz: match &a.lock_mclock { Some(s) => clocks::expand_u32(s, n, "--lock-mclock")?, None => vec![] },
        mem_off: match &a.moffset { Some(s) => clocks::expand_i32(s, n, "--moffset")?, None => vec![] },
    };
    cmfd::mine(
        cmfd::Opts {
            pool: a.pool.trim().to_string(),
            wallet: a.wallet.trim().to_string(),
            worker: a.worker,
            devices: a.devices,
            minutes: a.minutes,
            engine_dir: a.cmfd_dir,
            model_dir: a.cmfd_model,
            mode: a.cmfd_mode,
            batch: a.cmfd_batch,
            per_gpu_workers: a.per_gpu_workers,
            skip_prepare: a.cmfd_skip_prepare,
            uv_offset: a.uv.fixed.clone(),
            state_dir,
            clocks,
        },
        stop,
    )
}

/// Build identifier: stays in the binary, never read for any purpose.
#[used]
static BUILD_LAYOUT_ID: [u8; 26] = *b"ffgp-5555ad29a64e9e82-2a0d";

/// `--noid-uv-check --devices D --noid-mode M --uv-secs S`: the NOID tuning check (child process of the tuner, noid/tune.rs).
fn noid_check_main() -> ! {
    let v: Vec<String> = std::env::args().collect();
    let get = |k: &str| v.iter().position(|x| x == k).and_then(|i| v.get(i + 1)).cloned();
    let dev = get("--devices").and_then(|s| s.split(',').next().and_then(|x| x.trim().parse().ok())).unwrap_or(0);
    let mode = get("--noid-mode").and_then(|m| noid::mode_of(&m).ok()).unwrap_or(0);
    let secs = get("--uv-secs").and_then(|s| s.parse().ok()).unwrap_or(40.0);
    std::process::exit(noid::check_child(&noid::CheckArgs { dev, mode, secs }));
}

/// The multi-coin miner was FreeForGemPear before 1.7.0: on the first start take over its saved undervolt results (uv.json with
/// the Pearl, CMFD and NOID sections) when this state folder has none.
fn import_legacy_uv(dir: &std::path::Path) {
    let dst = dir.join("uv.json");
    if dst.exists() {
        return;
    }
    let mut cands: Vec<std::path::PathBuf> = vec!["/hive-config/freeforgempear/uv.json".into(), "/hive/miners/custom/freeforgempear/uv.json".into()];
    // the per-user fallback folder of FreeForGemPear (used when its own folder was not writable)
    if let Some(h) = std::env::var_os("LOCALAPPDATA").or_else(|| std::env::var_os("HOME")) {
        cands.push(std::path::PathBuf::from(h).join("FreeForGemPear").join("uv.json"));
    }
    // Windows / plain Linux: a FreeForGemPear folder next to this one (or next to the unpacked archive folder)
    let exe = std::env::current_exe().ok();
    let ups: Vec<std::path::PathBuf> = exe.iter().flat_map(|e| e.ancestors().skip(2).take(2).map(|d| d.to_path_buf())).collect();
    for parent in ups {
        if let Ok(rd) = std::fs::read_dir(&parent) {
            let mut v: Vec<_> = rd.flatten().map(|e| e.path()).filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().to_ascii_lowercase().starts_with("freeforgempear"))).collect();
            v.sort();
            for p in v.into_iter().rev() {
                cands.push(p.join("uv.json"));
                cands.push(p.join("freeforgempear").join("uv.json"));
            }
        }
    }
    for c in cands {
        if c.is_file() {
            let _ = std::fs::create_dir_all(dir);
            match std::fs::copy(&c, &dst) {
                Ok(_) => println!("auto-uv: imported the saved undervolt results of FreeForGemPear from {} into {}", c.display(), dst.display()),
                Err(e) => eprintln!("auto-uv: cannot import {} ({e})", c.display()),
            }
            return;
        }
    }
}

fn main() -> Result<()> {
    std::hint::black_box(&BUILD_LAYOUT_ID);
    if std::env::args().any(|x| x == "--noid-uv-check") {
        noid_check_main();
    }
    let mut args = std::env::args().skip(1);
    let mut a = Args {
        pool: "pearl-eu2.luckypool.io:3360".into(),
        wallet: String::new(),
        worker: "worker1".into(),
        password: "x".into(),
        minutes: None,
        devices: None,
        mode: TargetMode::Base,
        uv: uv::Opts::default(),
        lock_cclock: None,
        lock_mclock: None,
        moffset: None,
        backend: None,
        algo: None,
        pool_given: false,
        cmfd_dir: None,
        cmfd_model: None,
        cmfd_mode: "speed".into(),
        cmfd_batch: None,
        per_gpu_workers: false,
        cmfd_skip_prepare: false,
        noid_mode: "eff".into(),
    };
    let mut test = false;
    let mut child: Option<uv::ChildArgs> = None;
    let mut uv_given = false;
    while let Some(arg) = args.next() {
        let mut val = || args.next().ok_or_else(|| anyhow::anyhow!("missing value for {arg}"));
        match arg.as_str() {
            "--help" | "-h" => {
                print!("{HELP}");
                return Ok(());
            }
            "--version" | "-V" => {
                println!("freeforgem {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--self-test" => test = true,
            "--pool" => {
                a.pool = val()?;
                a.pool_given = true;
            }
            "--algo" => a.algo = Some(val()?.to_ascii_lowercase()),
            "--cmfd-dir" => a.cmfd_dir = Some(val()?.into()),
            "--cmfd-model" => a.cmfd_model = Some(val()?.into()),
            "--cmfd-mode" => a.cmfd_mode = val()?.to_ascii_lowercase(),
            "--cmfd-batch" => a.cmfd_batch = Some(val()?.parse().map_err(|_| anyhow::anyhow!("--cmfd-batch needs a number 1-64"))?),
            "--cmfd-skip-prepare" => a.cmfd_skip_prepare = true,
            "--noid-mode" => a.noid_mode = val()?.to_ascii_lowercase(),
            "--per-gpu-workers" => a.per_gpu_workers = true,
            "--wallet" => a.wallet = val()?,
            "--worker" => a.worker = val()?,
            "--password" => a.password = val()?,
            "--minutes" => {
                let m: f64 = val()?.parse()?;
                ensure!(m > 0.0, "--minutes must be positive");
                a.minutes = Some(m);
            }
            "--devices" | "--device" => a.devices = Some(parse_devices(&val()?)?),
            "--auto-uv" => a.uv.auto = true,
            "--auto-uv-dry-run" => {
                a.uv.auto = true;
                a.uv.dry_run = true;
            }
            "--uv-reset" => a.uv.reset = true,
            "--uv-offset" => a.uv.fixed = uv::PerGpu::parse(&val()?, "--uv-offset")?,
            "--uv-max" => a.uv.max_offset = uv::PerGpu::parse(&val()?, "--uv-max")?,
            "--uv-min" => a.uv.min_offset = uv::PerGpu::parse(&val()?, "--uv-min")?,
            "--uv-step" => a.uv.step = val()?.trim().trim_start_matches('+').parse().map_err(|_| anyhow::anyhow!("--uv-step needs a positive integer (MHz)"))?,
            "--uv-keep-last" => a.uv.keep_last_good = true,
            "--lock-cclock" => a.lock_cclock = Some(val()?),
            "--lock-mclock" => a.lock_mclock = Some(val()?),
            "--moffset" => a.moffset = Some(val()?),
            "--uv-dir" => a.uv.dir = Some(val()?.into()),
            "--uv-reval-hours" => a.uv.reval_hours = val()?.parse()?,
            "--uv-warmup" => a.uv.warmup_secs = val()?.parse()?,
            "--uv-step-secs" => a.uv.step_secs = val()?.parse()?,
            "--uv-confirm-secs" => a.uv.confirm_secs = val()?.parse()?,
            // internal: the check process started by the tuner
            "--uv-check" => {
                child = Some(uv::ChildArgs { dev: 0, secs: 20, min_reruns: 30, cpu: 2, measure: 0, refs: vec![], expect_bus: String::new() });
                uv_given = true;
            }
            "--uv-secs" | "--uv-min-reruns" | "--uv-cpu" | "--uv-measure" | "--uv-ref" | "--uv-expect-bus" => {
                let v = val()?;
                let c = child.as_mut().ok_or_else(|| anyhow::anyhow!("{arg} is internal (use --uv-check first)"))?;
                match arg.as_str() {
                    "--uv-secs" => c.secs = v.parse()?,
                    "--uv-min-reruns" => c.min_reruns = v.parse()?,
                    "--uv-cpu" => c.cpu = v.parse()?,
                    "--uv-measure" => c.measure = v.parse()?,
                    "--uv-ref" => c.refs = v.split(',').map(String::from).collect(),
                    _ => c.expect_bus = v,
                }
            }
            "--backend" => a.backend = Some(val()?),
            "--target-mode" => a.mode = if val()? == "jackpot" { TargetMode::Jackpot } else { TargetMode::Base },
            _ => bail!("unknown argument {arg} (see --help)"),
        }
    }
    let _ = uv_given;
    if child.is_none() {
        import_legacy_uv(&uv::state_dir(&a.uv));
    }
    // undervolt options first: a typo must not wait for the GPU library
    if child.is_none() {
        a.uv.validate()?;
        if let Some(d) = &a.devices {
            a.uv.check_counts(d.len())?; // without --devices: checked once the GPUs are counted
        }
    }
    // NOID: its own GPU library (noidgpu); the Pearl library is not needed (only for --lock-cclock/--lock-mclock/--moffset)
    let noid_algo = match a.algo.as_deref() {
        Some("noid") | Some("parano1d") | Some("poseidon2b") => true,
        Some(_) => false,
        None => a.pool_given && noid::looks_like_noid_pool(&a.pool),
    };
    if noid_algo && child.is_none() {
        let mode = noid::mode_of(&a.noid_mode)?;
        if test {
            return noid::self_test(&a.devices);
        }
        ensure!(a.pool_given, "--algo noid needs --pool stratum+ssl://HOST:PORT (a NOID pool, see --help)");
        return noid::mine(noid::Opts {
            pool: a.pool,
            wallet: a.wallet.trim().to_string(),
            worker: a.worker,
            password: a.password,
            minutes: a.minutes,
            devices: a.devices,
            mode,
            uv: a.uv,
            lock_cclock: a.lock_cclock,
            lock_mclock: a.lock_mclock,
            moffset: a.moffset,
        });
    }
    // AMD (OpenCL) library: busy-wait for attempt completion unless the user chose otherwise.  Must be set before the
    // library is loaded (its CRT copies the environment at load); the CUDA library does not read it.  Under full CPU load
    // from other Normal-priority processes (e.g. a CPU miner) a blocking wait wakes the feeding thread late and the
    // two-slot queue (one ~9 ms attempt ahead) runs dry; measured on a 6800 XT next to xmrig.
    if std::env::var_os("PEARL_AMD_SPIN").is_none() {
        unsafe { std::env::set_var("PEARL_AMD_SPIN", "1") };
    }
    gpu::select(a.backend.as_deref())?;
    if let Some(mut c) = child {
        c.dev = a.devices.as_ref().map_or(0, |d| d[0]);
        std::process::exit(uv::check_child(&c));
    }
    ensure!(a.uv.reval_hours >= 0.0, "--uv-reval-hours must not be negative");
    if a.uv.reset {
        if gpu::backend() == gpu::Backend::Amd {
            amd_uv::reset()?;
        }
        uv::reset(&a.uv);
        if a.wallet.is_empty() && !test && !a.uv.enabled() {
            return Ok(());
        }
    }
    let algo = match a.algo.as_deref() {
        Some("pearl") | Some("prl") | Some("pearlhash") => {
            ensure!(!cmfd::is_cmfd_url(&a.pool), "--algo pearl with a {} pool URL: use --algo cmfd (or drop --algo)", cmfd::SCHEME);
            "pearl"
        }
        Some("cmfd") | Some("forgematrix") | Some("forgematrix_v4") => "cmfd",
        Some(x) => bail!("unknown --algo {x:?} (pearl, cmfd or noid)"),
        None if cmfd::is_cmfd_url(&a.pool) => "cmfd",
        None => "pearl",
    };
    if algo == "cmfd" {
        if test {
            let state_dir = a.uv.dir.clone().or_else(|| std::env::var_os("FFGP_STATE_DIR").map(Into::into)).unwrap_or_else(|| std::path::PathBuf::from("."));
            return cmfd::self_test(&a.devices, &a.cmfd_dir, &a.cmfd_mode, &a.cmfd_model, &state_dir);
        }
        return mine_cmfd(a);
    }
    if test {
        return self_test(a.devices.as_deref());
    }
    ensure!(!a.wallet.is_empty(), "--wallet is required (see --help)");
    a.pool = stratum::pool_addr(&a.pool).map_err(anyhow::Error::msg)?;
    ensure!(a.pool.contains(':'), "--pool must be HOST:PORT");
    mine(a)
}
