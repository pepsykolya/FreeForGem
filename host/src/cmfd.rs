// FreeForGemPear — see LICENSE
//! Common Foundry (CMFD, ForgeMatrix V4): second coin of FreeForGemPear.
//!
//! The CMFD work is done by the FreeForgeMiner engine (MIT, https://github.com/pepsykolya/freeforgeminer), shipped
//! unchanged in the `cmfd/` folder next to this binary: `cmfd-miner` (certificate-pinned cmfd+tls pool client, share
//! search, the engine's own 1% dev fee) and `cmfd-v4-replay` (the CUDA worker: int8 tensor-core forward of the 6.4 GB
//! ForgeMatrix V4 model, BLAKE3 digest on the GPU).  This module is the FreeForGemPear side:
//!   * the same command line, HiveOS package and Windows launcher as for Pearl (`--algo cmfd`, or a `cmfd+tls://` pool URL);
//!   * model preparation (official Common Foundry download/verification script of the engine package);
//!   * one engine process per GPU (as FreeForgeMiner itself runs on HiveOS), restarted with back-off if it exits;
//!   * clock locks (`--lock-cclock/--lock-mclock/--moffset`, NVML) applied before the engine starts and undone on exit;
//!   * one periodic FreeForGemPear stats line (FW/s per GPU, watts, FW/kWh, shares) for the log and HiveOS h-stats;
//!   * `--self-test`: the engine's own GPU self-test (`cmfd-v4-replay --self-test`, exact differential vs the reference path).
//!
//! Dev fee in CMFD mode: only the engine's built-in 1% (36 s per hour per GPU to the FreeForgeMiner/FreeForGemPear developer
//! wallet on the user's pool, worker `ffm-fee`). The Pearl dev fee of this program is not run, so the total stays 1%.
use crate::{clocks, gpu, nvml};
use anyhow::{Context, Result, ensure};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub const SCHEME: &str = "cmfd+tls://";
/// Official Common Foundry download locations of the model (same as FreeForgeMiner 1.2.3 uses).
const RELEASE_BASE: &str = "https://downloads.commonfoundry.ai/v0.1.0-rc.1";
const FALLBACK_BASE: &str = "https://github.com/JustAResearcher/CommonFoundry-Binaries/releases/download/v0.1.0-devnet.16";
/// A stats sample older than this is not shown as hashrate (the engine reports every 10 s).
const MAX_SAMPLE_AGE: Duration = Duration::from_secs(45);
const RESTART_DELAYS: [u64; 5] = [15, 30, 60, 120, 300];
/// An engine process that dies this early counts as a failed start; after MAX_QUICK_FAILS in a row the GPU is left alone
/// (no endless reconnects: pools ban addresses for connection storms).
const QUICK_FAIL: Duration = Duration::from_secs(120);
const MAX_QUICK_FAILS: usize = 5;

/// PIDs of the running engine processes, for the "second Ctrl-C: exit at once" path (no orphaned engines on the GPU).
static CHILDREN: std::sync::Mutex<Vec<u32>> = std::sync::Mutex::new(Vec::new());

fn kill_pid(pid: u32) {
    #[cfg(unix)]
    let _ = Command::new("kill").args(["-KILL", &pid.to_string()]).status();
    #[cfg(windows)]
    let _ = Command::new("taskkill").args(["/PID", &pid.to_string(), "/T", "/F"]).stdout(Stdio::null()).stderr(Stdio::null()).status();
}

/// Kills every engine process at once (and on Windows its CUDA worker); used when the user insists on exiting.
pub fn kill_all() {
    if let Ok(v) = CHILDREN.lock() {
        for &pid in v.iter() {
            kill_pid(pid);
        }
    }
}

pub fn is_cmfd_url(pool: &str) -> bool {
    pool.trim().to_ascii_lowercase().starts_with(SCHEME)
}

fn is_hex64(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// `cmfd+tls://IPv4:PORT?pin=64HEX` — the only form the engine accepts (numeric address, certificate pin).
pub fn validate_pool(url: &str) -> Result<()> {
    let rest = url.strip_prefix(SCHEME).ok_or_else(|| anyhow::anyhow!("CMFD pool URL must start with {SCHEME}"))?;
    let (addr, query) = rest.split_once('?').ok_or_else(|| anyhow::anyhow!("CMFD pool URL needs ?pin=<64 hex> (the pool certificate pin)"))?;
    let (ip, port) = addr.rsplit_once(':').ok_or_else(|| anyhow::anyhow!("CMFD pool URL needs IP:PORT"))?;
    ensure!(ip.parse::<std::net::Ipv4Addr>().is_ok(), "CMFD pool address must be a numeric IPv4 address (got {ip:?})");
    ensure!(port.parse::<u16>().is_ok_and(|p| p > 0), "CMFD pool port must be 1-65535 (got {port:?})");
    let pin = query.strip_prefix("pin=").unwrap_or("");
    ensure!(is_hex64(pin), "CMFD pool URL must end with ?pin=<64 hex characters> (the pool certificate pin)");
    Ok(())
}

pub fn validate_wallet(w: &str) -> Result<()> {
    ensure!(is_hex64(w), "CMFD wallet must be the 64-character hexadecimal payout address (got {} characters)", w.len());
    Ok(())
}

pub fn validate_worker(w: &str, per_gpu: bool) -> Result<()> {
    let max = if per_gpu { 26 } else { 32 }; // the engine appends ".gpuN" per card with --per-gpu-workers
    ensure!(
        !w.is_empty() && w.len() <= max && w.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-')),
        "CMFD worker name must be 1-{max} letters, digits, dots, underscores or hyphens (got {w:?})"
    );
    Ok(())
}

pub struct Opts {
    pub pool: String,
    pub wallet: String,
    pub worker: String,
    pub devices: Option<Vec<i32>>,
    pub minutes: Option<f64>,
    pub engine_dir: Option<PathBuf>,
    pub model_dir: Option<PathBuf>,
    pub mode: String,
    pub batch: Option<u32>,
    pub per_gpu_workers: bool,
    pub skip_prepare: bool,
    pub uv_offset: Option<i32>,
    pub state_dir: PathBuf,
    pub clocks: clocks::ClockOpts,
}

fn exe_dir() -> PathBuf {
    // no canonicalize: on Windows it yields \\?\ paths that the engine refuses; on Linux current_exe is already resolved
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn absolute(p: &Path) -> PathBuf {
    if p.is_absolute() { p.to_path_buf() } else { std::env::current_dir().map(|d| d.join(p)).unwrap_or_else(|_| p.to_path_buf()) }
}

fn bin(name: &str) -> String {
    if cfg!(windows) { format!("{name}.exe") } else { name.to_string() }
}

/// The engine folder: `--cmfd-dir`, `$FFGP_CMFD_DIR`, else `cmfd/` next to this binary.
pub fn engine_dir(given: &Option<PathBuf>) -> Result<PathBuf> {
    let dir = given
        .clone()
        .or_else(|| std::env::var_os("FFGP_CMFD_DIR").map(Into::into))
        .unwrap_or_else(|| exe_dir().join("cmfd"));
    let dir = absolute(&dir);
    for f in [bin("cmfd-miner"), bin("cmfd-v4-replay")] {
        ensure!(
            dir.join(&f).is_file(),
            "CMFD engine file {} is missing (the cmfd/ folder of the package; see cmfd/README.md)",
            dir.join(&f).display()
        );
    }
    Ok(dir)
}

/// Engine version line written by cmfd/fetch-engine.* (e.g. "FreeForgeMiner 1.2.3"), for the banner.
fn engine_label(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("ENGINE"))
        .ok()
        .and_then(|t| t.lines().next().map(|l| l.trim().to_string()))
        .filter(|l| !l.is_empty())
        .unwrap_or_else(|| "FreeForgeMiner (version unknown)".into())
}

/// Model folder: `--cmfd-model`, `$FFGP_CMFD_MODEL`, the HiveOS folder FreeForgeMiner uses (shared, so the 6.4 GB are
/// downloaded once per rig), else `cmfd-model/` next to this binary.
fn model_dir(given: &Option<PathBuf>) -> PathBuf {
    let hive = Path::new("/hive/miners/custom");
    let d = given
        .clone()
        .or_else(|| std::env::var_os("FFGP_CMFD_MODEL").map(Into::into))
        .unwrap_or_else(|| if !cfg!(windows) && hive.is_dir() { hive.join("cmfd-model") } else { exe_dir().join("cmfd-model") });
    absolute(&d)
}

/// Environment of every engine process (and of the self-test).
fn engine_cmd(engine: &Path, program: &str, uuid: Option<&str>, mode: &str) -> Command {
    let mut c = Command::new(engine.join(bin(program)));
    c.current_dir(engine);
    if !cfg!(windows) {
        let lib = engine.join("lib");
        let old = std::env::var("LD_LIBRARY_PATH").unwrap_or_default();
        c.env("LD_LIBRARY_PATH", if old.is_empty() { lib.display().to_string() } else { format!("{}:{old}", lib.display()) });
    }
    if let Some(u) = uuid {
        // one GPU per engine process; the engine honours an inherited single selector and passes it to its CUDA worker
        c.env("CUDA_VISIBLE_DEVICES", u);
    }
    c.env("CMFD_MODE", mode);
    c
}

/// Runs a preparation step, its output goes to our log; fails on a non-zero exit.
fn run_step(what: &str, mut c: Command) -> Result<()> {
    println!("cmfd: {what}...");
    let st = c.status().with_context(|| format!("cmfd: cannot start {what}"))?;
    ensure!(st.success(), "cmfd: {what} failed ({st})");
    Ok(())
}

/// Official model download/verification (resumable, authenticated parts; an existing model is only re-hashed).
fn prepare_model(engine: &Path, model: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(model).with_context(|| format!("create {}", model.display()))?;
    if cfg!(windows) {
        let mut c = Command::new("powershell.exe");
        // a PSModulePath inherited from PowerShell 7 (start.bat prefers pwsh) hides the 5.1 modules (Get-FileHash not found):
        // without it powershell.exe uses its own default module path
        c.env_remove("PSModulePath");
        c.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
            .arg(engine.join("PREPARE-V4-INPUTS.ps1"))
            .args(["-Role", "PoolMiner", "-Destination"])
            .arg(model)
            .args(["-FallbackReleaseBase", FALLBACK_BASE])
            .current_dir(engine);
        run_step("preparing the 6.4 GB ForgeMatrix V4 model (downloaded once, then only verified)", c)?;
    } else {
        // a large JSON document: shown only if the check fails
        let out = engine_cmd(engine, "cmfd-miner", None, "speed").arg("mainnet-launch-info").output().context("cmfd: cannot start cmfd-miner")?;
        if !out.status.success() {
            println!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
            anyhow::bail!("cmfd: the packaged mainnet identity check failed ({})", out.status);
        }
        println!("cmfd: packaged mainnet identity ok");
        let mut c = Command::new("python3");
        c.arg(engine.join("production-v4-inputs.py"))
            .arg("--chunk-manifest")
            .arg(engine.join("V4-INPUT-CHUNKS.json"))
            .arg("--input-manifest")
            .arg(engine.join("production-v4-rcnet-1-inputs.json"))
            .arg("--fixed-record")
            .arg(engine.join("FORGEMATRIX-V4-FIXED-ARTIFACT-RECORD-V1.json"))
            .arg("--destination")
            .arg(model)
            .args(["--role", "pool-miner", "--release-base", RELEASE_BASE, "--fallback-release-base", FALLBACK_BASE])
            .current_dir(engine);
        run_step("preparing the 6.4 GB ForgeMatrix V4 model (downloaded once, then only verified)", c)?;
        if engine.join("cmfd-launch").is_file() {
            let mut c = engine_cmd(engine, "cmfd-launch", None, "speed");
            c.arg("fetch").arg("--runtime").arg(engine.join("cmfd-miner")).arg("--wait");
            run_step("checking the mainnet launch beacon", c)?;
        }
    }
    let bank = model.join("MODEL-V2.bank");
    ensure!(bank.is_file(), "model preparation did not produce {}", bank.display());
    Ok(bank)
}

fn select_devices(devices: &Option<Vec<i32>>) -> Result<Vec<(i32, String, String)>> {
    let count = gpu::device_count();
    ensure!(count > 0, "no CUDA devices visible (check the driver and CUDA_VISIBLE_DEVICES)");
    let list = match devices {
        Some(d) => {
            for &x in d {
                ensure!(x >= 0 && (x as usize) < count, "device {x} does not exist ({count} CUDA device(s) visible)");
            }
            d.clone()
        }
        None => (0..count as i32).collect(),
    };
    list.into_iter()
        .map(|d| Ok((d, gpu::device_uuid(d)?, gpu::device_name(d))))
        .collect()
}

/// `--self-test --algo cmfd`: the engine's GPU self-test on each selected device (small exact differential of the
/// tensor-core path against the DP4A reference path; no model, no pool).
pub fn self_test(devices: &Option<Vec<i32>>, engine_dir_opt: &Option<PathBuf>, mode: &str) -> Result<()> {
    let engine = engine_dir(engine_dir_opt)?;
    println!("== CMFD self-test (engine {}) ==", engine_label(&engine));
    let mut failed = 0;
    for (dev, uuid, name) in select_devices(devices)? {
        let t = Instant::now();
        let out = engine_cmd(&engine, "cmfd-v4-replay", Some(&uuid), mode).arg("--self-test").stderr(Stdio::piped()).stdout(Stdio::piped()).output()?;
        let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        for l in text.lines() {
            println!("  GPU{dev}: {l}");
        }
        if out.status.success() && text.lines().any(|l| l.trim() == "replay_self_test=EXACT") {
            println!("[ok] GPU{dev} ({name}, {uuid}): ForgeMatrix V4 GPU path == reference ({:?})", t.elapsed());
        } else {
            println!("[FAIL] GPU{dev} ({name}): {}", out.status);
            failed += 1;
        }
    }
    ensure!(failed == 0, "CMFD self-test FAILED on {failed} GPU(s)");
    println!("== CMFD self-test PASSED ==");
    Ok(())
}

/// Numbers parsed from the engine's `MINER STATS | GPU n (uuid) | height h | hashrate X FW/s | accepted a | rejected r | stale s | ...`.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Sample {
    pub rate: f64,
    pub accepted: u64,
    pub rejected: u64,
    pub stale: u64,
    pub height: u64,
}

fn field<T: std::str::FromStr>(line: &str, key: &str) -> Option<T> {
    let i = line.find(key)? + key.len();
    line[i..].split(|c: char| c == ' ' || c == '|').next()?.trim().parse().ok()
}

pub fn parse_stats(line: &str) -> Option<Sample> {
    if !line.contains("MINER STATS |") || !line.contains(" FW/s") {
        return None;
    }
    Some(Sample {
        rate: field(line, "| hashrate ")?,
        accepted: field(line, "| accepted ")?,
        rejected: field(line, "| rejected ")?,
        stale: field(line, "| stale ")?,
        height: field(line, "| height ").unwrap_or(0),
    })
}

struct Slot {
    dev: i32,
    uuid: String,
    name: String,
    child: Option<Child>,
    started: Instant,
    restart_at: Option<Instant>,
    restarts: usize,
    quick_fails: usize,
    disabled: bool,
    last: Option<(Instant, Sample)>,
    base: Sample, // totals of earlier (exited) engine processes of this GPU
    fee: bool,
    nv: Option<nvml::Dev>,
}

impl Slot {
    fn totals(&self) -> Sample {
        let l = self.last.map(|(_, s)| s).unwrap_or_default();
        Sample { rate: 0.0, accepted: self.base.accepted + l.accepted, rejected: self.base.rejected + l.rejected, stale: self.base.stale + l.stale, height: l.height }
    }
}

fn pipe<R: Read + Send + 'static>(r: R, idx: usize, tx: mpsc::Sender<(usize, String)>) {
    std::thread::spawn(move || {
        for line in BufReader::new(r).lines() {
            let Ok(l) = line else { break };
            if tx.send((idx, l)).is_err() {
                break;
            }
        }
    });
}

fn stop_child(c: &mut Child) {
    if let Ok(mut v) = CHILDREN.lock() {
        v.retain(|&p| p != c.id());
    }
    if let Ok(Some(_)) = c.try_wait() {
        return;
    }
    // ask politely first: the engine stops its CUDA worker and closes the pool session
    #[cfg(unix)]
    let _ = Command::new("kill").args(["-INT", &c.id().to_string()]).status(); // the engine stops gracefully on SIGINT (ctrlc)
    #[cfg(windows)]
    let _ = Command::new("taskkill").args(["/PID", &c.id().to_string(), "/T", "/F"]).stdout(Stdio::null()).stderr(Stdio::null()).status();
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(10) {
        if let Ok(Some(_)) = c.try_wait() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = c.kill();
    let _ = c.wait();
}

pub fn mine(o: Opts, stop: Arc<AtomicBool>) -> Result<()> {
    validate_pool(&o.pool)?;
    validate_wallet(&o.wallet)?;
    validate_worker(&o.worker, o.per_gpu_workers)?;
    ensure!(matches!(o.mode.as_str(), "speed" | "eco"), "--cmfd-mode must be speed or eco");
    if let Some(b) = o.batch {
        ensure!((1..=64).contains(&b), "--cmfd-batch must be 1-64");
    }
    let engine = engine_dir(&o.engine_dir)?;
    let gpus = select_devices(&o.devices)?;
    println!(
        "FreeForGemPear {} | algo cmfd (Common Foundry, ForgeMatrix V4; engine {}) | pool {} | worker {} | {} GPU(s) | mode {}",
        env!("CARGO_PKG_VERSION"),
        engine_label(&engine),
        o.pool,
        o.worker,
        gpus.len(),
        o.mode
    );
    println!("devfee: 1% inside the CMFD engine (36 s per hour per GPU, random slot, same pool, worker ffm-fee; log lines \"DEV FEE |\"); no other fee in CMFD mode");
    let model = model_dir(&o.model_dir);
    let bank = if o.skip_prepare {
        let b = model.join("MODEL-V2.bank");
        ensure!(b.is_file(), "--cmfd-skip-prepare: {} not found", b.display());
        println!("cmfd: model {} (verification skipped by --cmfd-skip-prepare)", b.display());
        b
    } else {
        prepare_model(&engine, &model)?
    };
    if stop.load(Ordering::Relaxed) {
        return Ok(());
    }

    // clock locks before the engine starts (the engine must not see clocks change under it)
    let devs: Vec<i32> = gpus.iter().map(|g| g.0).collect();
    clocks::recover(&o.state_dir);
    let _clock_guard = clocks::apply(&devs, &o.clocks, &o.state_dir);
    // a Pearl undervolt offset makes the engine fail: stock (0) for the run, or the explicit --uv-offset
    crate::cmfd_offset::recover(&o.state_dir);
    let _offset_guard = crate::cmfd_offset::apply(&devs, o.uv_offset, &o.state_dir);

    let nv = nvml::Nvml::load().ok();
    let mut slots: Vec<Slot> = gpus
        .iter()
        .map(|(dev, uuid, name)| Slot {
            dev: *dev,
            uuid: uuid.clone(),
            name: name.clone(),
            child: None,
            started: Instant::now(),
            restart_at: Some(Instant::now()),
            restarts: 0,
            quick_fails: 0,
            disabled: false,
            last: None,
            base: Sample::default(),
            fee: false,
            nv: nv.as_ref().and_then(|n| gpu::device_pci_bus_id(*dev).ok().and_then(|b| n.by_bus(&b).ok())),
        })
        .collect();
    for s in &slots {
        println!("GPU {}: {} ({})", s.dev, s.name, s.uuid);
    }
    // per-GPU scratch of the engine (small batch files); not in $FFGP_STATE_DIR, which is /hive-config (flash) on HiveOS
    let hive = Path::new("/hive/miners/custom");
    let work_root = absolute(&std::env::var_os("FFGP_CMFD_WORK").map(PathBuf::from).unwrap_or_else(|| {
        if !cfg!(windows) && hive.is_dir() { hive.join("freeforgempear-data").join("cmfd-work") } else { exe_dir().join("cmfd-work") }
    }));
    let (tx, rx) = mpsc::channel::<(usize, String)>();
    let start = Instant::now();
    let deadline = o.minutes.map(|m| Duration::from_secs_f64(m * 60.0));
    let mut last_report = Instant::now();

    while !stop.load(Ordering::Relaxed) && deadline.is_none_or(|d| start.elapsed() < d) {
        // (re)start engine processes
        for (i, s) in slots.iter_mut().enumerate() {
            if let Some(c) = &mut s.child {
                if let Ok(Some(st)) = c.try_wait() {
                    let lived = s.started.elapsed();
                    if lived > Duration::from_secs(600) {
                        s.restarts = 0;
                    }
                    if let Ok(mut v) = CHILDREN.lock() {
                        v.retain(|&p| p != c.id());
                    }
                    s.quick_fails = if lived < QUICK_FAIL { s.quick_fails + 1 } else { 0 };
                    s.base = s.totals();
                    s.last = None;
                    s.fee = false;
                    s.child = None;
                    if s.quick_fails >= MAX_QUICK_FAILS {
                        s.disabled = true;
                        s.restart_at = None;
                        println!(
                            "[GPU{}] CMFD engine exited ({st}) after {}s: {} failed starts in a row, this GPU is NOT restarted any more; fix the cause shown above and restart the miner",
                            s.dev,
                            lived.as_secs(),
                            s.quick_fails
                        );
                    } else {
                        let delay = RESTART_DELAYS[s.restarts.min(RESTART_DELAYS.len() - 1)];
                        s.restarts += 1;
                        println!("[GPU{}] CMFD engine exited ({st}) after {}s; restarting in {delay}s", s.dev, lived.as_secs());
                        s.restart_at = Some(Instant::now() + Duration::from_secs(delay));
                    }
                }
            }
            if s.child.is_none() && s.restart_at.is_some_and(|t| Instant::now() >= t) {
                let scratch = work_root.join(&s.uuid);
                std::fs::create_dir_all(&scratch).with_context(|| format!("create {}", scratch.display()))?;
                let mut c = engine_cmd(&engine, "cmfd-miner", Some(&s.uuid), &o.mode);
                c.arg("pool")
                    .args(["--pool", &o.pool, "--miner", &o.wallet, "--worker", &o.worker, "--gpu", &s.uuid])
                    .arg("--production-v4-bank")
                    .arg(&bank)
                    .arg("--production-v4-replay-worker")
                    .arg(engine.join(bin("cmfd-v4-replay")))
                    .arg("--production-v4-scratch")
                    .arg(&scratch)
                    .args(["--stats-seconds", "10"])
                    .env("FFM_SINGLE_WORKER", if o.per_gpu_workers { "0" } else { "1" })
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped());
                if let Some(b) = o.batch {
                    c.args(["--batch", &b.to_string()]);
                }
                match c.spawn() {
                    Ok(mut ch) => {
                        pipe(ch.stdout.take().unwrap(), i, tx.clone());
                        pipe(ch.stderr.take().unwrap(), i, tx.clone());
                        println!("[GPU{}] CMFD engine started (pid {})", s.dev, ch.id());
                        if let Ok(mut v) = CHILDREN.lock() {
                            v.push(ch.id());
                        }
                        s.child = Some(ch);
                        s.started = Instant::now();
                        s.restart_at = None;
                    }
                    Err(e) => {
                        let delay = RESTART_DELAYS[s.restarts.min(RESTART_DELAYS.len() - 1)];
                        s.restarts += 1;
                        println!("[GPU{}] cannot start the CMFD engine ({e}); retry in {delay}s", s.dev);
                        s.restart_at = Some(Instant::now() + Duration::from_secs(delay));
                    }
                }
            }
        }
        // engine output: shown with a GPU prefix, stats lines parsed
        while let Ok((i, line)) = rx.try_recv() {
            let s = &mut slots[i];
            if let Some(sample) = parse_stats(&line) {
                s.last = Some((Instant::now(), sample));
            }
            if line.starts_with("DEV FEE | mining") {
                s.fee = true;
            } else if line.starts_with("DEV FEE |") {
                s.fee = false;
            }
            println!("[GPU{}] {line}", s.dev);
            if line.contains("out of memory") {
                println!("[GPU{}] hint: CMFD needs about 7 GB of free GPU memory (6.4 GB model); stop other programs/miners on this GPU", s.dev);
            }
            if line.contains("illegal memory access") || line.contains("unspecified launch failure") || line.contains("INTEGRITY | ERROR") {
                println!(
                    "[GPU{}] hint: the GPU computed wrongly. Typical cause: a core clock offset / undervolt (e.g. one tuned for Pearl). CMFD loads the tensor cores harder: use offset 0 (stock) or only a power limit / --lock-cclock",
                    s.dev
                );
            }
        }
        if slots.iter().all(|s| s.disabled) {
            stop.store(true, Ordering::SeqCst);
            for s in slots.iter_mut() {
                if let Some(c) = &mut s.child {
                    stop_child(c);
                }
            }
            anyhow::bail!("CMFD: every GPU failed {MAX_QUICK_FAILS} starts in a row; see the messages above");
        }
        if last_report.elapsed() >= Duration::from_secs(10) {
            last_report = Instant::now();
            println!("{}", report_line(&slots, nv.as_ref(), start.elapsed()));
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    println!("cmfd: stopping the engine processes...");
    for s in slots.iter_mut() {
        if let Some(c) = &mut s.child {
            stop_child(c);
        }
    }
    // late output (final stats) of the stopped processes
    std::thread::sleep(Duration::from_millis(300));
    while let Ok((i, line)) = rx.try_recv() {
        if let Some(sample) = parse_stats(&line) {
            slots[i].last = Some((Instant::now(), sample));
        }
        println!("[GPU{}] {line}", slots[i].dev);
    }
    let (mut a, mut r, mut st) = (0, 0, 0);
    for s in &slots {
        let t = s.totals();
        a += t.accepted;
        r += t.rejected;
        st += t.stale;
    }
    println!("done: cmfd {:?}; shares accepted {a} rejected {r} stale {st} (engine counters, dev-fee slices included)", start.elapsed());
    Ok(())
}

/// `[ 120s] GPU0 20.31 (179 W, 408 FW/kWh)  GPU1 ... | total 35.10 FW/s | 360 W, 351 FW/kWh | user sub N acc A rej R | stale S | ...`
/// The h-stats.sh of the HiveOS package reads "GPUn <rate>", "total <rate> FW/s" and "user sub/acc/rej" as for Pearl.
fn report_line(slots: &[Slot], nv: Option<&nvml::Nvml>, up: Duration) -> String {
    let mut parts = Vec::new();
    let (mut total, mut total_w, mut all_w) = (0.0, 0.0, true);
    let (mut acc, mut rej, mut stale, mut fee, mut height) = (0, 0, 0, false, 0);
    for s in slots {
        let fresh = s.last.filter(|(t, _)| t.elapsed() <= MAX_SAMPLE_AGE).map(|(_, x)| x.rate);
        let rate = fresh.unwrap_or(0.0);
        total += rate;
        let w = nv.zip(s.nv).and_then(|(n, d)| n.power_w(d));
        let pw = match w {
            Some(w) if w > 0.0 => {
                total_w += w;
                format!(" ({:.0} W, {:.0} FW/kWh)", w, rate * 3_600_000.0 / w)
            }
            _ => {
                all_w = false;
                String::new()
            }
        };
        let state = if s.disabled {
            " DISABLED"
        } else if s.child.is_none() {
            " RESTARTING"
        } else if fresh.is_none() {
            " STARTING"
        } else {
            ""
        };
        parts.push(format!("GPU{} {:.2}{pw}{state}", s.dev, rate));
        let t = s.totals();
        acc += t.accepted;
        rej += t.rejected;
        stale += t.stale;
        height = height.max(t.height);
        fee |= s.fee;
    }
    let power = if all_w && total_w > 0.0 { format!(" | {:.0} W, {:.0} FW/kWh", total_w, total * 3_600_000.0 / total_w) } else { String::new() };
    format!(
        "[{:>4}s] {} | total {:.2} FW/s{} | user sub {} acc {} rej {} | stale {} | height {}{}",
        up.as_secs(),
        parts.join("  "),
        total,
        power,
        acc + rej + stale,
        acc,
        rej,
        stale,
        height,
        if fee { "  [DEVFEE]" } else { "" }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_line() {
        let l = "MINER STATS | GPU 1 (GPU-6b1c0a52-6ba4-0b7c-1e57-9d7f2b2b8f0e) | height 31337 | hashrate 20.31 FW/s | accepted 12 | rejected 1 | stale 2 | blocks 0 | credit 5 atoms | power 179.20 W | efficiency 408000.00 FW/kWh | temp 61 C | uptime 00:10:00";
        let s = parse_stats(l).unwrap();
        assert_eq!(s, Sample { rate: 20.31, accepted: 12, rejected: 1, stale: 2, height: 31337 });
        assert!(parse_stats("MINER STATS | height 1 | uptime 00:00:01 | blocks 0").is_none());
        assert!(parse_stats("Pool: cmfd+tls://1.2.3.4:5?pin=00").is_none());
    }

    #[test]
    fn pool_and_wallet() {
        let pin = "71ab1d2e36cd20226ac16184b91dda5527126fa8b29ee1de2b6f5cd8b7028041";
        assert!(is_cmfd_url(&format!("cmfd+tls://188.35.20.44:29445?pin={pin}")));
        assert!(!is_cmfd_url("pearl-eu2.luckypool.io:3360"));
        assert!(validate_pool(&format!("cmfd+tls://188.35.20.44:29445?pin={pin}")).is_ok());
        assert!(validate_pool(&format!("cmfd+tls://pool.example.com:29445?pin={pin}")).is_err());
        assert!(validate_pool("cmfd+tls://188.35.20.44:29445").is_err());
        assert!(validate_pool(&format!("cmfd+tls://188.35.20.44:0?pin={pin}")).is_err());
        assert!(validate_wallet(pin).is_ok());
        assert!(validate_wallet("prl1abc").is_err());
        assert!(validate_worker("rig-1.a_b", false).is_ok());
        assert!(validate_worker("bad name", false).is_err());
        assert!(validate_worker(&"x".repeat(30), true).is_err());
    }
}
