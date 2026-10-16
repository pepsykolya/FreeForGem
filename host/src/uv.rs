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
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const STEP: i32 = 15;
pub const HARD_CAP: i32 = 600; // never above this, whatever --uv-max / --uv-offset say (the driver range c.hi caps it too)
const DISABLE_SECS: u64 = 24 * 3600;

#[derive(Clone, Debug)]
pub struct Opts {
    pub auto: bool,
    pub fixed: Option<i32>,
    pub dry_run: bool,
    pub reset: bool,
    pub max_offset: i32,
    pub min_offset: Option<i32>, // --uv-min: start/floor of the search (default: the HiveOS/NVML offset, else 120)
    pub step: i32,               // --uv-step: MHz per step (default 15)
    pub keep_last_good: bool,    // --uv-keep-last: after a failed step keep the last good step (not last good - one step)
    pub dir: Option<PathBuf>,
    pub reval_hours: f64,
    pub warmup_secs: u32,
    pub step_secs: u32,
    pub confirm_secs: u32,
}

impl Default for Opts {
    fn default() -> Opts {
        Opts { auto: false, fixed: None, dry_run: false, reset: false, max_offset: 495, min_offset: None, step: STEP, keep_last_good: false, dir: None, reval_hours: 6.0, warmup_secs: 180, step_secs: 30, confirm_secs: 120 }
    }
}

impl Opts {
    pub fn enabled(&self) -> bool {
        self.auto || self.fixed.is_some()
    }
}

fn lg(msg: impl AsRef<str>) {
    println!("auto-uv: {}", msg.as_ref());
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn signed(v: i32) -> String {
    format!("{v:+}")
}

/// "0000:0D:00.0" -> "0d"
fn short_bus(bus: &str) -> String {
    bus.split(':').nth(1).unwrap_or(bus).to_ascii_lowercase()
}

fn short_name(n: &str) -> String {
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

fn writable_dir(p: &Path) -> bool {
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
    home.join("FreeForGemPear")
}

fn write_atomic(path: &Path, data: &str) -> Result<()> {
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
    m: usize, // rows of A per attempt the transcript digests were taken at
}

impl Entry {
    fn to_json(&self) -> Value {
        json!({"name": self.name, "bus": self.bus, "driver": self.driver, "kernel": self.kernel, "orig_offset": self.orig,
               "offset": self.offset, "clk_mhz": self.clk, "watts_before": self.w_before, "watts_after": self.w_after,
               "tested_at": self.tested_at, "verified_at": self.verified_at, "disabled_until": self.disabled_until, "refs": self.refs, "m": self.m})
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
        })
    }
}

fn results_path(dir: &Path) -> PathBuf {
    dir.join("uv.json")
}

fn restore_path(dir: &Path) -> PathBuf {
    dir.join("uv-restore.json")
}

fn load_db(dir: &Path) -> Value {
    std::fs::read_to_string(results_path(dir)).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()).filter(Value::is_object).unwrap_or_else(|| json!({"version": 1, "gpus": {}}))
}

/// `--uv-reset`: forget saved results (the restore file is not touched).
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
        return exe.is_some_and(|p| p.to_string_lossy().contains("freeforgempear"));
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
    lg(format!("a previous run did not exit cleanly ({}): restoring the owner's offset", rp.display()));
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
        let res: Result<String> = (|| {
            let d = nv.by_bus(bus)?;
            ensure!(nv.uuid(d)? == uuid, "UUID at bus {bus} differs from the saved one");
            let (mode, cur, _, _) = nv.probe_offset(d)?;
            if cur != orig {
                nv.set_offset(d, mode, orig)?;
                std::thread::sleep(Duration::from_millis(150));
                let rb = nv.get_offset(d, mode)?;
                ensure!(rb == orig, "read-back {rb} != {orig}");
            }
            Ok(format!("offset {} -> {} restored", signed(cur), signed(orig)))
        })();
        match res {
            Ok(m) => lg(format!("(bus {}) {m}", short_bus(bus))),
            Err(e) => {
                all_ok = false;
                lg(format!("(bus {}) restore FAILED: {e:#}", short_bus(bus)));
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
    m: usize, // rows of A this card mines (its SM group); the undervolt check must stress it at this m
    cur: AtomicI32,
    owned: AtomicBool, // we still own the setting (false after tuning gave up or somebody else changed it)
    bad_seen: Mutex<u64>,
    reval_busy: AtomicBool,
    next_reval: Mutex<Instant>,
    next_readback: Mutex<Instant>,
}

impl Card {
    fn label(&self) -> String {
        format!("GPU{} (bus {}, {})", self.dev, short_bus(&self.bus), short_name(&self.name))
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

static ACTIVE: OnceLock<Mutex<Option<Arc<Manager>>>> = OnceLock::new();

fn active() -> &'static Mutex<Option<Arc<Manager>>> {
    ACTIVE.get_or_init(|| Mutex::new(None))
}

/// Puts the owner's offsets back (idempotent).  Called on exit, on a second interrupt and from the guard's Drop.
pub fn restore_all() {
    let m = active().lock().unwrap().clone();
    if let Some(m) = m {
        m.restore();
    }
}

pub struct Guard;
impl Drop for Guard {
    fn drop(&mut self) {
        restore_all();
    }
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
    if let Some(f) = o.fixed {
        if !(-1000..=HARD_CAP).contains(&f) {
            lg(format!("--uv-offset {f} is outside the allowed range (up to +{HARD_CAP}); undervolting disabled"));
            return None;
        }
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
    let kernel = gpu::kernel_label();
    let total = nv.device_count().unwrap_or(0);
    lg(format!("NVML loaded, driver {driver}, kernel {kernel}, {total} GPU(s) in the system; only the {} mining device(s) below are touched", devices.len()));
    let mut cards = Vec::new();
    for &dev in devices {
        match map_card(&nv, dev) {
            Ok(c) => {
                let idx = nv.index(c.nv).map_or("?".to_string(), |i| i.to_string());
                lg(format!("{}: CUDA device {dev} -> NVML index {idx}, PCI {}, {}, current offset {} (allowed {}..{})", c.label(), c.bus, c.uuid, signed(c.orig), c.lo, c.hi));
                cards.push(Arc::new(c));
            }
            Err(e) => lg(format!("GPU{dev}: cannot manage ({e:#}); left alone")),
        }
    }
    if cards.is_empty() {
        lg("no GPU can be managed (V/F offset unavailable or no permission?); mining at the current clocks and voltage");
        return None;
    }
    if o.dry_run {
        for c in &cards {
            let start = o.min_offset.unwrap_or(if c.orig != 0 { c.orig } else { 120 });
            let first = start + o.step;
            lg(format!("DRY RUN: would change {} (NVML handle of PCI {}): start {}, {} -> {} ... up to {}; nothing is changed", c.label(), c.bus, signed(start), signed(start), signed(first), signed(o.max_offset.min(c.hi))));
        }
        return None;
    }
    // permission probe: write the value the GPU already has (changes nothing); NVML refuses it without root/administrator rights
    cards.retain(|c| match nv.set_offset(c.nv, c.mode, c.orig) {
        Ok(()) => true,
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
        db: Mutex::new(load_db(&dir)),
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
    std::thread::scope(|s| {
        for c in &m.cards {
            let (m, c) = (m.clone(), c.clone());
            s.spawn(move || {
                if let Err(e) = m.run_card(&c) {
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

fn map_card(nv: &Nvml, dev: i32) -> Result<Card> {
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
    Ok(Card {
        dev,
        name,
        bus,
        uuid,
        nv: h,
        mode,
        orig: cur,
        lo,
        hi,
        m: crate::mining::device_m(gpu::sm_count(dev)),
        cur: AtomicI32::new(cur),
        owned: AtomicBool::new(true),
        bad_seen: Mutex::new(0),
        reval_busy: AtomicBool::new(false),
        next_reval: Mutex::new(Instant::now()),
        next_readback: Mutex::new(Instant::now() + Duration::from_secs(30)),
    })
}

impl Manager {
    fn set_checked(&self, c: &Card, v: i32) -> Result<()> {
        ensure!(v >= c.lo && v <= c.hi, "offset {v} outside the driver's range {}..{}", c.lo, c.hi);
        ensure!(self.nv_target_ok(c), "internal: handle check failed");
        self.nv.set_offset(c.nv, c.mode, v)?;
        std::thread::sleep(Duration::from_millis(150));
        let rb = self.nv.get_offset(c.nv, c.mode)?;
        ensure!(rb == v, "read-back {rb} differs from the value just set ({v})");
        c.cur.store(v, Ordering::Relaxed);
        Ok(())
    }

    /// The handle must still be the GPU at the bus id we mapped (cheap guard against any mix-up).
    fn nv_target_ok(&self, c: &Card) -> bool {
        match (self.nv.pci(c.nv), parse_bus(&c.bus)) {
            (Ok(p), Some(b)) => p == b,
            _ => false,
        }
    }

    fn restore_card(&self, c: &Card) {
        if !c.owned.swap(false, Ordering::SeqCst) {
            return;
        }
        let cur = c.cur.load(Ordering::Relaxed);
        for attempt in 0..3 {
            match self.set_checked(c, c.orig) {
                Ok(()) => {
                    lg(format!("{} offset {} -> {} restored", c.label(), signed(cur), signed(c.orig)));
                    return;
                }
                Err(e) => {
                    lg(format!("{} restore attempt {} failed: {e:#}", c.label(), attempt + 1));
                    std::thread::sleep(Duration::from_millis(500));
                }
            }
        }
        lg(format!("{} COULD NOT RESTORE the offset {}; the restore file stays and is applied at the next start", c.label(), signed(c.orig)));
        c.owned.store(true, Ordering::SeqCst);
    }

    pub fn restore(&self) {
        if self.restored.swap(true, Ordering::SeqCst) {
            return;
        }
        for c in &self.cards {
            self.restore_card(c);
        }
        if self.cards.iter().all(|c| !c.owned.load(Ordering::SeqCst)) {
            let _ = std::fs::remove_file(restore_path(&self.dir));
        } else {
            self.restored.store(false, Ordering::SeqCst);
        }
    }

    fn entry(&self, c: &Card) -> Option<Entry> {
        let db = self.db.lock().unwrap();
        db["gpus"].get(&c.uuid).and_then(Entry::from_json)
    }

    fn save_entry(&self, c: &Card, e: &Entry) {
        let mut db = self.db.lock().unwrap();
        db["version"] = json!(1);
        if !db["gpus"].is_object() {
            db["gpus"] = json!({});
        }
        db["gpus"][&c.uuid] = e.to_json();
        if let Err(err) = write_atomic(&results_path(&self.dir), &serde_json::to_string_pretty(&*db).unwrap()) {
            lg(format!("{} cannot save {}: {err:#}", c.label(), results_path(&self.dir).display()));
        }
    }

    fn new_entry(&self, c: &Card) -> Entry {
        Entry { name: c.name.clone(), bus: c.bus.clone(), driver: self.driver.clone(), kernel: self.kernel.clone(), orig: c.orig, ..Default::default() }
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    // ----- per-card flow at start ---------------------------------------------------------------------------------

    fn run_card(&self, c: &Arc<Card>) -> Result<()> {
        if let Some(f) = self.opts.fixed {
            return self.apply_fixed(c, f);
        }
        let saved = self.entry(c);
        if let Some(e) = &saved {
            if e.driver != self.driver || e.kernel != self.kernel {
                lg(format!("{} saved result is for driver {} / kernel {}, now driver {} / kernel {}: tuning again", c.label(), e.driver, e.kernel, self.driver, self.kernel));
            } else if e.orig != c.orig {
                lg(format!("{} the starting offset changed ({} when tuned, {} now): tuning again", c.label(), signed(e.orig), signed(c.orig)));
            } else if e.disabled_until > now() {
                lg(format!("{} auto-uv is paused for this GPU after a failure ({} h left); keeping {}", c.label(), (e.disabled_until - now()) / 3600 + 1, signed(c.orig)));
                return Ok(());
            } else if e.offset <= c.orig {
                lg(format!("{} no safe offset above {} was found when tuned; keeping it (--uv-reset to tune again)", c.label(), signed(c.orig)));
                return Ok(());
            } else {
                return self.apply_saved(c, e);
            }
        }
        self.tune(c, saved)
    }

    fn quick_spec(&self, refs: Vec<String>) -> Spec {
        Spec { secs: 20, min_reruns: 30, cpu: 2, measure: 8, refs }
    }

    fn apply_fixed(&self, c: &Card, f: i32) -> Result<()> {
        let f = f.min(HARD_CAP);
        let before = c.cur.load(Ordering::Relaxed);
        if f == before {
            lg(format!("{} already at {}", c.label(), signed(f)));
            return Ok(());
        }
        self.set_checked(c, f)?;
        std::thread::sleep(Duration::from_secs(3));
        let r = self.run_check(c, &self.quick_spec(vec![]));
        if r.ok {
            lg(format!("{} offset {} -> {}, {} (fixed by --uv-offset), check passed ({} reruns, cpu ok {})", c.label(), signed(before), signed(f), fmt_w(r.watts), r.reruns, r.cpu_ok));
        } else {
            lg(format!("{} fixed offset {} FAILED the check ({}); back to {}", c.label(), signed(f), r.reason, signed(c.orig)));
            self.restore_card(c);
        }
        Ok(())
    }

    fn apply_saved(&self, c: &Card, e: &Entry) -> Result<()> {
        let before = c.cur.load(Ordering::Relaxed);
        self.set_checked(c, e.offset)?;
        std::thread::sleep(Duration::from_secs(3));
        let r = self.run_check(c, &self.quick_spec(e.refs.clone()));
        if r.ok {
            lg(format!("{} offset {} -> {}, saved {} ago ({:.1} W at tuning), quick check passed ({} reruns, cpu ok {})", c.label(), signed(before), signed(e.offset), fmt_age(now().saturating_sub(e.tested_at)), e.w_after, r.reruns, r.cpu_ok));
            let mut e2 = e.clone();
            e2.verified_at = now();
            self.save_entry(c, &e2);
        } else {
            lg(format!("{} saved offset {} FAILED the quick check ({}); back to {}, auto-uv paused for 24 h", c.label(), signed(e.offset), r.reason, signed(c.orig)));
            self.restore_card(c);
            let mut e2 = e.clone();
            e2.disabled_until = now() + DISABLE_SECS;
            self.save_entry(c, &e2);
        }
        Ok(())
    }

    fn tune(&self, c: &Arc<Card>, prev: Option<Entry>) -> Result<()> {
        let step = self.opts.step;
        let cap = self.opts.max_offset.min(HARD_CAP).min(c.hi);
        // search base/floor: --uv-min, else the offset HiveOS/NVML already has, else 120; never above the cap
        let start = self.opts.min_offset.unwrap_or(if c.orig != 0 { c.orig } else { 120 }).clamp(0, cap);
        if start + step > cap {
            lg(format!("{} start offset {} leaves no room below the cap {}; nothing to do", c.label(), signed(start), signed(cap)));
            return Ok(());
        }
        lg(format!("{} tuning: {} s warm-up at {}, then +{step} MHz steps up to {} ({} s each)", c.label(), self.opts.warmup_secs, signed(start), signed(cap), self.opts.step_secs));
        // the transcript digests depend on m too: a reference saved at another m must not seed the check
        let golden = prev.as_ref().filter(|e| e.driver == self.driver && e.kernel == self.kernel && e.m == c.m)
            .map(|e| e.refs.clone()).unwrap_or_default();
        // the baseline is measured at the start offset, which may differ from the card's current one
        if start != c.cur.load(Ordering::Relaxed) {
            self.set_checked(c, start).context("GPU does not accept the start offset (--uv-min)")?;
            std::thread::sleep(Duration::from_secs(3));
        }
        let base = self.run_check(c, &Spec { secs: self.opts.warmup_secs, min_reruns: 60, cpu: 2, measure: 20, refs: golden.clone() });
        if self.stopped() {
            return Ok(());
        }
        if !base.ok {
            lg(format!("{} the start offset {} fails the self-check ({}); not tuning, restoring {}. Lower the start with --uv-min N (or --uv-offset N for a fixed one); if the saved reference is stale: --uv-reset", c.label(), signed(start), base.reason, signed(c.orig)));
            let _ = self.set_checked(c, c.orig);
            return Ok(());
        }
        let refs = if golden.is_empty() { base.refs.clone() } else { golden };
        let base_w = base.watts;
        lg(format!("{} baseline {}: ok, {}, {} MHz, {} C", c.label(), signed(start), fmt_w(base_w), base.clk.unwrap_or(0), base.temp.unwrap_or(0)));
        let mut good = start;
        let mut good_w = base_w;
        let mut failed: Option<(i32, String)> = None;
        while good + step <= cap {
            let cand = good + step;
            self.set_checked(c, cand)?;
            std::thread::sleep(Duration::from_secs(3));
            let r = self.run_check(c, &Spec { secs: self.opts.step_secs, min_reruns: 30, cpu: 2, measure: 12, refs: refs.clone() });
            if self.stopped() {
                return Ok(());
            }
            if r.ok {
                lg(format!("{} step {}: ok ({} reruns, cpu ok {}), {}, {} MHz, {} C", c.label(), signed(cand), r.reruns, r.cpu_ok, fmt_w(r.watts), r.clk.unwrap_or(0), r.temp.unwrap_or(0)));
                good = cand;
                good_w = r.watts.or(good_w);
            } else {
                lg(format!("{} step {}: FAILED ({})", c.label(), signed(cand), r.reason));
                failed = Some((cand, r.reason));
                break;
            }
        }
        // result when a failure was found: the last good step, minus one step of margin by default
        // (--uv-keep-last keeps the last good step itself); at the cap with no failure: the cap
        let mut fin = if failed.is_some() {
            if self.opts.keep_last_good { good } else { good - step }
        } else {
            good
        };
        if let Some((fo, _)) = &failed {
            // the GPU may need a moment (and must still answer) after a failed step
            std::thread::sleep(Duration::from_secs(5));
            let t = fin.max(start);
            self.set_checked(c, t).context("GPU does not accept the safe offset after the failed step")?;
            lg(format!("{} edge found at {}: stepping back to {}", c.label(), signed(*fo), signed(t)));
        }
        fin = fin.max(start);
        // confirmation with a longer check; on failure one more step down
        let mut conf_w = good_w;
        while fin > start {
            self.set_checked(c, fin)?;
            std::thread::sleep(Duration::from_secs(3));
            let r = self.run_check(c, &Spec { secs: self.opts.confirm_secs, min_reruns: 150, cpu: 4, measure: 20, refs: refs.clone() });
            if self.stopped() {
                return Ok(());
            }
            if r.ok {
                lg(format!("{} confirmation at {}: ok ({} reruns, cpu ok {}), {}", c.label(), signed(fin), r.reruns, r.cpu_ok, fmt_w(r.watts)));
                conf_w = r.watts.or(conf_w);
                break;
            }
            lg(format!("{} confirmation at {}: FAILED ({}); one step lower", c.label(), signed(fin), r.reason));
            fin = (fin - step).max(start);
        }
        if fin == start {
            self.set_checked(c, start)?;
            conf_w = base_w;
        }
        let mut e = self.new_entry(c);
        e.offset = fin;
        e.refs = refs;
        e.m = c.m;
        e.clk = base.clk.unwrap_or(0);
        e.w_before = base_w.unwrap_or(0.0);
        e.w_after = conf_w.unwrap_or(0.0);
        e.tested_at = now();
        e.verified_at = now();
        self.save_entry(c, &e);
        lg(format!("{} offset {} -> {}, {} -> {}, saved", c.label(), signed(start), signed(fin), fmt_w(base_w), fmt_w(conf_w)));
        Ok(())
    }

    // ----- check child ----------------------------------------------------------------------------------------------

    /// Runs the deterministic GPU check + CPU verification of hits in a child process and samples NVML meanwhile.
    fn run_check(&self, c: &Card, sp: &Spec) -> Res {
        match self.run_check_inner(c, sp) {
            Ok(r) => r,
            Err(e) => Res { reason: format!("{e:#}"), ..Default::default() },
        }
    }

    fn run_check_inner(&self, c: &Card, sp: &Spec) -> Result<Res> {
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
            res.reason = format!("check process ended without a result (exit {:?}){}", exit, {
                std::thread::sleep(Duration::from_millis(200));
                let t = errtail.lock().unwrap().clone();
                if t.is_empty() { String::new() } else { format!(": {t}") }
            });
        }
        if let Some(k) = killed {
            res.ok = false;
            res.reason = k.to_string();
        }
        if let Some(x) = xid {
            res.ok = false;
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
            // somebody else changed the offset: do not fight
            {
                let mut nr = c.next_readback.lock().unwrap();
                if Instant::now() >= *nr {
                    *nr = Instant::now() + Duration::from_secs(30);
                    if !c.reval_busy.load(Ordering::Relaxed) {
                        match self.nv.get_offset(c.nv, c.mode) {
                            Ok(v) if v != c.cur.load(Ordering::Relaxed) => {
                                lg(format!("{} the offset was changed by someone else ({} now, {} expected): auto-uv stands down on this GPU and leaves it as is", c.label(), signed(v), signed(c.cur.load(Ordering::Relaxed))));
                                c.owned.store(false, Ordering::SeqCst);
                                continue;
                            }
                            _ => {}
                        }
                    }
                }
            }
            let Some(gs) = gstats.iter().find(|g| g.dev == c.dev) else { continue };
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

    /// One step down (never below the owner's offset); below that: give up and restore.
    fn step_back(&self, c: &Card, why: &str) {
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

    fn revalidate(&self, c: &Arc<Card>, gs: &Arc<GpuStat>) {
        lg(format!("{} periodic re-validation at {} (mining on this GPU pauses for a short while)", c.label(), signed(c.cur.load(Ordering::Relaxed))));
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
                if r.ok {
                    lg(format!("{} re-validation at {}: ok ({} reruns, cpu ok {}), {}", c.label(), signed(cur), r.reruns, r.cpu_ok, fmt_w(r.watts)));
                    if c.cur.load(Ordering::Relaxed) == cur {
                        let mut e = self.entry(c).unwrap_or_else(|| self.new_entry(c));
                        e.offset = cur;
                        e.verified_at = now();
                        self.save_entry(c, &e);
                    }
                    break;
                }
                lg(format!("{} re-validation at {} FAILED ({})", c.label(), signed(cur), r.reason));
                self.step_back(c, "re-validation failed");
            }
        } else {
            lg(format!("{} re-validation skipped (the mining thread did not pause)", c.label()));
        }
        gs.pause.store(false, Ordering::SeqCst);
    }
}

/// 0 = never (only after a step back)
fn reval_interval(hours: f64) -> Duration {
    if hours <= 0.0 { Duration::from_secs(10 * 365 * 24 * 3600) } else { Duration::from_secs_f64((hours * 3600.0).max(30.0)) }
}

fn fmt_w(w: Option<f64>) -> String {
    w.map_or("? W".to_string(), |w| format!("{w:.1} W"))
}

fn fmt_age(s: u64) -> String {
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
fn die_with_parent() {
    unsafe extern "C" {
        fn prctl(option: i32, arg2: u64, arg3: u64, arg4: u64, arg5: u64) -> i32;
    }
    unsafe { prctl(1, 9, 0, 0, 0) }; // PR_SET_PDEATHSIG, SIGKILL: no orphaned check after a kill -9 of the miner
}
#[cfg(not(target_os = "linux"))]
fn die_with_parent() {}

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
