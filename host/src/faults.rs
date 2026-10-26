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
pub fn attribute(w: Option<&Watch>, dead: &[(i32, String)], alive: usize, injected: Option<i32>) -> Verdict {
    let mut xids: Vec<(usize, Vec<u64>)> = w.map(|w| w.take()).unwrap_or_default();
    if let (Some(w), Some(d)) = (w, injected) {
        if let Some(i) = w.by_dev(d) {
            if !xids.iter().any(|x| x.0 == i) {
                xids.push((i, vec![13]));
            }
        }
    }
    let mut lines = Vec::new();
    let mut culprits: Vec<(usize, u64)> = xids.iter().map(|(i, x)| (*i, x[0])).collect();
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
        // --uv-reset is for the start the owner made: the restart keeps what this run found (and the remembered faults)
        let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).filter(|a| a != "--uv-reset").collect();
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
    fn lower_steps() {
        // the incident: owner's +325 is an ffgs leftover, HiveOS profile +275
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
