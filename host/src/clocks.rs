// FreeForGem — see LICENSE
//! Optional GPU clock control via NVML: lock the core clock, lock the memory clock and set a memory-clock
//! offset (`--lock-cclock` / `--lock-mclock` / `--moffset`).  Applied to the mining devices at start and
//! undone on exit (Ctrl-C/SIGTERM and after a crash, via a small marker file): a lock that was there before (HiveOS OC profile,
//! or $FFG_RESTORE_CCLOCK / $FFG_RESTORE_MCLOCK, also the FreeForGemPear names $FFGP_RESTORE_*) is put back, otherwise the clock is reset.  Needs root/Administrator.
//! Independent of `--auto-uv`; the kernel and mining are untouched.
use crate::gpu;
use crate::nvml::{Dev, Nvml};
use anyhow::Result;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Per mining device (in --devices order); `None` leaves that clock untouched.
#[derive(Clone, Default)]
pub struct ClockOpts {
    pub core_mhz: Vec<Option<u32>>,
    pub mem_mhz: Vec<Option<u32>>,
    pub mem_off: Vec<Option<i32>>,
}

impl ClockOpts {
    pub fn any(&self) -> bool {
        self.core_mhz.iter().any(Option::is_some)
            || self.mem_mhz.iter().any(Option::is_some)
            || self.mem_off.iter().any(Option::is_some)
    }
}

/// The lock a clock had before this run changed it.  NVML can set and reset a clock lock but cannot read one back, so it comes
/// from the HiveOS OC profile (`hiveoc`), or from $FFG_RESTORE_CCLOCK / $FFG_RESTORE_MCLOCK (or the FreeForGemPear names
/// $FFGP_RESTORE_CCLOCK / $FFGP_RESTORE_MCLOCK); without either it is unknown
/// and the lock is reset on exit (the behaviour before 1.6.3).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Prev {
    Unknown,
    Unlocked,
    Locked(u32),
}

impl Prev {
    fn to_json(self) -> serde_json::Value {
        match self {
            Prev::Unknown => json!(null),
            Prev::Unlocked => json!(0),
            Prev::Locked(m) => json!(m),
        }
    }
    fn from_json(v: Option<&serde_json::Value>) -> Prev {
        match v.and_then(|x| x.as_u64()) {
            None => Prev::Unknown,
            Some(0) => Prev::Unlocked,
            Some(m) => Prev::Locked(m as u32),
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Core,
    Mem,
}

impl Kind {
    fn name(self) -> &'static str {
        if self == Kind::Core { "core" } else { "memory" }
    }
    /// The variable names, FreeForGem first; the FreeForGemPear names are accepted too (same rigs, same scripts).
    fn envs(self) -> [&'static str; 2] {
        if self == Kind::Core { ["FFG_RESTORE_CCLOCK", "FFGP_RESTORE_CCLOCK"] } else { ["FFG_RESTORE_MCLOCK", "FFGP_RESTORE_MCLOCK"] }
    }
    /// The first of `envs` that is set (non-empty), with its value.
    fn env(self) -> Option<(&'static str, String)> {
        self.envs().into_iter().find_map(|n| std::env::var(n).ok().filter(|v| !v.trim().is_empty()).map(|v| (n, v)))
    }
}

/// $FFG_RESTORE_CCLOCK / $FFG_RESTORE_MCLOCK (or $FFGP_RESTORE_*): the lock to put back on exit, one value for all GPUs or
/// one per GPU in --devices order (0 = no lock, i.e. reset; '-' or empty = not given for that GPU).
fn prev_from_env(k: Kind, pos: usize, n: usize) -> Option<(Prev, &'static str)> {
    let (name, raw) = k.env()?;
    let p = prev_from_env_value(&raw, pos, n)?;
    Some((p, name))
}

fn prev_from_env_value(raw: &str, pos: usize, n: usize) -> Option<Prev> {
    let parts: Vec<&str> = raw.split(',').map(str::trim).collect();
    let s = if parts.len() == 1 {
        parts[0]
    } else if parts.len() == n {
        parts[pos]
    } else {
        return None;
    };
    match s.parse::<u32>() {
        Ok(0) => Some(Prev::Unlocked),
        Ok(m) => Some(Prev::Locked(m)),
        Err(_) => None,
    }
}

/// What the HiveOS OC profile says for this GPU (by its NVML index), right now; `None` = not HiveOS.
fn prev_from_hive(nv: &Nvml, d: Dev, k: Kind) -> Option<Prev> {
    let h = crate::hiveoc::load()?;
    let i = nv.index(d)? as usize;
    let v = if k == Kind::Core { h.core_lock(i) } else { h.mem_lock(i) };
    Some(v.map_or(Prev::Unlocked, Prev::Locked))
}

/// The lock to put back for the device at `pos` of `n` (NVML handle `d`), and where that comes from (for the log).
fn prev_lock(nv: &Nvml, d: Dev, k: Kind, pos: usize, n: usize) -> (Prev, &'static str) {
    if let Some((p, name)) = prev_from_env(k, pos, n) {
        return (p, name);
    }
    if let Some(p) = prev_from_hive(nv, d, k) {
        return (p, "HiveOS OC profile");
    }
    (Prev::Unknown, "unknown")
}

fn fmt_prev(p: Prev, src: &str) -> String {
    match p {
        Prev::Unknown => "lock before: not readable (no HiveOS OC profile, no $FFG_RESTORE_*), reset to default on exit".into(),
        Prev::Unlocked => format!("lock before: none ({src}), reset on exit"),
        Prev::Locked(m) => format!("lock before: {m} MHz ({src}), put back on exit"),
    }
}

/// Puts one clock back: the lock it had before, else reset.  With an env value that one is used; on HiveOS the profile is
/// read again (HiveOS may have applied a new OC meanwhile).  Returns the log text.
fn put_back(nv: &Nvml, d: Dev, k: Kind, stored: Prev, pos: usize, n: usize) -> String {
    let target = prev_from_env(k, pos, n).map(|(p, _)| p).or_else(|| prev_from_hive(nv, d, k)).unwrap_or(stored);
    match target {
        Prev::Locked(m) => {
            let r = if k == Kind::Core { nv.set_core_locked(d, m) } else { nv.set_mem_locked(d, m) };
            match r {
                Ok(()) => format!("{} clock lock {m} MHz put back", k.name()),
                Err(e) => {
                    // never leave this run's lock behind: reset instead
                    let _ = if k == Kind::Core { nv.reset_core_locked(d) } else { nv.reset_mem_locked(d) };
                    format!("{} clock: could not put the lock {m} MHz back ({e:#}); reset to default", k.name())
                }
            }
        }
        Prev::Unlocked | Prev::Unknown => {
            let _ = if k == Kind::Core { nv.reset_core_locked(d) } else { nv.reset_mem_locked(d) };
            format!("{} clock lock reset to default{}", k.name(), if target == Prev::Unlocked { " (there was none before)" } else { "" })
        }
    }
}

struct Item {
    dev: Dev,
    bus: String,
    pos: usize,
    n: usize,
    core_locked: bool,
    core_mhz: u32,
    mem_locked: bool,
    mem_mhz: u32,
    mem_off: bool,
    core_prev: Prev,
    mem_prev: Prev,
}

struct State {
    nv: Nvml,
    marker: PathBuf,
    items: Vec<Item>,
}

fn state() -> &'static Mutex<Option<State>> {
    static S: Mutex<Option<State>> = Mutex::new(None);
    &S
}

fn lg(m: impl AsRef<str>) {
    println!("clocks: {}", m.as_ref());
}

fn marker_path(dir: &Path) -> PathBuf {
    dir.join("clocks-restore.json")
}

/// Undo clock locks left by a previous run that did not exit cleanly (crash / power loss / SIGKILL): the locks persist in the
/// driver until reset or reboot, so put back exactly those we recorded (the lock that was there before, else reset), then
/// delete the marker.
pub fn recover(dir: &Path) {
    let p = marker_path(dir);
    let Ok(txt) = std::fs::read_to_string(&p) else { return };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt) else {
        let _ = std::fs::remove_file(&p);
        return;
    };
    let nv = match Nvml::load() {
        Ok(n) => n,
        Err(_) => return,
    };
    let gpus = v.get("gpus").and_then(|x| x.as_array()).map(|a| a.as_slice()).unwrap_or(&[]);
    let mut n = 0;
    for g in gpus {
        let Some(bus) = g.get("bus").and_then(|x| x.as_str()) else { continue };
        let Ok(d) = nv.by_bus(bus) else { continue };
        let pos = g.get("pos").and_then(|x| x.as_u64()).unwrap_or(0) as usize;
        let cnt = g.get("n").and_then(|x| x.as_u64()).unwrap_or(1) as usize;
        let mut what = Vec::new();
        if g.get("core_locked").and_then(|x| x.as_bool()).unwrap_or(false) {
            what.push(put_back(&nv, d, Kind::Core, Prev::from_json(g.get("core_prev")), pos, cnt));
        }
        if g.get("mem_locked").and_then(|x| x.as_bool()).unwrap_or(false) {
            what.push(put_back(&nv, d, Kind::Mem, Prev::from_json(g.get("mem_prev")), pos, cnt));
        }
        if g.get("mem_off").and_then(|x| x.as_bool()).unwrap_or(false) {
            let _ = nv.set_mem_offset(d, 0);
            what.push("memory offset 0".into());
        }
        lg(format!("GPU bus {bus}: left changed by an earlier run that did not exit cleanly: {}", what.join(", ")));
        n += 1;
    }
    if n > 0 {
        lg(format!("undid clock settings left by an earlier run ({n} GPU(s))"));
    }
    let _ = std::fs::remove_file(&p);
}

/// Applies the requested clock locks / memory offset to `devices`.  Returns a guard that restores them on drop;
/// `None` if nothing was requested, NVML is missing, or no device could be changed (then mining goes on as is).
/// The lock each clock had before is noted first (HiveOS OC profile / $FFG_RESTORE_* or $FFGP_RESTORE_*) and put back on exit.
pub fn apply(devices: &[i32], o: &ClockOpts, dir: &Path) -> Option<Guard> {
    if !o.any() {
        return None;
    }
    let nv = match Nvml::load() {
        Ok(n) => n,
        Err(e) => {
            lg(format!("NVML not available ({e:#}); clock settings skipped"));
            return None;
        }
    };
    let mut items = Vec::new();
    let n = devices.len();
    // one value given for every GPU (not a per-GPU list)
    let single_core = n > 1 && o.core_mhz.len() == n && o.core_mhz.iter().all(|v| v.is_some() && *v == o.core_mhz[0]);
    for (i, &dev) in devices.iter().enumerate() {
        let core = o.core_mhz.get(i).copied().flatten();
        let mem = o.mem_mhz.get(i).copied().flatten();
        let moff = o.mem_off.get(i).copied().flatten();
        if core.is_none() && mem.is_none() && moff.is_none() {
            continue;
        }
        let bus = match gpu::device_pci_bus_id(dev) {
            Ok(b) => b,
            Err(e) => {
                lg(format!("GPU{dev}: no PCI bus id ({e:#}); left alone"));
                continue;
            }
        };
        let d = match nv.by_bus(&bus) {
            Ok(d) => d,
            Err(e) => {
                lg(format!("GPU{dev} (bus {bus}): NVML handle failed ({e:#}); left alone"));
                continue;
            }
        };
        let mut it = Item { dev: d, bus: bus.clone(), pos: i, n, core_locked: false, core_mhz: 0, mem_locked: false, mem_mhz: 0, mem_off: false, core_prev: Prev::Unknown, mem_prev: Prev::Unknown };
        if let Some(mhz) = core {
            let (prev, src) = prev_lock(&nv, d, Kind::Core, i, n);
            match nv.set_core_locked(d, mhz) {
                Ok(()) => {
                    it.core_locked = true;
                    it.core_mhz = mhz;
                    it.core_prev = prev;
                    lg(format!("GPU{dev} (bus {bus}): core clock locked to {mhz} MHz; {}", fmt_prev(prev, src)));
                    // one --lock-cclock value for all GPUs replaces per-GPU HiveOS locks (Rig01: 1805 replaced 1775 on two cards)
                    if let (Prev::Locked(p), true) = (prev, single_core && src == "HiveOS OC profile") {
                        if p != mhz {
                            lg(format!(
                                "GPU{dev} (bus {bus}): WARNING the single --lock-cclock {mhz} replaces this GPU's own HiveOS lock {p} MHz while mining (it is put back on exit);                                  for per-GPU locks give a list (one value per GPU, '-' = leave that GPU alone) or leave --lock-cclock out: the HiveOS profile locks stay then"
                            ));
                        }
                    }
                }
                Err(e) => lg(format!("GPU{dev} (bus {bus}): cannot lock core clock ({e:#}); needs root/Administrator and a driver that supports it", )),
            }
        }
        if let Some(mhz) = mem {
            let (prev, src) = prev_lock(&nv, d, Kind::Mem, i, n);
            match nv.set_mem_locked(d, mhz) {
                Ok(()) => {
                    it.mem_locked = true;
                    it.mem_mhz = mhz;
                    it.mem_prev = prev;
                    lg(format!("GPU{dev} (bus {bus}): memory clock locked to {mhz} MHz; {}", fmt_prev(prev, src)));
                }
                Err(e) => lg(format!("GPU{dev} (bus {bus}): cannot lock memory clock ({e:#})")),
            }
        }
        if let Some(v) = moff {
            match nv.set_mem_offset(d, v) {
                Ok(()) => {
                    it.mem_off = true;
                    lg(format!("GPU{dev} (bus {bus}): memory clock offset {v:+} MHz"));
                }
                Err(e) => lg(format!("GPU{dev} (bus {bus}): cannot set memory offset ({e:#}); needs driver 555+ and root/Administrator")),
            }
        }
        if it.core_locked || it.mem_locked || it.mem_off {
            items.push(it);
        }
    }
    if items.is_empty() {
        return None;
    }
    // marker for crash recovery (locks persist in the driver until reset/reboot), with the locks to put back
    let marker = marker_path(dir);
    let doc = json!({
        "pid": std::process::id(),
        "gpus": items.iter().map(|it| json!({"bus": it.bus, "pos": it.pos, "n": it.n, "core_locked": it.core_locked, "mem_locked": it.mem_locked,
            "mem_off": it.mem_off, "core_prev": it.core_prev.to_json(), "mem_prev": it.mem_prev.to_json()})).collect::<Vec<_>>(),
    });
    let _ = std::fs::write(&marker, serde_json::to_string_pretty(&doc).unwrap());
    *state().lock().unwrap() = Some(State { nv, marker, items });
    Some(Guard)
}

/// Core clock lock this run set on each of `n` mining GPUs (by position; None = not set by the miner).
pub fn core_locks(n: usize) -> Vec<Option<u32>> {
    let mut v = vec![None; n];
    if let Some(st) = state().lock().unwrap().as_ref() {
        for it in &st.items {
            if it.core_locked && it.pos < n {
                v[it.pos] = Some(it.core_mhz);
            }
        }
    }
    v
}

/// Memory clock lock this run set on each of `n` mining GPUs (by position; None = not set by the miner).
pub fn mem_locks(n: usize) -> Vec<Option<u32>> {
    let mut v = vec![None; n];
    if let Some(st) = state().lock().unwrap().as_ref() {
        for it in &st.items {
            if it.mem_locked && it.pos < n {
                v[it.pos] = Some(it.mem_mhz);
            }
        }
    }
    v
}

/// Puts back every clock setting this run applied (the lock from before, else default), and removes the marker.  Safe to
/// call more than once.
pub fn restore_all() {
    let Some(st) = state().lock().unwrap().take() else { return };
    let mut lines = Vec::new();
    for it in &st.items {
        let mut what = Vec::new();
        if it.core_locked {
            what.push(put_back(&st.nv, it.dev, Kind::Core, it.core_prev, it.pos, it.n));
        }
        if it.mem_locked {
            what.push(put_back(&st.nv, it.dev, Kind::Mem, it.mem_prev, it.pos, it.n));
        }
        if it.mem_off {
            let _ = st.nv.set_mem_offset(it.dev, 0);
            what.push("memory offset 0".into());
        }
        lines.push(format!("GPU bus {}: {}", it.bus, what.join(", ")));
    }
    let _ = std::fs::remove_file(&st.marker);
    // logged only after every card is put back (the log pipe may be gone at shutdown)
    for l in lines {
        lg(l);
    }
}

/// Restores clock settings when dropped (normal shutdown).  Signal handlers call `restore_all` directly.
pub struct Guard;
impl Drop for Guard {
    fn drop(&mut self) {
        restore_all();
    }
}

/// Expands a `--lock-cclock`/… value into one entry per device: a single number applies to all, a
/// comma-separated list is taken in `--devices` order and must have one value per device.
pub fn expand_u32(raw: &str, n: usize, flag: &str) -> Result<Vec<Option<u32>>> {
    expand(raw, n, flag, |s| s.parse::<u32>().ok().filter(|&v| v > 0))
}
pub fn expand_i32(raw: &str, n: usize, flag: &str) -> Result<Vec<Option<i32>>> {
    expand(raw, n, flag, |s| s.trim().trim_start_matches('+').parse::<i32>().ok())
}
fn expand<T: Copy>(raw: &str, n: usize, flag: &str, parse: impl Fn(&str) -> Option<T>) -> Result<Vec<Option<T>>> {
    let parts: Vec<&str> = raw.split(',').map(str::trim).collect();
    if parts.len() == 1 {
        let v = parse(parts[0]).ok_or_else(|| anyhow::anyhow!("{flag}: '{}' is not a valid number", parts[0]))?;
        return Ok(vec![Some(v); n]);
    }
    anyhow::ensure!(parts.len() == n, "{flag}: {} values for {n} device(s) (give one value for all, or one per device in --devices order)", parts.len());
    parts
        .iter()
        .map(|s| {
            if s.is_empty() || *s == "-" {
                Ok(None) // skip this device
            } else {
                parse(s).map(Some).ok_or_else(|| anyhow::anyhow!("{flag}: '{s}' is not a valid number"))
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restore_env_values() {
        assert_eq!(prev_from_env_value("1200", 0, 3), Some(Prev::Locked(1200)));
        assert_eq!(prev_from_env_value("0", 2, 3), Some(Prev::Unlocked));
        assert_eq!(prev_from_env_value("1200,-,1400", 1, 3), None);
        assert_eq!(prev_from_env_value("1200,-,1400", 2, 3), Some(Prev::Locked(1400)));
        assert_eq!(prev_from_env_value("1200,1400", 0, 3), None); // wrong count: not used
    }
}
