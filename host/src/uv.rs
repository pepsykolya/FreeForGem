// FreeForGemPear — see LICENSE
//! Opt-in undervolt auto-tuner (`--auto-uv`).  Raises the GPU V/F offset in 15 MHz steps (NVML), checks every step on the GPU
//! against a bit-exact reference and on the CPU, keeps the last good step minus one as the result and restores the owner's
//! offset on exit.  Off by default.  Design and numbers: MANUAL.md, section "Undervolt / --auto-uv".
//!
//! Safety rules implemented here:
//!  * a mining CUDA device is mapped to its NVML handle by PCI bus id (and cross-checked by UUID): NVML indices differ from
//!    CUDA indices, and no other GPU of the machine is ever touched;
//!  * the owner's offset is written to a state file (fsync) before the first change and restored on exit / signal / next start;
//!  * every set is read back; a mismatch aborts tuning of that GPU and restores it;
//!  * every step is checked in a separate child process (`--uv-check`), so a CUDA fault or hang cannot hurt the miner.
use crate::mining::*;
use crate::nvml::{Dev, Nvml, OffMode};
use crate::stratum::Stats;
use crate::worker::GpuStat;
use crate::{gpu, hit_hash, prove_hit, test_header};
use anyhow::{Context, Result, ensure};
use primitive_types::U256;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const STEP: i32 = 15;
const NEWGEN_MIN: i32 = 200; // default search start on RTX 40 (compute capability 8.9)
const BLACKWELL_MIN: i32 = 300; // default search start on RTX 50 (sm_120): they hold +390..480, starting at +300 saves ~7 steps
pub const HARD_CAP: i32 = 600; // never above this, whatever --uv-max / --uv-offset say (the driver range c.hi caps it too)
pub const DEFAULT_MAX: i32 = 495; // default --uv-max (RTX 30 / 40 / 50 and everything not listed in card_defaults)
const TURING_MAX: i32 = 225; // default --uv-max on Turing (RTX 20 / GTX 16, sm_75): +200..260 measured, 225 keeps a margin
const CMP_MAX: i32 = 150; // default --uv-max on CMP 30HX / 40HX (TU116 / TU106): a CMP 40HX hit Xid at +255, rigs run +150
const CMP_50HX_PCI_ID: u16 = 0x1e09; // TU102 of the CMP 50HX: holds +220 and more (owner's rigs), so it gets the Turing limit
pub(crate) const DISABLE_SECS: u64 = 24 * 3600;
const OC_RESETS_MAX: usize = 4; // OC applies per hour that get the verified offset back; more: auto-uv stands down on that GPU
// a check process peaks at ~2.3 GB of RAM (measured on an 8x RTX 3070 HiveOS rig); checks of several GPUs only start
// together while the free RAM covers them, otherwise the kernel's OOM killer ends them (12 GB rig, 8 GPUs, 1.5.4-rc2)
const CHECK_MB: u64 = 2400;
const CHECK_RESERVE_MB: u64 = 600;

/// A per-GPU option (`--uv-max`, `--uv-min`, `--uv-offset`): not given, one value for every GPU, or a comma list with one
/// value per mining GPU, in `--devices` order (without `--devices`: GPU0, GPU1, ... as the miner numbers them; on HiveOS
/// that is the PCI bus order of the dashboard).  In a list `-` (or an empty field) means "the default for this GPU".
/// A list must have exactly one value per mining GPU (like --lock-cclock): a shorter list is an error, never extended,
/// because extending the last value could give a weak card the limit of a strong one.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum PerGpu {
    #[default]
    Unset,
    All(i32),
    List(Vec<Option<i32>>),
}

impl PerGpu {
    pub fn parse(raw: &str, flag: &str) -> Result<PerGpu> {
        let num = |s: &str| -> Result<i32> {
            let t = s.strip_prefix('+').unwrap_or(s);
            t.parse::<i32>().ok().filter(|_| !t.starts_with('+')).ok_or_else(|| anyhow::anyhow!("{flag}: '{s}' is not a whole number of MHz"))
        };
        let parts: Vec<&str> = raw.split(',').map(str::trim).collect();
        if parts.len() == 1 {
            return match parts[0] {
                "" => anyhow::bail!("{flag} needs a value (one number, or a comma list with one value per GPU)"),
                "-" | "auto" => Ok(PerGpu::Unset),
                s => Ok(PerGpu::All(num(s)?)),
            };
        }
        let v = parts.iter().map(|&s| if s.is_empty() || s == "-" || s == "auto" { Ok(None) } else { num(s).map(Some) }).collect::<Result<Vec<_>>>()?;
        Ok(PerGpu::List(v))
    }

    /// Value for the GPU at position `i` of the mining device list.
    pub fn get(&self, i: usize) -> Option<i32> {
        match self {
            PerGpu::Unset => None,
            PerGpu::All(v) => Some(*v),
            PerGpu::List(l) => l.get(i).copied().flatten(),
        }
    }

    pub fn is_set(&self) -> bool {
        match self {
            PerGpu::Unset => false,
            PerGpu::All(_) => true,
            PerGpu::List(l) => l.iter().any(Option::is_some),
        }
    }

    /// every explicit value
    pub fn values(&self) -> Vec<i32> {
        match self {
            PerGpu::Unset => vec![],
            PerGpu::All(v) => vec![*v],
            PerGpu::List(l) => l.iter().flatten().copied().collect(),
        }
    }

    /// A list must name every mining GPU (n = number of mining GPUs).
    pub fn check_count(&self, n: usize, flag: &str) -> Result<()> {
        if let PerGpu::List(l) = self {
            ensure!(
                l.len() == n,
                "{flag}: {} values for {n} mining GPU(s); give one value for all, or exactly one per GPU in --devices order ('-' = the default for that GPU)",
                l.len()
            );
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct Opts {
    pub auto: bool,
    pub fixed: PerGpu, // --uv-offset
    pub dry_run: bool,
    pub reset: bool,
    pub max_offset: PerGpu,   // --uv-max; a GPU without a value gets its card default (card_defaults)
    pub min_offset: PerGpu,   // --uv-min: start/floor of the search (no value: see Card::start)
    pub step: i32,            // --uv-step: MHz per step (default 15)
    pub keep_last_good: bool, // --uv-keep-last: after a failed step keep the last good step (not last good - one step)
    pub dir: Option<PathBuf>,
    pub reval_hours: f64,
    pub warmup_secs: u32,
    pub step_secs: u32,
    pub confirm_secs: u32,
    pub locks: Vec<Option<u32>>, // --lock-cclock of this run per mining GPU (filled by main; the profile key of a result)
    pub mlocks: Vec<Option<u32>>, // --lock-mclock of this run per mining GPU
    pub reset_all: bool,          // --uv-reset-all: forget every profile (--uv-reset: only the profile of the clocks now)
}

impl Default for Opts {
    fn default() -> Opts {
        Opts { auto: false, fixed: PerGpu::Unset, dry_run: false, reset: false, max_offset: PerGpu::Unset, min_offset: PerGpu::Unset, step: STEP, keep_last_good: false, dir: None, reval_hours: 6.0, warmup_secs: 180, step_secs: 30, confirm_secs: 120, locks: Vec::new(), mlocks: Vec::new(), reset_all: false }
    }
}

impl Opts {
    pub fn enabled(&self) -> bool {
        self.auto || self.fixed.is_set()
    }

    /// Checks of the values that need no GPU (at argument parsing).
    pub fn validate(&self) -> Result<()> {
        ensure!(!(self.auto && self.fixed.is_set()), "--auto-uv and --uv-offset exclude each other");
        ensure!(self.step >= 1, "--uv-step must be at least 1 MHz");
        for v in self.max_offset.values() {
            ensure!(v >= self.step, "--uv-max {v}: must be at least one step (--uv-step {})", self.step);
            ensure!(v <= HARD_CAP, "--uv-max {v}: must not exceed {HARD_CAP}");
        }
        for v in self.min_offset.values() {
            ensure!((0..=HARD_CAP).contains(&v), "--uv-min {v}: must be between 0 and {HARD_CAP}");
        }
        for v in self.fixed.values() {
            ensure!((-1000..=HARD_CAP).contains(&v), "--uv-offset {v}: must be between -1000 and +{HARD_CAP}");
        }
        // min <= max wherever both are given for the same GPU
        let len = |p: &PerGpu| if let PerGpu::List(l) = p { l.len() } else { 1 };
        let n = len(&self.max_offset).max(len(&self.min_offset));
        for i in 0..n {
            if let (Some(mn), Some(mx)) = (self.min_offset.get(i), self.max_offset.get(i)) {
                ensure!(mn <= mx, "--uv-min {mn} is above --uv-max {mx}{}", if n > 1 { format!(" (value {} of the list)", i + 1) } else { String::new() });
            }
        }
        Ok(())
    }

    /// Checks that need the number of mining GPUs.
    pub fn check_counts(&self, n: usize) -> Result<()> {
        self.max_offset.check_count(n, "--uv-max")?;
        self.min_offset.check_count(n, "--uv-min")?;
        self.fixed.check_count(n, "--uv-offset")
    }
}

/// Built-in limits of one card type, used where --uv-max / --uv-min give no value for that GPU.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Defaults {
    pub max: i32,
    pub max_src: &'static str,
    /// lowest search start (the search starts at max(current offset, this)); None: the current offset, else +120
    pub start_floor: Option<i32>,
    pub start_src: &'static str,
}

/// Card type from the NVML name, compute capability (`cc`) and PCI device id (`pci_id`); None when NVML does not report it.
///  * CMP 30HX / 40HX (Turing, sm_75): max +150 (a CMP 40HX hit Xid at +255; user rigs run +150)
///  * CMP 50HX (TU102, PCI id 0x1e09, or "50HX" in the name) and Turing RTX 20xx / GTX 16xx: max +225 (+200..260 measured
///    on a 2060 SUPER; CMP 50HX rigs hold +220 and more)
///  * RTX 50 (Blackwell, sm_120): search starts at +300 at least (they hold +390..480; a shorter search)
///  * RTX 40 (Ada, sm_89): search starts at +200 at least
///  * everything else (RTX 30, Ampere CMP 70HX / 90HX): max +495, start at the current offset or +120
pub fn card_defaults(name: &str, cc: Option<(i32, i32)>, pci_id: Option<u16>) -> Defaults {
    let n = name.to_ascii_uppercase();
    let words: Vec<&str> = n.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let cmp = words.contains(&"CMP");
    // the CMP 30HX/40HX/50HX are Turing; CMP 70HX/90HX are Ampere (sm_86), CMP 170HX is GA100 (sm_80)
    let ampere_cmp = ["70HX", "90HX", "170HX"].iter().any(|m| words.contains(m));
    let turing_name = n.contains("RTX 20") || n.contains("GTX 16") || n.contains("TITAN RTX") || n.contains("QUADRO RTX") || words.contains(&"T4");
    let turing = match cc {
        Some(c) => c == (7, 5),
        None => turing_name || (cmp && !ampere_cmp), // no compute capability: the name decides, a CMP counts as Turing
    };
    let cmp50 = pci_id == Some(CMP_50HX_PCI_ID) || (cmp && words.contains(&"50HX"));
    let base = Defaults { max: DEFAULT_MAX, max_src: "default", start_floor: None, start_src: "current offset or +120" };
    if turing && cmp50 {
        Defaults { max: TURING_MAX, max_src: "CMP 50HX default", ..base }
    } else if turing && cmp {
        Defaults { max: CMP_MAX, max_src: "CMP default", ..base }
    } else if turing {
        Defaults { max: TURING_MAX, max_src: "Turing default", ..base }
    } else if cc.is_some_and(|c| c >= (12, 0)) {
        Defaults { start_floor: Some(BLACKWELL_MIN), start_src: "RTX 50 default", ..base }
    } else if cc.is_some_and(|c| c >= (8, 9)) {
        Defaults { start_floor: Some(NEWGEN_MIN), start_src: "RTX 40 default", ..base }
    } else {
        base
    }
}

fn lg(msg: impl AsRef<str>) {
    println!("auto-uv: {}", msg.as_ref());
}

pub(crate) fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

pub(crate) fn signed(v: i32) -> String {
    format!("{v:+}")
}

/// "0000:0D:00.0" -> "0d"
pub(crate) fn short_bus(bus: &str) -> String {
    bus.split(':').nth(1).unwrap_or(bus).to_ascii_lowercase()
}

pub(crate) fn short_name(n: &str) -> String {
    n.replace("NVIDIA ", "").replace("GeForce ", "")
}

/// (domain, bus, device) of "dddd:bb:dd.f" (hex).
fn parse_bus(s: &str) -> Option<(u32, u32, u32)> {
    let mut it = s.split(|c| c == ':' || c == '.');
    let d = u32::from_str_radix(it.next()?, 16).ok()?;
    let b = u32::from_str_radix(it.next()?, 16).ok()?;
    let v = u32::from_str_radix(it.next()?, 16).ok()?;
    Some((d, b, v))
}

// ---------------------------------------------------------------------------------------------------------------------
// state files
// ---------------------------------------------------------------------------------------------------------------------

pub(crate) fn writable_dir(p: &Path) -> bool {
    if std::fs::create_dir_all(p).is_err() {
        return false;
    }
    let t = p.join(".ffgp-write-test");
    let ok = std::fs::write(&t, b"x").is_ok();
    let _ = std::fs::remove_file(&t);
    ok
}

/// Directory of the state files: --uv-dir, $FFGP_STATE_DIR, next to the binary (then a per-user directory).
pub fn state_dir(o: &Opts) -> PathBuf {
    if let Some(d) = &o.dir {
        return d.clone();
    }
    if let Ok(d) = std::env::var("FFGP_STATE_DIR") {
        if !d.trim().is_empty() {
            return PathBuf::from(d);
        }
    }
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)).unwrap_or_else(|| PathBuf::from("."));
    if writable_dir(&exe_dir) {
        return exe_dir;
    }
    let home = std::env::var_os("LOCALAPPDATA").or_else(|| std::env::var_os("HOME")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    home.join("FreeForGem")
}

pub(crate) fn write_atomic(path: &Path, data: &str) -> Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = path.with_extension("tmp");
    {
        let mut f = std::fs::File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
        f.write_all(data.as_bytes())?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path).with_context(|| format!("rename to {}", path.display()))?;
    #[cfg(unix)]
    if let Some(d) = path.parent() {
        if let Ok(f) = std::fs::File::open(d) {
            let _ = f.sync_all();
        }
    }
    Ok(())
}

#[derive(Clone, Default, Debug)]
struct Entry {
    name: String,
    bus: String,
    driver: String,
    kernel: String,
    orig: i32,
    offset: i32,
    clk: u32,
    w_before: f64,
    w_after: f64,
    tested_at: u64,
    verified_at: u64,
    disabled_until: u64,
    refs: Vec<String>,
    m: usize,   // rows of A per attempt the transcript digests were taken at
    cap: i32,   // the upper limit (--uv-max or the card default) of the tuning that found `offset`; 0 = unknown (before 1.6.2)
    // 1.7.2: the clocks the result was found at (the edge moves with the core clock: +495 verified at 1410 MHz crashed at 1805)
    lock: u32,          // core clock lock this miner set (--lock-cclock) when tuned; 0 = none set by the miner (or unknown)
    start: Option<i32>, // offset of the baseline whose load clock is `clk` (None: before 1.7.2)
    clk_ok: u32,        // load clock at the verified offset (confirmation); 0 = unknown
    edge: i32,          // first failing step (0 = none found / unknown)
    mlock: u32,         // memory clock lock this miner set (--lock-mclock); 0 = none / unknown
    unlocked: bool,     // the core clock floated during the tuning (no lock): the profile of the unlocked clock
    migrated: bool,     // a result from before 1.7.2 (no lock recorded, keyed by its clock under load): never used without a check
}

impl Entry {
    fn to_json(&self) -> Value {
        json!({"name": self.name, "bus": self.bus, "driver": self.driver, "kernel": self.kernel, "orig_offset": self.orig,
               "offset": self.offset, "clk_mhz": self.clk, "watts_before": self.w_before, "watts_after": self.w_after,
               "tested_at": self.tested_at, "verified_at": self.verified_at, "disabled_until": self.disabled_until, "refs": self.refs, "m": self.m, "cap": self.cap,
               "core_lock": self.lock, "start_offset": self.start, "clk_ok_mhz": self.clk_ok, "edge": self.edge,
               "mem_lock": self.mlock, "unlocked": self.unlocked, "migrated": self.migrated})
    }

    fn from_json(v: &Value) -> Option<Entry> {
        Some(Entry {
            name: v["name"].as_str().unwrap_or("").into(),
            bus: v["bus"].as_str().unwrap_or("").into(),
            driver: v["driver"].as_str()?.into(),
            kernel: v["kernel"].as_str()?.into(),
            orig: v["orig_offset"].as_i64()? as i32,
            offset: v["offset"].as_i64()? as i32,
            clk: v["clk_mhz"].as_u64().unwrap_or(0) as u32,
            w_before: v["watts_before"].as_f64().unwrap_or(0.0),
            w_after: v["watts_after"].as_f64().unwrap_or(0.0),
            tested_at: v["tested_at"].as_u64().unwrap_or(0),
            verified_at: v["verified_at"].as_u64().unwrap_or(0),
            disabled_until: v["disabled_until"].as_u64().unwrap_or(0),
            refs: v["refs"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default(),
            m: v["m"].as_u64().unwrap_or(0) as usize,
            cap: v["cap"].as_i64().unwrap_or(0) as i32,
            lock: v["core_lock"].as_u64().unwrap_or(0) as u32,
            start: v["start_offset"].as_i64().map(|x| x as i32),
            clk_ok: v["clk_ok_mhz"].as_u64().unwrap_or(0) as u32,
            edge: v["edge"].as_i64().unwrap_or(0) as i32,
            mlock: v["mem_lock"].as_u64().unwrap_or(0) as u32,
            unlocked: v["unlocked"].as_bool().unwrap_or(false),
            migrated: v["migrated"].as_bool().unwrap_or(false),
        })
    }
}

pub(crate) fn results_path(dir: &Path) -> PathBuf {
    dir.join("uv.json")
}

fn restore_path(dir: &Path) -> PathBuf {
    dir.join("uv-restore.json")
}

fn load_db(dir: &Path) -> Value {
    std::fs::read_to_string(results_path(dir)).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()).filter(Value::is_object).unwrap_or_else(|| json!({"version": 1, "gpus": {}}))
}

/// `--uv-reset-all`: forget every saved result of every coin, lock and mode (the restore file is not touched).  `--uv-reset` only
/// forgets the profile of the clocks / mode this run has (each tuner at its start).
pub fn reset(o: &Opts) {
    let p = results_path(&state_dir(o));
    match std::fs::remove_file(&p) {
        Ok(()) => lg(format!("saved results deleted ({})", p.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => lg(format!("no saved results ({})", p.display())),
        Err(e) => lg(format!("cannot delete {}: {e}", p.display())),
    }
}

fn pid_alive(pid: u32) -> bool {
    if pid == std::process::id() {
        return false;
    }
    #[cfg(target_os = "linux")]
    {
        let exe = std::fs::read_link(format!("/proc/{pid}/exe")).ok();
        return exe.is_some_and(|p| p.to_string_lossy().contains("freeforgem"));
    }
    #[allow(unreachable_code)]
    false
}

/// Crash recovery: a restore file left by a previous run that did not exit cleanly puts the owner's offsets back.
pub fn recover(o: &Opts) {
    let dir = state_dir(o);
    let rp = restore_path(&dir);
    let Ok(txt) = std::fs::read_to_string(&rp) else { return };
    let Ok(v) = serde_json::from_str::<Value>(&txt) else {
        lg(format!("unreadable restore file {} (left in place)", rp.display()));
        return;
    };
    if let Some(pid) = v["pid"].as_u64() {
        if pid_alive(pid as u32) {
            lg(format!("restore file belongs to a running miner (pid {pid}); not touching it"));
            return;
        }
    }
    let who = v["pid"].as_u64().map_or(String::new(), |p| format!(", pid {p}"));
    lg(format!("a previous run{who} did not exit cleanly ({}): putting back the offsets it left changed", rp.display()));
    let nv = match Nvml::load() {
        Ok(n) => n,
        Err(e) => {
            lg(format!("NVML not available ({e:#}); the restore file stays, offsets unchanged"));
            return;
        }
    };
    let mut all_ok = true;
    for g in v["gpus"].as_array().into_iter().flatten() {
        let (bus, uuid, orig) = (g["bus"].as_str().unwrap_or(""), g["uuid"].as_str().unwrap_or(""), g["orig_offset"].as_i64().unwrap_or(0) as i32);
        let name = g["name"].as_str().map_or(String::new(), |n| format!(", {}", short_name(n)));
        let res: Result<String> = (|| {
            let d = nv.by_bus(bus)?;
            ensure!(nv.uuid(d)? == uuid, "UUID at bus {bus} differs from the saved one");
            let (mode, cur, lo, hi) = nv.probe_offset(d)?;
            let orig = hive_offset(&nv, d).filter(|v| (lo..=hi).contains(v)).unwrap_or(orig);
            if cur == orig {
                return Ok(format!("already at the owner's offset {}", signed(orig)));
            }
            nv.set_offset(d, mode, orig)?;
            std::thread::sleep(Duration::from_millis(150));
            let rb = nv.get_offset(d, mode)?;
            ensure!(rb == orig, "read-back {rb} != {orig}");
            // worded unlike the shutdown line ("GPUn ... restored"): this is the previous run's card, put back at this start
            Ok(format!("offset {} -> {} put back (left changed by the previous run)", signed(cur), signed(orig)))
        })();
        match res {
            Ok(m) => lg(format!("(bus {}{name}) {m}", short_bus(bus))),
            Err(e) => {
                all_ok = false;
                lg(format!("(bus {}{name}) restore FAILED: {e:#}", short_bus(bus)));
            }
        }
    }
    if all_ok {
        let _ = std::fs::remove_file(&rp);
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// the managed cards
// ---------------------------------------------------------------------------------------------------------------------

pub struct Card {
    pub dev: i32,
    name: String,
    bus: String,
    uuid: String,
    nv: Dev,
    mode: OffMode,
    orig: i32,
    lo: i32,
    hi: i32,
    max: i32,              // upper limit of the search on this card: --uv-max for it, else its card default; <= HARD_CAP, <= hi
    max_wanted: i32,       // `max` before the driver range cut it
    max_src: &'static str, // where `max` comes from ("--uv-max", "CMP default", ...)
    max_default: Defaults, // the card type's built-in limits (for the warning when --uv-max is above them)
    min: Option<i32>,      // --uv-min for this card
    fixed: Option<i32>,    // --uv-offset for this card
    m: usize, // rows of A this card mines (its SM group); the undervolt check must stress it at this m
    cur: AtomicI32,
    owned: AtomicBool, // we still own the setting (false after tuning gave up or somebody else changed it)
    bad_seen: Mutex<u64>,
    reval_busy: AtomicBool,
    next_reval: Mutex<Instant>,
    next_readback: Mutex<Instant>,
    last_good: AtomicI32, // highest step that passed in the tuning now running (i32::MIN = none); goes into the crash marker
    oc_resets: Mutex<Vec<Instant>>, // OC applies (HiveOS nvidia-oc ...) that reset the offset while mining, for the rate limit
    fault: AtomicBool, // a check ended in a driver fault (Xid / hang): its crash marker must survive the shutdown
    lock: Option<u32>,  // core clock lock this run sets on the card (--lock-cclock); None = not set by the miner (lock unknown)
    mlock: Option<u32>, // memory clock lock this run sets (--lock-mclock)
    probe: Mutex<Option<ProbeSt>>,     // the clock under mining load picks the profile (lock unknown, 1.7.2)
    run_clk: Mutex<RunClk>,            // load clock while mining at a verified offset, to see a lock change mid-run
    profile: Mutex<Option<String>>,    // key of the profile this run uses (uv.json "profiles")
    plan: Mutex<Option<Plan>>,         // what runs once mining has started (attach)
    gs: Mutex<Option<Arc<GpuStat>>>,   // the mining thread of this GPU (pause handshake for its checks)
    crashed_at: Mutex<Option<i32>>,    // it crashed while mining at this offset (faults.rs): decide() steps back from it
}

/// The clock probe of a card whose lock the miner does not know.
struct ProbeSt {
    seen: Option<(Instant, u64)>, // first moment mining was seen, attempts at the last look
    samples: Vec<u32>,
    reset: bool, // --uv-reset: forget the profile of these clocks first
    oc: bool,    // after an OC apply reset the offset: a quick check before the offset goes back
}

impl ProbeSt {
    fn new(reset: bool, oc: bool) -> ProbeSt {
        ProbeSt { seen: None, samples: Vec::new(), reset, oc }
    }
}

/// What a card does at start (run_card / decide), carried out once mining runs (attach / launch).
enum Plan {
    None,
    Apply { e: Entry, want: i32 },
    Check { e: Entry, want: i32 },
    Tune { prev: Option<Entry>, crashed: Option<Crash>, resume: Option<i32> },
    Fixed(i32),
}

/// The mining thread of a GPU paused for one of its checks; resumes on drop.
struct Hold(Option<Arc<GpuStat>>);

impl Drop for Hold {
    fn drop(&mut self) {
        if let Some(g) = &self.0 {
            g.pause.store(false, Ordering::SeqCst);
        }
    }
}

#[derive(Default)]
struct RunClk {
    base: Option<u32>,
    samples: Vec<u32>,
    att: u64,
}

impl Card {
    fn label(&self) -> String {
        format!("GPU{} (bus {}, {})", self.dev, short_bus(&self.bus), short_name(&self.name))
    }

    /// search base/floor: --uv-min for this card; without it the offset HiveOS/NVML already has, else 120;
    /// on RTX 40 at least +200, on RTX 50 at least +300 (they hold +300..480, starting low only costs tuning time).
    /// Returns the offset and where it comes from (for the log).
    fn start(&self) -> (i32, &'static str) {
        if let Some(m) = self.min {
            (m, "--uv-min")
        } else if let Some(f) = self.max_default.start_floor.filter(|&f| f < self.max) {
            // (a default floor at or above a lower --uv-max is dropped: the search then starts as on other cards)
            (self.orig.max(f), self.max_default.start_src)
        } else if self.orig != 0 {
            (self.orig, "current offset")
        } else {
            (120, "default")
        }
    }

    /// One log line with the offset range (or the fixed offset) this card gets, and a warning when an explicit value is
    /// above the card type's safe default.
    fn range_lines(&self, auto: bool) -> Vec<String> {
        let mut out = Vec::new();
        let d = self.max_default;
        if let Some(f) = self.fixed {
            out.push(format!("{}: fixed offset {} (--uv-offset)", self.label(), signed(f.min(HARD_CAP))));
            if d.max < DEFAULT_MAX && f > d.max {
                out.push(format!("{}: WARNING --uv-offset {} is above the recommended +{} for this card ({}); applied as given", self.label(), signed(f), d.max, d.max_src));
            }
            return out;
        }
        if !auto {
            out.push(format!("{}: no --uv-offset for this GPU, left alone", self.label()));
            return out;
        }
        let (start, ssrc) = self.start();
        let hi_note = if self.max < self.max_wanted { format!(", driver limit +{}", self.hi) } else { String::new() };
        out.push(format!("{}: range {}..{} (max: {}{hi_note}; start: {ssrc})", self.label(), signed(start.clamp(0, self.max)), signed(self.max), self.max_src));
        if self.max_src == "--uv-max" && d.max < DEFAULT_MAX && self.max_wanted > d.max {
            out.push(format!("{}: WARNING --uv-max {} is above the recommended +{} for this card ({}); used as given", self.label(), signed(self.max_wanted), d.max, d.max_src));
        }
        if start > self.max {
            out.push(format!("{}: the start {} ({ssrc}) is above the limit {}: this GPU is not tuned", self.label(), signed(start), signed(self.max)));
        }
        out
    }

}

pub struct Manager {
    nv: Nvml,
    opts: Opts,
    dir: PathBuf,
    cards: Vec<Arc<Card>>,
    driver: String,
    kernel: String,
    db: Mutex<Value>,
    stop: Arc<AtomicBool>,
    rej_hist: Mutex<VecDeque<(Instant, u64, u64, u64)>>,
    rej_cooldown: Mutex<Instant>,
    last_tick: Mutex<Instant>,
    restored: AtomicBool,
    started: Instant,
    /// test hook: FFGP_UV_TEST_INJECT_BAD=<secs> reports one CPU-verification failure after that many seconds of mining
    inject_bad: Option<u64>,
    injected: AtomicBool,
}

/// RAM the OS can still hand out, MB (Linux: MemAvailable; Windows: available physical memory)
pub(crate) fn mem_available_mb() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let t = std::fs::read_to_string("/proc/meminfo").ok()?;
        let l = t.lines().find(|l| l.starts_with("MemAvailable:"))?;
        return l.split_whitespace().nth(1)?.parse::<u64>().ok().map(|kb| kb / 1024);
    }
    #[cfg(windows)]
    {
        #[repr(C)]
        struct MemStatus { len: u32, load: u32, total_phys: u64, avail_phys: u64, total_pf: u64, avail_pf: u64, total_virt: u64, avail_virt: u64, avail_ext: u64 }
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GlobalMemoryStatusEx(b: *mut MemStatus) -> i32;
        }
        let mut m = MemStatus { len: std::mem::size_of::<MemStatus>() as u32, load: 0, total_phys: 0, avail_phys: 0, total_pf: 0, avail_pf: 0, total_virt: 0, avail_virt: 0, avail_ext: 0 };
        return if unsafe { GlobalMemoryStatusEx(&mut m) } != 0 { Some(m.avail_phys / (1024 * 1024)) } else { None };
    }
    #[allow(unreachable_code)]
    None
}

fn check_mb() -> u64 {
    std::env::var("FFGP_UV_CHECK_MB").ok().and_then(|v| v.parse().ok()).unwrap_or(CHECK_MB)
}

/// running check processes; a new one starts only when the free RAM covers it on top of the ones already running
fn check_slots() -> &'static (Mutex<u64>, Condvar) {
    static S: OnceLock<(Mutex<u64>, Condvar)> = OnceLock::new();
    S.get_or_init(|| (Mutex::new(0), Condvar::new()))
}

struct CheckSlot;

thread_local! {
    // this thread already holds a check slot (taken before mining on the GPU was paused): the check itself takes no second one
    static SLOT_HELD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

impl Drop for CheckSlot {
    fn drop(&mut self) {
        SLOT_HELD.with(|h| h.set(false));
        let (m, cv) = check_slots();
        *m.lock().unwrap() -= 1;
        cv.notify_all();
    }
}

/// May one more check process start? `running` checks are already running (each counted in full: they may not have
/// reached their peak yet), each needs `need` MB; `avail` = free RAM in MB (None = unknown: no limit).
fn check_fits(running: u64, need: u64, avail: Option<u64>) -> bool {
    running == 0 || avail.is_none_or(|a| a.saturating_sub(running * need) >= need + CHECK_RESERVE_MB)
}

static ACTIVE: OnceLock<Mutex<Option<Arc<Manager>>>> = OnceLock::new();

fn active() -> &'static Mutex<Option<Arc<Manager>>> {
    ACTIVE.get_or_init(|| Mutex::new(None))
}

/// Puts the owner's offsets back (idempotent).  Called on exit, on a second interrupt and from the guard's Drop.
pub fn restore_all() {
    let m = active().lock().unwrap().clone();
    if let Some(m) = m {
        m.restore();
        // a stop (Ctrl-C, SIGTERM, HiveOS miner stop / restart: Ctrl-C, then SIGHUP = "second interrupt" while a check still
        // runs) is not a crash: without this the check's crash marker stayed, and the next start took the offset under check
        // for a crash point ("not tuning, keeping +0", saved for good)
        if m.stopped() {
            m.forget_trying();
        }
    }
}

/// Does the running auto-uv / --uv-offset change the offset of CUDA device `dev`?
pub fn manages(dev: i32) -> bool {
    active().lock().unwrap().as_ref().is_some_and(|m| m.cards.iter().any(|c| c.dev == dev))
}

enum FaultPlan {
    Go,           // nothing remembered that changes this start
    Done,         // handled (lowered offset set, or left out of mining)
    Crashed(i32), // this undervolt offset crashed while mining: tune below it
}

pub struct Guard;
impl Drop for Guard {
    fn drop(&mut self) {
        restore_all();
    }
}

/// What a previous run left in the crash marker: the offset it died at, and the highest step that had passed before.
#[derive(Clone, Copy)]
struct Crash {
    at: i32,
    good: Option<i32>,
}

struct Spec {
    secs: u32,
    min_reruns: u32,
    cpu: u32,
    measure: u32,
    refs: Vec<String>,
}

#[derive(Default, Debug)]
struct Res {
    ok: bool,
    driver_fault: bool, // Xid or a hung check: the GPU may be lost until a reboot, so the crash marker stays
    reason: String,
    reruns: u64,
    cpu_ok: u32,
    refs: Vec<String>,
    watts: Option<f64>,
    clk: Option<u32>,
    temp: Option<u32>,
}

/// Maps the mining CUDA devices to NVML handles (by PCI bus id), applies/tunes, and returns the manager.
/// `None` = nothing to supervise (disabled, NVML missing, no permission, dry run ...); mining just goes on.
pub fn start(o: &Opts, devices: &[i32], stop: &Arc<AtomicBool>) -> Option<(Arc<Manager>, Guard)> {
    if !o.enabled() {
        return None;
    }
    // main() validated the values and the list lengths; a wrong one here only disables undervolting
    if let Err(e) = o.validate().and_then(|_| o.check_counts(devices.len())) {
        lg(format!("{e:#}; undervolting disabled"));
        return None;
    }
    let dir = state_dir(o);
    if !writable_dir(&dir) {
        lg(format!("state directory {} is not writable: undervolting disabled (set --uv-dir or $FFGP_STATE_DIR)", dir.display()));
        return None;
    }
    let nv = match Nvml::load() {
        Ok(n) => n,
        Err(e) => {
            lg(format!("NVML not available ({e:#}); mining at the current clocks and voltage"));
            return None;
        }
    };
    let driver = nv.driver_version();
    let kernel = gpu::kernel_label_for(devices.first().copied().unwrap_or(0));
    let total = nv.device_count().unwrap_or(0);
    lg(format!("NVML loaded, driver {driver}, kernel {kernel}, {total} GPU(s) in the system; only the {} mining device(s) below are touched", devices.len()));
    let mut cards = Vec::new();
    for (pos, &dev) in devices.iter().enumerate() {
        match map_card(&nv, dev, pos, o) {
            Ok(c) => {
                let idx = nv.index(c.nv).map_or("?".to_string(), |i| i.to_string());
                lg(format!("{}: CUDA device {dev} -> NVML index {idx}, PCI {}, {}, current offset {} (allowed {}..{})", c.label(), c.bus, c.uuid, signed(c.orig), c.lo, c.hi));
                cards.push(Arc::new(c));
            }
            Err(e) => lg(format!("GPU{dev}: cannot manage ({e:#}); left alone")),
        }
    }
    for c in &cards {
        for l in c.range_lines(o.auto) {
            lg(l);
        }
    }
    // with a --uv-offset list, a GPU without a value is not touched at all
    cards.retain(|c| o.auto || c.fixed.is_some());
    if cards.is_empty() {
        return None;
    }
    if cards.is_empty() {
        lg("no GPU can be managed (V/F offset unavailable or no permission?); mining at the current clocks and voltage");
        return None;
    }
    if o.dry_run {
        for c in &cards {
            if let Some(f) = c.fixed {
                lg(format!("DRY RUN: would set {} (NVML handle of PCI {}) to the fixed offset {}; nothing is changed", c.label(), c.bus, signed(f.min(HARD_CAP))));
                continue;
            }
            let start = c.start().0.clamp(0, c.max);
            let first = start + o.step;
            lg(format!("DRY RUN: would change {} (NVML handle of PCI {}): start {}, {} -> {} ... up to {}; nothing is changed", c.label(), c.bus, signed(start), signed(start), signed(first), signed(c.max)));
        }
        return None;
    }
    // permission probe: write the owner's value (changes nothing, or puts the HiveOS profile's value back over a leftover);
    // NVML refuses it without root/administrator rights
    cards.retain(|c| match nv.set_offset(c.nv, c.mode, c.orig) {
        Ok(()) => {
            let cur = c.cur.swap(c.orig, Ordering::Relaxed);
            if cur != c.orig {
                lg(format!(
                    "{} offset {} found, the HiveOS OC profile has {}: set to {} (the {} was left by another program; {} is also what goes back on exit)",
                    c.label(),
                    signed(cur),
                    signed(c.orig),
                    signed(c.orig),
                    signed(cur),
                    signed(c.orig)
                ));
            }
            true
        }
        Err(e) => {
            lg(format!("{} cannot change the V/F offset ({e:#}); this needs root (Linux/HiveOS) or Administrator (Windows) rights. Left alone, mining normally", c.label()));
            false
        }
    });
    if cards.is_empty() {
        return None;
    }
    // owner's values to disk before the first change
    let doc = json!({"pid": std::process::id(), "started": now(), "gpus": cards.iter().map(|c| json!({"bus": c.bus, "uuid": c.uuid, "name": c.name, "orig_offset": c.orig})).collect::<Vec<_>>()});
    if let Err(e) = write_atomic(&restore_path(&dir), &serde_json::to_string_pretty(&doc).unwrap()) {
        lg(format!("cannot write the restore file ({e:#}): undervolting disabled"));
        return None;
    }
    for c in &cards {
        if !nv.watch_xid(c.nv) {
            // best effort only
        }
    }
    let m = Arc::new(Manager {
        nv,
        opts: o.clone(),
        dir: dir.clone(),
        cards,
        driver,
        kernel,
        db: Mutex::new({
            let mut db = load_db(&dir);
            let n = migrate(&mut db);
            if n > 0 {
                lg(format!("{n} saved result(s) from before 1.7.2 (no clock lock recorded) kept as profiles of the clock they were found at under load; each is used only after a check at these clocks"));
                let _ = write_atomic(&results_path(&dir), &serde_json::to_string_pretty(&db).unwrap());
            }
            db
        }),
        stop: stop.clone(),
        rej_hist: Mutex::new(VecDeque::new()),
        rej_cooldown: Mutex::new(Instant::now()),
        last_tick: Mutex::new(Instant::now()),
        restored: AtomicBool::new(false),
        started: Instant::now(),
        inject_bad: std::env::var("FFGP_UV_TEST_INJECT_BAD").ok().and_then(|v| v.parse().ok()),
        injected: AtomicBool::new(false),
    });
    *active().lock().unwrap() = Some(m.clone());
    let guard = Guard;
    // only decisions and NVML sets here: mining starts right after; checks and tunings run then (attach), one GPU at a time
    // pausing only itself
    std::thread::scope(|s| {
        for c in &m.cards {
            let (m, c) = (m.clone(), c.clone());
            s.spawn(move || match m.run_card(&c) {
                Ok(Plan::Apply { e, want }) => m.launch(&c, Plan::Apply { e, want }),
                Ok(Plan::None) => {}
                Ok(p) => *c.plan.lock().unwrap() = Some(p),
                Err(e) => {
                    lg(format!("{} error: {e:#}; restoring {}", c.label(), signed(c.orig)));
                    m.restore_card(&c);
                }
            });
        }
    });
    if m.stop.load(Ordering::Relaxed) {
        m.restore();
    }
    let hrs = m.opts.reval_hours;
    for c in &m.cards {
        *c.next_reval.lock().unwrap() = Instant::now() + reval_interval(hrs);
    }
    Some((m, guard))
}

/// `pos`: position of `dev` in the mining device list (the index into the per-GPU option lists).
fn map_card(nv: &Nvml, dev: i32, pos: usize, o: &Opts) -> Result<Card> {
    let bus = gpu::device_pci_bus_id(dev)?;
    let cuda_uuid = gpu::device_uuid(dev)?;
    let cuda_name = gpu::device_name(dev);
    let h = nv.by_bus(&bus)?;
    // the handle must be the very GPU CUDA device `dev` is: PCI address, UUID and name all have to agree
    let (cd, cb, cv) = parse_bus(&bus).context("cannot parse the CUDA PCI bus id")?;
    let (nd, nb, nvdev) = nv.pci(h)?;
    ensure!((cd, cb, cv) == (nd, nb, nvdev), "NVML handle is at {nd:04x}:{nb:02x}:{nvdev:02x}, CUDA device is at {bus}");
    let uuid = nv.uuid(h)?;
    ensure!(uuid.eq_ignore_ascii_case(&cuda_uuid), "UUID mismatch: NVML {uuid}, CUDA {cuda_uuid}");
    let name = nv.name(h).unwrap_or_else(|_| cuda_name.clone());
    ensure!(name == cuda_name, "name mismatch: NVML {name:?}, CUDA {cuda_name:?}");
    let (mode, cur, lo, hi) = nv.probe_offset(h)?;
    let (cc, pci_id) = (nv.compute_capability(h), nv.pci_device_id(h));
    let m = crate::mining::device_m(gpu::sm_count(dev));
    // on HiveOS the owner's offset is the OC profile's Core Clock value: what the card has now may be left by another program
    // (Rig01: +325 from an interrupted ffgs restore instead of the profile's +275)
    let orig = hive_offset(nv, h).filter(|v| (lo..=hi).contains(v)).unwrap_or(cur);
    Ok(new_card(CardHw { dev, name, bus, uuid, nv: h, mode, cur, orig, lo, hi, cc, pci_id, m }, pos, o))
}

/// Core Clock offset of the HiveOS OC profile for this GPU (None: not HiveOS).
fn hive_offset(nv: &Nvml, h: Dev) -> Option<i32> {
    crate::hiveoc::load().and_then(|p| nv.index(h).map(|i| p.core_offset(i as usize)))
}

/// What NVML/CUDA tell about one mining GPU.
struct CardHw {
    dev: i32,
    name: String,
    bus: String,
    uuid: String,
    nv: Dev,
    mode: OffMode,
    cur: i32,
    orig: i32, // the offset put back on exit: the HiveOS profile's Core Clock value on HiveOS, else `cur`
    lo: i32,
    hi: i32,
    cc: Option<(i32, i32)>,
    pci_id: Option<u16>,
    m: usize,
}

/// The card with its limits: --uv-max / --uv-min / --uv-offset at position `pos` of the lists, else the card type's defaults.
fn new_card(hw: CardHw, pos: usize, o: &Opts) -> Card {
    let def = card_defaults(&hw.name, hw.cc, hw.pci_id);
    let (max_wanted, max_src) = match o.max_offset.get(pos) {
        Some(v) => (v.min(HARD_CAP), "--uv-max"),
        None => (def.max, def.max_src),
    };
    Card {
        dev: hw.dev,
        name: hw.name,
        bus: hw.bus,
        uuid: hw.uuid,
        nv: hw.nv,
        mode: hw.mode,
        orig: hw.orig,
        lo: hw.lo,
        hi: hw.hi,
        max: max_wanted.min(hw.hi),
        max_wanted,
        max_src,
        max_default: def,
        min: o.min_offset.get(pos),
        fixed: o.fixed.get(pos),
        m: hw.m,
        cur: AtomicI32::new(hw.cur),
        owned: AtomicBool::new(true),
        bad_seen: Mutex::new(0),
        reval_busy: AtomicBool::new(false),
        next_reval: Mutex::new(Instant::now()),
        next_readback: Mutex::new(Instant::now() + Duration::from_secs(30)),
        last_good: AtomicI32::new(i32::MIN),
        oc_resets: Mutex::new(Vec::new()),
        fault: AtomicBool::new(false),
        lock: o.locks.get(pos).copied().flatten(),
        mlock: o.mlocks.get(pos).copied().flatten(),
        probe: Mutex::new(None),
        run_clk: Mutex::new(RunClk::default()),
        profile: Mutex::new(None),
        plan: Mutex::new(None),
        gs: Mutex::new(None),
        crashed_at: Mutex::new(None),
    }
}

impl Manager {
    fn set_checked(&self, c: &Card, v: i32) -> Result<()> {
        ensure!(v == c.orig || !self.stopped(), "stopping: no new offset");
        ensure!(v >= c.lo && v <= c.hi, "offset {v} outside the driver's range {}..{}", c.lo, c.hi);
        ensure!(self.nv_target_ok(c), "internal: handle check failed");
        self.nv.set_offset(c.nv, c.mode, v)?;
        std::thread::sleep(Duration::from_millis(150));
        let rb = self.nv.get_offset(c.nv, c.mode)?;
        ensure!(rb == v, "read-back {rb} differs from the value just set ({v})");
        c.cur.store(v, Ordering::Relaxed);
        *c.run_clk.lock().unwrap() = RunClk::default();
        Ok(())
    }

    /// The handle must still be the GPU at the bus id we mapped (cheap guard against any mix-up).
    fn nv_target_ok(&self, c: &Card) -> bool {
        match (self.nv.pci(c.nv), parse_bus(&c.bus)) {
            (Ok(p), Some(b)) => p == b,
            _ => false,
        }
    }

    /// Puts the owner's offset back on one card (up to 3 tries) without logging; returns the log lines.
    fn put_back(&self, c: &Card) -> Vec<String> {
        let mut out = Vec::new();
        if !c.owned.swap(false, Ordering::SeqCst) {
            return out;
        }
        let cur = c.cur.load(Ordering::Relaxed);
        for attempt in 0..3 {
            match self.set_checked(c, c.orig) {
                Ok(()) => {
                    out.push(format!("{} offset {} -> {} restored", c.label(), signed(cur), signed(c.orig)));
                    return out;
                }
                Err(e) => {
                    out.push(format!("{} restore attempt {} failed: {e:#}", c.label(), attempt + 1));
                    std::thread::sleep(Duration::from_millis(500));
                }
            }
        }
        out.push(format!("{} COULD NOT RESTORE the offset {}; the restore file stays and is applied at the next start", c.label(), signed(c.orig)));
        c.owned.store(true, Ordering::SeqCst);
        out
    }

    /// One card (an error while tuning/mining): put back, then log.
    fn restore_card(&self, c: &Card) {
        for l in self.put_back(c) {
            lg(l);
        }
    }

    /// All cards, at shutdown: every card is put back (in parallel) before a single line is logged, so a dead log pipe or
    /// a slow console cannot leave a GPU undervolted.
    pub fn restore(&self) {
        if self.restored.swap(true, Ordering::SeqCst) {
            return;
        }
        let lines: Vec<Vec<String>> = std::thread::scope(|s| {
            let hs: Vec<_> = self.cards.iter().map(|c| s.spawn(move || self.put_back(c))).collect();
            hs.into_iter().map(|h| h.join().unwrap_or_default()).collect()
        });
        if self.cards.iter().all(|c| !c.owned.load(Ordering::SeqCst)) {
            let _ = std::fs::remove_file(restore_path(&self.dir));
        } else {
            self.restored.store(false, Ordering::SeqCst);
        }
        for l in lines.into_iter().flatten() {
            lg(l);
        }
    }

    /// All saved profiles of this card (one per core clock lock / clock regime, 1.7.2).
    fn profiles(&self, c: &Card) -> Vec<Entry> {
        let db = self.db.lock().unwrap();
        let mut v: Vec<Entry> = db["profiles"][&c.uuid].as_object().map(|m| m.values().filter_map(Entry::from_json).collect()).unwrap_or_default();
        v.sort_by_key(|e| (e.lock, e.clk));
        v
    }

    /// The profile this run uses on the card (chosen at start or after the clock probe).
    fn entry(&self, c: &Card) -> Option<Entry> {
        let key = c.profile.lock().unwrap().clone()?;
        let db = self.db.lock().unwrap();
        db["profiles"][&c.uuid].get(&key).and_then(Entry::from_json)
    }

    /// Saves `e` as the profile of its clocks: a profile of the same clocks is replaced, the others stay untouched.
    fn save_entry(&self, c: &Card, e: &Entry) {
        let key = profile_key(e);
        let mut db = self.db.lock().unwrap();
        db["version"] = json!(2);
        if !db["profiles"].is_object() {
            db["profiles"] = json!({});
        }
        if !db["profiles"][&c.uuid].is_object() {
            db["profiles"][&c.uuid] = json!({});
        }
        let same: Vec<String> = db["profiles"][&c.uuid]
            .as_object()
            .unwrap()
            .iter()
            .filter(|(k, v)| **k != key && Entry::from_json(v).is_some_and(|o| same_clocks(&o, e)))
            .map(|(k, _)| k.clone())
            .collect();
        let m = db["profiles"][&c.uuid].as_object_mut().unwrap();
        for k in same {
            m.remove(&k);
        }
        m.insert(key.clone(), e.to_json());
        *c.profile.lock().unwrap() = Some(key);
        self.write_db(c, &db);
    }

    /// --uv-reset: forgets the profile(s) of the clocks this run has; the other profiles stay.
    fn forget_profiles(&self, c: &Card, fits: impl Fn(&Entry) -> bool) -> Vec<String> {
        let mut db = self.db.lock().unwrap();
        let Some(m) = db["profiles"][&c.uuid].as_object_mut() else { return Vec::new() };
        let keys: Vec<String> = m.iter().filter(|(_, v)| Entry::from_json(v).is_some_and(|e| fits(&e))).map(|(k, _)| k.clone()).collect();
        for k in &keys {
            m.remove(k);
        }
        if !keys.is_empty() {
            self.write_db(c, &db);
        }
        *c.profile.lock().unwrap() = None;
        keys
    }

    /// "1410:+450, 1805:+300 (paused)" for the start log.
    fn profiles_line(&self, c: &Card) -> String {
        let v = self.profiles(c);
        if v.is_empty() {
            return "none".into();
        }
        v.iter()
            .map(|e| {
                let mut s = format!("{}:{}", profile_key(e), signed(e.offset));
                if e.disabled_until > now() {
                    s.push_str(" (paused)");
                }
                if e.migrated {
                    s.push_str(" (from before 1.7.2: checked before use)");
                }
                s
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn write_db(&self, c: &Card, db: &Value) {
        if let Err(err) = write_atomic(&results_path(&self.dir), &serde_json::to_string_pretty(db).unwrap()) {
            lg(format!("{} cannot save {}: {err:#}", c.label(), results_path(&self.dir).display()));
        }
    }

    // Crash marker: the offset under check is written to uv.json before the check and removed after it. A miner or
    // driver crash during a check (seen on Windows: the whole miner exits with an access violation) leaves the marker
    // behind, so the next start counts that offset as failed instead of climbing into the same crash again.
    fn mark_trying(&self, c: &Card, v: i32) {
        let mut db = self.db.lock().unwrap();
        if !db["trying"].is_object() {
            db["trying"] = json!({});
        }
        let good = c.last_good.load(Ordering::Relaxed);
        db["trying"][&c.uuid] = if good == i32::MIN { json!({"offset": v}) } else { json!({"offset": v, "good": good}) };
        self.write_db(c, &db);
    }

    fn clear_trying(&self, c: &Card) {
        let mut db = self.db.lock().unwrap();
        let had = db["trying"].as_object_mut().is_some_and(|t| t.remove(&c.uuid).is_some());
        if had {
            self.write_db(c, &db);
        }
    }

    /// What a previous run was checking when it died, if anything (the marker is removed).
    /// 1.5.2 wrote a bare number; later versions write {"offset", "good"}.
    fn take_trying(&self, c: &Card) -> Option<Crash> {
        let v = self.db.lock().unwrap()["trying"].get(&c.uuid).cloned()?;
        self.clear_trying(c);
        let at = v.as_i64().or_else(|| v["offset"].as_i64())? as i32;
        Some(Crash { at, good: v["good"].as_i64().map(|g| g as i32) })
    }

    /// Kernel the mining worker of this card will run (t256 on Ada, gen13 otherwise): the undervolt results are keyed by it.
    fn kernel_of(&self, c: &Card) -> String {
        gpu::kernel_label_for(c.dev)
    }

    fn new_entry(&self, c: &Card) -> Entry {
        Entry { name: c.name.clone(), bus: c.bus.clone(), driver: self.driver.clone(), kernel: self.kernel_of(c), orig: c.orig, ..Default::default() }
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    // ----- per-card flow at start ---------------------------------------------------------------------------------

    /// What this card does at start.  Decided before mining starts, with no checks here (only NVML sets): the checks and the
    /// tunings run while the miner mines (`attach`), and each one pauses only its own GPU (1.7.2;
    /// before, every GPU waited for the slowest tuning, ~25 min on an 8-GPU rig).
    fn run_card(&self, c: &Arc<Card>) -> Result<Plan> {
        let mut crashed = self.take_trying(c);
        if let Some(k) = crashed {
            lg(format!("{} the previous run died while checking offset {} (miner or driver crash): that offset counts as failed", c.label(), signed(k.at)));
        }
        // crashes while mining, remembered by faults.rs (1.7.2): a crashed undervolt offset counts as failed; a crash at the
        // owner's / kept offset lowers it after 2 crash starts in a row; nothing left to lower: the GPU is left out of mining
        match self.fault_plan(c)? {
            FaultPlan::Go => {}
            FaultPlan::Done => return Ok(Plan::None),
            FaultPlan::Crashed(at) => {
                if let Some(f) = c.fixed {
                    if f.min(HARD_CAP) == at {
                        crashed = Some(Crash { at, good: None });
                    }
                } else if crashed.is_none() {
                    // with a profile of these clocks: back to it / one step below the crash (decide), no tuning from scratch;
                    // without one: tuned below the crash
                    *c.crashed_at.lock().unwrap() = Some(at);
                }
            }
        }
        // died during a quick check of a saved profile (not during a tuning: that has steps that passed): the same as a crash
        // while mining at that offset, a step back from the profile instead of a tuning from scratch
        if let Some(k) = crashed.filter(|k| k.good.is_none() && c.fixed.is_none() && !self.profiles(c).is_empty()) {
            *c.crashed_at.lock().unwrap() = Some(k.at);
            crashed = None;
        }
        if let Some(f) = c.fixed {
            if crashed.is_some_and(|k| k.at == f.min(HARD_CAP)) {
                lg(format!("{} not applying the fixed offset {} this time; keeping {} (lower --uv-offset)", c.label(), signed(f), signed(c.orig)));
                return Ok(Plan::None);
            }
            return Ok(Plan::Fixed(f));
        }
        let regime = match c.lock {
            Some(l) => format!("the core clock lock {l} MHz"),
            None => "these clocks".to_string(),
        };
        if c.lock.is_none() && crashed.is_none() && !self.profiles(c).is_empty() {
            // (a mining crash stays in crashed_at: the profile the clock picks steps back from it)
            // the miner cannot read a lock it did not set (HiveOS / another miner may have changed it): the clock under mining
            // load picks the profile (probe_tick)
            lg(format!(
                "{} profiles: {}; the core clock lock is not set by the miner (no --lock-cclock): mining starts at {}, the clock under mining load picks the profile",
                c.label(),
                self.profiles_line(c),
                signed(c.cur.load(Ordering::Relaxed))
            ));
            *c.probe.lock().unwrap() = Some(ProbeSt::new(self.opts.reset, false));
            return Ok(Plan::None);
        }
        if self.opts.reset {
            let gone = self.forget_profiles(c, |e| profile_fits(e, c.lock, c.mlock, None));
            lg(format!(
                "{} --uv-reset: {} for {regime} forgotten; the profiles of other clocks are kept ({}); --uv-reset-all forgets them all",
                c.label(),
                if gone.is_empty() { "no profile".to_string() } else { format!("profile {}", gone.join(", ")) },
                self.profiles_line(c)
            ));
        }
        let pick = best_fit(&self.profiles(c), c.lock, c.mlock, None);
        if pick.is_none() {
            if let Some(at) = c.crashed_at.lock().unwrap().take() {
                crashed = Some(Crash { at, good: None }); // no profile of these clocks: tuned below the crash
            }
        }
        lg(format!(
            "{} profiles: {}; {}",
            c.label(),
            self.profiles_line(c),
            pick.as_ref().map_or(format!("none for {regime}"), |e| format!("using {} for {regime}", profile_key(e)))
        ));
        if crashed.is_some() {
            if let Some(e) = &pick {
                *c.profile.lock().unwrap() = Some(profile_key(e));
            }
            return Ok(Plan::Tune { prev: pick, crashed, resume: None });
        }
        Ok(self.decide(c, pick, false))
    }

    /// What to do with the profile chosen for the clocks now (`oc`: after an OC apply reset the offset: always a quick check).
    fn decide(&self, c: &Card, pick: Option<Entry>, oc: bool) -> Plan {
        let Some(e) = pick else {
            lg(format!("{} no saved result for these clocks: tuning (the other GPUs mine meanwhile)", c.label()));
            return Plan::Tune { prev: None, crashed: None, resume: None };
        };
        *c.profile.lock().unwrap() = Some(profile_key(&e));
        let key = profile_key(&e);
        if let Some(at) = c.crashed_at.lock().unwrap().take() {
            // it crashed while mining at `at` (faults.rs): back to the verified offset of this lock when that is lower, else one
            // step below the crash; a quick check either way, no tuning from scratch
            let mut e = e;
            if e.offset < at {
                lg(format!("{} crashed while mining at {}: back to the verified {} of profile {key} after a quick check (no tuning from scratch)", c.label(), signed(at), signed(e.offset)));
                return Plan::Check { want: e.offset, e };
            }
            let want = at - self.opts.step;
            lg(format!("{} crashed while mining at {} (the offset of profile {key}): one step below it, {}, after a quick check (no tuning from scratch)", c.label(), signed(at), signed(want)));
            e.offset = want;
            e.edge = at;
            e.verified_at = 0;
            self.save_entry(c, &e);
            return Plan::Check { want, e };
        }
        if e.driver != self.driver || e.kernel != self.kernel_of(c) {
            lg(format!("{} profile {key} is for driver {} / kernel {}, now driver {} / kernel {}: tuning again", c.label(), e.driver, e.kernel, self.driver, self.kernel_of(c)));
            return Plan::Tune { prev: Some(e), crashed: None, resume: None };
        }
        if e.m != 0 && e.m != c.m {
            // the saved reference digests were taken at another m (rows per attempt, changed by the miner version): re-check
            // the saved offset without them (reruns + CPU check) and store new ones; a pause set against such references is ignored
            lg(format!("{} profile {key} was checked at m {}, now m {}: re-checking its offset {} with new references", c.label(), e.m, c.m, signed(e.offset)));
            return if e.offset != c.orig { Plan::Check { want: e.offset, e } } else { Plan::None };
        }
        match start_choice(&e, c.orig, now()) {
            Choice::Lower => {
                // the result verified at these clocks is below the owner's / starting offset: the tuning proved the edge below it
                // (Rig01, 1.7.1: results +270..+315 at 1805 MHz, the start offset +325 an ffgs leftover; "keeping" the +325 without a
                // check crashed GPU3 with Xid 13/43 and stopped all 8 GPUs).  The verified offset is used, never the unchecked start.
                lg(format!(
                    "{} the offset {} verified at these clocks (profile {key}{}) is below your offset {}: mining at {} (yours is put back on exit)",
                    c.label(),
                    signed(e.offset),
                    if e.edge != 0 { format!(", {} failed", signed(e.edge)) } else { String::new() },
                    signed(c.orig),
                    signed(e.offset)
                ));
                return if e.migrated { Plan::Check { want: e.offset, e } } else { Plan::Apply { want: e.offset, e } };
            }
            Choice::Paused => {
                lg(format!("{} auto-uv is paused for this GPU at these clocks after a failure ({} h left); keeping {}", c.label(), (e.disabled_until - now()) / 3600 + 1, signed(c.orig)));
                return Plan::None;
            }
            Choice::Keep => {
                // offset == the owner's: it passed the baseline check of that tuning (the start of the search)
                lg(format!("{} no safe offset above {} was found at these clocks (it passed as the start of the tuning); keeping it (--uv-reset to tune again)", c.label(), signed(c.orig)));
                return Plan::None;
            }
            Choice::Higher => {}
        }
        match saved_vs_limit(e.offset, e.cap, c.max) {
            Saved::Retune => {
                lg(format!("{} the limit was raised ({} when tuned, {} now) and the saved {} is the old limit: tuning on from it", c.label(), signed(e.cap), signed(c.max), signed(e.offset)));
                Plan::Tune { resume: Some(e.offset), prev: Some(e), crashed: None }
            }
            Saved::Clamp if c.max <= c.orig => {
                lg(format!("{} saved offset {} is above the limit {} ({}), which is not above your offset {}: keeping {}", c.label(), signed(e.offset), signed(c.max), c.max_src, signed(c.orig), signed(c.orig)));
                Plan::None
            }
            Saved::Clamp => {
                lg(format!("{} saved offset {} is above the limit {} ({}): the limit gets a quick check", c.label(), signed(e.offset), signed(c.max), c.max_src));
                Plan::Check { want: c.max, e }
            }
            Saved::Apply => {
                // verified recently at these clocks: applied right away (a restart costs no mining); the periodic re-validation and
                // the rejected-share guard still cover it.  A result from before 1.7.2 (no lock recorded) always gets the check.
                let fresh = !oc && !e.migrated && e.orig == c.orig && self.opts.reval_hours > 0.0 && now().saturating_sub(e.verified_at) < (self.opts.reval_hours * 3600.0) as u64;
                if fresh { Plan::Apply { want: e.offset, e } } else { Plan::Check { want: e.offset, e } }
            }
        }
    }

    /// Runs a plan: Apply at once (an NVML set); the others in a thread of their own while the miner mines.
    fn launch(self: &Arc<Self>, c: &Arc<Card>, p: Plan) {
        match p {
            Plan::None => {}
            Plan::Apply { e, want } => {
                if let Err(err) = self.apply_now(c, &e, want) {
                    lg(format!("{} error: {err:#}; restoring {}", c.label(), signed(c.orig)));
                    self.restore_card(c);
                }
            }
            p => {
                c.reval_busy.store(true, Ordering::SeqCst);
                let (m, c2) = (self.clone(), c.clone());
                std::thread::spawn(move || {
                    let r = match p {
                        Plan::Tune { prev, crashed, resume } => m.tune(&c2, prev, crashed, resume),
                        Plan::Check { e, want } => m.check_saved(&c2, &e, want),
                        Plan::Fixed(f) => m.apply_fixed(&c2, f),
                        _ => Ok(()),
                    };
                    if let Err(err) = r {
                        if !m.stopped() {
                            lg(format!("{} error: {err:#}; restoring {}", c2.label(), signed(c2.orig)));
                            m.restore_card(&c2);
                        }
                    }
                    *c2.next_reval.lock().unwrap() = Instant::now() + reval_interval(m.opts.reval_hours);
                    c2.reval_busy.store(false, Ordering::SeqCst);
                });
            }
        }
    }

    /// Mining has started: the mining threads are known (pause handshake), the start checks / tunings begin.
    pub fn attach(self: &Arc<Self>, gstats: &[Arc<GpuStat>]) {
        for c in &self.cards {
            *c.gs.lock().unwrap() = gstats.iter().find(|g| g.dev == c.dev).cloned();
        }
        for c in &self.cards {
            let p = c.plan.lock().unwrap().take();
            if let Some(p) = p {
                self.launch(c, p);
            }
        }
    }

    fn apply_now(&self, c: &Card, e: &Entry, want: i32) -> Result<()> {
        let before = c.cur.load(Ordering::Relaxed);
        self.set_checked(c, want)?;
        lg(format!(
            "{} offset {} -> {} (profile {}), verified {} ago ({:.1} W at tuning): applied without a start check",
            c.label(),
            signed(before),
            signed(want),
            profile_key(e),
            fmt_age(now().saturating_sub(e.verified_at)),
            e.w_after
        ));
        Ok(())
    }

    /// Pauses the mining thread of this GPU (nothing in flight) until dropped; no-op before mining / for a stopped worker.
    fn hold(&self, c: &Card) -> Hold {
        let g = c.gs.lock().unwrap().clone().filter(|g| g.alive.load(Ordering::Relaxed));
        if let Some(g) = &g {
            g.pause.store(true, Ordering::SeqCst);
            let t = Instant::now();
            while !g.paused.load(Ordering::SeqCst) && g.alive.load(Ordering::Relaxed) && t.elapsed() < Duration::from_secs(60) && !self.stopped() {
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        Hold(g)
    }

    /// One check of offset `at`.  The RAM slot is taken while this GPU still mines (other GPUs' checks may fill the RAM), then
    /// its mining pauses for the check only: after a passed check it mines on at `at`, after a failed one at `back`.
    fn step_check(&self, c: &Card, at: i32, back: i32, sp: &Spec) -> Res {
        let _slot = self.check_slot(c);
        let _hold = self.hold(c);
        if self.stopped() {
            return Res { reason: "interrupted".into(), ..Default::default() };
        }
        if c.cur.load(Ordering::Relaxed) != at {
            if let Err(e) = self.set_checked(c, at) {
                return Res { reason: format!("the GPU does not accept the offset {}: {e:#}", signed(at)), ..Default::default() };
            }
            std::thread::sleep(Duration::from_secs(3));
        }
        let r = self.run_check(c, sp);
        if !r.ok && !self.stopped() && c.cur.load(Ordering::Relaxed) != back {
            // the GPU may need a moment (and must still answer) after a failed check
            std::thread::sleep(Duration::from_secs(if r.driver_fault { 5 } else { 1 }));
            if let Err(e) = self.set_checked(c, back) {
                lg(format!("{} cannot set {} after the failed check: {e:#}", c.label(), signed(back)));
            }
        }
        r
    }

    /// A saved profile gets a quick check before it is used (not verified recently, from before 1.7.2, after an OC apply).
    /// A saved profile gets a quick check before it is used (not verified recently, from before 1.7.2, after an OC apply, after a
    /// crash).  A failure steps down from it (one step per quick check, at most 4) instead of tuning from scratch; nothing passes
    /// above the floor (the owner's offset, or the profile's own start when lower): back there, auto-uv paused at these clocks.
    fn check_saved(&self, c: &Card, e: &Entry, want: i32) -> Result<()> {
        let before = c.cur.load(Ordering::Relaxed);
        let m_changed = e.m != c.m;
        let step = self.opts.step;
        let floor = c.orig.min(e.start.unwrap_or(c.orig));
        let mut want = want;
        let mut e2 = e.clone();
        if e2.migrated {
            // checked at these clocks now: it becomes the profile of this run's lock either way
            e2.migrated = false;
            e2.lock = c.lock.unwrap_or(0);
            e2.mlock = c.mlock.unwrap_or(0);
        }
        for attempt in 0..5 {
            // the reference digests depend on the test job and m, not on the offset
            let back = if want - step >= floor { want - step } else { c.orig };
            let r = self.step_check(c, want, back, &self.quick_spec(if m_changed { vec![] } else { e.refs.clone() }));
            if self.stopped() {
                return Ok(());
            }
            if r.ok {
                lg(format!(
                    "{} offset {} -> {} (profile {}{}), quick check passed ({} reruns, cpu ok {}), {}, {} MHz",
                    c.label(),
                    signed(before),
                    signed(want),
                    profile_key(e),
                    if want != e.offset { format!(", stepped down from {}", signed(e.offset)) } else { String::new() },
                    r.reruns,
                    r.cpu_ok,
                    fmt_w(r.watts),
                    r.clk.unwrap_or(0)
                ));
                if want <= e.offset {
                    e2.offset = want;
                    e2.verified_at = now();
                    e2.orig = c.orig;
                    if e2.clk_ok == 0 || want != e.offset {
                        e2.clk_ok = r.clk.unwrap_or(e2.clk_ok);
                    }
                    if m_changed && !r.refs.is_empty() {
                        e2.refs = r.refs.clone();
                        e2.m = c.m;
                        e2.disabled_until = 0;
                    }
                    self.save_entry(c, &e2);
                }
                return Ok(());
            }
            e2.m = c.m; // the failure is a real one at the current m (refs were valid or not used)
            e2.edge = want;
            if want - step < floor || attempt == 4 {
                lg(format!("{} offset {} (profile {}) FAILED the quick check ({}); nothing passes down to {}: back to {}, auto-uv paused at these clocks for 24 h", c.label(), signed(want), profile_key(e), r.reason, signed(floor), signed(c.orig)));
                e2.offset = c.orig.min(e2.offset);
                e2.disabled_until = now() + DISABLE_SECS;
                self.save_entry(c, &e2);
                return Ok(());
            }
            lg(format!("{} offset {} (profile {}) FAILED the quick check ({}): one step down to {} and checking it (no tuning from scratch)", c.label(), signed(want), profile_key(e), r.reason, signed(want - step)));
            want -= step;
        }
        Ok(())
    }

    fn apply_fixed(&self, c: &Card, f: i32) -> Result<()> {
        let f = f.min(HARD_CAP);
        let before = c.cur.load(Ordering::Relaxed);
        if f == before {
            lg(format!("{} already at {}", c.label(), signed(f)));
            return Ok(());
        }
        let r = self.step_check(c, f, c.orig, &self.quick_spec(vec![]));
        if self.stopped() {
            return Ok(()); // a stop is not a failure of the offset (the shutdown restore puts the owner's back)
        }
        if r.ok {
            lg(format!("{} offset {} -> {}, {} (fixed by --uv-offset), check passed ({} reruns, cpu ok {})", c.label(), signed(before), signed(f), fmt_w(r.watts), r.reruns, r.cpu_ok));
        } else {
            lg(format!("{} fixed offset {} FAILED the check ({}); back to {}", c.label(), signed(f), r.reason, signed(c.orig)));
        }
        Ok(())
    }

    /// What faults.rs remembers about crashes of this GPU while mining.
    fn fault_plan(&self, c: &Card) -> Result<FaultPlan> {
        let Some(mut r) = crate::faults::get(&self.dir, &c.uuid) else { return Ok(FaultPlan::Go) };
        if let Some(left) = crate::faults::excluded(&self.dir, &c.uuid) {
            lg(crate::faults::excluded_line(&c.label(), &r, left));
            return Ok(FaultPlan::Done);
        }
        let xid = if r.xid != 0 { format!("Xid {}", r.xid) } else { "no Xid".into() };
        if r.n >= 1 && r.offset > c.orig {
            lg(format!("{} crashed while mining at {} in the previous run ({xid}): that offset counts as failed", c.label(), signed(r.offset)));
            r.n = 0;
            crate::faults::put(&self.dir, &c.uuid, &r);
            return Ok(FaultPlan::Crashed(r.offset));
        }
        if r.n >= crate::faults::LOWER_AFTER {
            let hive = crate::hiveoc::load().and_then(|h| self.nv.index(c.nv).map(|i| h.core_offset(i as usize)));
            match crate::faults::lower_step(r.offset, hive) {
                Some(v) if v >= c.lo => {
                    lg(format!(
                        "{} CRASHED while mining at {} {} starts in a row ({xid}): running it at {} ({}) instead; no undervolt tuning on this GPU while lowered; your {} is put back on exit (delete gpu-faults.json in the state directory to forget it)",
                        c.label(),
                        signed(r.offset),
                        r.n,
                        signed(v),
                        if hive == Some(v) { "the HiveOS profile Core Clock value" } else { "60 MHz lower" },
                        signed(c.orig)
                    ));
                    r.safe = Some(v);
                    r.safe_orig = c.orig;
                    r.n = 0;
                    crate::faults::put(&self.dir, &c.uuid, &r);
                    self.set_checked(c, v)?;
                    return Ok(FaultPlan::Done);
                }
                _ => {
                    r.excluded_until = now() + crate::faults::EXCLUDE_SECS;
                    crate::faults::put(&self.dir, &c.uuid, &r);
                    lg(crate::faults::excluded_line(&c.label(), &r, crate::faults::EXCLUDE_SECS));
                    return Ok(FaultPlan::Done);
                }
            }
        }
        if let Some(v) = r.safe {
            if r.safe_orig == c.orig {
                lg(format!("{} runs at the lowered {} instead of {} (it crashed while mining at the higher one; delete gpu-faults.json in the state directory to forget it)", c.label(), signed(v), signed(c.orig)));
                self.set_checked(c, v)?;
                return Ok(FaultPlan::Done);
            }
            lg(format!("{} the starting offset changed ({} when it was lowered to {}, {} now): the lowered offset is forgotten", c.label(), signed(r.safe_orig), signed(v), signed(c.orig)));
            r.safe = None;
            crate::faults::put(&self.dir, &c.uuid, &r);
        }
        if r.n == 1 {
            lg(format!("{} crashed once while mining at {} ({xid}); one more crash at it and it is lowered", c.label(), signed(r.offset)));
        }
        Ok(FaultPlan::Go)
    }

    /// While mining (once a second from `tick`): the clock probe of a card whose lock the miner does not know.  10 s after mining
    /// was first seen, 8 samples of the core clock under mining load pick the saved profile of these clocks (or none: tuning).
    fn probe_tick(self: &Arc<Self>, c: &Arc<Card>, gs: &Arc<GpuStat>) {
        let mut g = c.probe.lock().unwrap();
        let Some(st) = g.as_mut() else { return };
        let att = gs.attempts.load(Ordering::Relaxed);
        if gs.paused.load(Ordering::Relaxed) || att == 0 {
            return;
        }
        let mining = match st.seen {
            None => {
                st.seen = Some((Instant::now(), att));
                false
            }
            Some((t, last)) => {
                st.seen = Some((t, att));
                att > last && t.elapsed() >= Duration::from_secs(10)
            }
        };
        if !mining {
            return;
        }
        if let Some(clk) = self.nv.clock_mhz(c.nv) {
            st.samples.push(clk);
        }
        if st.samples.len() < 8 {
            return;
        }
        let st = g.take().unwrap();
        drop(g);
        let mut s = st.samples.clone();
        s.sort_unstable();
        let clk = s[s.len() / 2];
        let cur = c.cur.load(Ordering::Relaxed);
        if st.reset {
            let gone = self.forget_profiles(c, |e| profile_fits(e, None, c.mlock, Some((clk, cur))));
            lg(format!(
                "{} --uv-reset: {} for {clk} MHz under load forgotten; the profiles of other clocks are kept ({}); --uv-reset-all forgets them all",
                c.label(),
                if gone.is_empty() { "no profile".to_string() } else { format!("profile {}", gone.join(", ")) },
                self.profiles_line(c)
            ));
        }
        let pick = best_fit(&self.profiles(c), None, c.mlock, Some((clk, cur)));
        lg(format!(
            "{} core clock under mining load {clk} MHz at {}: profiles {}; {}",
            c.label(),
            signed(cur),
            self.profiles_line(c),
            pick.as_ref().map_or("none fits these clocks".to_string(), |e| format!("using {} ({} MHz under load when tuned)", profile_key(e), e.clk))
        ));
        let p = self.decide(c, pick, st.oc);
        self.launch(c, p);
    }

    fn quick_spec(&self, refs: Vec<String>) -> Spec {
        Spec { secs: 20, min_reruns: 30, cpu: 2, measure: 8, refs }
    }

    /// Full tuning of the card at the clocks it has now, while the other GPUs mine (this GPU's mining pauses for each check; the
    /// checks follow each other, so it mines again when the tuning is over).
    fn tune(&self, c: &Arc<Card>, prev: Option<Entry>, crashed: Option<Crash>, resume: Option<i32>) -> Result<()> {
        let step = self.opts.step;
        let mut cap = c.max;
        if let Some(x) = crashed.map(|k| k.at) {
            let want = c.start().0.clamp(0, cap);
            if x - step < want + step {
                // it died at (or one step above) the start: nothing left to search; remember "nothing above your offset"
                // for these clocks so the next starts do not try again (--uv-reset to retry)
                lg(format!("{} the crash was at or right above the start {}: not tuning, keeping {} (lower --uv-min, or --uv-reset later)", c.label(), signed(want), signed(c.orig)));
                let mut e = prev.clone().unwrap_or_else(|| self.new_entry(c));
                e.offset = c.orig;
                e.tested_at = now();
                e.lock = c.lock.unwrap_or(e.lock);
                e.mlock = c.mlock.unwrap_or(e.mlock);
                e.migrated = false;
                self.save_entry(c, &e);
                return Ok(());
            }
            cap = cap.min(x - step);
            lg(format!("{} tuning again below the crash: up to {}", c.label(), signed(cap)));
        }
        // search base/floor: --uv-min for this card, else its default (Card::start); never above the cap
        let start = c.start().0.clamp(0, cap);
        if start + step > cap {
            lg(format!("{} start offset {} leaves no room below the cap {} ({}); nothing to do", c.label(), signed(start), signed(cap), c.max_src));
            return Ok(());
        }
        lg(format!(
            "{} tuning: {} s warm-up at {}, then +{step} MHz steps up to {} ({} s each); the other GPUs mine meanwhile",
            c.label(),
            self.opts.warmup_secs,
            signed(start),
            signed(cap),
            self.opts.step_secs
        ));
        // the transcript digests depend on m too: a reference saved at another m must not seed the check
        let golden = prev.as_ref().filter(|e| e.driver == self.driver && e.kernel == self.kernel_of(c) && e.m == c.m).map(|e| e.refs.clone()).unwrap_or_default();
        let base = self.step_check(c, start, c.orig, &Spec { secs: self.opts.warmup_secs, min_reruns: 60, cpu: 2, measure: 20, refs: golden.clone() });
        if self.stopped() {
            return Ok(());
        }
        if !base.ok {
            // the start itself fails at these clocks: never keep it (or the owner's offset) unchecked; down in 60 MHz steps to the
            // first one that passes, which becomes the profile of these clocks (below the owner's offset: used instead of it)
            lg(format!("{} the start offset {} FAILS the check at these clocks ({}): looking for a lower one that passes (60 MHz steps)", c.label(), signed(start), base.reason));
            let mut v = if start > c.orig { c.orig } else { start - 60 };
            loop {
                if v < c.lo || self.stopped() {
                    break;
                }
                let r = self.step_check(c, v, (v - 60).max(c.lo), &Spec { secs: self.opts.confirm_secs, min_reruns: 150, cpu: 4, measure: 20, refs: golden.clone() });
                if self.stopped() {
                    return Ok(());
                }
                if r.ok {
                    let mut e = self.new_entry(c);
                    e.offset = v;
                    e.cap = c.max;
                    e.refs = if golden.is_empty() { r.refs.clone() } else { golden.clone() };
                    e.m = c.m;
                    e.clk = r.clk.unwrap_or(0);
                    e.clk_ok = e.clk;
                    e.w_after = r.watts.unwrap_or(0.0);
                    e.tested_at = now();
                    e.verified_at = now();
                    e.lock = c.lock.unwrap_or(0);
                    e.mlock = c.mlock.unwrap_or(0);
                    e.start = Some(v);
                    e.edge = start;
                    self.save_entry(c, &e);
                    self.clear_trying(c);
                    lg(format!("{} {} passes ({} reruns, cpu ok {}), {}, {} MHz: mining at it, saved as profile {} (your {} is put back on exit)", c.label(), signed(v), r.reruns, r.cpu_ok, fmt_w(r.watts), r.clk.unwrap_or(0), profile_key(&e), signed(c.orig)));
                    return Ok(());
                }
                lg(format!("{} {} FAILS too ({})", c.label(), signed(v), r.reason));
                if v <= 0 {
                    break;
                }
                v = (v - 60).max(0);
            }
            lg(format!("{} no offset down to {} passes the check at these clocks: check the clocks / memory / power of this GPU (left at {})", c.label(), signed(v.max(c.lo)), signed(c.cur.load(Ordering::Relaxed))));
            return Ok(());
        }
        let refs = if golden.is_empty() { base.refs.clone() } else { golden };
        let base_w = base.watts;
        lg(format!("{} baseline {}: ok, {}, {} MHz, {} C", c.label(), signed(start), fmt_w(base_w), base.clk.unwrap_or(0), base.temp.unwrap_or(0)));
        let mut good = start;
        let mut good_w = base_w;
        let mut failed: Option<(i32, String)> = None;
        let mut clks: Vec<u32> = base.clk.into_iter().collect();
        // after a crash: the steps up to the last good one already passed in the previous run, so do not climb them again
        // (the result is still confirmed by the long check below)
        if let Some(g) = crashed.and_then(|k| k.good).filter(|&g| g > start && g <= cap) {
            lg(format!("{} steps up to {} passed before the crash: not climbing them again", c.label(), signed(g)));
            good = g;
        }
        // a verified result of these clocks (the limit was raised): the search goes on from it, not from the start
        if let Some(g) = resume.filter(|&g| g > good && g <= cap) {
            lg(format!("{} {} was verified at these clocks: going on from it", c.label(), signed(g)));
            good = g;
        }
        c.last_good.store(good, Ordering::Relaxed);
        while good + step <= cap {
            let cand = good + step;
            let r = self.step_check(c, cand, good, &Spec { secs: self.opts.step_secs, min_reruns: 30, cpu: 2, measure: 12, refs: refs.clone() });
            if self.stopped() {
                return Ok(());
            }
            if r.ok {
                lg(format!("{} step {}: ok ({} reruns, cpu ok {}), {}, {} MHz, {} C", c.label(), signed(cand), r.reruns, r.cpu_ok, fmt_w(r.watts), r.clk.unwrap_or(0), r.temp.unwrap_or(0)));
                good = cand;
                good_w = r.watts.or(good_w);
                clks.extend(r.clk);
                c.last_good.store(good, Ordering::Relaxed);
            } else {
                lg(format!("{} step {}: FAILED ({})", c.label(), signed(cand), r.reason));
                failed = Some((cand, r.reason));
                break;
            }
        }
        if let Some(x) = crashed.map(|k| k.at) {
            if failed.is_none() && good == cap {
                failed = Some((x, "crashed in the previous run".into()));
            }
        }
        // result when a failure was found: the last good step, minus one step of margin by default
        // (--uv-keep-last keeps the last good step itself); at the cap with no failure: the cap
        let mut fin = if failed.is_some() && !self.opts.keep_last_good { good - step } else { good };
        fin = fin.max(start);
        if let Some((fo, _)) = &failed {
            lg(format!("{} edge found at {}: stepping back to {}", c.label(), signed(*fo), signed(fin)));
        }
        // confirmation with a longer check; on failure one more step down
        let mut conf_w = good_w;
        let mut conf_clk: Option<u32> = None;
        while fin > start {
            let r = self.step_check(c, fin, (fin - step).max(start), &Spec { secs: self.opts.confirm_secs, min_reruns: 150, cpu: 4, measure: 20, refs: refs.clone() });
            if self.stopped() {
                return Ok(());
            }
            if r.ok {
                lg(format!("{} confirmation at {}: ok ({} reruns, cpu ok {}), {}, {} MHz", c.label(), signed(fin), r.reruns, r.cpu_ok, fmt_w(r.watts), r.clk.unwrap_or(0)));
                conf_w = r.watts.or(conf_w);
                conf_clk = r.clk;
                break;
            }
            lg(format!("{} confirmation at {}: FAILED ({}); one step lower", c.label(), signed(fin), r.reason));
            fin = (fin - step).max(start);
        }
        if fin == start {
            self.set_checked(c, start)?;
            conf_w = base_w;
        }
        let (lo, hi) = (clks.iter().min().copied().unwrap_or(0), clks.iter().max().copied().unwrap_or(0));
        let mut e = self.new_entry(c);
        e.offset = fin;
        e.cap = c.max;
        e.refs = refs;
        e.m = c.m;
        e.clk = base.clk.unwrap_or(0);
        e.w_before = base_w.unwrap_or(0.0);
        e.w_after = conf_w.unwrap_or(0.0);
        e.tested_at = now();
        e.verified_at = now();
        e.lock = c.lock.unwrap_or(0);
        e.mlock = c.mlock.unwrap_or(0);
        e.unlocked = c.lock.is_none() && hi >= lo + 30;
        e.start = Some(start);
        e.clk_ok = conf_clk.or(base.clk).unwrap_or(0);
        e.edge = failed.as_ref().map_or(0, |f| f.0);
        self.save_entry(c, &e);
        self.clear_trying(c);
        lg(format!("{} offset {} -> {}, {} -> {}, saved as profile {} (profiles now: {})", c.label(), signed(start), signed(fin), fmt_w(base_w), fmt_w(conf_w), profile_key(&e), self.profiles_line(c)));
        if e.unlocked {
            lg(format!(
                "{} the core clock was NOT locked during the tuning: it moved {lo}..{hi} MHz with the offset; this profile holds for the unlocked clock only (a lock - HiveOS Core Clock lock, --lock-cclock - has its own profile)",
                c.label()
            ));
        }
        if fin < c.orig && failed.as_ref().is_some_and(|f| f.0 <= c.orig) {
            lg(format!(
                "{} WARNING your offset {} is above the edge found at these clocks ({} failed): mining at the confirmed {} instead (yours is put back on exit)",
                c.label(),
                signed(c.orig),
                signed(failed.as_ref().unwrap().0),
                signed(fin)
            ));
        }
        Ok(())
    }

    // ----- check child ----------------------------------------------------------------------------------------------

    /// Runs the deterministic GPU check + CPU verification of hits in a child process and samples NVML meanwhile.
    fn run_check(&self, c: &Card, sp: &Spec) -> Res {
        if self.stopped() {
            return Res { reason: "interrupted".into(), ..Default::default() };
        }
        let cur = c.cur.load(Ordering::Relaxed);
        if cur != c.orig {
            self.mark_trying(c, cur);
        }
        let mut r = Res::default();
        for attempt in 0..3 {
            r = match self.run_check_inner(c, sp) {
                Ok(r) => r,
                Err(e) => Res { reason: format!("{e:#}"), ..Default::default() },
            };
            // an OC apply (HiveOS nvidia-oc at miner start or from the web UI) may have reset the offset during the check:
            // then the check ran at that offset, not at `cur`, and says nothing about `cur`: set it again and repeat
            if r.driver_fault || self.stopped() {
                break;
            }
            match self.nv.get_offset(c.nv, c.mode) {
                Ok(v) if v != cur => match self.oc_apply_kind(c, v) {
                    Some(why) if attempt < 2 => {
                        lg(format!("{} the offset was reset to {} during the check ({why}): setting {} again and repeating the check", c.label(), signed(v), signed(cur)));
                        if let Err(e) = self.set_checked(c, cur) {
                            r = Res { reason: format!("could not set {} again after the OC apply: {e:#}", signed(cur)), ..Default::default() };
                            break;
                        }
                        std::thread::sleep(Duration::from_secs(3));
                    }
                    _ => {
                        r.ok = false;
                        r.reason = format!("the offset was changed to {} by someone else during the check ({} expected)", signed(v), signed(cur));
                        break;
                    }
                },
                _ => break,
            }
        }
        // a driver fault (Xid, hang) often leaves the GPU unusable until a reboot ("GPU requires reset"), and then
        // nothing after this point runs: keep the marker, so the next start tunes below this offset instead of
        // climbing into the same fault again. tune() / apply_*() remove it once a result is saved.
        if r.driver_fault {
            c.fault.store(true, Ordering::SeqCst);
        } else {
            self.clear_trying(c);
        }
        r
    }

    /// At a stop: removes the crash markers of checks that were interrupted (not those of a driver fault).
    fn forget_trying(&self) {
        for c in &self.cards {
            if !c.fault.load(Ordering::SeqCst) {
                self.clear_trying(c);
            }
        }
    }

    /// A slot for one check process; waits while the free RAM does not cover one more (the GPU keeps mining meanwhile when
    /// the slot is taken before the pause, see `recheck`).  `None` = this thread holds one already.
    fn check_slot(&self, c: &Card) -> Option<CheckSlot> {
        if SLOT_HELD.with(|h| h.get()) {
            return None;
        }
        let (m, cv) = check_slots();
        let need = check_mb();
        let mut running = m.lock().unwrap();
        let mut said = false;
        loop {
            // already-running checks may not have reached their peak yet: count each of them in full
            if self.stopped() || check_fits(*running, need, mem_available_mb()) {
                *running += 1;
                SLOT_HELD.with(|h| h.set(true));
                return Some(CheckSlot);
            }
            if !said {
                said = true;
                lg(format!("{} waits for RAM: {} check(s) running, {} MB free, a check needs ~{} MB", c.label(), *running, mem_available_mb().unwrap_or(0), need));
            }
            running = cv.wait_timeout(running, Duration::from_secs(1)).unwrap().0;
        }
    }

    fn run_check_inner(&self, c: &Card, sp: &Spec) -> Result<Res> {
        let _slot = self.check_slot(c);
        let exe = std::env::current_exe()?;
        let mut cmd = Command::new(exe);
        cmd.args(["--uv-check", "--devices", &c.dev.to_string(), "--uv-secs", &sp.secs.to_string(), "--uv-min-reruns", &sp.min_reruns.to_string(), "--uv-cpu", &sp.cpu.to_string(), "--uv-measure", &sp.measure.to_string(), "--uv-expect-bus", &c.bus]);
        if !sp.refs.is_empty() {
            cmd.args(["--uv-ref", &sp.refs.join(",")]);
        }
        cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = cmd.spawn().context("cannot start the check process")?;
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        let out = child.stdout.take().unwrap();
        std::thread::spawn(move || {
            for l in BufReader::new(out).lines().map_while(|l| l.ok()) {
                if tx.send(l).is_err() {
                    break;
                }
            }
        });
        let errtail = Arc::new(Mutex::new(String::new()));
        {
            let (mut er, t) = (child.stderr.take().unwrap(), errtail.clone());
            std::thread::spawn(move || {
                let mut b = Vec::new();
                let _ = er.read_to_end(&mut b);
                let s = String::from_utf8_lossy(&b).into_owned();
                let tail: String = s.lines().rev().take(3).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join(" | ");
                *t.lock().unwrap() = tail;
            });
        }
        let t0 = Instant::now();
        let deadline = Duration::from_secs((sp.secs + sp.measure) as u64 + 150);
        let mut load_start: Option<Instant> = None;
        let mut samples: Vec<(Instant, f64, u32, u32)> = Vec::new();
        let mut result: Option<String> = None;
        let mut xid: Option<u64> = None;
        let mut killed: Option<&str> = None;
        #[allow(unused_mut)]
        let mut signal: Option<i32> = None;
        let mut last_sample = Instant::now();
        let exit: Option<i32> = loop {
            if self.stopped() {
                killed = Some("interrupted");
            } else if t0.elapsed() > deadline {
                killed = Some("timeout (GPU hang?)");
            } else if let Some(x) = self.nv.take_xids(c.nv).first() {
                xid = Some(*x);
                killed = Some("Xid error");
            }
            if killed.is_some() {
                let _ = child.kill();
            }
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(l) => {
                    if l.starts_with("UVMEASURE") {
                        load_start = Some(Instant::now());
                    } else if l.starts_with("UVRESULT") {
                        result = Some(l);
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {}
                Err(_) => {}
            }
            if load_start.is_some() && result.is_none() && last_sample.elapsed() >= Duration::from_millis(250) {
                last_sample = Instant::now();
                if let Some(w) = self.nv.power_w(c.nv) {
                    samples.push((Instant::now(), w, self.nv.clock_mhz(c.nv).unwrap_or(0), self.nv.temp_c(c.nv).unwrap_or(0)));
                }
            }
            if let Some(s) = child.try_wait()? {
                #[cfg(unix)]
                {
                    use std::os::unix::process::ExitStatusExt;
                    if let Some(sig) = s.signal() {
                        signal = Some(sig);
                    }
                }
                break s.code();
            }
        };
        while let Ok(l) = rx.recv_timeout(Duration::from_millis(500)) {
            if l.starts_with("UVRESULT") {
                result = Some(l);
            }
        }
        std::thread::sleep(Duration::from_millis(300));
        if xid.is_none() {
            xid = self.nv.take_xids(c.nv).first().copied();
        }
        let mut res = Res::default();
        if let Some(line) = &result {
            parse_result(line, &mut res);
        } else {
            let how = match (exit, signal) {
                (_, Some(9)) => "killed by signal 9 (out of memory? see dmesg)".to_string(),
                (_, Some(sig)) => format!("killed by signal {sig}"),
                (Some(code), None) => format!("exit {code}"),
                (None, None) => "no exit code".to_string(),
            };
            res.reason = format!("check process ended without a result ({how}){}", {
                std::thread::sleep(Duration::from_millis(200));
                let t = errtail.lock().unwrap().clone();
                if t.is_empty() { String::new() } else { format!(": {t}") }
            });
        }
        if let Some(k) = killed {
            res.ok = false;
            res.driver_fault = k != "interrupted";
            res.reason = k.to_string();
        }
        if let Some(x) = xid {
            res.ok = false;
            res.driver_fault = true;
            res.reason = format!("Xid {x} reported by the driver ({})", res.reason);
        }
        // power / clock / temperature: second half of the load window
        if samples.len() >= 4 {
            let half = samples.len() / 2;
            let part = &samples[half..];
            res.watts = Some(part.iter().map(|s| s.1).sum::<f64>() / part.len() as f64);
            let mut clks: Vec<u32> = part.iter().map(|s| s.2).collect();
            clks.sort_unstable();
            res.clk = Some(clks[clks.len() / 2]);
            res.temp = part.iter().map(|s| s.3).max();
        }
        Ok(res)
    }

    // ----- supervision while mining ---------------------------------------------------------------------------------

    /// Called from the coordinator loop; cheap (once a second does anything).
    pub fn tick(self: &Arc<Self>, gstats: &[Arc<GpuStat>], user: &Arc<Stats>) {
        {
            let mut lt = self.last_tick.lock().unwrap();
            if lt.elapsed() < Duration::from_secs(1) {
                return;
            }
            *lt = Instant::now();
        }
        if self.stopped() {
            return;
        }
        if let Some(secs) = self.inject_bad {
            if self.started.elapsed() >= Duration::from_secs(secs) && !self.injected.swap(true, Ordering::SeqCst) {
                lg("TEST HOOK: reporting one CPU-verification failure");
                for g in gstats {
                    g.uv_bad.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        for c in &self.cards {
            if !c.owned.load(Ordering::SeqCst) {
                continue;
            }
            let Some(gs) = gstats.iter().find(|g| g.dev == c.dev) else { continue };
            if c.probe.lock().unwrap().is_some() {
                self.probe_tick(c, gs);
            }
            // the offset changed under us: an OC apply (HiveOS nvidia-oc at miner start or after an OC change in the web UI
            // writes 0 when the Core Clock field is empty, or the profile's value) gets the verified offset back after a
            // quick check; any other value is the owner's own choice: do not fight
            let due = {
                let mut nr = c.next_readback.lock().unwrap();
                let due = Instant::now() >= *nr;
                if due {
                    *nr = Instant::now() + Duration::from_secs(30);
                }
                due
            };
            if due && !c.reval_busy.load(Ordering::Relaxed) {
                let want = c.cur.load(Ordering::Relaxed);
                if let Ok(v) = self.nv.get_offset(c.nv, c.mode) {
                    if v != want {
                        match self.oc_apply_kind(c, v) {
                            Some(why) if want < c.orig && self.oc_reset_allowed(c) => {
                                // a lowered offset (it crashed higher, or the owner's is past the edge): set again at once
                                match self.set_checked(c, want) {
                                    Ok(()) => lg(format!("{} the offset was reset to {} by an OC apply ({why}); set back to the lowered {}", c.label(), signed(v), signed(want))),
                                    Err(e) => {
                                        lg(format!("{} the offset was reset to {} by an OC apply ({why}); cannot set {} again ({e:#}): left as it is", c.label(), signed(v), signed(want)));
                                        c.owned.store(false, Ordering::SeqCst);
                                    }
                                }
                            }
                            Some(why) if want > c.orig && c.lock.is_none() && self.entry(c).is_some_and(|e| e.clk != 0) && self.oc_reset_allowed(c) => {
                                // the OC apply may have changed the clock lock too: the clock under mining load at the reset offset
                                // decides first (the verified offset back + quick check, or tuning again)
                                lg(format!("{} the offset was reset to {} by an OC apply ({why}); the core clock under mining load is compared with the tuning before the verified {} goes back", c.label(), signed(v), signed(want)));
                                c.cur.store(v, Ordering::Relaxed);
                                *c.probe.lock().unwrap() = Some(ProbeSt::new(false, true));
                            }
                            Some(why) if want > c.orig && self.oc_reset_allowed(c) => {
                                lg(format!("{} the offset was reset to {} by an OC apply ({why}); re-applying the verified {} after a quick check", c.label(), signed(v), signed(want)));
                                c.cur.store(v, Ordering::Relaxed);
                                c.reval_busy.store(true, Ordering::SeqCst);
                                let (m, c2, gs2) = (self.clone(), c.clone(), gs.clone());
                                std::thread::spawn(move || {
                                    m.reapply(&c2, &gs2, want);
                                    *c2.next_reval.lock().unwrap() = Instant::now() + reval_interval(m.opts.reval_hours);
                                    c2.reval_busy.store(false, Ordering::SeqCst);
                                });
                            }
                            Some(why) if want > c.orig => {
                                lg(format!("{} the offset was reset to {} by an OC apply ({why}) again: {OC_RESETS_MAX} times within an hour already, so auto-uv stands down on this GPU and leaves it as is (restart the miner once the OC is settled)", c.label(), signed(v)));
                                c.owned.store(false, Ordering::SeqCst);
                            }
                            _ => {
                                lg(format!("{} the offset was changed by someone else ({} now, {} expected): auto-uv stands down on this GPU and leaves it as is", c.label(), signed(v), signed(want)));
                                c.owned.store(false, Ordering::SeqCst);
                            }
                        }
                        continue;
                    }
                }
                if c.cur.load(Ordering::Relaxed) > c.orig && c.probe.lock().unwrap().is_none() && !gs.paused.load(Ordering::Relaxed) {
                    if let Some(why) = self.clock_watch(c, gs.attempts.load(Ordering::Relaxed)) {
                        lg(format!("{} the core clock under mining load changed at the same offset ({why}): the clock lock changed while mining; back to {}, the clock picks the profile again", c.label(), signed(c.orig)));
                        let _ = self.set_checked(c, c.orig);
                        *c.probe.lock().unwrap() = Some(ProbeSt::new(false, false));
                        continue;
                    }
                }
            }
            // a hit that failed CPU verification: the GPU computed something wrong (handled once a running re-validation is over)
            if !c.reval_busy.load(Ordering::Relaxed) {
                let bad = gs.uv_bad.load(Ordering::Relaxed);
                let new_bad = {
                    let mut seen = c.bad_seen.lock().unwrap();
                    let n = bad.saturating_sub(*seen);
                    *seen = bad;
                    n
                };
                if new_bad > 0 && c.cur.load(Ordering::Relaxed) > c.orig {
                    self.step_back(c, &format!("{new_bad} found result(s) failed the CPU verification"));
                }
            }
            if !c.reval_busy.load(Ordering::Relaxed) && Instant::now() >= *c.next_reval.lock().unwrap() && c.cur.load(Ordering::Relaxed) > c.orig {
                c.reval_busy.store(true, Ordering::SeqCst);
                let (m, c2, gs2) = (self.clone(), c.clone(), gs.clone());
                std::thread::spawn(move || {
                    m.revalidate(&c2, &gs2);
                    *c2.next_reval.lock().unwrap() = Instant::now() + reval_interval(m.opts.reval_hours);
                    c2.reval_busy.store(false, Ordering::SeqCst);
                });
            }
        }
        // burst of rejected shares (low-difficulty rejects are a target-mode matter, not an undervolt one)
        let (rej, acc, low) = (user.rejected.load(Ordering::Relaxed), user.accepted.load(Ordering::Relaxed), user.low_diff.load(Ordering::Relaxed));
        let mut h = self.rej_hist.lock().unwrap();
        if h.back().is_none_or(|b| b.0.elapsed() >= Duration::from_secs(30)) {
            h.push_back((Instant::now(), rej, acc, low));
        }
        while h.front().is_some_and(|f| f.0.elapsed() > Duration::from_secs(300)) {
            h.pop_front();
        }
        if let Some(f) = h.front() {
            if f.0.elapsed() >= Duration::from_secs(120) {
                let (drej, dacc) = ((rej - f.1).saturating_sub(low - f.3), acc - f.2);
                if drej >= 5 && drej > dacc && Instant::now() >= *self.rej_cooldown.lock().unwrap() {
                    *self.rej_cooldown.lock().unwrap() = Instant::now() + Duration::from_secs(600);
                    h.clear();
                    drop(h);
                    for c in &self.cards {
                        if c.owned.load(Ordering::SeqCst) && c.cur.load(Ordering::Relaxed) > c.orig {
                            self.step_back(c, &format!("burst of rejected shares ({drej} rejected vs {dacc} accepted in 5 min)"));
                        }
                    }
                }
            }
        }
    }

    /// Every 30 s while mining at an undervolt offset: the load clock against the one seen after the offset was set (3 samples
    /// each).  Some(text) when it rose by more than 75 MHz / 5 %: the clock lock was raised (or removed) under the miner.
    fn clock_watch(&self, c: &Card, attempts: u64) -> Option<String> {
        let clk = self.nv.clock_mhz(c.nv)?;
        let mut rc = c.run_clk.lock().unwrap();
        // only while the GPU mines (no job / pool down: idle clocks)
        let mining = attempts > rc.att;
        rc.att = attempts;
        if !mining {
            rc.samples.clear();
            return None;
        }
        rc.samples.push(clk);
        if rc.samples.len() > 3 {
            rc.samples.remove(0);
        }
        if rc.samples.len() < 3 {
            return None;
        }
        let mut s = rc.samples.clone();
        s.sort_unstable();
        let med = s[1];
        match rc.base {
            None => {
                rc.base = Some(med);
                None
            }
            Some(b) => {
                let tol = (b as i64 * 5 / 100).max(75);
                (med as i64 - b as i64 > tol).then(|| format!("{b} -> {med} MHz"))
            }
        }
    }

    /// One step down (never below the owner's offset); below that: give up and restore.
    fn step_back(&self, c: &Card, why: &str) {
        // after the shutdown restore (or a failed restore) the card is not ours any more: never set an offset again
        if !c.owned.load(Ordering::SeqCst) || self.stopped() {
            return;
        }
        let cur = c.cur.load(Ordering::Relaxed);
        let new = (cur - self.opts.step).max(c.orig);
        let mut e = self.entry(c).unwrap_or_else(|| self.new_entry(c));
        if new == cur {
            return;
        }
        match self.set_checked(c, new) {
            Ok(()) => {
                lg(format!("{} {why}: offset {} -> {}, saved", c.label(), signed(cur), signed(new)));
                e.offset = new;
                if new <= c.orig {
                    e.disabled_until = now() + DISABLE_SECS;
                    lg(format!("{} back at the starting offset {}: auto-uv paused for this GPU for 24 h", c.label(), signed(c.orig)));
                }
                self.save_entry(c, &e);
            }
            Err(err) => {
                lg(format!("{} {why}: could not step back ({err:#}); restoring", c.label()));
                self.restore_card(c);
            }
        }
        *c.next_reval.lock().unwrap() = Instant::now() + Duration::from_secs(60);
    }

    /// Is `v` (read back instead of the offset this run set) the work of an OC apply rather than the owner's own choice?
    /// The HiveOS nvidia-oc writes the profile's Core Clock value at every miner start and OC change, 0 when the field is
    /// empty; other OC tools put the owner's offset back.  Returns how it was recognised, for the log.
    fn oc_apply_kind(&self, c: &Card, v: i32) -> Option<&'static str> {
        let hive = crate::hiveoc::load().and_then(|h| self.nv.index(c.nv).map(|i| h.core_offset(i as usize)));
        if hive == Some(v) {
            Some("the value of the HiveOS OC profile")
        } else if v == 0 {
            Some("offset 0: HiveOS nvidia-oc with an empty Core Clock field, or an OC reset")
        } else if v == c.orig {
            Some("your offset from before the miner started")
        } else {
            None
        }
    }

    /// Rate limit of the re-applies: an OC tool that keeps resetting the offset is not fought for ever.
    fn oc_reset_allowed(&self, c: &Card) -> bool {
        let mut r = c.oc_resets.lock().unwrap();
        r.retain(|t| t.elapsed() < Duration::from_secs(3600));
        if r.len() >= OC_RESETS_MAX {
            return false;
        }
        r.push(Instant::now());
        true
    }

    /// After an OC apply reset the offset while mining: set the verified offset again and quick-check it (mining on this GPU
    /// pauses for ~30 s); a failure steps back as in the re-validation.
    fn reapply(&self, c: &Arc<Card>, gs: &Arc<GpuStat>, want: i32) {
        if self.stopped() || !c.owned.load(Ordering::SeqCst) {
            return;
        }
        if let Err(e) = self.set_checked(c, want) {
            lg(format!("{} could not set {} again after the OC apply ({e:#}); auto-uv stands down on this GPU", c.label(), signed(want)));
            c.owned.store(false, Ordering::SeqCst);
            return;
        }
        self.recheck(c, gs, "quick check after the OC apply");
    }

    fn revalidate(&self, c: &Arc<Card>, gs: &Arc<GpuStat>) {
        lg(format!("{} periodic re-validation at {} (mining on this GPU pauses for a short while)", c.label(), signed(c.cur.load(Ordering::Relaxed))));
        self.recheck(c, gs, "re-validation");
    }

    /// Quick check of the current offset with mining on this GPU paused; on failure one step back and again.
    fn recheck(&self, c: &Arc<Card>, gs: &Arc<GpuStat>, what: &str) {
        // RAM for the check first: while other GPUs' checks fill the RAM this GPU keeps mining instead of idling paused
        let _slot = self.check_slot(c);
        gs.pause.store(true, Ordering::SeqCst);
        let t = Instant::now();
        while !gs.paused.load(Ordering::SeqCst) && t.elapsed() < Duration::from_secs(60) && !self.stopped() {
            std::thread::sleep(Duration::from_millis(100));
        }
        if gs.paused.load(Ordering::SeqCst) {
            let e = self.entry(c);
            let refs = e.as_ref().map(|e| e.refs.clone()).unwrap_or_default();
            loop {
                let cur = c.cur.load(Ordering::Relaxed);
                if cur <= c.orig || !c.owned.load(Ordering::SeqCst) || self.stopped() {
                    break;
                }
                let r = self.run_check(c, &self.quick_spec(refs.clone()));
                if self.stopped() {
                    break;
                }
                if r.ok {
                    lg(format!("{} {what} at {}: ok ({} reruns, cpu ok {}), {}", c.label(), signed(cur), r.reruns, r.cpu_ok, fmt_w(r.watts)));
                    if c.cur.load(Ordering::Relaxed) == cur {
                        let mut e = self.entry(c).unwrap_or_else(|| self.new_entry(c));
                        // running below the saved result because of the limit (apply_clamped): the saved one stays as it is
                        if e.offset <= cur {
                            e.offset = cur;
                            e.verified_at = now();
                            self.save_entry(c, &e);
                        }
                    }
                    break;
                }
                lg(format!("{} {what} at {} FAILED ({})", c.label(), signed(cur), r.reason));
                self.step_back(c, &format!("{what} failed"));
            }
        } else {
            lg(format!("{} {what} skipped (the mining thread did not pause)", c.label()));
        }
        gs.pause.store(false, Ordering::SeqCst);
    }
}

/// What a saved result means under this run's limit (`max`: --uv-max for the card, or its default).
#[derive(Debug, PartialEq)]
pub(crate) enum Saved {
    Apply,  // within the limit: apply it as saved
    Clamp,  // above the limit (the limit was lowered): run at the limit, keep the saved result for later
    Retune, // the saved result is the old limit itself (no failing step below it) and the limit is higher now
}

/// `cap`: the limit the saved result was tuned with (0 = unknown, results of 1.6.1 and older: never re-tuned for it).
pub(crate) fn saved_vs_limit(offset: i32, cap: i32, max: i32) -> Saved {
    if offset > max {
        Saved::Clamp
    } else if cap > 0 && max > cap && offset >= cap {
        Saved::Retune
    } else {
        Saved::Apply
    }
}

/// How a saved result's clocks compare with this run's (1.7.2).
#[derive(Debug, PartialEq)]
pub(crate) enum Regime {
    Same,
    Differs(String),
}

/// Key of a profile in uv.json "profiles"/<uuid>: the core lock ("1805"), "unlocked", or for a lock the miner did not set the
/// clock under load ("~1815"); "/mem810" when the miner also locked the memory clock.
fn profile_key(e: &Entry) -> String {
    let core = if e.lock != 0 {
        e.lock.to_string()
    } else if e.unlocked {
        "unlocked".to_string()
    } else if e.clk != 0 {
        format!("~{}", e.clk)
    } else {
        "?".to_string()
    };
    if e.mlock != 0 { format!("{core}/mem{}", e.mlock) } else { core }
}

/// Does profile `e` hold for the clocks now: the lock `lock` (and memory lock `mlock`) set by this run, or (lock unknown) the
/// clock under mining load `probe` = (MHz, at offset).
fn profile_fits(e: &Entry, lock: Option<u32>, mlock: Option<u32>, probe: Option<(u32, i32)>) -> bool {
    if let Some(m) = mlock {
        if e.mlock != 0 && e.mlock != m {
            return false;
        }
    }
    if let Some(l) = lock {
        return if e.lock != 0 { e.lock == l } else { !e.unlocked && e.clk != 0 && (e.clk as i64 - l as i64).abs() <= clk_tol(e.clk) };
    }
    match probe {
        Some((p, at)) => e.clk != 0 && regime_probe(e.clk, e.start.unwrap_or(at), p, at) == Regime::Same,
        None => false,
    }
}

/// The best profile for the clocks now: the exact lock first, a checked one before one from before 1.7.2, the nearest clock,
/// the most recently verified.
fn best_fit(v: &[Entry], lock: Option<u32>, mlock: Option<u32>, probe: Option<(u32, i32)>) -> Option<Entry> {
    let target = lock.or(probe.map(|p| p.0)).unwrap_or(0) as i64;
    v.iter()
        .filter(|e| profile_fits(e, lock, mlock, probe))
        .min_by_key(|e| (lock.is_some_and(|l| e.lock != l), e.migrated, (e.clk as i64 - target).abs(), u64::MAX - e.verified_at))
        .cloned()
}

/// Two profiles of the same clocks (a new result replaces the old one of its clocks only).
fn same_clocks(a: &Entry, b: &Entry) -> bool {
    if a.mlock != 0 && b.mlock != 0 && a.mlock != b.mlock {
        return false;
    }
    if a.lock != 0 && b.lock != 0 {
        return a.lock == b.lock;
    }
    if a.unlocked || b.unlocked {
        return a.unlocked && b.unlocked && a.lock == 0 && b.lock == 0;
    }
    let (ca, cb) = (if a.lock != 0 { a.lock + 10 } else { a.clk }, if b.lock != 0 { b.lock + 10 } else { b.clk });
    ca != 0 && cb != 0 && (ca as i64 - cb as i64).abs() <= clk_tol(ca)
}

/// uv.json before 1.7.2: one result per GPU ("gpus"), no lock recorded.  Each one with a clock under load becomes the profile of
/// that clock, marked migrated (used only after a check at these clocks); one without a clock is dropped.  Returns how many.
fn migrate(db: &mut Value) -> usize {
    let Some(old) = db["gpus"].as_object().cloned() else { return 0 };
    let mut n = 0;
    for (uuid, v) in old {
        let Some(mut e) = Entry::from_json(&v) else { continue };
        if e.clk == 0 {
            continue;
        }
        e.migrated = true;
        e.lock = 0;
        e.verified_at = 0;
        if !db["profiles"].is_object() {
            db["profiles"] = json!({});
        }
        if !db["profiles"][&uuid].is_object() {
            db["profiles"][&uuid] = json!({});
        }
        db["profiles"][&uuid][profile_key(&e)] = e.to_json();
        n += 1;
    }
    if let Some(m) = db.as_object_mut() {
        m.remove("gpus");
    }
    n
}

/// Allowed difference of two load clocks of the same regime (clock bins, lock 1805 reads 1815).
fn clk_tol(clk: u32) -> i64 {
    (clk as i64 * 3 / 100).max(45)
}

/// While mining at offset `at`: the load clock `probe` against the saved baseline clock `clk` at offset `start`.  Without a lock
/// the clock follows the offset (about 1 MHz per MHz), so the offset difference widens the tolerance; a lock change moves it by
/// hundreds of MHz (Rig01: 1410 when tuned, 1815 under the new lock).  A lower clock needs a bigger gap (power / thermal limits).
pub(crate) fn regime_probe(clk: u32, start: i32, probe: u32, at: i32) -> Regime {
    let tol = clk_tol(clk) + 15 + (start - at).abs() as i64;
    let d = probe as i64 - clk as i64;
    if d > tol {
        Regime::Differs(format!("{d:+} MHz"))
    } else if -d > tol + 90 {
        Regime::Differs(format!("{d:+} MHz"))
    } else {
        Regime::Same
    }
}

/// What a profile means for the owner's / starting offset `orig` at start.
#[derive(Debug, PartialEq)]
enum Choice {
    Lower,  // verified below `orig`: used instead of it (an unchecked start offset is never kept over a verified lower one)
    Paused, // a failure paused auto-uv at these clocks: the owner's offset stays
    Keep,   // the result is `orig` itself (it passed as the tuning's start)
    Higher, // an undervolt above `orig`: applied / checked as saved
}

fn start_choice(e: &Entry, orig: i32, t: u64) -> Choice {
    if e.offset < orig {
        Choice::Lower
    } else if e.disabled_until > t {
        Choice::Paused
    } else if e.offset == orig {
        Choice::Keep
    } else {
        Choice::Higher
    }
}

/// 0 = never (only after a step back)
fn reval_interval(hours: f64) -> Duration {
    if hours <= 0.0 { Duration::from_secs(10 * 365 * 24 * 3600) } else { Duration::from_secs_f64((hours * 3600.0).max(30.0)) }
}

pub(crate) fn fmt_w(w: Option<f64>) -> String {
    w.map_or("? W".to_string(), |w| format!("{w:.1} W"))
}

pub(crate) fn fmt_age(s: u64) -> String {
    if s >= 86400 {
        format!("{:.1} d", s as f64 / 86400.0)
    } else if s >= 3600 {
        format!("{:.1} h", s as f64 / 3600.0)
    } else {
        format!("{} min", s / 60)
    }
}

fn parse_result(line: &str, r: &mut Res) {
    let (head, reason) = match line.split_once(" reason=") {
        Some((h, rs)) => (h, rs.to_string()),
        None => (line, String::new()),
    };
    for t in head.split_whitespace().skip(1) {
        if let Some((k, v)) = t.split_once('=') {
            match k {
                "ok" => r.ok = v == "1",
                "reruns" => r.reruns = v.parse().unwrap_or(0),
                "cpu_ok" => r.cpu_ok = v.parse().unwrap_or(0),
                "refs" => r.refs = v.split(',').filter(|x| !x.is_empty()).map(String::from).collect(),
                _ => {}
            }
        }
    }
    r.reason = if r.ok { String::new() } else if reason.is_empty() { "unknown".into() } else { reason };
}

// ---------------------------------------------------------------------------------------------------------------------
// the check process (`freeforgempear --uv-check ...`, started by the tuner; not meant for users)
// ---------------------------------------------------------------------------------------------------------------------

pub struct ChildArgs {
    pub dev: i32,
    pub secs: u32,
    pub min_reruns: u32,
    pub cpu: u32,
    pub measure: u32,
    pub refs: Vec<String>,
    pub expect_bus: String,
}

/// Reruns of one seed queued back to back per library call.
const CHECK_REPS: u32 = 32;
/// Mining-mode attempts after each transcript batch.
const BURST: u32 = 48;

fn hex_digest(d: &[u64; 4]) -> String {
    format!("{:016x}{:016x}{:016x}{:016x}", d[0], d[1], d[2], d[3])
}

#[cfg(target_os = "linux")]
pub(crate) fn die_with_parent() {
    unsafe extern "C" {
        fn prctl(option: i32, arg2: u64, arg3: u64, arg4: u64, arg5: u64) -> i32;
    }
    unsafe { prctl(1, 9, 0, 0, 0) }; // PR_SET_PDEATHSIG, SIGKILL: no orphaned check after a kill -9 of the miner
}
#[cfg(not(target_os = "linux"))]
pub(crate) fn die_with_parent() {}

pub fn check_child(a: &ChildArgs) -> i32 {
    die_with_parent();
    match check_child_inner(a) {
        Ok((ok, line)) => {
            println!("{line}");
            if ok { 0 } else { 1 }
        }
        Err(e) => {
            println!("UVRESULT ok=0 reason=error: {}", format!("{e:#}").replace('\n', " "));
            2
        }
    }
}

/// CPU verification of a few hits of an easy-target attempt (the same path the miner uses for every share).
fn cpu_round(job: &Job, counter: u64, n: u32, m: usize) -> Result<u32> {
    let row0 = job.nonce_row(counter);
    let seed = job.a_noise_seed(&row0);
    let mut target = [0xffff_ffffu32; 8];
    target[7] = 0x00ff_ffff;
    let tgt = U256::from_little_endian(&crate::target_words_bytes(&target));
    let mut hits = vec![gpu::Hit::default(); 64];
    let (_, stored) = gpu::attempt(&seed, &[], &[], m, &target, &mut hits)?;
    let usable: Vec<&gpu::Hit> = hits[..stored].iter().filter(|h| h.row != 0).collect();
    ensure!(!usable.is_empty(), "no usable hit with the easy target");
    let mut ok = 0;
    for i in 0..(n as usize).min(usable.len()) {
        let h = usable[i * usable.len() / (n as usize).min(usable.len())];
        prove_hit(job, &row0, h, &seed, tgt).map_err(|e| anyhow::anyhow!("CPU verification of hit tile=({}, {}) jackpot={} failed: {e:#}", h.row, h.col, crate::hex(&hit_hash(h))))?;
        ok += 1;
    }
    Ok(ok)
}

/// `n` mining-mode attempts of one seed through the two-slot pipeline (what the miner does all day); returns the total hit count
/// reported by each attempt, which is deterministic for a given job and seed.
fn burst(seed: &[u8; 32], n: u32, target: &[u32; 8], m: usize) -> Result<Vec<usize>> {
    ensure!(n >= 2, "burst needs at least 2 attempts");
    let mut totals = Vec::with_capacity(n as usize);
    for i in 0..n {
        let slot = (i & 1) as i32;
        if i >= 2 {
            totals.push(gpu::wait(slot, &mut [])?.0);
        }
        gpu::launch(slot, seed, &[], &[], m, target)?;
    }
    for i in n - 2..n {
        totals.push(gpu::wait((i & 1) as i32, &mut [])?.0);
    }
    Ok(totals)
}

fn check_child_inner(a: &ChildArgs) -> Result<(bool, String)> {
    let count = gpu::device_count();
    ensure!((a.dev as usize) < count && a.dev >= 0, "device {} does not exist", a.dev);
    let bus = gpu::device_pci_bus_id(a.dev)?;
    ensure!(bus.eq_ignore_ascii_case(&a.expect_bus), "CUDA device {} is at {bus}, expected {}: refusing to run", a.dev, a.expect_bus);
    let cm = crate::mining::device_m(gpu::sm_count(a.dev)); // this card's m (its SM group); FFGP_M env inherited from the parent
    let job = Job::new(&test_header(7), 3, cm)?;
    gpu::init(a.dev)?;
    gpu::set_job(&job.br_t, &job.q1, &job.q2, N, k())?;
    // fixed seeds: the digests depend only on the job (from the fixed test header), the seed and the kernel, never on the process
    let seeds: Vec<[u8; 32]> = (0..4u32).map(|i| std::array::from_fn(|j| (j as u32 * 37 + i * 101 + 5) as u8)).collect();
    let mut refs: Vec<Option<String>> = (0..4).map(|i| a.refs.get(i).cloned()).collect();
    let mut hit_refs: [Option<usize>; 4] = [None; 4];
    let mut target = [0xffff_ffffu32; 8];
    target[7] = 0x00ff_ffff; // easy: thousands of hits per attempt, counted by the kernel; a changed count means a changed tile
    let t0 = Instant::now();
    println!("UVSTART");
    let mut cpu_ok = 0;
    let mut rounds_done = 0u32;
    let (mut reruns, mut mined) = (0u64, 0u64);
    let fail = |msg: String, reruns: u64, cpu_ok: u32| (false, format!("UVRESULT ok=0 reruns={reruns} cpu_ok={cpu_ok} secs={:.1} reason={msg}", t0.elapsed().as_secs_f64()));
    macro_rules! cpu {
        ($c:expr) => {
            match cpu_round(&job, $c, a.cpu, cm) {
                Ok(n) => cpu_ok += n,
                Err(e) => return Ok(fail(format!("{e:#}"), reruns, cpu_ok)),
            }
        };
    }
    macro_rules! mine_burst {
        ($s:expr, $n:expr) => {{
            let totals = burst(&seeds[$s], $n, &target, cm)?;
            mined += totals.len() as u64;
            for t in totals {
                match hit_refs[$s] {
                    None => hit_refs[$s] = Some(t),
                    Some(r) if r == t => {}
                    Some(r) => return Ok(fail(format!("rerun_diff: hit count of seed {} changed ({r} -> {t}) after {mined} mining attempts", $s), reruns, cpu_ok)),
                }
            }
        }};
    }
    cpu!(1000);
    let secs = Duration::from_secs(a.secs as u64);
    // phase 1: transcript batches (bit-exact, every tile) interleaved with bursts of mining-mode attempts (hit counts)
    loop {
        let el = t0.elapsed();
        if el >= secs && reruns >= a.min_reruns as u64 {
            break;
        }
        let s = ((reruns / CHECK_REPS as u64) % 4) as usize;
        let (dg, diff) = gpu::check_run(&seeds[s], cm, CHECK_REPS)?;
        if diff != 0 {
            return Ok(fail(format!("rerun_diff: {diff} transcript word(s) differ between reruns of seed {s} (after {reruns} reruns)"), reruns, cpu_ok));
        }
        let d = hex_digest(&dg);
        match &refs[s] {
            None => refs[s] = Some(d),
            Some(r) if *r == d => {}
            Some(r) => {
                return Ok(fail(format!("rerun_diff: transcript digest of seed {s} differs after {reruns} reruns (expected {}.., got {}..)", &r[..12], &d[..12]), reruns, cpu_ok));
            }
        }
        reruns += CHECK_REPS as u64;
        if std::env::var_os("FFGP_UV_NOBURST").is_none() {
            mine_burst!(s, BURST);
        }
        if rounds_done == 0 && el >= secs / 2 {
            rounds_done = 1;
            cpu!(1001);
        }
    }
    // phase 2: pure mining load for the power measurement (the parent samples NVML from the UVMEASURE line on)
    if a.measure > 0 {
        println!("UVMEASURE");
        let m0 = Instant::now();
        let mut i = 0usize;
        while m0.elapsed() < Duration::from_secs(a.measure as u64) {
            mine_burst!(i % 4, 200);
            i += 1;
        }
    }
    cpu!(1002);
    let refs: Vec<String> = refs.into_iter().map(|r| r.unwrap_or_default()).collect();
    Ok((true, format!("UVRESULT ok=1 reruns={reruns} mined={mined} cpu_ok={cpu_ok} secs={:.1} refs={}", t0.elapsed().as_secs_f64(), refs.join(","))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn per_gpu_parse() {
        assert_eq!(PerGpu::parse("495", "--uv-max").unwrap(), PerGpu::All(495));
        assert_eq!(PerGpu::parse("+225", "--uv-max").unwrap(), PerGpu::All(225));
        assert_eq!(PerGpu::parse(" 150 ", "--uv-max").unwrap(), PerGpu::All(150));
        assert_eq!(PerGpu::parse("-", "--uv-max").unwrap(), PerGpu::Unset);
        assert_eq!(PerGpu::parse("auto", "--uv-max").unwrap(), PerGpu::Unset);
        assert_eq!(PerGpu::parse("-100", "--uv-offset").unwrap(), PerGpu::All(-100));
        assert_eq!(
            PerGpu::parse("225,225,495,495,495,150", "--uv-max").unwrap(),
            PerGpu::List(vec![Some(225), Some(225), Some(495), Some(495), Some(495), Some(150)])
        );
        assert_eq!(PerGpu::parse("150, +330", "--uv-max").unwrap(), PerGpu::List(vec![Some(150), Some(330)]));
        assert_eq!(PerGpu::parse("225,-,150", "--uv-max").unwrap(), PerGpu::List(vec![Some(225), None, Some(150)]));
        assert_eq!(PerGpu::parse("225,,150", "--uv-max").unwrap(), PerGpu::List(vec![Some(225), None, Some(150)]));
        assert_eq!(PerGpu::parse("-50,-", "--uv-offset").unwrap(), PerGpu::List(vec![Some(-50), None]));
        for bad in ["", "abc", "1.5", "150,abc", "150;225", "150 225", "++150"] {
            assert!(PerGpu::parse(bad, "--uv-max").is_err(), "{bad:?} must be rejected");
        }
        assert!(format!("{:#}", PerGpu::parse("150,x", "--uv-max").unwrap_err()).contains("--uv-max: 'x'"));
    }

    #[test]
    fn per_gpu_get_and_count() {
        let l = PerGpu::parse("225,-,150", "--uv-max").unwrap();
        assert_eq!((l.get(0), l.get(1), l.get(2), l.get(3)), (Some(225), None, Some(150), None));
        assert!(l.is_set());
        assert_eq!(l.values(), vec![225, 150]);
        assert!(l.check_count(3, "--uv-max").is_ok());
        // a list must name every mining GPU: shorter or longer is an error, never extended
        let short = l.check_count(6, "--uv-max").unwrap_err();
        assert!(format!("{short:#}").contains("3 values for 6 mining GPU(s)"), "{short:#}");
        assert!(l.check_count(2, "--uv-max").is_err());
        // one value: every GPU, any count
        let one = PerGpu::All(300);
        assert!(one.check_count(1, "x").is_ok() && one.check_count(8, "x").is_ok());
        assert_eq!((one.get(0), one.get(7)), (Some(300), Some(300)));
        assert!(PerGpu::Unset.check_count(5, "x").is_ok());
        assert_eq!(PerGpu::Unset.get(0), None);
        assert!(!PerGpu::Unset.is_set());
        assert!(!PerGpu::List(vec![None, None]).is_set());
    }

    fn opts(max: &str, min: &str) -> Opts {
        let mut o = Opts { auto: true, ..Default::default() };
        if !max.is_empty() {
            o.max_offset = PerGpu::parse(max, "--uv-max").unwrap();
        }
        if !min.is_empty() {
            o.min_offset = PerGpu::parse(min, "--uv-min").unwrap();
        }
        o
    }

    #[test]
    fn opts_validate() {
        assert!(opts("", "").validate().is_ok());
        assert!(opts("495", "120").validate().is_ok());
        assert!(opts("150,225,495", "120,-,300").validate().is_ok());
        assert!(opts("601", "").validate().is_err()); // above HARD_CAP
        assert!(opts("150,700", "").validate().is_err());
        assert!(opts("10", "").validate().is_err()); // below one step
        assert!(opts("", "-5").validate().is_err());
        assert!(opts("200", "250").validate().is_err()); // min above max
        assert!(opts("150,495", "200").validate().is_err()); // min 200 above the first card's max 150
        assert!(opts("150,495", "-,200").validate().is_ok());
        assert!(opts("150,495", "100,500").validate().is_err());
        let mut o = opts("", "");
        o.fixed = PerGpu::All(200);
        assert!(o.validate().is_err(), "--auto-uv and --uv-offset exclude each other");
        o.auto = false;
        assert!(o.validate().is_ok() && o.enabled());
        o.fixed = PerGpu::parse("200,700", "--uv-offset").unwrap();
        assert!(o.validate().is_err());
        o.fixed = PerGpu::parse("-,-", "--uv-offset").unwrap();
        assert!(!o.enabled());
        let o = opts("150,225,495", "");
        assert!(o.check_counts(3).is_ok());
        assert!(o.check_counts(4).is_err());
    }

    #[test]
    fn defaults_by_card() {
        let t = |name: &str, cc: Option<(i32, i32)>| card_defaults(name, cc, None);
        // CMP 30HX / 40HX: 150
        for n in ["NVIDIA CMP 40HX", "NVIDIA CMP 30HX"] {
            assert_eq!(t(n, Some((7, 5))).max, 150, "{n}");
            assert_eq!(t(n, Some((7, 5))).max_src, "CMP default");
            assert_eq!(t(n, None).max, 150, "{n} without compute capability");
        }
        // CMP 50HX (TU102): 225, by name or by PCI device id 0x1e09
        assert_eq!(t("NVIDIA CMP 50HX", Some((7, 5))).max, 225);
        assert_eq!(t("NVIDIA CMP 50HX", Some((7, 5))).max_src, "CMP 50HX default");
        assert_eq!(t("NVIDIA CMP 50HX", None).max, 225);
        assert_eq!(card_defaults("NVIDIA Graphics Device", Some((7, 5)), Some(0x1e09)).max, 225);
        assert_eq!(card_defaults("NVIDIA CMP 40HX", Some((7, 5)), Some(0x1f0b)).max, 150); // TU106 id: still 150
        // Turing GeForce
        for n in ["NVIDIA GeForce RTX 2060 SUPER", "NVIDIA GeForce RTX 2080 Ti", "NVIDIA GeForce GTX 1660 Ti", "NVIDIA GeForce GTX 1650"] {
            let d = t(n, Some((7, 5)));
            assert_eq!((d.max, d.max_src, d.start_floor), (225, "Turing default", None), "{n}");
            assert_eq!(t(n, None).max, 225, "{n} by name only");
        }
        // sm_75 decides even when the name is unknown
        assert_eq!(t("Some Turing Board", Some((7, 5))).max, 225);
        // Ampere CMP and RTX 30: the default; a "CMP" on Ampere is not capped
        for (n, cc) in [
            ("NVIDIA CMP 90HX", Some((8, 6))),
            ("NVIDIA CMP 70HX", Some((8, 6))),
            ("NVIDIA CMP 90HX", None),
            ("NVIDIA GeForce RTX 3070", Some((8, 6))),
            ("NVIDIA GeForce RTX 3070 Laptop GPU", Some((8, 6))),
            ("NVIDIA GeForce RTX 3070", None),
        ] {
            let d = t(n, cc);
            assert_eq!((d.max, d.start_floor), (DEFAULT_MAX, None), "{n} {cc:?}");
        }
        // "cmp" inside another word is no CMP
        assert_eq!(t("NVIDIA Compute Board", None).max, DEFAULT_MAX);
        // RTX 40: start at +200, RTX 50: at +300, both up to 495
        let d = t("NVIDIA GeForce RTX 4070 Ti", Some((8, 9)));
        assert_eq!((d.max, d.start_floor, d.start_src), (495, Some(200), "RTX 40 default"));
        let d = t("NVIDIA GeForce RTX 5070", Some((12, 0)));
        assert_eq!((d.max, d.start_floor, d.start_src), (495, Some(300), "RTX 50 default"));
        // the mixed HiveOS rig of the report: 2x 2060 SUPER, 3x 3070 Laptop, CMP 40HX
        let rig = [
            ("NVIDIA GeForce RTX 2060 SUPER", (7, 5)),
            ("NVIDIA GeForce RTX 2060 SUPER", (7, 5)),
            ("NVIDIA GeForce RTX 3070 Laptop GPU", (8, 6)),
            ("NVIDIA GeForce RTX 3070 Laptop GPU", (8, 6)),
            ("NVIDIA GeForce RTX 3070 Laptop GPU", (8, 6)),
            ("NVIDIA CMP 40HX", (7, 5)),
        ];
        let caps: Vec<i32> = rig.iter().map(|(n, cc)| card_defaults(n, Some(*cc), None).max).collect();
        assert_eq!(caps, vec![225, 225, 495, 495, 495, 150]);
    }

    fn card(name: &str, cc: (i32, i32), cur: i32, pos: usize, o: &Opts) -> Card {
        let hw = CardHw { dev: pos as i32, name: name.into(), bus: format!("0000:0{pos}:00.0"), uuid: format!("GPU-{pos}"), nv: Dev(std::ptr::null_mut()), mode: OffMode::Gpc, cur, orig: cur, lo: -1000, hi: 1000, cc: Some(cc), pci_id: None, m: 0 };
        new_card(hw, pos, o)
    }

    #[test]
    fn card_range_per_gpu() {
        const CMP: (&str, (i32, i32)) = ("NVIDIA CMP 40HX", (7, 5));
        const TU: (&str, (i32, i32)) = ("NVIDIA GeForce RTX 2060 SUPER", (7, 5));
        const AMP: (&str, (i32, i32)) = ("NVIDIA GeForce RTX 3070 Laptop GPU", (8, 6));
        const BW: (&str, (i32, i32)) = ("NVIDIA GeForce RTX 5070", (12, 0));
        // --auto-uv alone: per-card defaults
        let o = opts("", "");
        let c = card(CMP.0, CMP.1, 0, 5, &o);
        assert_eq!((c.max, c.max_src, c.start()), (150, "CMP default", (120, "default")));
        let l = c.range_lines(true);
        assert_eq!(l.len(), 1, "{l:?}");
        assert!(l[0].ends_with("range +120..+150 (max: CMP default; start: default)"), "{}", l[0]);
        assert_eq!(card(TU.0, TU.1, 0, 0, &o).max, 225);
        let c50 = card("NVIDIA CMP 50HX", (7, 5), 0, 1, &o);
        assert!(c50.range_lines(true)[0].ends_with("range +120..+225 (max: CMP 50HX default; start: default)"), "{:?}", c50.range_lines(true));
        assert_eq!(card(AMP.0, AMP.1, 0, 2, &o).max, 495);
        // the HiveOS offset is the start
        assert_eq!(card(TU.0, TU.1, 150, 0, &o).start(), (150, "current offset"));
        // a HiveOS offset at the cap: nothing to tune (start > max is logged)
        let c = card(CMP.0, CMP.1, 200, 0, &o);
        assert!(c.range_lines(true).iter().any(|l| l.contains("is above the limit +150: this GPU is not tuned")));
        // RTX 50: start at +300 at least
        assert_eq!(card(BW.0, BW.1, 0, 0, &o).start(), (300, "RTX 50 default"));
        assert_eq!(card(BW.0, BW.1, 345, 0, &o).start(), (345, "RTX 50 default"));
        // one explicit --uv-max for all: used as given, with a warning on Turing/CMP
        let o = opts("495", "");
        let c = card(CMP.0, CMP.1, 0, 5, &o);
        assert_eq!((c.max, c.max_src), (495, "--uv-max"));
        let l = c.range_lines(true);
        assert!(l.iter().any(|l| l.contains("WARNING --uv-max +495 is above the recommended +150 for this card (CMP default)")), "{l:?}");
        assert!(!card(AMP.0, AMP.1, 0, 2, &o).range_lines(true).iter().any(|l| l.contains("WARNING")));
        // explicit, lower than the default: no warning
        assert!(!card(CMP.0, CMP.1, 0, 0, &opts("120", "")).range_lines(true).iter().any(|l| l.contains("WARNING")));
        // a list: by position; '-' = that card's default
        let o = opts("225,-,330", "");
        assert_eq!((card(TU.0, TU.1, 0, 0, &o).max, card(CMP.0, CMP.1, 0, 1, &o).max, card(AMP.0, AMP.1, 0, 2, &o).max), (225, 150, 330));
        assert_eq!(card(CMP.0, CMP.1, 0, 1, &o).max_src, "CMP default");
        // RTX 50 with a --uv-max below its default start: the +300 floor is dropped, it still tunes
        let o = opts("250", "");
        assert_eq!(card(BW.0, BW.1, 0, 0, &o).start(), (120, "default"));
        // --uv-min per card
        let o = opts("", "135,-");
        assert_eq!(card(AMP.0, AMP.1, 0, 0, &o).start(), (135, "--uv-min"));
        assert_eq!(card(AMP.0, AMP.1, 0, 1, &o).start(), (120, "default"));
        // --uv-min for all above a card's default limit: that card is not tuned (logged)
        let c = card(CMP.0, CMP.1, 0, 0, &opts("", "200"));
        assert!(c.range_lines(true).iter().any(|l| l.contains("this GPU is not tuned")));
        // the driver range cuts the limit, and the log says so
        let mut c = card(AMP.0, AMP.1, 0, 0, &opts("", ""));
        c.hi = 300;
        c.max = c.max_wanted.min(c.hi);
        assert!(c.range_lines(true)[0].contains("range +120..+300 (max: default, driver limit +300"), "{:?}", c.range_lines(true));
        // --uv-offset list
        let mut o = Opts::default();
        o.fixed = PerGpu::parse("200,-", "--uv-offset").unwrap();
        let c = card(CMP.0, CMP.1, 0, 0, &o);
        assert_eq!(c.fixed, Some(200));
        assert!(c.range_lines(false).iter().any(|l| l.contains("WARNING --uv-offset +200 is above the recommended +150")));
        let c = card(AMP.0, AMP.1, 0, 1, &o);
        assert_eq!(c.fixed, None);
        assert!(c.range_lines(false)[0].contains("no --uv-offset for this GPU, left alone"));
    }

    #[test]
    fn ram_gate_for_checks() {
        // the first check always runs; more only while the free RAM covers them (each running one counted in full)
        assert!(check_fits(0, 2400, Some(100)));
        assert!(check_fits(1, 2400, None));
        assert!(check_fits(1, 2400, Some(2400 + 2400 + CHECK_RESERVE_MB)));
        assert!(!check_fits(1, 2400, Some(2400 + 2400 + CHECK_RESERVE_MB - 1)));
        assert!(!check_fits(3, 2400, Some(8000)));
        assert!(check_fits(3, 2400, Some(3 * 2400 + 2400 + CHECK_RESERVE_MB)));
    }

    #[test]
    fn saved_result_under_a_new_limit() {
        // within the limit: applied
        assert_eq!(saved_vs_limit(270, 495, 495), Saved::Apply);
        assert_eq!(saved_vs_limit(270, 0, 495), Saved::Apply); // 1.6.1 entry (no cap saved)
        // the limit was lowered below the saved result: run at the limit
        assert_eq!(saved_vs_limit(315, 495, 225), Saved::Clamp);
        assert_eq!(saved_vs_limit(315, 0, 150), Saved::Clamp); // 1.6.1 entry: a lower limit is respected too
        // the result is the old limit itself and the limit went up: tune again
        assert_eq!(saved_vs_limit(225, 225, 495), Saved::Retune);
        // an edge was found below the old limit: a higher limit changes nothing
        assert_eq!(saved_vs_limit(210, 225, 495), Saved::Apply);
        // unknown old limit: never re-tuned for it
        assert_eq!(saved_vs_limit(225, 0, 495), Saved::Apply);
        assert_eq!(saved_vs_limit(150, 150, 150), Saved::Apply);
    }

    fn prof(lock: u32, clk: u32, offset: i32) -> Entry {
        Entry { driver: "d".into(), kernel: "k".into(), lock, clk, offset, start: Some(180), verified_at: 100, ..Default::default() }
    }

    #[test]
    fn profiles_per_lock() {
        let p1410 = prof(1400, 1410, 450);
        let p1805 = prof(1805, 1815, 300);
        let v = vec![p1410.clone(), p1805.clone()];
        // switching locks: each lock's own profile, the other one untouched
        assert_eq!(best_fit(&v, Some(1805), None, None).map(|e| e.offset), Some(300));
        assert_eq!(best_fit(&v, Some(1400), None, None).map(|e| e.offset), Some(450));
        assert!(best_fit(&v, Some(1600), None, None).is_none()); // a lock with no profile: tuned
        assert_eq!((profile_key(&p1410), profile_key(&p1805)), ("1400".to_string(), "1805".to_string()));
        // lock not set by the miner: the clock under mining load picks (Rig01: 1815 MHz under the HiveOS 1805 lock)
        assert_eq!(best_fit(&v, None, None, Some((1815, 275))).map(|e| e.offset), Some(300));
        assert_eq!(best_fit(&v, None, None, Some((1410, 325))).map(|e| e.offset), Some(450));
        assert!(best_fit(&v, None, None, Some((2040, 325))).is_none()); // unlocked: no profile yet
        // an unlocked profile follows the offset (about 1 MHz per MHz)
        let un = Entry { unlocked: true, ..prof(0, 2025, 225) };
        assert_eq!(profile_key(&un), "unlocked");
        assert!(profile_fits(&un, None, None, Some((2160, 325))));
        assert!(!profile_fits(&un, Some(1805), None, None));
        // the memory lock is part of the key when the miner set it
        let m = Entry { mlock: 810, ..prof(1805, 1815, 300) };
        assert_eq!(profile_key(&m), "1805/mem810");
        assert!(!profile_fits(&m, Some(1805), Some(5001), None));
        assert!(profile_fits(&m, Some(1805), Some(810), None));
        // a new result replaces only the profile of its own clocks
        assert!(same_clocks(&prof(1805, 1815, 300), &prof(1805, 1820, 315)));
        assert!(same_clocks(&prof(1805, 1815, 300), &prof(0, 1815, 300))); // measured under the same lock
        assert!(!same_clocks(&prof(1805, 1815, 300), &prof(1400, 1410, 450)));
        assert!(!same_clocks(&un, &prof(0, 2025, 225)));
    }

    #[test]
    fn migration_of_old_results() {
        // 1.7.1 uv.json: tuned under the ffgs leftover lock (1410 MHz), and one entry without a clock
        let old = Entry { driver: "d".into(), kernel: "k".into(), clk: 1410, offset: 495, verified_at: 5, ..Default::default() };
        let noclk = Entry { driver: "d".into(), kernel: "k".into(), offset: 300, ..Default::default() };
        let mut db = json!({"version": 1, "gpus": {"GPU-a": old.to_json(), "GPU-b": noclk.to_json()}});
        assert_eq!(migrate(&mut db), 1);
        assert!(db.get("gpus").is_none());
        let e = Entry::from_json(&db["profiles"]["GPU-a"]["~1410"]).unwrap();
        assert!(e.migrated && e.verified_at == 0);
        assert!(db["profiles"].get("GPU-b").is_none());
        // the old result fits the lock it was found at only, never the 1805 lock of the incident
        assert!(profile_fits(&e, Some(1400), None, None));
        assert!(!profile_fits(&e, Some(1805), None, None));
    }

    #[test]
    fn verified_offset_below_the_start_is_used() {
        let e = |offset, disabled_until| Entry { driver: "d".into(), kernel: "k".into(), offset, disabled_until, edge: 285, ..Default::default() };
        // Rig01 1.7.1 regression: results +270..+315 at 1805 MHz, start offset +325 (ffgs leftover) -> never "keeping it"
        for o in [270, 285, 315] {
            assert_eq!(start_choice(&e(o, 0), 325, 1000), Choice::Lower, "+{o} vs +325");
        }
        // with the HiveOS profile value as the owner's offset (+275): +270 lower, +315 an undervolt above it
        assert_eq!(start_choice(&e(270, 0), 275, 1000), Choice::Lower);
        assert_eq!(start_choice(&e(315, 0), 275, 1000), Choice::Higher);
        // a pause never keeps an offset above a verified lower one
        assert_eq!(start_choice(&e(270, 5000), 325, 1000), Choice::Lower);
        assert_eq!(start_choice(&e(315, 5000), 275, 1000), Choice::Paused);
        // the result is the start itself (it passed the baseline check)
        assert_eq!(start_choice(&e(275, 0), 275, 1000), Choice::Keep);
    }

    #[test]
    fn entry_cap_roundtrip() {
        let e = Entry { driver: "580".into(), kernel: "k".into(), offset: 150, cap: 150, orig: 0, ..Default::default() };
        let back = Entry::from_json(&e.to_json()).unwrap();
        assert_eq!((back.offset, back.cap), (150, 150));
        // an entry written by 1.6.1 has no "cap"
        let mut v = e.to_json();
        v.as_object_mut().unwrap().remove("cap");
        assert_eq!(Entry::from_json(&v).unwrap().cap, 0);
    }
}
