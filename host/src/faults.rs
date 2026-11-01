// FreeForGem — see LICENSE
//! GPU faults while mining (1.7.2): which GPU failed, remembered across restarts.
//!
//! Why: a CUDA fault on one GPU (Xid 13 "Out Of Range Address", Xid 43, "an illegal memory access") ends the CUDA state of
//! the whole process: on the 8x RTX 3070 rig of 26.10.2026 one card (PCI 04) faulted and all 8 workers stopped with
//! "cudaEventSynchronize(sl.done): an illegal memory access" (the other seven had no Xid at all), the miner exited and HiveOS
//! restarted it 485 times into the same fault.  A worker thread cannot be restarted in such a process, so:
//!   * the failing GPU is found from the driver's Xid events (NVML, by device: exact even when every worker reports the same
//!     error), logged, and written to `gpu-faults.json` in the state directory with the offset it had (`record_crash`);
//!   * the next start (or the miner's own restart, `restart_self`) acts on it: auto-uv takes a crashed undervolt offset for a
//!     failed one and tunes below it; a crash at the owner's / kept offset lowers that offset after 2 crash starts in a row
//!     (the HiveOS profile value first, then 60 MHz steps down to 0); a GPU that still crashes with nothing left to lower is
//!     left out of mining for an hour so the other GPUs keep mining (`excluded`).
//!   * 30 min of mining without a fault clears the count (`mined_ok`).
use crate::nvml::{Dev, Nvml};
use crate::uv::{now, short_bus, short_name, signed, write_atomic};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Crash starts in a row at the owner's / kept offset before it is lowered (a single fault may be a fluke).
pub const LOWER_AFTER: u32 = 2;
/// How long a GPU with nothing left to lower is left out of mining.
pub const EXCLUDE_SECS: u64 = 3600;
/// A crash older than this does not count towards "in a row".
const STREAK_SECS: u64 = 3600;
/// Mining this long without a fault clears the count.
pub const OK_SECS: u64 = 30 * 60;
/// Restarts of the miner by itself within an hour before it leaves the restart to HiveOS / start.ps1.
const SELF_RESTARTS_MAX: usize = 12;

static LOCK: Mutex<()> = Mutex::new(());

fn path(dir: &Path) -> PathBuf {
    dir.join("gpu-faults.json")
}

fn load(dir: &Path) -> Value {
    std::fs::read_to_string(path(dir)).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()).filter(Value::is_object).unwrap_or_else(|| json!({"version": 1, "gpus": {}}))
}

fn store(dir: &Path, v: &Value) {
    if let Err(e) = write_atomic(&path(dir), &serde_json::to_string_pretty(v).unwrap()) {
        eprintln!("faults: cannot save {}: {e:#}", path(dir).display());
    }
}

/// What is remembered about one GPU (by UUID).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rec {
    pub bus: String,
    pub name: String,
    pub n: u32,        // crash starts in a row (within STREAK_SECS of each other)
    pub last: u64,     // time of the last crash
    pub xid: u64,      // its Xid (0 = none reported)
    pub offset: i32,   // core offset the GPU had when it crashed
    pub safe: Option<i32>, // offset auto-uv runs this GPU at instead of the owner's one (that one crashed)
    pub safe_orig: i32,    // the owner's offset `safe` replaces (another owner's offset = the owner changed the OC: forgotten)
    pub excluded_until: u64,
}

impl Rec {
    fn to_json(&self) -> Value {
        json!({"bus": self.bus, "name": self.name, "crashes_in_a_row": self.n, "last_crash": self.last, "xid": self.xid,
               "offset_at_crash": self.offset, "safe_offset": self.safe, "safe_replaces": self.safe_orig, "excluded_until": self.excluded_until})
    }
    fn from_json(v: &Value) -> Rec {
        Rec {
            bus: v["bus"].as_str().unwrap_or("").into(),
            name: v["name"].as_str().unwrap_or("").into(),
            n: v["crashes_in_a_row"].as_u64().unwrap_or(0) as u32,
            last: v["last_crash"].as_u64().unwrap_or(0),
            xid: v["xid"].as_u64().unwrap_or(0),
            offset: v["offset_at_crash"].as_i64().unwrap_or(0) as i32,
            safe: v["safe_offset"].as_i64().map(|x| x as i32),
            safe_orig: v["safe_replaces"].as_i64().unwrap_or(0) as i32,
            excluded_until: v["excluded_until"].as_u64().unwrap_or(0),
        }
    }
}

pub fn get(dir: &Path, uuid: &str) -> Option<Rec> {
    let _g = LOCK.lock().unwrap();
    load(dir)["gpus"].get(uuid).map(Rec::from_json)
}

pub fn put(dir: &Path, uuid: &str, r: &Rec) {
    let _g = LOCK.lock().unwrap();
    let mut v = load(dir);
    if !v["gpus"].is_object() {
        v["gpus"] = json!({});
    }
    v["gpus"][uuid] = r.to_json();
    store(dir, &v);
}

pub fn forget(dir: &Path, uuid: &str) {
    let _g = LOCK.lock().unwrap();
    let mut v = load(dir);
    if v["gpus"].as_object_mut().is_some_and(|m| m.remove(uuid).is_some()) {
        store(dir, &v);
    }
}

/// A crash of this GPU while mining; returns the record with the updated count.
pub fn record_crash(dir: &Path, uuid: &str, bus: &str, name: &str, xid: u64, offset: Option<i32>) -> Rec {
    let mut r = get(dir, uuid).unwrap_or_default();
    let t = now();
    r.n = if r.n > 0 && t.saturating_sub(r.last) <= STREAK_SECS { r.n + 1 } else { 1 };
    r.last = t;
    r.xid = xid;
    r.bus = bus.into();
    r.name = name.into();
    if let Some(o) = offset {
        r.offset = o;
    }
    put(dir, uuid, &r);
    r
}

/// Mining went on for OK_SECS without a fault: the crash count of these GPUs is cleared (a lowered offset stays).
pub fn mined_ok(dir: &Path, uuids: &[String]) {
    for u in uuids {
        if let Some(mut r) = get(dir, u) {
            if r.n > 0 {
                r.n = 0;
                if r.safe.is_none() && r.excluded_until <= now() {
                    forget(dir, u);
                } else {
                    put(dir, u, &r);
                }
            }
        }
    }
}

/// A process-wide fault that no GPU could be blamed for (no Xid): counted; returns how many in a row (within STREAK_SECS).  After
/// 3 the caller records a crash for every GPU (each one steps back from its offset at the next start): no endless loop.
pub fn unknown_crash(dir: &Path) -> u32 {
    let _g = LOCK.lock().unwrap();
    let mut v = load(dir);
    let t = now();
    let (n, last) = (v["unknown"]["n"].as_u64().unwrap_or(0) as u32, v["unknown"]["last"].as_u64().unwrap_or(0));
    let n = if n > 0 && t.saturating_sub(last) <= STREAK_SECS { n + 1 } else { 1 };
    v["unknown"] = json!({"n": n, "last": t});
    store(dir, &v);
    n
}

/// Seconds this GPU is still left out of mining (None: it mines).
pub fn excluded(dir: &Path, uuid: &str) -> Option<u64> {
    let r = get(dir, uuid)?;
    (r.excluded_until > now()).then(|| r.excluded_until - now())
}

/// The next lower offset after crashes at `cur` (the owner's / kept one): the HiveOS profile value when it is lower, else 60 MHz
/// lower (not below 0); None = nothing left to lower (at or below 0).
pub fn lower_step(cur: i32, hive: Option<i32>) -> Option<i32> {
    if let Some(h) = hive.filter(|&h| h < cur && h >= 0) {
        return Some(h);
    }
    (cur > 0).then(|| (cur - 60).max(0))
}

/// Text of a worker error that leaves the CUDA state of the process unusable (sticky errors).
pub fn sticky(err: &str) -> bool {
    let e = err.to_ascii_lowercase();
    ["illegal memory access", "illegal address", "unspecified launch failure", "misaligned address", "illegal instruction", "hardware stack error",
     "an exception occurred on the device", "launch timed out", "ecc error", "uncorrectable", "gpu has fallen off", "cuda_error_illegal",
     "invalid program counter", "context is destroyed", "injected fault"]
        .iter()
        .any(|k| e.contains(k))
}

/// Watches the mining GPUs for driver Xid events (read-only NVML), to tell which GPU failed.
pub struct Watch {
    nv: Nvml,
    pub cards: Vec<WCard>,
}

pub struct WCard {
    pub dev: i32,
    pub bus: String,
    pub uuid: String,
    pub name: String,
    h: Dev,
}

impl WCard {
    pub fn label(&self) -> String {
        format!("GPU{} (bus {}, {})", self.dev, short_bus(&self.bus), short_name(&self.name))
    }
}

impl Watch {
    /// `devs`: (CUDA device, PCI bus id, name).  None without NVML.
    pub fn new(devs: &[(i32, String, String)]) -> Option<Watch> {
        let nv = Nvml::load().ok()?;
        let mut cards = Vec::new();
        for (dev, bus, name) in devs {
            let Ok(h) = nv.by_bus(bus) else { continue };
            let uuid = nv.uuid(h).unwrap_or_else(|_| bus.clone());
            nv.watch_xid(h);
            cards.push(WCard { dev: *dev, bus: bus.clone(), uuid, name: name.clone(), h });
        }
        Some(Watch { nv, cards })
    }

    /// Critical Xids per card since the last call (index into `cards`).
    pub fn take(&self) -> Vec<(usize, Vec<u64>)> {
        self.cards.iter().enumerate().map(|(i, c)| (i, self.nv.take_xids(c.h))).filter(|(_, x)| !x.is_empty()).collect()
    }

    /// The core offset the card has now.
    pub fn offset(&self, i: usize) -> Option<i32> {
        self.nv.probe_offset(self.cards[i].h).ok().map(|p| p.1)
    }

    pub fn by_dev(&self, dev: i32) -> Option<usize> {
        self.cards.iter().position(|c| c.dev == dev)
    }
}

/// Test hook (tests only): FFG_TEST_FAULT=<dev>:<secs> makes the worker of CUDA device `dev` fail with a sticky-looking error
/// after `secs` seconds of mining and reports a fake Xid 13 for it, as if its kernel had faulted (FFG_TEST_FAULT_ALL=1: every
/// worker stops, as in a real CUDA fault).  Never set in normal use.
/// FFG_TEST_FAULT_UNTIL=<n>: only in the first n runs of a chain of the miner's own restarts.
pub fn test_fault() -> Option<(i32, u64)> {
    let v = std::env::var("FFG_TEST_FAULT").ok()?;
    if let Some(until) = std::env::var("FFG_TEST_FAULT_UNTIL").ok().and_then(|u| u.parse::<usize>().ok()) {
        let done = std::env::var("FFG_SELF_RESTARTS").unwrap_or_default().split(',').filter(|x| !x.is_empty()).count();
        if done >= until {
            return None;
        }
    }
    let (d, s) = v.split_once(':')?;
    Some((d.trim().parse().ok()?, s.trim().parse().ok()?))
}

/// The worker of `dev` that takes the injected fault: one worker only (`--devices 0,0` runs two workers on one GPU: one of them
/// fails, the other must keep mining), every worker with FFG_TEST_FAULT_ALL=1.  Returns the seconds of mining before it.
pub fn test_fault_for(dev: i32) -> Option<u64> {
    static TAKEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    let (d, secs) = test_fault()?;
    if test_fault_all() {
        return Some(secs);
    }
    (d == dev && !TAKEN.swap(true, std::sync::atomic::Ordering::SeqCst)).then_some(secs)
}

pub fn test_fault_all() -> bool {
    std::env::var("FFG_TEST_FAULT_ALL").is_ok_and(|v| v == "1")
}

/// What the main loop concluded about stopped workers.
pub struct Verdict {
    pub culprits: Vec<(usize, u64)>, // (card index in the watch, Xid)
    pub lines: Vec<String>,
}

/// Called by the main loop when workers stopped: waits a moment for the driver's Xid events and for the other workers, then
/// tells which GPU failed.  `dead`: (CUDA device, error text) of every stopped worker; `alive`: how many still mine.
pub fn attribute(w: Option<&Watch>, dead: &[(i32, String)], alive: usize, injected: Option<i32>, expected: &dyn Fn(i32) -> Option<i32>) -> Verdict {
    let mut xids: Vec<(usize, Vec<u64>)> = w.map(|w| w.take()).unwrap_or_default();
    if let (Some(w), Some(d)) = (w, injected) {
        if let Some(i) = w.by_dev(d) {
            if !xids.iter().any(|x| x.0 == i) {
                xids.push((i, vec![13]));
            }
        }
    }
    let mut lines: Vec<String> = Vec::new();
    let mut culprits: Vec<(usize, u64)> = xids.iter().map(|(i, x)| (*i, x[0])).collect();
    // 1.7.6: an Xid that came with another tool's change of the card (its offset is not the one we set, or an OC apply was seen on it
    // a moment ago) is not counted: it must not lower our offset at the next start
    if let Some(w) = w {
        culprits.retain(|&(i, x)| {
            let c = &w.cards[i];
            let foreign = foreign_offset(expected(c.dev), w.offset(i)) || foreign_near(&c.bus);
            if foreign {
                lines.push(format!("{} Xid {x}: another tool changed the clocks / offset of this GPU at the same time (HiveOS nvidia-oc, an OC tool): not counted as a failure of this miner's offset", c.label()));
            }
            !foreign
        });
    }
    let fatal = dead.iter().any(|(_, e)| sticky(e));
    if let Some(w) = w {
        for &(i, x) in &culprits {
            let others: Vec<String> = dead.iter().filter(|(d, _)| *d != w.cards[i].dev).map(|(d, _)| format!("GPU{d}")).collect();
            lines.push(format!(
                "{} FAILED: Xid {x} reported by the driver for this GPU (a GPU fault: offset/clock too high, or hardware){}",
                w.cards[i].label(),
                if others.is_empty() { String::new() } else { format!("; the workers of {} stopped with it: a CUDA fault ends CUDA for the whole process, they are not at fault", others.join(", ")) }
            ));
        }
        if culprits.is_empty() {
            // no Xid: the one GPU that stopped alone is the failing one; several without an Xid cannot be told apart
            if dead.len() == 1 {
                if let Some(i) = w.by_dev(dead[0].0) {
                    culprits.push((i, 0));
                    lines.push(format!("{} FAILED ({}); no Xid reported by the driver", w.cards[i].label(), dead[0].1));
                }
            } else {
                lines.push(format!("{} GPU workers stopped and the driver reported no Xid: the failing GPU cannot be told (see dmesg / nvidia-smi)", dead.len()));
            }
        }
    } else {
        lines.push("no NVML: the failing GPU cannot be told from the driver's Xid events".into());
    }
    if fatal && alive == 0 && dead.len() > 1 {
        lines.push("the CUDA state of this process is lost (a fault on one GPU stops every GPU of the process): the miner restarts, the failing GPU is handled at that start".into());
    } else if alive > 0 {
        lines.push(format!("{alive} GPU worker(s) keep mining; the stopped one(s) come back at the next miner start"));
    }
    Verdict { culprits, lines }
}

// ---------------------------------------------------------------------------------------------------------------------
// a clock / offset change by another tool is not a crash of our offset (ported from ffgs 0.5.4, Rig02: nvidia-oc LCLOCK 1680 over
// our 1500 + 330: Xid 43 in the window of the OC apply, counted as a failure of +330)
// ---------------------------------------------------------------------------------------------------------------------

/// A crash of a card whose core offset is not the one this miner set is not the miner's: someone (HiveOS nvidia-oc, a tuning tool)
/// changed the card's V/F state, and the crash belongs to that.  `expected`: the offset this miner set; `actual`: what NVML says now.
pub fn foreign_offset(expected: Option<i32>, actual: Option<i32>) -> bool {
    matches!((expected, actual), (Some(e), Some(a)) if e != a)
}

/// How near in time a detected OC apply must be to a crash to be blamed for it: on that card / on any card of the rig (nvidia-oc
/// sets the cards one after another).
pub const FOREIGN_CARD_SECS: u64 = 30;
pub const FOREIGN_RIG_SECS: u64 = 15;

static FOREIGN: Mutex<Vec<(String, std::time::Instant)>> = Mutex::new(Vec::new());

/// The tuners call this when they see an OC apply / another tool change the offset of the card at `bus` (their 30 s read-back).
pub fn note_foreign_change(bus: &str) {
    let mut g = FOREIGN.lock().unwrap();
    g.retain(|(_, t)| t.elapsed() < std::time::Duration::from_secs(600));
    g.push((bus.to_ascii_lowercase(), std::time::Instant::now()));
}

/// Did another tool change the card's state within `FOREIGN_CARD_SECS` of now (or any card's within `FOREIGN_RIG_SECS`)?
pub fn foreign_near(bus: &str) -> bool {
    let b = bus.to_ascii_lowercase();
    FOREIGN.lock().unwrap().iter().any(|(x, t)| {
        let age = t.elapsed().as_secs();
        age <= FOREIGN_RIG_SECS || (*x == b && age <= FOREIGN_CARD_SECS)
    })
}

/// Restarts the miner in the same process (Linux: exec, same PID and log pipe, HiveOS does not count a restart) after the caller
/// put the owner's offsets and clocks back.  Returns only when it cannot (Windows, or more than SELF_RESTARTS_MAX within an
/// hour): then the caller exits with an error and HiveOS / start.ps1 restart the miner as before.
pub fn restart_self(why: &str) -> String {
    let t = now();
    let mut hist: Vec<u64> = std::env::var("FFG_SELF_RESTARTS").unwrap_or_default().split(',').filter_map(|s| s.parse().ok()).filter(|&x: &u64| t.saturating_sub(x) < 3600).collect();
    if hist.len() >= SELF_RESTARTS_MAX {
        return format!("{} restarts by the miner itself within an hour: leaving the restart to HiveOS / start.ps1", hist.len());
    }
    hist.push(t);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let Ok(exe) = std::env::current_exe() else { return "cannot find the miner executable".into() };
        println!("{} restarting itself ({why}; restart {} within an hour)", crate::PRODUCT, hist.len());
        std::thread::sleep(std::time::Duration::from_millis(1500));
        let list = hist.iter().map(u64::to_string).collect::<Vec<_>>().join(",");
        let args = restart_args(std::env::args_os().skip(1));
        // the same argv[0] as before (HiveOS / scripts that look for "./freeforgem" still find the process)
        let arg0 = std::env::args_os().next().unwrap_or_else(|| exe.clone().into_os_string());
        let err = std::process::Command::new(&exe).arg0(arg0).args(args).env("FFG_SELF_RESTARTS", list).exec();
        return format!("restart failed: {err}");
    }
    #[allow(unreachable_code)]
    {
        let _ = why;
        "no in-process restart on this OS".into()
    }
}

/// One-shot flags: for the start the owner made, never for a restart by the miner itself (it keeps what this run found and the
/// remembered faults; with --uv-reset-all left in a flight sheet every crash restart wiped every profile -> a full retune loop).
/// The other flags are settings that hold for every start (--minutes restarts its count; --self-test / checks never restart).
#[cfg_attr(not(unix), allow(dead_code))]
pub const ONE_SHOT_FLAGS: [&str; 2] = ["--uv-reset", "--uv-reset-all"];

/// The arguments of a self-restart: the miner's own without the one-shot flags.
#[cfg_attr(not(unix), allow(dead_code))]
pub fn restart_args(args: impl Iterator<Item = std::ffi::OsString>) -> Vec<std::ffi::OsString> {
    args.filter(|a| !ONE_SHOT_FLAGS.iter().any(|f| a == f)).collect()
}

/// Linux: the OOM killer's preference for a process (`pid` None = this one): -1000..1000, higher = killed first.  Lowering it
/// needs root (the miner on HiveOS); a failure changes nothing.  1.7.3: check processes 900, the miner -500.
pub fn oom_score_adj(pid: Option<u32>, v: i32) {
    #[cfg(target_os = "linux")]
    {
        let p = pid.map_or("/proc/self/oom_score_adj".to_string(), |p| format!("/proc/{p}/oom_score_adj"));
        let _ = std::fs::write(p, v.to_string());
    }
    #[cfg(not(target_os = "linux"))]
    let _ = (pid, v);
}

/// One log line for a GPU left out of mining.
pub fn excluded_line(label: &str, r: &Rec, left: u64) -> String {
    format!(
        "{label} is LEFT OUT of mining for {} more min: it crashed {} start(s) in a row (last Xid {}, at offset {}) and there is no lower offset left to try; the other GPUs mine. Check its OC / riser / power; deleting gpu-faults.json in the state directory brings it back now (--uv-reset does not: it may stay in a flight sheet)",
        left / 60 + 1,
        r.n.max(LOWER_AFTER),
        r.xid,
        signed(r.offset)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restart_drops_one_shot_flags() {
        let a = |v: &[&str]| v.iter().map(std::ffi::OsString::from).collect::<Vec<_>>();
        let got = restart_args(a(&["--algo", "noid", "--uv-reset-all", "--auto-uv", "--uv-reset", "--lock-cclock", "1500"]).into_iter());
        assert_eq!(got, a(&["--algo", "noid", "--auto-uv", "--lock-cclock", "1500"]));
        // a value that only looks like a flag stays (the worker name of a flight sheet)
        assert_eq!(restart_args(a(&["--worker", "--uv-reset-rig"]).into_iter()), a(&["--worker", "--uv-reset-rig"]));
    }

    #[test]
    fn lower_steps() {
        // the incident: owner's +325 is a leftover of another miner, HiveOS profile +275
        assert_eq!(lower_step(325, Some(275)), Some(275));
        assert_eq!(lower_step(275, Some(275)), Some(215));
        assert_eq!(lower_step(40, None), Some(0));
        assert_eq!(lower_step(0, Some(0)), None);
        assert_eq!(lower_step(-50, None), None);
        assert_eq!(lower_step(200, Some(-100)), Some(140)); // a negative profile value is not a target
    }

    #[test]
    fn sticky_errors() {
        assert!(sticky("pg_wait: cudaEventSynchronize(sl.done): an illegal memory access was encountered"));
        assert!(sticky("CUDA_ERROR_ILLEGAL_ADDRESS"));
        assert!(!sticky("proof worker died"));
    }

    #[test]
    fn rec_roundtrip_and_streak() {
        let d = std::env::temp_dir().join(format!("ffg-faults-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let r = record_crash(&d, "GPU-x", "0000:04:00.0", "RTX 3070", 13, Some(325));
        assert_eq!((r.n, r.offset, r.xid), (1, 325, 13));
        let r = record_crash(&d, "GPU-x", "0000:04:00.0", "RTX 3070", 43, Some(325));
        assert_eq!(r.n, 2);
        let mut r2 = r.clone();
        r2.safe = Some(275);
        r2.safe_orig = 325;
        put(&d, "GPU-x", &r2);
        assert_eq!(get(&d, "GPU-x"), Some(r2.clone()));
        mined_ok(&d, &["GPU-x".into()]);
        let r3 = get(&d, "GPU-x").unwrap();
        assert_eq!((r3.n, r3.safe), (0, Some(275))); // the count goes, the lowered offset stays
        assert_eq!(excluded(&d, "GPU-x"), None);
        let _ = std::fs::remove_dir_all(&d);
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// whose Xid is it (1.7.6)
// ---------------------------------------------------------------------------------------------------------------------
//
// NVML's Xid event says which GPU, not which process.  On the Rig01 test of 28.10.2026 another miner (ffgs) on the same rig
// crashed GPUs and 1.7.3 wrote `faults: GPU1 crash 1 in a row at offset +300` for them: false records that the next start would
// have answered with -30 MHz.  The kernel log says it: `NVRM: Xid (PCI:0000:05:00): 43, pid=1234, name=ffgs, ...`.  Where it can be
// read (Linux, `dmesg`), an Xid whose line names a process that is not this miner (nor a child of it) is not this miner's: it is
// reported once and not counted (checks, crash records, step-backs).  Where it cannot be read, or the line has no pid
// (Xid 79 "fallen off the bus"), the Xid counts as before: no evidence of a foreign process = ours.

/// One `NVRM: Xid` line of the kernel log.
#[derive(Debug, PartialEq, Clone)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub struct XidLine {
    pub age_s: f64, // seconds before `uptime_s`
    pub pci: String,
    pub xid: u64,
    pub pid: Option<u32>,
    pub name: String,
}

/// Parses the `NVRM: Xid` lines of dmesg output (`[  1234.567890] NVRM: Xid (PCI:0000:05:00): 43, pid=1234, name=ffgs, ...`).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn parse_xid_lines(text: &str, uptime_s: f64) -> Vec<XidLine> {
    let mut out = Vec::new();
    for l in text.lines() {
        let Some(i) = l.find("NVRM: Xid (PCI:") else { continue };
        let t = l.trim_start().strip_prefix('[').and_then(|r| r.split_once(']')).and_then(|(t, _)| t.trim().parse::<f64>().ok());
        let rest = &l[i + "NVRM: Xid (PCI:".len()..];
        let Some((pci, rest)) = rest.split_once("): ") else { continue };
        let xid: u64 = rest.split(|c: char| !c.is_ascii_digit()).next().and_then(|x| x.parse().ok()).unwrap_or(0);
        if xid == 0 {
            continue;
        }
        let field = |k: &str| rest.find(k).map(|p| rest[p + k.len()..].split(',').next().unwrap_or("").trim().to_string());
        let pid = field("pid=").and_then(|p| p.trim_matches('\'').parse::<u32>().ok());
        let name = field("name=").unwrap_or_default();
        out.push(XidLine { age_s: t.map_or(0.0, |t| (uptime_s - t).max(0.0)), pci: pci.to_ascii_lowercase(), xid, pid, name });
    }
    out
}

/// Is a process named `name` (pid `pid`) this miner or something it started?
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn ours(pid: u32, name: &str, own_pid: u32, own_comm: &str, parent_of: &dyn Fn(u32) -> Option<u32>) -> bool {
    if pid == own_pid {
        return true;
    }
    // our own binary under any copy (the check child is the same binary), by the kernel's 15-character process name
    if !name.is_empty() && !own_comm.is_empty() && own_comm.starts_with(name.trim_matches('<').trim_matches('>')) && name.len() >= 4 {
        return true;
    }
    // a descendant of ours (the CMFD engine and its GPU worker, check processes)
    let mut p = pid;
    for _ in 0..8 {
        match parent_of(p) {
            Some(pp) if pp == own_pid => return true,
            Some(pp) if pp > 1 => p = pp,
            _ => break,
        }
    }
    false
}

/// For an Xid `xid` of the GPU at `pci` ("0000:05:00") seen just now: Some((pid, name)) when the kernel log says another process
/// caused it; None: ours, or no evidence.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn foreign_cause(lines: &[XidLine], pci: &str, xid: u64, own_pid: u32, own_comm: &str, parent_of: &dyn Fn(u32) -> Option<u32>) -> Option<(u32, String)> {
    let pci = pci.to_ascii_lowercase();
    // the newest line of this GPU and Xid within the last two minutes
    let l = lines.iter().filter(|l| l.pci == pci && l.xid == xid && l.age_s <= 120.0).min_by(|a, b| a.age_s.partial_cmp(&b.age_s).unwrap())?;
    let pid = l.pid?;
    if l.name.is_empty() || l.name.contains("unknown") {
        return None;
    }
    (!ours(pid, &l.name, own_pid, own_comm, parent_of)).then(|| (pid, l.name.clone()))
}

#[cfg(target_os = "linux")]
fn ppid(pid: u32) -> Option<u32> {
    let s = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // pid (comm) state ppid ...: comm may contain spaces and ')'
    let rest = &s[s.rfind(')')? + 2..];
    rest.split_whitespace().nth(1)?.parse().ok()
}

/// Reads the kernel log and tells whether the Xid `xid` of the GPU at (domain, bus, device) was caused by another process.  Linux
/// only (`dmesg`); anywhere else, or when the log cannot be read, None.
pub fn foreign_xid(pci: (u32, u32, u32), xid: u64) -> Option<(u32, String)> {
    #[cfg(target_os = "linux")]
    {
        let key = format!("{:04x}:{:02x}:{:02x}", pci.0, pci.1, pci.2);
        let own_comm: String = std::env::current_exe().ok()?.file_name()?.to_string_lossy().chars().take(15).collect();
        let up: f64 = std::fs::read_to_string("/proc/uptime").ok()?.split_whitespace().next()?.parse().ok()?;
        // the driver prints the line a moment before or after the NVML event: look twice
        for attempt in 0..2 {
            if attempt > 0 {
                std::thread::sleep(std::time::Duration::from_millis(300));
            }
            let out = std::process::Command::new("dmesg").output().ok()?;
            if !out.status.success() {
                return None;
            }
            let lines = parse_xid_lines(&String::from_utf8_lossy(&out.stdout), up);
            if lines.iter().any(|l| l.pci == key && l.xid == xid && l.age_s <= 120.0) {
                return foreign_cause(&lines, &key, xid, std::process::id(), &own_comm, &ppid);
            }
        }
        None
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (pci, xid);
        None
    }
}

#[cfg(test)]
mod xid_tests {
    use super::*;

    const LOG: &str = "\
[ 1000.100000] NVRM: Xid (PCI:0000:02:00): 43, pid=4242, name=freeforgem, Ch 00000016
[ 1000.200000] NVRM: Xid (PCI:0000:03:00): 43, pid=5555, name=ffgs, Ch 00000008
[ 1000.300000] NVRM: Xid (PCI:0000:05:00): 13, pid=5555, name=ffgs, Graphics SM Warp Exception on (GPC 0, TPC 1, SM 0): Out Of Range Address
[ 1000.400000] NVRM: Xid (PCI:0000:06:00): 79, pid='<unknown>', name=<unknown>, GPU has fallen off the bus.
[  800.000000] NVRM: Xid (PCI:0000:07:00): 43, pid=5555, name=ffgs, Ch 00000008
[ 1000.500000] some other line
";

    #[test]
    fn parse() {
        let v = parse_xid_lines(LOG, 1001.0);
        assert_eq!(v.len(), 5);
        assert_eq!((v[0].pci.as_str(), v[0].xid, v[0].pid, v[0].name.as_str()), ("0000:02:00", 43, Some(4242), "freeforgem"));
        assert_eq!(v[2].xid, 13);
        assert_eq!((v[3].xid, v[3].pid), (79, None));
        assert!((v[0].age_s - 0.9).abs() < 1e-6);
        assert!((v[4].age_s - 201.0).abs() < 1e-6);
    }

    #[test]
    fn whose() {
        let v = parse_xid_lines(LOG, 1001.0);
        let no_parent = |_: u32| None;
        let tree = |p: u32| if p == 7777 { Some(4242) } else { None };
        // our own binary (same process name), as the kernel prints it (15 characters at most)
        assert_eq!(foreign_cause(&v, "0000:02:00", 43, 4242, "freeforgem", &no_parent), None);
        assert_eq!(foreign_cause(&v, "0000:02:00", 43, 1, "freeforgem", &no_parent), None); // our name, another copy of us: not "foreign"
        // another miner on the same rig: not ours
        assert_eq!(foreign_cause(&v, "0000:03:00", 43, 4242, "freeforgem", &no_parent), Some((5555, "ffgs".into())));
        assert_eq!(foreign_cause(&v, "0000:05:00", 13, 4242, "freeforgem", &no_parent), Some((5555, "ffgs".into())));
        // no pid (fallen off the bus): counts as ours
        assert_eq!(foreign_cause(&v, "0000:06:00", 79, 4242, "freeforgem", &no_parent), None);
        // older than two minutes, or another Xid number: no evidence
        assert_eq!(foreign_cause(&v, "0000:07:00", 43, 4242, "freeforgem", &no_parent), None);
        assert_eq!(foreign_cause(&v, "0000:03:00", 13, 4242, "freeforgem", &no_parent), None);
        // a child of ours (the CMFD worker) is ours whatever its name
        let w = parse_xid_lines("[ 10.0] NVRM: Xid (PCI:0000:0a:00): 13, pid=7777, name=cmfd-v4-replay, x\n", 11.0);
        assert_eq!(foreign_cause(&w, "0000:0a:00", 13, 4242, "freeforgem", &tree), None);
        assert_eq!(foreign_cause(&w, "0000:0a:00", 13, 9999, "freeforgem", &tree), Some((7777, "cmfd-v4-replay".into())));
    }
}

#[cfg(test)]
mod foreign_tests {
    use super::*;

    #[test]
    fn foreign_changes_are_not_crashes() {
        assert!(foreign_offset(Some(330), Some(0)));
        assert!(!foreign_offset(Some(330), Some(330)));
        assert!(!foreign_offset(None, Some(0)));
        assert!(!foreign_offset(Some(330), None));
        assert!(!foreign_near("0000:05:00.0"));
        note_foreign_change("0000:05:00.0");
        assert!(foreign_near("0000:05:00.0")); // that card
        assert!(foreign_near("0000:06:00.0")); // any card of the rig within 15 s
    }
}
