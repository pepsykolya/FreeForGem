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
/// Official Common Foundry download locations of the model (same as FreeForgeMiner 1.2.4 uses).
const RELEASE_BASE: &str = "https://downloads.commonfoundry.ai/v0.1.0-rc.1";
const FALLBACK_BASE: &str = "https://github.com/JustAResearcher/CommonFoundry-Binaries/releases/download/v0.1.0-devnet.16";
/// A stats sample older than this is not shown as hashrate (the engine reports every 10 s).
const MAX_SAMPLE_AGE: Duration = Duration::from_secs(45);
const RESTART_DELAYS: [u64; 5] = [15, 30, 60, 120, 300];
/// An engine process that dies this early counts as a failed start; after MAX_QUICK_FAILS in a row the GPU is left alone
/// (no endless reconnects: pools ban addresses for connection storms).
const QUICK_FAIL: Duration = Duration::from_secs(120);
const MAX_QUICK_FAILS: usize = 5;

/// 1.7.8 pool lists: the engine has ONE pool and retries a lost / unreachable pool by itself for ever, so this side watches the
/// engines' log lines and moves them to the next `--pool` entry (restart with the new `--pool`) when none of them has the pool for
/// this long; the failback check of the primary is `probe_pool` (TLS with the pin + login + a job), the same restart goes back.
const FAILOVER_AFTER: Duration = Duration::from_secs(60);

/// What an engine says about its pool connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PoolSt {
    Unknown,
    Up,
    Down,
}

/// `Connected to pool at height N.` = up; `Pool unavailable (..); retrying...` / `Pool connection lost (..); reconnecting...` = down.
pub fn pool_line(line: &str) -> Option<PoolSt> {
    if line.starts_with("Connected to pool at height") {
        Some(PoolSt::Up)
    } else if line.starts_with("Pool unavailable (") || line.starts_with("Pool connection lost (") {
        Some(PoolSt::Down)
    } else {
        None
    }
}

/// The pool refused address-only mining: the engine exits (it does not retry), another entry may allow it.
pub fn pool_refused_line(line: &str) -> bool {
    line.contains("does not allow address-only mining")
}

/// Fail over when no engine has its pool and at least one has been without it for `after` (engines that have not reported yet,
/// e.g. still loading the model, do not count either way).
pub fn failover_due(states: &[(PoolSt, Instant)], now: Instant, after: Duration) -> bool {
    !states.iter().any(|s| s.0 == PoolSt::Up) && states.iter().any(|s| s.0 == PoolSt::Down && now.duration_since(s.1) >= after)
}

fn failover_after() -> Duration {
    std::env::var("FFGP_CMFD_FAILOVER_S").ok().and_then(|v| v.parse::<f64>().ok()).map_or(FAILOVER_AFTER, |s| Duration::from_secs_f64(s.max(1.0)))
}

/// The connection the engine keeps itself can die silently (a dead route, a DROP rule: no reset, no error): the engine keeps its
/// pool state "up" and shows its hash rate, but the block height it reports never changes again (Rig05, DROP in both directions:
/// 6 min 41 s without noticing, no failover).  This watches the height the engines report: no change for STALL_MIN of the mean block
/// interval times STALL_MEAN_MULT = the connection is taken as dead and the engines restart on the next `--pool` entry.
///
/// The threshold is long on purpose.  CMFD finds a block every ~60 s on average (Rig05: 63157 -> 63178 in 22 min, gaps of 20-190 s),
/// and block times are random (exponential): a gap of 3x the mean (3 min) happens after 5 % of the blocks, i.e. dozens of times a
/// day; 12x the mean (12 min) after 0.006 % (about once in 2 weeks), 8 min is the floor, 30 min the cap.  A false trigger only
/// costs a restart (one minute of model loading), a real silence costs the rest of the session.
const STALL_MEAN_MULT: f64 = 12.0;
const STALL_MIN: Duration = Duration::from_secs(480);
const STALL_MAX: Duration = Duration::from_secs(1800);
const STALL_DEFAULT_MEAN: f64 = 60.0; // the mean block interval until the run has seen enough blocks of its own
const STALL_SAMPLES: usize = 4; // block intervals seen before the measured mean replaces the default

fn stall_override() -> Option<Duration> {
    std::env::var("FFGP_CMFD_STALL_S").ok().and_then(|v| v.parse::<f64>().ok()).map(|s| Duration::from_secs_f64(s.max(1.0)))
}

#[derive(Debug)]
pub struct HeightWatch {
    height: u64,
    since: Instant,
    intervals: Vec<f64>, // seconds between height changes, newest last (at most 16)
    mult: u32,           // x2 per restart that did not bring a new height (a chain that really stands is not restarted every 12 min)
}

impl HeightWatch {
    pub fn new(now: Instant) -> HeightWatch {
        HeightWatch { height: 0, since: now, intervals: Vec::new(), mult: 1 }
    }

    /// The engines restarted / are not mining for the user (paused, fee slot, loading): the clock starts over, the measured block
    /// interval stays.
    pub fn reset(&mut self, now: Instant) {
        self.height = 0;
        self.since = now;
    }

    /// How long without a new height counts as a dead connection.
    pub fn threshold(&self) -> Duration {
        if let Some(d) = stall_override() {
            return d * self.mult;
        }
        let mean = if self.intervals.len() >= STALL_SAMPLES { self.intervals.iter().sum::<f64>() / self.intervals.len() as f64 } else { STALL_DEFAULT_MEAN };
        Duration::from_secs_f64((mean * STALL_MEAN_MULT).clamp(STALL_MIN.as_secs_f64(), STALL_MAX.as_secs_f64())) * self.mult
    }

    /// Every status pass. `height`: the highest height the engines report (0 = none yet); `live`: at least one engine has its pool
    /// and mines for the user.  Returns how long the height has stood when that passed the threshold (the caller restarts the engines).
    pub fn tick(&mut self, now: Instant, height: u64, live: bool) -> Option<Duration> {
        if !live || height == 0 {
            self.reset(now);
            return None;
        }
        if self.height == 0 {
            self.height = height;
            self.since = now;
            return None;
        }
        if height != self.height {
            let dt = now.duration_since(self.since).as_secs_f64();
            if height > self.height && height - self.height == 1 && dt > 1.0 {
                self.intervals.push(dt); // one block: its interval is a sample of the block time (a jump after a catch-up is not)
                if self.intervals.len() > 16 {
                    self.intervals.remove(0);
                }
            }
            self.height = height;
            self.since = now;
            self.mult = 1;
            return None;
        }
        let silent = now.duration_since(self.since);
        (silent >= self.threshold()).then_some(silent)
    }

    /// The engines were restarted because of a silence: the clock starts over and the next threshold is longer (x2, up to x4).
    pub fn restarted(&mut self, now: Instant) {
        self.reset(now);
        self.mult = (self.mult * 2).min(4);
    }
}

#[derive(Debug)]
struct PinVerifier {
    pin: [u8; 32],
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl rustls::client::danger::ServerCertVerifier for PinVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &rustls::pki_types::CertificateDer<'_>,
        _i: &[rustls::pki_types::CertificateDer<'_>],
        _n: &rustls::pki_types::ServerName<'_>,
        _o: &[u8],
        _t: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        use sha2::{Digest, Sha256};
        let actual: [u8; 32] = Sha256::digest(end_entity.as_ref()).into();
        if actual != self.pin {
            return Err(rustls::Error::General("pool certificate SHA-256 pin mismatch".into()));
        }
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(&self, m: &[u8], c: &rustls::pki_types::CertificateDer<'_>, d: &rustls::DigitallySignedStruct) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(m, c, d, &self.provider.signature_verification_algorithms)
    }
    fn verify_tls13_signature(&self, m: &[u8], c: &rustls::pki_types::CertificateDer<'_>, d: &rustls::DigitallySignedStruct) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(m, c, d, &self.provider.signature_verification_algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

fn read_frame(s: &mut impl Read) -> Result<serde_json::Value, String> {
    let mut len = [0u8; 4];
    s.read_exact(&mut len).map_err(|e| format!("pool closed or silent: {e}"))?;
    let n = u32::from_be_bytes(len) as usize;
    if n == 0 || n > 1 << 22 {
        return Err(format!("bad frame length {n}"));
    }
    let mut body = vec![0u8; n];
    s.read_exact(&mut body).map_err(|e| format!("pool closed or silent: {e}"))?;
    serde_json::from_slice(&body).map_err(|e| format!("bad frame: {e}"))
}

/// The failback check of a CMFD pool: connect, TLS 1.3 with the certificate pin, the pool's login challenge, the address-only login
/// (the same Hello the engine sends: no payout signature), the acknowledgement and the first job. Nothing is mined or submitted.
/// The pool protocol is the engine's (JSON frames behind a 4-byte length); only its handshake is repeated here.
pub fn probe_pool(url: &str, wallet_hex: &str, worker: &str, timeout: Duration) -> Result<(), String> {
    use std::io::Write;
    let rest = url.strip_prefix(SCHEME).ok_or("not a cmfd+tls:// URL")?;
    let (addr, query) = rest.split_once('?').ok_or("no ?pin=")?;
    let pin_hex = query.split('&').find_map(|q| q.strip_prefix("pin=")).ok_or("no pin=")?;
    let pin: [u8; 32] = crate::stratum::hex_decode(pin_hex).and_then(|b| b.try_into().ok()).ok_or("bad pin")?;
    let payout: Vec<u8> = crate::stratum::hex_decode(wallet_hex).filter(|b| b.len() == 32).ok_or("bad wallet")?;
    let sa: std::net::SocketAddr = addr.parse().map_err(|e| format!("bad address: {e}"))?;
    let t0 = Instant::now();
    let left = || timeout.saturating_sub(t0.elapsed()).max(Duration::from_millis(200));
    let mut sock = std::net::TcpStream::connect_timeout(&sa, left().min(Duration::from_secs(5))).map_err(|e| format!("connect: {e}"))?;
    sock.set_read_timeout(Some(left())).map_err(|e| e.to_string())?;
    sock.set_write_timeout(Some(Duration::from_secs(5))).map_err(|e| e.to_string())?;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let cfg = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| e.to_string())?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PinVerifier { pin, provider }))
        .with_no_client_auth();
    let mut conn = rustls::ClientConnection::new(Arc::new(cfg), rustls::pki_types::ServerName::IpAddress(sa.ip().into())).map_err(|e| e.to_string())?;
    while conn.is_handshaking() {
        conn.complete_io(&mut sock).map_err(|e| format!("TLS: {e}"))?;
    }
    let mut st = rustls::StreamOwned::new(conn, sock);
    let ch = read_frame(&mut st)?;
    match ch.get("type").and_then(|v| v.as_str()) {
        Some("auth_challenge") => {}
        Some("error") => return Err(format!("pool error: {}", ch.get("message").and_then(|v| v.as_str()).unwrap_or("?"))),
        _ => return Err("no login challenge from the pool".into()),
    }
    let hello = serde_json::json!({
        "type": "hello",
        "protocol_version": ch["protocol_version"],
        "network_id": ch["network_id"],
        "consensus_fingerprint": ch["consensus_fingerprint"],
        "worker": worker,
        "payout": payout,
        "payout_signature": [],
    });
    let body = serde_json::to_vec(&hello).map_err(|e| e.to_string())?;
    st.write_all(&(body.len() as u32).to_be_bytes()).and_then(|_| st.write_all(&body)).and_then(|_| st.flush()).map_err(|e| format!("send: {e}"))?;
    let ack = read_frame(&mut st)?;
    match ack.get("type").and_then(|v| v.as_str()) {
        Some("hello_ack") => {}
        Some("error") => return Err(format!("login refused: {}", ack.get("code").and_then(|v| v.as_str()).unwrap_or("?"))),
        _ => return Err("no login acknowledgement from the pool".into()),
    }
    let job = read_frame(&mut st)?;
    match job.get("type").and_then(|v| v.as_str()) {
        Some("job") => Ok(()),
        Some("error") => Err(format!("pool error: {}", job.get("code").and_then(|v| v.as_str()).unwrap_or("?"))),
        _ => Err("no job from the pool".into()),
    }
}

/// PIDs of the running engine processes, for the "second Ctrl-C: exit at once" path (no orphaned engines on the GPU).
pub(crate) static CHILDREN: std::sync::Mutex<Vec<u32>> = std::sync::Mutex::new(Vec::new());

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

#[cfg(test)]
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
    /// every --pool entry (primary first, then backups); `pool` is the first
    pub pools: Vec<String>,
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
    pub uv_offset: crate::uv::PerGpu, // --uv-offset: one value or one per GPU (in `devices` order)
    pub state_dir: PathBuf,
    pub clocks: clocks::ClockOpts,
    pub uv: crate::uv::Opts, // --auto-uv (NVIDIA): the CMFD-specific tuner (cmfd_uv.rs), with --uv-max/--uv-min/--uv-step ...
}

pub(crate) fn exe_dir() -> PathBuf {
    // no canonicalize: on Windows it yields \\?\ paths that the engine refuses; on Linux current_exe is already resolved
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub(crate) fn absolute(p: &Path) -> PathBuf {
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
pub(crate) fn replay_worker() -> &'static str {
    if amd() { "cmfd-v4-replay-ocl" } else { "cmfd-v4-replay" }
}

pub(crate) fn bin(name: &str) -> String {
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
    check_glibc(&dir)?;
    Ok(dir)
}

/// glibc the engine binaries need: build.sh writes "need GLIBC_2.30" into cmfd/ENGINE for the binaries it built on Ubuntu 20.04
/// (cmfd/build-engine-compat.sh); the FreeForgeMiner release binaries (built on Ubuntu 22.04) need 2.34.  Pearl itself runs
/// from glibc 2.31.  Without this check an older system only shows "GLIBC_2.3x not found" from the engine and a restart loop.
fn check_glibc(dir: &Path) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        unsafe extern "C" {
            fn gnu_get_libc_version() -> *const std::os::raw::c_char;
        }
        let parse = |v: &str| {
            let mut it = v.split('.').map(|x| x.trim().parse::<u32>().unwrap_or(0));
            (it.next().unwrap_or(0), it.next().unwrap_or(0))
        };
        let need = std::fs::read_to_string(dir.join("ENGINE"))
            .ok()
            .and_then(|t| t.split("need GLIBC_").nth(1).map(|r| parse(r.split_whitespace().next().unwrap_or(""))))
            .filter(|n| n.0 == 2)
            .unwrap_or((2, 34));
        let v = unsafe { std::ffi::CStr::from_ptr(gnu_get_libc_version()) }.to_string_lossy().into_owned();
        ensure!(
            parse(&v) >= need,
            "the CMFD engine in {} needs glibc {}.{} or newer; this system has glibc {v}. Pearl mining works here; for CMFD use the              release package (its engine runs from glibc 2.30, Ubuntu 20.04 / any HiveOS)",
            dir.display(),
            need.0,
            need.1
        );
    }
    #[cfg(not(target_os = "linux"))]
    let _ = dir;
    Ok(())
}

/// Engine version line written by cmfd/fetch-engine.* (e.g. "FreeForgeMiner 1.2.4"), for the banner.
pub(crate) fn engine_label(dir: &Path) -> String {
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
pub(crate) fn engine_cmd(engine: &Path, program: &str, uuid: Option<&str>, mode: &str) -> Command {
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
pub(crate) const REF_COEFFICIENTS: &str = "fixtures/mainnet-template-nonce0-count32.coefficients.bin";
pub(crate) const REF_SHA256: &str = "fe67b80af4671210cd33306d0976c7dbb285dd55203d8eff733866cd7ef4a7a7";
const REF_RUNS: usize = 2;

pub(crate) fn sha256_hex(path: &Path) -> Result<String> {
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
    // NVML only; on AMD nothing to reset (an amduv point is only active while the miner runs and is restored on exit)
    if !amd() {
        crate::cmfd_offset::recover(state_dir);
    }
    let _offset_guard = if with_model && !amd() { crate::cmfd_offset::apply(&devs, &crate::uv::PerGpu::Unset, state_dir) } else { None };
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
    mem_checked: bool, // memory-clock warning check done (once per GPU and run)
    uv_restart: bool,  // stopped here after an --auto-uv step back: restart soon, not counted as a failed start
    pool: (PoolSt, Instant), // what the engine last said about its pool connection, and since when (pool lists, see FAILOVER_AFTER)
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

/// Stops every running engine process: all are asked at once, then waited for together (10 s, then killed).
/// Linux: the GPU worker processes (`cmfd-v4-replay*`) started by an engine process. An engine stopped while it is still
/// loading the model can exit (or be killed after 10 s) without stopping its worker, which then keeps the GPU and 7 GB of its
/// memory (seen on a 4x CMP 50HX HiveOS rig): they are collected before the stop and ended after it.
fn worker_children(pid: u32) -> Vec<u32> {
    #[cfg(target_os = "linux")]
    {
        let mut out = Vec::new();
        for t in std::fs::read_dir(format!("/proc/{pid}/task")).into_iter().flatten().flatten() {
            let kids = std::fs::read_to_string(t.path().join("children")).unwrap_or_default();
            out.extend(kids.split_whitespace().filter_map(|k| k.parse::<u32>().ok()).filter(|&k| is_worker(k)));
        }
        return out;
    }
    #[allow(unreachable_code)]
    {
        let _ = pid;
        Vec::new()
    }
}

#[cfg(target_os = "linux")]
fn is_worker(pid: u32) -> bool {
    std::fs::read(format!("/proc/{pid}/cmdline")).is_ok_and(|c| String::from_utf8_lossy(&c).split('\0').next().is_some_and(|a| a.contains("cmfd-v4-replay")))
}

/// Pool switch (failover / failback): every engine is stopped (all asked at once, see stop_children), then they restart one by one,
/// 1.5 s apart, with the new `--pool` (the caller changed `cur_pool`). Not counted as a failed start.
fn switch_pool(slots: &mut [Slot], stagger: u64) {
    stop_children(slots);
    for (k, s) in slots.iter_mut().enumerate() {
        if s.child.take().is_some() {
            s.base = s.totals();
            s.last = None;
            s.fee = false;
            s.uv_restart = false;
        }
        s.pool = (PoolSt::Unknown, Instant::now());
        if !s.disabled {
            s.restart_at = Some(Instant::now() + Duration::from_millis(2000 + k as u64 * stagger));
            s.restarts = 0;
            s.quick_fails = 0;
        }
    }
}

fn stop_children(slots: &mut [Slot]) {
    let mut kids: Vec<&mut Child> = slots.iter_mut().filter_map(|s| s.child.as_mut()).collect();
    let workers: Vec<u32> = kids.iter().flat_map(|c| worker_children(c.id())).collect();
    for c in kids.iter_mut() {
        if let Ok(mut v) = CHILDREN.lock() {
            v.retain(|&p| p != c.id());
        }
        if let Ok(Some(_)) = c.try_wait() {
            continue;
        }
        // ask politely first: the engine stops its CUDA worker and closes the pool session
        #[cfg(unix)]
        let _ = Command::new("kill").args(["-INT", &c.id().to_string()]).status(); // the engine stops gracefully on SIGINT (ctrlc)
        #[cfg(windows)]
        let _ = Command::new("taskkill").args(["/PID", &c.id().to_string(), "/T", "/F"]).stdout(Stdio::null()).stderr(Stdio::null()).status();
    }
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(10) && kids.iter_mut().any(|c| !matches!(c.try_wait(), Ok(Some(_)))) {
        std::thread::sleep(Duration::from_millis(100));
    }
    for c in kids {
        if !matches!(c.try_wait(), Ok(Some(_))) {
            let _ = c.kill();
        }
        let _ = c.wait();
    }
    // a worker left behind by its engine (still loading the model): give it 5 s to notice its closed stdin, then end it
    #[cfg(target_os = "linux")]
    {
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(5) && workers.iter().any(|&w| is_worker(w)) {
            std::thread::sleep(Duration::from_millis(100));
        }
        for &w in &workers {
            if is_worker(w) {
                kill_pid(w);
            }
        }
    }
    let _ = workers;
}

/// CMFD needs the memory clock: a memory clock locked low (a Pearl overclock, e.g. HiveOS "Mem lock" 810 / nvidia-smi -lmc)
/// cuts it about 6x (RTX 3070: 3.7 instead of 21.8 FW/s).  Checked once per GPU while the engine mines; only a warning,
/// the miner never changes the owner's OC by itself.
fn check_mem_clock(nv: &nvml::Nvml, d: nvml::Dev, dev: i32, own_lock: Option<u32>) {
    let (Some(cur), Some(max)) = (nv.mem_clock_mhz(d), nv.max_mem_clock_mhz(d)) else { return };
    if max == 0 || cur * 2 >= max {
        return;
    }
    let why = match own_lock {
        Some(l) => format!("--lock-mclock {l} of this run"),
        None => "the OC locks the memory clock: HiveOS \"Mem lock\", nvidia-smi -lmc, ...".to_string(),
    };
    println!(
        "[GPU{dev}] WARNING: memory clock {cur} MHz of {max} MHz while mining CMFD ({why}). CMFD runs about 6x slower with the memory held low \
         (RTX 3070: 3.7 instead of 21.8 FW/s). Use a separate OC profile for CMFD without the memory clock lock; the miner does not change your OC"
    );
}

pub fn mine(o: Opts, stop: Arc<AtomicBool>) -> Result<()> {
    let pools: Vec<String> = if o.pools.is_empty() { vec![o.pool.clone()] } else { o.pools.clone() };
    for p in &pools {
        validate_pool(p)?;
    }
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
        "{} {} | algo cmfd (Common Foundry, ForgeMatrix V4; engine {}) | pool {} | worker {} | {} GPU(s) | mode {}",
        crate::PRODUCT,
        env!("CARGO_PKG_VERSION"),
        engine_label(&engine),
        if pools.len() > 1 { format!("{} (primary; backup: {})", crate::failover::label(&pools[0]), pools[1..].iter().map(|p| crate::failover::label(p)).collect::<Vec<_>>().join(", ")) } else { o.pool.clone() },
        o.worker,
        gpus.len(),
        o.mode
    );
    let link = crate::failover::Link::new(pools.len());
    link.begin(0);
    let mut cur_pool = 0usize; // the entry the engines are started with
    let mut failback_at: Option<Instant> = None; // when the last failback to the primary was made
    if pools.len() > 1 {
        let fb = crate::failover::opts().failback_min;
        println!(
            "pool list: {} entries, the first is the primary; the engines move to the next entry when none of them has its pool for {} s (they are restarted with it), {}",
            pools.len(),
            failover_after().as_secs(),
            if fb > 0.0 { format!("back to the primary after 2 good checks (login + job on a separate connection every {fb} min while on a backup, --failback-minutes)") } else { "failback is off (--failback-minutes 0)".to_string() }
        );
        let (labels, urls, w, wk) = (pools.clone(), pools.clone(), wallet.clone(), o.worker.clone());
        crate::failover::spawn_prober("user", link.clone(), stop.clone(), move |e| crate::failover::label(&labels[e]), move |e| probe_pool(&urls[e], &w, &wk, crate::failover::opts().probe_timeout));
    }
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
        ensure!(!o.uv_offset.is_set(), "--uv-offset is NVIDIA-only (NVML); not available for CMFD with the AMD backend");
    }
    if !amd() {
        clocks::recover(&o.state_dir);
    }
    let _clock_guard = if amd() { None } else { Some(clocks::apply(&devs, &o.clocks, &o.state_dir)) };
    // a Pearl undervolt offset makes the engine fail: stock (0) for the run, or the explicit --uv-offset
    if !amd() {
        crate::cmfd_offset::recover(&o.state_dir);
    }
    // --auto-uv (NVIDIA): every card is recorded first (the owner's offset goes back on exit), the base is 0 (a positive offset
    // is set to 0 as without --auto-uv), then the CMFD tuner applies its saved result or tunes on the CMFD load (cmfd_uv.rs)
    let auto_uv = !amd() && o.uv.auto && !o.uv.dry_run;
    if !amd() && o.uv.auto && o.uv.dry_run {
        crate::cmfd_uv::dry_run(&o.uv, &devs, &o.clocks.core_mhz);
    }
    let (_offset_guard, bases) = if amd() {
        (None, vec![])
    } else if auto_uv {
        crate::cmfd_offset::apply_managed(&devs, &o.state_dir)
    } else {
        (crate::cmfd_offset::apply(&devs, &o.uv_offset, &o.state_dir), vec![])
    };
    // per-GPU scratch of the engine (small batch files); not in $FFGP_STATE_DIR, which is /hive-config (flash) on HiveOS
    let hive = Path::new("/hive/miners/custom");
    let work_root = absolute(&std::env::var_os("FFGP_CMFD_WORK").map(PathBuf::from).unwrap_or_else(|| {
        if !cfg!(windows) && hive.is_dir() { hive.join("freeforgempear-data").join("cmfd-work") } else { exe_dir().join("cmfd-work") }
    }));
    if auto_uv && !stop.load(Ordering::Relaxed) {
        let ctx = crate::cmfd_uv::Ctx { engine: engine.clone(), bank: bank.clone(), mode: o.mode.clone(), batch: o.batch, work: work_root.clone(), core_lock: o.clocks.core_mhz.clone() };
        crate::cmfd_uv::start(&o.uv, &devs, &bases, &ctx, &stop);
    }
    // AMD --auto-uv: amduv applies its CMFD point (tuned on the CMFD load first if there is none) before the engine starts; its
    // guard process puts the owner's clocks / voltage / power limit back when this process exits (also on a crash or kill)
    let _amd_uv_guard = if amd() && o.uv.auto && !o.uv.dry_run {
        if devs.len() > 1 {
            println!("auto-uv (amd): more than one AMD GPU is not supported yet; tuning only device {}", devs[0]);
        }
        let goal = std::env::var("FFGP_AMD_UV_GOAL").unwrap_or_else(|_| "bal".into());
        let worker = engine.join(bin(replay_worker()));
        crate::amd_uv::start(devs[0], true, &goal, crate::amd_uv::Load::Cmfd { worker: &worker, model: &bank })
    } else {
        None
    };

    let nv = if amd() { None } else { nvml::Nvml::load().ok() };
    // 1.7.3: the engines start one by one, 1.5 s apart (FFG_START_STAGGER_MS), not all GPUs at full power in the same instant
    let stagger = std::env::var("FFG_START_STAGGER_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(1500u64);
    if gpus.len() > 1 && stagger > 0 {
        println!("cmfd: the engines start one by one, {:.1} s apart (no power surge of all cards at once)", stagger as f64 / 1000.0);
    }
    let mut slots: Vec<Slot> = gpus
        .iter()
        .enumerate()
        .map(|(k, (dev, uuid, name))| Slot {
            dev: *dev,
            uuid: uuid.clone(),
            name: name.clone(),
            child: None,
            started: Instant::now(),
            restart_at: Some(Instant::now() + Duration::from_millis(k as u64 * stagger)),
            restarts: 0,
            quick_fails: 0,
            disabled: false,
            last: None,
            base: Sample::default(),
            fee: false,
            nv: nv.as_ref().and_then(|n| gpu::device_pci_bus_id(*dev).ok().and_then(|b| n.by_bus(&b).ok())),
            mem_checked: false,
            uv_restart: false,
            pool: (PoolSt::Unknown, Instant::now()),
        })
        .collect();
    for s in &slots {
        println!("GPU {}: {} ({})", s.dev, s.name, s.uuid);
    }
    // 1.7.6 (--auto-uv): which GPU the driver reports an Xid for (NVML events; an Xid the kernel log pins on another process, e.g.
    // another miner on the same rig, does not count), remembered in gpu-faults.json as for Pearl and NOID: the next start does not
    // climb back into an offset that crashed; a burst of rejected shares of one engine steps its offset back
    let fwatch = if auto_uv {
        crate::faults::Watch::new(&slots.iter().map(|s| (s.dev, gpu::device_pci_bus_id(s.dev).unwrap_or_default(), s.name.clone())).collect::<Vec<_>>())
    } else {
        None
    };
    let mut bursts: Vec<crate::monitor::RejectBurst> = slots.iter().map(|_| crate::monitor::RejectBurst::new(Instant::now())).collect();
    let mut mined_ok_done = false;
    let mut pool_refused = false;
    let mut watchdog = crate::failover::Watch::default();
    let mut share_rate = crate::failover::ShareRate::default(); // seconds per share of the pool the session is on (the engine does not tell the difficulty)
    let mut hw = HeightWatch::new(Instant::now()); // silent engine connection: the height the engines report stands (HeightWatch)
    let mut fo_mult = 1u32; // failover waits failover_after() x this (grows while no pool works)
    let (tx, rx) = mpsc::channel::<(usize, String)>();
    let start = Instant::now();
    let deadline = o.minutes.map(|m| Duration::from_secs_f64(m * 60.0));
    let mut last_report = Instant::now();

    // idle mode (1.7.6): while the PC is in use the engines are stopped (the external CMFD engine has no light load: `--active light`
    // pauses too), the owner's offsets and clocks are on the cards; the engine's dev fee runs on its own mining time, so a pause
    // does not count
    let (mut paused, mut idle_logged) = (false, false);
    while !stop.load(Ordering::Relaxed) && deadline.is_none_or(|d| start.elapsed() < d) {
        let in_use = crate::idle::mode() != crate::idle::Mode::Full;
        if in_use && !paused && crate::cmfd_uv::idle_ready() {
            println!("idle: the PC is in use: the CMFD engines stop, the owner's clocks and offsets are put back");
            stop_children(&mut slots);
            for s in slots.iter_mut() {
                if s.child.take().is_some() {
                    s.base = s.totals();
                    s.last = None;
                    s.fee = false;
                    s.uv_restart = false;
                }
                s.restart_at = None;
            }
            crate::cmfd_offset::suspend();
            clocks::suspend();
            paused = true;
            idle_logged = false;
        } else if in_use && !paused && !idle_logged {
            idle_logged = true;
            println!("idle: the PC is in use; waiting for the running auto-uv check / tuning to finish before the engines stop");
        } else if !in_use && paused {
            println!("idle: the PC is idle again: clocks and offsets back, the CMFD engines start");
            clocks::resume();
            crate::cmfd_offset::resume();
            for (k, s) in slots.iter_mut().enumerate() {
                if !s.disabled {
                    s.restart_at = Some(Instant::now() + Duration::from_millis(k as u64 * stagger));
                    s.restarts = 0;
                }
            }
            paused = false;
        }
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
                    let uv_restart = std::mem::take(&mut s.uv_restart);
                    s.quick_fails = if lived < QUICK_FAIL && !uv_restart { s.quick_fails + 1 } else if uv_restart { s.quick_fails } else { 0 };
                    s.base = s.totals();
                    s.last = None;
                    s.fee = false;
                    s.pool = (PoolSt::Unknown, Instant::now());
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
                    } else if uv_restart {
                        println!("[GPU{}] CMFD engine stopped by --auto-uv (a step back, or the quick check after an OC apply); restarting in 5s (after the check)", s.dev);
                        s.restart_at = Some(Instant::now() + Duration::from_secs(5));
                    } else {
                        let delay = RESTART_DELAYS[s.restarts.min(RESTART_DELAYS.len() - 1)];
                        s.restarts += 1;
                        println!("[GPU{}] CMFD engine exited ({st}) after {}s; restarting in {delay}s", s.dev, lived.as_secs());
                        s.restart_at = Some(Instant::now() + Duration::from_secs(delay));
                    }
                }
            }
            if s.child.is_none() && s.restart_at.is_some_and(|t| Instant::now() >= t) && !crate::cmfd_uv::busy(s.dev) && !paused && !in_use {
                let scratch = work_root.join(&s.uuid);
                std::fs::create_dir_all(&scratch).with_context(|| format!("create {}", scratch.display()))?;
                let mut c = engine_cmd(&engine, "cmfd-miner", Some(&s.uuid), &o.mode);
                c.arg("pool")
                    .args(["--pool", &pools[cur_pool], "--miner", &wallet, "--worker", &o.worker, "--gpu", &s.uuid])
                    .arg("--production-v4-bank")
                    .arg(&bank)
                    .arg("--production-v4-replay-worker")
                    .arg(engine.join(bin(replay_worker())))
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
            let mut check_mem = false;
            if let Some(st) = pool_line(&line) {
                if s.pool.0 != st {
                    s.pool = (st, Instant::now());
                }
            }
            if pool_refused_line(&line) {
                pool_refused = true;
            }
            if let Some(sample) = parse_stats(&line) {
                s.last = Some((Instant::now(), sample));
                // the engine mines (model loaded, work running): the memory clock now is the one it really gets
                check_mem = !s.mem_checked && sample.rate > 0.0 && s.started.elapsed() >= Duration::from_secs(30);
            }
            if line.starts_with("DEV FEE | mining") {
                s.fee = true;
            } else if line.starts_with("DEV FEE |") {
                s.fee = false;
            }
            println!("[GPU{}] {line}", s.dev);
            if check_mem {
                s.mem_checked = true;
                if let (Some(n), Some(d)) = (nv.as_ref(), s.nv) {
                    check_mem_clock(n, d, s.dev, o.clocks.mem_mhz.get(i).copied().flatten());
                }
            }
            if line.contains("out of memory") {
                println!("[GPU{}] hint: CMFD needs about 7 GB of free GPU memory (6.4 GB model); stop other programs/miners on this GPU", s.dev);
            }
            if line.contains("illegal memory access") || line.contains("unspecified launch failure") || line.contains("misaligned address") || line.contains("INTEGRITY | ERROR") {
                // --auto-uv: a tuned card steps its CMFD offset back; the engine restarts at the new offset (it exits by itself
                // after a CUDA fault; after an INTEGRITY error it is stopped here)
                let why = if line.contains("INTEGRITY | ERROR") { "engine integrity check: wrong results" } else { "CUDA fault in the engine" };
                if crate::cmfd_uv::on_fault(s.dev, why) {
                    s.uv_restart = true;
                    if line.contains("INTEGRITY | ERROR") {
                        if let Some(c) = s.child.as_ref() {
                            let workers = worker_children(c.id());
                            kill_pid(c.id());
                            for w in workers {
                                kill_pid(w); // its GPU worker too: the next engine needs the GPU memory
                            }
                        }
                    }
                } else {
                    println!(
                        "[GPU{}] hint: the GPU computed wrongly. Typical cause: a core clock offset / undervolt (e.g. one tuned for Pearl). CMFD loads the tensor cores harder: use offset 0 (stock) or only a power limit / --lock-cclock",
                        s.dev
                    );
                }
            }
        }
        // pool lists (1.7.8): failover when no engine has its pool; failback when the prober asks for the primary
        if !paused && !in_use {
            let now = Instant::now();
            let states: Vec<(PoolSt, Instant)> = slots.iter().filter(|s| s.child.is_some()).map(|s| s.pool).collect();
            let req = link.take();
            // silent connection: engines with their pool "up" and mining for the user whose height has not changed for too long
            let live = slots.iter().any(|s| s.child.is_some() && s.pool.0 == PoolSt::Up) && !slots.iter().any(|s| s.fee);
            let height = slots.iter().filter(|s| s.child.is_some() && s.pool.0 == PoolSt::Up).map(|s| s.totals().height).max().unwrap_or(0);
            let stalled = hw.tick(now, height, live);
            let mut to: Option<(usize, String)> = None;
            let mut is_failback = false;
            if states.iter().any(|s| s.0 == PoolSt::Up) {
                fo_mult = 1; // an engine has its pool: the next failover waits the base time again
            }
            if let Some((t, why)) = req {
                // a failback (the primary is back), or the watchdog: the next entry / the same entry again (the engines restart)
                is_failback = t == 0 && cur_pool != 0 && why.starts_with("the primary");
                to = Some((t, why));
            } else if (pools.len() > 1 && (pool_refused || failover_due(&states, now, failover_after() * fo_mult))) || stalled.is_some() {
                let next = (cur_pool + 1) % pools.len();
                let why = if let Some(d) = stalled {
                    hw.restarted(now);
                    format!("the engines report the same block height for {:.1} min: the pool connection is silent", d.as_secs_f64() / 60.0)
                } else if pool_refused {
                    "the pool refused the login".to_string()
                } else {
                    format!("no engine has the pool for {} s", failover_after().as_secs())
                };
                if cur_pool == 0 && failback_at.is_some_and(|t| t.elapsed().as_secs_f64() < crate::failover::opts().bad_within_s) {
                    let mut fb = link.fb.lock().unwrap();
                    fb.left_bad(&crate::failover::opts());
                    println!("pool: the primary dropped again soon after the failback: the next checks come less often (x{})", fb.hold);
                }
                link.failovers.fetch_add(1, Ordering::Relaxed);
                failback_at = None;
                // no pool worked for the whole round: the engines restart less often (x2, up to x8 of the base time)
                if stalled.is_none() {
                    fo_mult = (fo_mult * 2).min(8);
                }
                to = Some((next, format!("pool {} of {} ({}) is not usable ({why}): failing over", cur_pool + 1, pools.len(), crate::failover::label(&pools[cur_pool]))));
            }
            pool_refused = false;
            if let Some((t, why)) = to {
                if t == 0 && is_failback {
                    failback_at = Some(now);
                    link.failbacks.fetch_add(1, Ordering::Relaxed);
                }
                println!("pool: {why}: the engines restart on pool {} of {} ({}); work in flight is dropped, the engines reload the model (about a minute)", t + 1, pools.len(), crate::failover::label(&pools[t]));
                cur_pool = t;
                link.begin(t);
                switch_pool(&mut slots, stagger);
            }
        } else {
            let _ = link.take();
            pool_refused = false;
            hw.reset(Instant::now());
        }
        if let Some(w) = &fwatch {
            for (k, xids) in w.take() {
                let c = &w.cards[k];
                let Some(s) = slots.iter_mut().find(|s| s.dev == c.dev) else { continue };
                if crate::cmfd_uv::busy(s.dev) {
                    continue; // a check of this GPU runs: its Xid is the check's verdict (cmfd_uv), not a crash while mining
                }
                let cur = crate::cmfd_offset::current(&c.bus);
                // another tool changed the card's offset (an OC apply): not a crash of the offset this miner set
                if crate::faults::foreign_offset(cur, crate::cmfd_offset::read(&c.bus)) || crate::faults::foreign_near(&c.bus) {
                    println!("faults: {} Xid {}: another tool changed the clocks / offset of this GPU at the same time: not counted as a failure of this miner's offset", c.label(), xids[0]);
                    continue;
                }
                // one fault often reports several Xids (13, then 43) within a second: one crash
                if crate::faults::get(&o.state_dir, &c.uuid).is_some_and(|p| p.n > 0 && crate::uv::now().saturating_sub(p.last) < 60) {
                    continue;
                }
                let r = crate::faults::record_crash(&o.state_dir, &c.uuid, &c.bus, &c.name, xids[0], cur);
                println!("faults: {} FAILED: Xid {} reported by the driver at the core offset {} (crash {} in a row); remembered in gpu-faults.json", c.label(), xids[0], cur.map_or("?".into(), |v| format!("{v:+}")), r.n);
                if crate::cmfd_uv::on_fault(s.dev, &format!("Xid {} reported by the driver", xids[0])) {
                    s.uv_restart = true;
                    if let Some(ch) = s.child.as_ref() {
                        let workers = worker_children(ch.id());
                        kill_pid(ch.id());
                        for wk in workers {
                            kill_pid(wk);
                        }
                    }
                }
            }
            // 30 minutes of mining without a fault clear the count of crashes in a row
            if !mined_ok_done && start.elapsed() >= Duration::from_secs(crate::faults::OK_SECS) {
                mined_ok_done = true;
                let ok: Vec<String> = slots.iter().filter(|s| s.child.is_some()).map(|s| s.uuid.clone()).collect();
                crate::faults::mined_ok(&o.state_dir, &ok);
            }
            // a burst of rejected shares from one engine: the wrong results come from its GPU
            for (i, s) in slots.iter_mut().enumerate() {
                if s.last.is_none() || s.uv_restart || crate::cmfd_uv::busy(s.dev) {
                    continue;
                }
                let t = s.totals();
                if let Some((drej, dacc)) = bursts[i].tick(Instant::now(), t.rejected, t.accepted, 0) {
                    if crate::cmfd_uv::on_fault(s.dev, &format!("burst of rejected shares ({drej} rejected vs {dacc} accepted in 5 min)")) {
                        s.uv_restart = true;
                        if let Some(ch) = s.child.as_ref() {
                            let workers = worker_children(ch.id());
                            kill_pid(ch.id());
                            for wk in workers {
                                kill_pid(wk);
                            }
                        }
                    }
                }
            }
        }
        // --auto-uv: an OC apply (HiveOS) reset a tuned card's CMFD offset: its engine stops, the saved offset gets a quick check
        // (cmfd_uv), then the engine restarts at it
        for dev in crate::cmfd_uv::poll_oc() {
            if let Some(s) = slots.iter_mut().find(|s| s.dev == dev) {
                if let Some(c) = s.child.as_ref() {
                    s.uv_restart = true;
                    let workers = worker_children(c.id());
                    kill_pid(c.id());
                    for w in workers {
                        kill_pid(w); // its GPU worker too: the check needs the GPU memory
                    }
                }
            }
            crate::cmfd_uv::begin_reapply(dev);
        }
        if slots.iter().all(|s| s.disabled) {
            stop.store(true, Ordering::SeqCst);
            stop_children(&mut slots);
            anyhow::bail!("CMFD: every GPU failed {MAX_QUICK_FAILS} starts in a row; see the messages above");
        }
        if let Some(l) = crate::status::summary_due() {
            println!("{l}");
        }
        if last_report.elapsed() >= Duration::from_secs(10) {
            // accepted-shares watchdog (the engine does not tell the share difficulty: shares are expected at the rate seen so far, one per
            // 6 minutes until there are 3 of them, so a pool is not left before ~60 minutes without a share at the start)
            if pools.len() > 0 && !paused && !in_use {
                let dt = last_report.elapsed().as_secs_f64();
                let (mut acc, mut rej, mut stale) = (0u64, 0u64, 0u64);
                for s in &slots {
                    let t = s.totals();
                    (acc, rej, stale) = (acc + t.accepted, rej + t.rejected, stale + t.stale);
                }
                let allowed = slots.iter().any(|s| s.pool.0 == PoolSt::Up) && !slots.iter().any(|s| s.fee);
                let est = share_rate.tick(link.entry().unwrap_or(cur_pool), dt, allowed, acc);
                crate::failover::watch_tick(&mut watchdog, &link, "user", dt, allowed, 1.0, Some(est), acc, rej + stale, acc + rej + stale);
            }
            last_report = Instant::now();
            println!("{}", report_line(&slots, nv.as_ref(), start.elapsed()));
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    // shutdown: the engines stop first (they must not compute with a Pearl undervolt back), then the owner's offsets and
    // clocks go back on every card, and only then the log (the log pipe may be gone already under HiveOS `miner stop`;
    // console writes never fail the miner, see the macros in main.rs)
    stop.store(true, Ordering::SeqCst); // a background start check / tuning stops too (1.7.2: it runs while the engines mine)
    stop_children(&mut slots);
    // the check workers of a background tuning (registered in CHILDREN): no orphan keeps 7 GB of a GPU after the miner exits
    kill_all();
    crate::cmfd_offset::restore_all();
    clocks::restore_all();
    println!("cmfd: engine processes stopped");
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

/// `[ 380s] GPU0 21.76 (237 W, 330742 FW/kWh)  GPU1 ... | total 87.46 FW/s | 948 W, 332248 FW/kWh | user sub N acc A rej R | stale S | ...`
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
        } else if s.child.is_none() && crate::idle::mode() != crate::idle::Mode::Full {
            " PAUSED: PC in use"
        } else if s.child.is_none() {
            " RESTARTING"
        } else if fresh.is_none() {
            " STARTING"
        } else {
            ""
        };
        parts.push(format!("GPU{} {:.2}{pw}{state}{}", s.dev, rate, crate::status::tag(s.dev)));
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
        let pin = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        assert!(is_cmfd_url(&format!("cmfd+tls://203.0.113.1:29445?pin={pin}")));
        assert!(!is_cmfd_url("pearl-eu2.luckypool.io:3360"));
        assert!(validate_pool(&format!("cmfd+tls://203.0.113.1:29445?pin={pin}")).is_ok());
        assert!(validate_pool(&format!("cmfd+tls://pool.example.com:29445?pin={pin}")).is_err());
        assert!(validate_pool("cmfd+tls://203.0.113.1:29445").is_err());
        assert!(validate_pool(&format!("cmfd+tls://203.0.113.1:0?pin={pin}")).is_err());
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
    #[test]
    fn pool_lines_and_failover_decision() {
        assert_eq!(pool_line("Connected to pool at height 123."), Some(PoolSt::Up));
        assert_eq!(pool_line("Pool unavailable (connection refused); retrying..."), Some(PoolSt::Down));
        assert_eq!(pool_line("Pool connection lost (broken pipe); reconnecting..."), Some(PoolSt::Down));
        assert_eq!(pool_line("DEV FEE | pool refused the fee session (x); skipped"), None);
        assert_eq!(pool_line("MINER STATS | GPU 0 | hashrate 1.0 FW/s"), None);
        assert!(pool_refused_line("Error: this pool does not allow address-only mining; the pool operator must enable address-only payouts"));
        let t0 = Instant::now();
        let later = t0 + Duration::from_secs(70);
        let a = Duration::from_secs(60);
        // nobody reported yet (model loading): no failover
        assert!(!failover_due(&[(PoolSt::Unknown, t0), (PoolSt::Unknown, t0)], later, a));
        // down for 70 s on every engine: fail over
        assert!(failover_due(&[(PoolSt::Down, t0), (PoolSt::Down, t0)], later, a));
        // down for 30 s only
        assert!(!failover_due(&[(PoolSt::Down, t0)], t0 + Duration::from_secs(30), a));
        // one engine still has its pool: stay (the others reconnect by themselves)
        assert!(!failover_due(&[(PoolSt::Down, t0), (PoolSt::Up, t0)], later, a));
        // an engine that has not reported yet does not prevent it
        assert!(failover_due(&[(PoolSt::Down, t0), (PoolSt::Unknown, t0)], later, a));
        assert!(!failover_due(&[], later, a));
    }

    #[test]
    fn height_watch_fires_only_after_a_long_silence() {
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let mut w = HeightWatch::new(t0);
        // a block about every 60 s with random gaps up to 300 s: never fires
        let mut t = 0;
        let mut h = 63157;
        for gap in [20u64, 30, 60, 190, 71, 140, 70, 181, 20, 40, 100, 300, 60] {
            for _ in 0..gap / 10 {
                t += 10;
                assert!(w.tick(at(t), h, true).is_none(), "t {t}");
            }
            h += 1;
            t += 10;
            assert!(w.tick(at(t), h, true).is_none());
        }
        // the height stands: not at 6 min 41 s (the Rig05 DROP case), but at the threshold (12 x the measured mean, 8 min floor)
        let thr = w.threshold().as_secs();
        assert!((480..=1800).contains(&thr), "{thr}");
        let t1 = t;
        let mut fired = None;
        for k in 1..400 {
            if w.tick(at(t1 + k * 10), h, true).is_some() {
                fired = Some(k * 10);
                break;
            }
        }
        let f = fired.expect("must fire");
        assert!(f >= 480 && f <= thr + 10, "{f} vs {thr}");
        // not live (paused, fee slot, engines loading, no pool): the clock does not run
        let mut w = HeightWatch::new(t0);
        w.tick(at(1), 100, true);
        assert!(w.tick(at(5000), 100, false).is_none());
        assert!(w.tick(at(5010), 100, true).is_none());
        assert!(w.tick(at(5100), 100, true).is_none());
        // a height of 0 (engine still loading) never counts
        assert!(w.tick(at(9000), 0, true).is_none());
        // after a restart because of a silence the next threshold is longer
        let mut w = HeightWatch::new(t0);
        w.tick(at(0), 5, true);
        let base = w.threshold();
        w.restarted(at(1));
        assert_eq!(w.threshold(), base * 2);
        // a new height brings the base threshold back
        w.tick(at(2), 5, true);
        w.tick(at(3), 6, true);
        assert_eq!(w.threshold(), base);
    }

    #[test]
    fn height_watch_threshold_follows_a_slow_chain() {
        let t0 = Instant::now();
        let mut w = HeightWatch::new(t0);
        w.tick(t0, 10, true);
        // blocks every 200 s: 12 x 200 s = 40 min, capped at 30
        for i in 1..=6u64 {
            w.tick(t0 + Duration::from_secs(i * 200), 10 + i, true);
        }
        assert_eq!(w.threshold(), STALL_MAX);
        // fast blocks (10 s): the floor, 8 min
        let mut w = HeightWatch::new(t0);
        w.tick(t0, 10, true);
        for i in 1..=6u64 {
            w.tick(t0 + Duration::from_secs(i * 10), 10 + i, true);
        }
        assert_eq!(w.threshold(), STALL_MIN);
    }
}
