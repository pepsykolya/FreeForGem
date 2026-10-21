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

// CMFD address text forms, ported from Common Foundry 1.0.16 crates/cmfd-node/src/address.rs (MIT,
// Copyright (c) 2026 CommonFoundry contributors): the 32-byte x-only key as 64 hex characters, or bech32m (BIP-350) with
// the network prefix ("cmfd" mainnet, "tcmfd" test networks), "1", 52 key symbols and a 6-symbol checksum, no witness version.
const BECH32_CHARSET: &[u8; 32] = b"qpzry9x8gf2tvdw0s3jn54khce6mua7l";
const BECH32M_CONST: u32 = 0x2bc8_30a3;
const MAINNET_HRP: &str = "cmfd";
const TEST_NETWORK_HRP: &str = "tcmfd";

fn bech32_polymod(values: &[u8]) -> u32 {
    const GEN: [u32; 5] = [0x3b6a_57b2, 0x2650_8e6d, 0x1ea1_19fa, 0x3d42_33dd, 0x2a14_62b3];
    let mut chk: u32 = 1;
    for &v in values {
        let top = chk >> 25;
        chk = ((chk & 0x01ff_ffff) << 5) ^ u32::from(v);
        for (i, g) in GEN.iter().enumerate() {
            if (top >> i) & 1 == 1 {
                chk ^= g;
            }
        }
    }
    chk
}

fn hrp_expand(hrp: &str) -> Vec<u8> {
    let b = hrp.as_bytes();
    b.iter().map(|c| c >> 5).chain(std::iter::once(0)).chain(b.iter().map(|c| c & 31)).collect()
}

/// `cmfd1...` address of a 32-byte key (used by the tests; the pool gets hex).
#[cfg(test)]
fn encode_address(hrp: &str, key: &[u8; 32]) -> String {
    let (mut sym, mut buf, mut bits) = (Vec::new(), 0u32, 0u32);
    for &byte in key {
        buf = ((buf << 8) | u32::from(byte)) & 0xfff;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            sym.push(((buf >> bits) & 31) as u8);
        }
    }
    if bits > 0 {
        sym.push(((buf << (5 - bits)) & 31) as u8);
    }
    let mut v = hrp_expand(hrp);
    v.extend(&sym);
    v.extend([0u8; 6]);
    let pm = bech32_polymod(&v) ^ BECH32M_CONST;
    sym.extend((0..6).map(|i| ((pm >> (5 * (5 - i))) & 31) as u8));
    format!("{hrp}1{}", sym.iter().map(|&s| BECH32_CHARSET[s as usize] as char).collect::<String>())
}

/// The payout key as 64 lowercase hex characters (what the pool and the engine take) from either text form:
/// 64 hex characters, or a checksummed mainnet `cmfd1...` address. A test-network `tcmfd1...` address is refused.
pub fn wallet_hex(w: &str) -> Result<String> {
    let w = w.trim();
    if w.bytes().all(|b| b.is_ascii_hexdigit()) {
        ensure!(w.len() == 64, "CMFD wallet: expected a cmfd1... address or 64 hexadecimal characters (got {} hex characters)", w.len());
        return Ok(w.to_ascii_lowercase());
    }
    ensure!(w.len() <= 90 && w.bytes().all(|b| (33..=126).contains(&b)), "CMFD wallet: not a cmfd1... address (wrong length or characters)");
    ensure!(
        !(w.bytes().any(|b| b.is_ascii_lowercase()) && w.bytes().any(|b| b.is_ascii_uppercase())),
        "CMFD wallet: the address mixes upper and lower case"
    );
    let lower = w.to_ascii_lowercase();
    let sep = lower.rfind('1').ok_or_else(|| anyhow::anyhow!("CMFD wallet: expected a cmfd1... address or 64 hexadecimal characters"))?;
    let (hrp, data) = (&lower[..sep], &lower[sep + 1..]);
    ensure!(!hrp.is_empty() && data.len() >= 6, "CMFD wallet: the address has the wrong length");
    let sym: Vec<u8> = data
        .bytes()
        .map(|c| BECH32_CHARSET.iter().position(|&x| x == c).map(|p| p as u8))
        .collect::<Option<_>>()
        .ok_or_else(|| anyhow::anyhow!("CMFD wallet: the address contains a character that is not used in addresses"))?;
    let mut v = hrp_expand(hrp);
    v.extend(&sym);
    ensure!(bech32_polymod(&v) == BECH32M_CONST, "CMFD wallet: address checksum does not match: the address is mistyped or incomplete");
    if hrp == TEST_NETWORK_HRP {
        anyhow::bail!("CMFD wallet: {hrp}1... is a test-network address; mainnet pools need a {MAINNET_HRP}1... address");
    }
    ensure!(hrp == MAINNET_HRP, "CMFD wallet: address is for another network: expected {MAINNET_HRP}1..., found {hrp}1...");
    let key = &sym[..sym.len() - 6];
    ensure!(key.len() == 52, "CMFD wallet: the address does not hold a 32-byte key");
    let (mut hex, mut buf, mut bits) = (String::new(), 0u32, 0u32);
    for &s in key {
        buf = ((buf << 5) | u32::from(s)) & 0xfff;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            hex.push_str(&format!("{:02x}", (buf >> bits) & 0xff));
        }
    }
    ensure!(bits == 4 && buf & 0xf == 0, "CMFD wallet: the address does not hold a 32-byte key");
    Ok(hex)
}

pub fn validate_wallet(w: &str) -> Result<()> {
    wallet_hex(w).map(|_| ())
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

/// `--backend amd`: the CMFD work runs on the OpenCL worker `cmfd-v4-replay-ocl` (same stdin protocol and output files as the
/// CUDA worker) driven by `cmfd-miner-ffgp`, the engine rebuilt from FreeForgeMiner source with the FreeForGemPear patch in
/// cmfd/patches/ (MIT): `FFM_GPU_BACKEND=opencl` + `FFM_GPU_UUID` select the GPU without nvidia-smi (until FreeForgeMiner has
/// that option itself; then the release binary is used here too).
fn amd() -> bool {
    gpu::backend() == gpu::Backend::Amd
}

/// Pool client binary: the FFGP-patched engine on AMD (on NVIDIA only with FFGP_CMFD_PATCHED=1, for tests), else the release.
fn miner_program() -> &'static str {
    let opt_in = std::env::var("FFGP_CMFD_PATCHED").is_ok_and(|v| v == "1");
    if amd() || opt_in { "cmfd-miner-ffgp" } else { "cmfd-miner" }
}

/// The GPU worker: CUDA `cmfd-v4-replay`, or the OpenCL `cmfd-v4-replay-ocl` with the AMD backend.
fn replay_worker() -> &'static str {
    if amd() { "cmfd-v4-replay-ocl" } else { "cmfd-v4-replay" }
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
    for f in [bin(miner_program()), bin(replay_worker())] {
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
    // AMD: the patched pool client and the OpenCL worker (see amd())
    let program = match program {
        "cmfd-miner" => miner_program(),
        "cmfd-v4-replay" => replay_worker(),
        p => p,
    };
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
    if amd() {
        if let Some(u) = uuid {
            c.env("FFM_GPU_BACKEND", "opencl"); // FFGP engine patch: no nvidia-smi, the GPU is FFM_GPU_UUID
            c.env("FFM_GPU_UUID", u);
            // OpenCL device index (same AMD device order as the Pearl library): looked up by identity
            if let Some(d) = (0..gpu::device_count() as i32).find(|&d| device_uuid(d).is_ok_and(|x| x == u)) {
                c.env("CMFD_CL_DEVICE", d.to_string());
            }
        }
    }
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
    ensure!(count > 0, "no {} GPU devices visible (check the driver{})", gpu::backend().name(), if amd() { "" } else { " and CUDA_VISIBLE_DEVICES" });
    let list = match devices {
        Some(d) => {
            for &x in d {
                ensure!(x >= 0 && (x as usize) < count, "device {x} does not exist ({count} {} device(s) visible)", gpu::backend().name());
            }
            d.clone()
        }
        None => (0..count as i32).collect(),
    };
    list.into_iter()
        .map(|d| Ok((d, device_uuid(d)?, gpu::device_name(d))))
        .collect()
}

/// GPU identity for the engine (`--gpu`, nvidia-smi format). AMD OpenCL has no device UUID: current pearlgpu_amd libraries
/// return a PCI-based stand-in themselves (`GPU-a3d00000-0000-0000-0000-0000<bus><dev><fn>`); older ones are covered here.
fn device_uuid(dev: i32) -> Result<String> {
    match gpu::device_uuid(dev) {
        Ok(u) => Ok(u),
        Err(e) if amd() => {
            let bus = gpu::device_pci_bus_id(dev).map_err(|_| e)?;
            let hex: String = bus.chars().filter(|c| c.is_ascii_hexdigit()).collect();
            let tail = format!("{:0>12}", &hex[hex.len().saturating_sub(12)..]).to_ascii_lowercase();
            Ok(format!("GPU-a3d00000-0000-0000-0000-{tail}"))
        }
        Err(e) => Err(e),
    }
}

/// `--self-test --algo cmfd`: the engine's GPU self-test on each selected device (small exact differential of the
/// tensor-core path against the DP4A reference path; no model, no pool).
/// Official test vector of Common Foundry 1.0.16 (AMD HIP worker qualification, MIT, Copyright (c) 2026 Common Foundry AMD HIP
/// contributors): the mainnet template at nonce 0, 32 nonces; the final activations of one RUNBATCH 32 hash to REF_SHA256.
const REF_COEFFICIENTS: &str = "fixtures/mainnet-template-nonce0-count32.coefficients.bin";
const REF_SHA256: &str = "fe67b80af4671210cd33306d0976c7dbb285dd55203d8eff733866cd7ef4a7a7";
const REF_RUNS: usize = 2;

fn sha256_hex(path: &Path) -> Result<String> {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    let mut f = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    std::io::copy(&mut f, &mut h)?;
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Loads the model on the GPU and runs the official 32-nonce vector REF_RUNS times through the engine's CUDA worker
/// (`--server` protocol: RUNBATCH -> CMFD_V4_REPLAY_DONE). Returns the SHA-256 of every run.
fn reference_vector(engine: &Path, uuid: &str, mode: &str, bank: &Path, work: &Path) -> Result<Vec<String>> {
    use std::io::Write;
    std::fs::create_dir_all(work)?;
    let coef = engine.join(REF_COEFFICIENTS);
    ensure!(coef.is_file(), "test vector {} missing", coef.display());
    let mut child = engine_cmd(engine, "cmfd-v4-replay", Some(uuid), mode)
        .arg("--server")
        .arg(bank)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdin = child.stdin.take().unwrap();
    let (tx, rx) = mpsc::channel::<(usize, String)>();
    pipe(child.stdout.take().unwrap(), 0, tx.clone());
    pipe(child.stderr.take().unwrap(), 0, tx);
    let wait_for = |tag: &str, secs: u64| -> Result<()> {
        let end = Instant::now() + Duration::from_secs(secs);
        loop {
            let left = end.saturating_duration_since(Instant::now());
            match rx.recv_timeout(left) {
                Ok((_, l)) if l.contains(tag) => return Ok(()),
                Ok((_, l)) if l.starts_with("error") => anyhow::bail!("GPU worker: {l}"),
                Ok(_) => {}
                Err(_) => anyhow::bail!("GPU worker: no {tag} within {secs} s"),
            }
        }
    };
    let res = (|| -> Result<Vec<String>> {
        wait_for("CMFD_V4_REPLAY_READY", 300)?;
        let mut hashes = Vec::new();
        for run in 1..=REF_RUNS {
            let out = work.join(format!("refvec-run{run}"));
            writeln!(stdin, "RUNBATCH\t32\t{}\t{}", coef.display(), out.display())?;
            stdin.flush()?;
            wait_for("CMFD_V4_REPLAY_DONE", 300)?;
            let file = work.join(format!("refvec-run{run}-final-activation.bin"));
            hashes.push(sha256_hex(&file)?);
            let _ = std::fs::remove_file(&file);
        }
        Ok(hashes)
    })();
    let _ = writeln!(stdin, "QUIT");
    drop(stdin);
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(20) {
        if let Ok(Some(_)) = child.try_wait() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = child.kill();
    let _ = child.wait();
    res
}

/// `--self-test --algo cmfd`: per GPU, (1) the engine's own GPU self-test (small exact differential of the tensor-core path
/// against the DP4A reference, no model) and, when the model is present, (2) the official Common Foundry 1.0.16 test vector
/// (mainnet template, 32 nonces, SHA-256 of the final activations) run twice on the real model.
pub fn self_test(devices: &Option<Vec<i32>>, engine_dir_opt: &Option<PathBuf>, mode: &str, model_opt: &Option<PathBuf>, state_dir: &Path) -> Result<()> {
    let engine = engine_dir(engine_dir_opt)?;
    println!("== CMFD self-test (engine {}) ==", engine_label(&engine));
    let gpus = select_devices(devices)?;
    let bank = model_dir(model_opt).join("MODEL-V2.bank");
    let with_model = bank.is_file();
    let devs: Vec<i32> = gpus.iter().map(|g| g.0).collect();
    // the same stock-offset rule as for mining: a Pearl undervolt would fail the test vector
    // NVML only; on AMD nothing to reset (the amduv undervolt is only active while Pearl mines and is restored on exit)
    if !amd() {
        crate::cmfd_offset::recover(state_dir);
    }
    let _offset_guard = if with_model && !amd() { crate::cmfd_offset::apply(&devs, None, state_dir) } else { None };
    let mut failed = 0;
    for (dev, uuid, name) in gpus {
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
            continue;
        }
        if !with_model {
            continue;
        }
        let t = Instant::now();
        let work = std::env::temp_dir().join(format!("ffgp-cmfd-selftest-{}", std::process::id()));
        match reference_vector(&engine, &uuid, mode, &bank, &work) {
            Ok(h) if h.iter().all(|x| x == REF_SHA256) => println!(
                "[ok] GPU{dev}: official Common Foundry 1.0.16 test vector (32 nonces, {} runs) sha256 {REF_SHA256} ({:?}, model load included)",
                h.len(),
                t.elapsed()
            ),
            Ok(h) => {
                println!("[FAIL] GPU{dev}: test vector sha256 {h:?}, expected {REF_SHA256}: the GPU computes wrongly (offset/clocks?)");
                failed += 1;
            }
            Err(e) => {
                println!("[FAIL] GPU{dev}: test vector: {e:#}");
                failed += 1;
            }
        }
        let _ = std::fs::remove_dir_all(&work);
    }
    if !with_model {
        println!("(test vector skipped: no model at {}; it runs once the model is downloaded, or give --cmfd-model DIR)", bank.display());
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
    let wallet = wallet_hex(&o.wallet)?;
    if wallet != o.wallet.trim() {
        println!("cmfd: wallet {} = payout key {wallet}", o.wallet.trim());
    }
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
    // clocks, the undervolt offset and the power readout are NVML (NVIDIA only); AMD runs stock, power comes from the library
    if amd() {
        ensure!(!o.clocks.any(), "--lock-cclock/--lock-mclock/--moffset are NVIDIA-only (NVML); not available with the AMD backend");
        ensure!(o.uv_offset.is_none(), "--uv-offset is NVIDIA-only (NVML); not available for CMFD with the AMD backend");
    }
    if !amd() {
        clocks::recover(&o.state_dir);
    }
    let _clock_guard = if amd() { None } else { Some(clocks::apply(&devs, &o.clocks, &o.state_dir)) };
    // a Pearl undervolt offset makes the engine fail: stock (0) for the run, or the explicit --uv-offset
    if !amd() {
        crate::cmfd_offset::recover(&o.state_dir);
    }
    let _offset_guard = if amd() { None } else { crate::cmfd_offset::apply(&devs, o.uv_offset, &o.state_dir) };

    let nv = if amd() { None } else { nvml::Nvml::load().ok() };
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
                    .args(["--pool", &o.pool, "--miner", &wallet, "--worker", &o.worker, "--gpu", &s.uuid])
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
                // AMD default 32: same FW/s as 64 on the RX 6800 XT (~3.3 s per pass instead of ~6.4 s, fewer stale shares)
                if let Some(b) = o.batch.or(if amd() { Some(32) } else { None }) {
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
        let w = nv.zip(s.nv).and_then(|(n, d)| n.power_w(d)).or_else(|| gpu::amd_power_w(s.dev)); // AMD: ADLX / hwmon
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
    }

    fn key_of(hex: &str) -> [u8; 32] {
        let mut k = [0u8; 32];
        for (i, b) in k.iter_mut().enumerate() {
            *b = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap();
        }
        k
    }

    #[test]
    fn cmfd1_addresses() {
        // fixed vectors of Common Foundry 1.0.16 (crates/cmfd-node/src/address.rs tests): x of the secp256k1 generator
        let g = "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";
        let main = "cmfd10xlxvlhemja6c4dqv22uapctqupfhlxm9h8z3k2e72q4k9hcz7vq4yhywr";
        let test = "tcmfd10xlxvlhemja6c4dqv22uapctqupfhlxm9h8z3k2e72q4k9hcz7vq236tfw";
        assert_eq!(encode_address(MAINNET_HRP, &key_of(g)), main);
        assert_eq!(encode_address(TEST_NETWORK_HRP, &key_of(g)), test);
        assert_eq!(wallet_hex(main).unwrap(), g);
        assert_eq!(wallet_hex(&main.to_ascii_uppercase()).unwrap(), g);
        assert_eq!(wallet_hex(g).unwrap(), g);
        assert_eq!(wallet_hex(&g.to_ascii_uppercase()).unwrap(), g);
        assert!(wallet_hex(test).unwrap_err().to_string().contains("test-network"));
        // a changed checksum symbol, a dropped symbol, mixed case, junk
        assert!(wallet_hex(&main.replace("4yhywr", "4yhywq")).unwrap_err().to_string().contains("checksum"));
        assert!(wallet_hex(&main[..main.len() - 1]).is_err());
        assert!(wallet_hex("cmfd10XLXVLhemja6c4dqv22uapctqupfhlxm9h8z3k2e72q4k9hcz7vq4yhywr").is_err());
        for junk in ["", "cmfd", "cmfd1", &g[..63], "not an address", "prl1pjcsjdpjzhccamllqmy6fncpjy526nrrza626y27zsa6hwqzt03ssfw8fyr"] {
            assert!(wallet_hex(junk).is_err(), "{junk}");
        }
        // the test payout key and the dev-fee key round-trip through their cmfd1 form
        for hex in [
            "307df1d1ac94b025c2768ef48ac9e739d7b74a8920a57d3393d21d025c8005f8",
            "a6b0915b0620997b2606574726fcf05628da6dc9ae9f2ff5bffe6d0a33ea9f2c",
        ] {
            let a = encode_address(MAINNET_HRP, &key_of(hex));
            assert!(a.starts_with("cmfd1") && a.len() == 63, "{a}");
            assert_eq!(wallet_hex(&a).unwrap(), hex);
        }
        assert!(validate_worker("rig-1.a_b", false).is_ok());
        assert!(validate_worker("bad name", false).is_err());
        assert!(validate_worker(&"x".repeat(30), true).is_err());
    }
}
