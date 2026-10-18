// FreeForGemPear — see LICENSE
//! FreeForGemPear: Pearl (PRL) GPU miner host: pool client + job/commitment logic around libpearlgpu.
//!
//!   freeforgempear --pool host:port --wallet ADDR [--worker NAME] [--devices 0,1] [--minutes N]
//!   freeforgempear --self-test [--devices 0]
mod clocks;
mod devfee;
mod gpu;
mod mining;
mod nvml;
mod stratum;
mod uv;
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

fn self_test(device: i32) -> Result<()> {
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

    gpu::init(device)?;
    // item 5: GPU A_L == reference
    let row0 = job.nonce_row(0);
    let seed = job.a_noise_seed(&row0);
    for first in [0usize, m() - 4] {
        let g = gpu::debug_al(&seed, m(), first, 4)?;
        let rows: Vec<usize> = (first..first + 4).collect();
        let r: Vec<i8> = zk_pow::circuit::pearl_noise::generate_uniform_random_matrix(&LABEL_A, &seed, &rows, RANK).concat();
        ensure!(g == r, "A_L mismatch for rows {first}..{}", first + 4);
        println!("[ok] GPU A_L == generate_uniform_random_matrix for rows {first}..{}", first + 4);
    }

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

    // end-to-end with an easy target
    set_job_gpu(&job)?;
    let (p1, p2) = a_perm(&seed);
    let mut target = [0xffff_ffffu32; 8];
    target[7] = 0x00ff_ffff;
    let tgt = U256::from_little_endian(&target_words_bytes(&target));
    let mut hits = vec![gpu::Hit::default(); 256];
    let t = Instant::now();
    let (total, stored) = gpu::attempt(&seed, &p1, &p2, m(), &target, &mut hits)?;
    println!("attempt: {total} hits reported ({stored} stored) in {:?}", t.elapsed());
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
        let proof = prove_hit(&job, &row0, h, &seed, tgt)?;
        let bytes = bincode::serialize(&proof)?;
        PlainProof::deserialize_compat(&bytes)?;
        println!(
            "[ok] hit tile=({}, {}) jackpot={}: GPU == CPU, verify_plain_proof passed, proof {} B ({:?})",
            h.row,
            h.col,
            hex(&hit_hash(h)),
            bytes.len(),
            t.elapsed()
        );
    }
    println!("== self-test PASSED ==");
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
    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = stop.clone();
        ctrlc::set_handler(move || {
            if stop.swap(true, Ordering::SeqCst) {
                eprintln!("second interrupt: exiting at once");
                uv::restore_all();
                clocks::restore_all();
                std::process::exit(130);
            }
            eprintln!("shutting down (finishing in-flight work)...");
        })?;
    }
    let count = gpu::device_count();
    ensure!(count > 0, "no CUDA devices visible (check the driver and CUDA_VISIBLE_DEVICES)");
    let devices = match &a.devices {
        Some(d) => {
            for &x in d {
                ensure!((x as usize) < count && x >= 0, "device {x} does not exist ({count} CUDA device(s) visible)");
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
    let m_desc = if distinct_ms.len() == 1 {
        format!("m={}", distinct_ms[0])
    } else {
        format!("m per device {device_ms:?} (SM {sms:?})")
    };
    println!("FreeForGemPear {} | pool {} | worker {} | {} GPU(s) | {m_desc} n={N} k={}", env!("CARGO_PKG_VERSION"), a.pool, a.worker, devices.len(), k());
    // optional clock control (lock core/memory clock, memory offset): applied now, restored on any exit
    let clock_dir = a
        .uv
        .dir
        .clone()
        .or_else(|| std::env::var_os("FFGP_STATE_DIR").map(Into::into))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    clocks::recover(&clock_dir);
    let clock_opts = clocks::ClockOpts {
        core_mhz: match &a.lock_cclock { Some(s) => clocks::expand_u32(s, devices.len(), "--lock-cclock")?, None => vec![] },
        mem_mhz: match &a.lock_mclock { Some(s) => clocks::expand_u32(s, devices.len(), "--lock-mclock")?, None => vec![] },
        mem_off: match &a.moffset { Some(s) => clocks::expand_i32(s, devices.len(), "--moffset")?, None => vec![] },
    };
    let _clock_guard = clocks::apply(&devices, &clock_opts, &clock_dir);
    // undervolt (opt-in): first put back what a crashed run left behind, then map/tune/apply; the guard restores the owner's offsets on any exit
    uv::recover(&a.uv);
    let uv_state = uv::start(&a.uv, &devices, &stop);
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
        println!("GPU {dev}: {}", gs.name);
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
    let power = nvml::Nvml::load().ok().map(|nv| {
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
            Change::Started => println!("devfee: mining {}s for FreeForGemPear dev ({}%)", devfee::Params::from_env().slot.as_secs(), devfee::FEE_PERCENT),
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
                let w = power.as_ref().and_then(|(nv, d)| d[i].and_then(|h| nv.power_w(h)));
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

    // shutdown: workers finish their in-flight attempts, provers drain their queues
    stop.store(true, Ordering::SeqCst);
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
FreeForGemPear - Pearl (PRL) GPU miner

USAGE:
    freeforgempear --pool HOST:PORT --wallet ADDRESS [OPTIONS]
    freeforgempear --self-test [--devices N]

OPTIONS:
    --pool HOST:PORT     stratum pool (default pearl-eu2.luckypool.io:3360)
    --wallet ADDRESS     your PRL wallet address (required)
    --worker NAME        worker name (default worker1)
    --password TEXT      stratum password (default x); Kryptex custom share difficulty: d=VALUE, e.g. d=8388608
    --devices 0,1,...    CUDA devices to mine on (default: all visible; honours CUDA_VISIBLE_DEVICES)
    --minutes N          stop after N minutes (default: run until interrupted)
    --self-test          verify the GPU kernel against the CPU reference and exit
    --auto-uv            opt-in: find and apply a safe GPU undervolt (V/F offset, needs root/admin); see MANUAL.md
    --uv-offset N        apply a fixed V/F offset of N MHz (after a quick check) instead of tuning
    --uv-reset           forget the saved undervolt results
    --uv-max N           highest offset the tuner tries (default 495, at most 600)
    --uv-min N           offset the tuner starts from and never goes below (default: the HiveOS/driver offset, else 120;
                         RTX 40/50: at least 200)
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

Mining runs until SIGINT/SIGTERM (Ctrl-C), reconnecting to the pool automatically.
A 1% developer fee (60 s in every 100 min, on the same pool) is part of the license terms.
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

/// Build identifier: stays in the binary, never read for any purpose.
#[used]
static BUILD_LAYOUT_ID: [u8; 26] = *b"ffgp-5555ad29a64e9e82-2a0d";

fn main() -> Result<()> {
    std::hint::black_box(&BUILD_LAYOUT_ID);
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
                println!("freeforgempear {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--self-test" => test = true,
            "--pool" => a.pool = val()?,
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
            "--uv-offset" => a.uv.fixed = Some(val()?.trim().trim_start_matches('+').parse().map_err(|_| anyhow::anyhow!("--uv-offset needs an integer (MHz)"))?),
            "--uv-max" => a.uv.max_offset = val()?.trim().trim_start_matches('+').parse().map_err(|_| anyhow::anyhow!("--uv-max needs an integer (MHz)"))?,
            "--uv-min" => a.uv.min_offset = Some(val()?.trim().trim_start_matches('+').parse().map_err(|_| anyhow::anyhow!("--uv-min needs an integer (MHz)"))?),
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
            "--target-mode" => a.mode = if val()? == "jackpot" { TargetMode::Jackpot } else { TargetMode::Base },
            _ => bail!("unknown argument {arg} (see --help)"),
        }
    }
    let _ = uv_given;
    if let Some(mut c) = child {
        c.dev = a.devices.as_ref().map_or(0, |d| d[0]);
        std::process::exit(uv::check_child(&c));
    }
    ensure!(!(a.uv.auto && a.uv.fixed.is_some()), "--auto-uv and --uv-offset exclude each other");
    ensure!(a.uv.step >= 1, "--uv-step must be at least 1 MHz");
    ensure!(a.uv.max_offset >= a.uv.step, "--uv-max must be at least one step (--uv-step {})", a.uv.step);
    ensure!(a.uv.max_offset <= uv::HARD_CAP, "--uv-max must not exceed {}", uv::HARD_CAP);
    if let Some(mn) = a.uv.min_offset {
        ensure!(mn >= 0 && mn <= a.uv.max_offset, "--uv-min must be between 0 and --uv-max ({})", a.uv.max_offset);
    }
    ensure!(a.uv.reval_hours >= 0.0, "--uv-reval-hours must not be negative");
    if a.uv.reset {
        uv::reset(&a.uv);
        if a.wallet.is_empty() && !test && !a.uv.enabled() {
            return Ok(());
        }
    }
    if test {
        return self_test(a.devices.as_ref().map_or(0, |d| d[0]));
    }
    ensure!(!a.wallet.is_empty(), "--wallet is required (see --help)");
    ensure!(a.pool.contains(':'), "--pool must be HOST:PORT");
    mine(a)
}
