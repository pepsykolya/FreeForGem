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

/// What the host does with the memory clock while a coin mines (1.8.0, per coin: coins.rs `Spec::mem`).  Set with
/// `ClockOpts::set_policy`; every lock this run sets is put back on exit / pause (restore marker), so a run of one coin never
/// leaves its memory clock to the next run of another.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum MemPolicy {
    /// Pearl, NOID, PROX: the HiveOS OC profile's memory lock is applied as nvidia-oc sets it (Pearl wants 810 MHz)
    #[default]
    HiveProfile,
    /// CMFD (1.7.6): the profile's memory lock is not applied (full memory clock needed); nothing else is set
    NoHiveLock,
    /// NEB (FFGN D-007): memory bound: a profile memory lock below the card's maximum (Pearl's 810) is replaced by a lock at the
    /// maximum while mining and put back on exit (`--keep-hive-mem` = HiveProfile); `--table-mclock` lowers it only on the pure
    /// digest-table path (`table_mem`)
    FullClock,
}

/// Per mining device (in --devices order); `None` leaves that clock untouched.
#[derive(Clone, Default)]
pub struct ClockOpts {
    pub core_mhz: Vec<Option<u32>>,
    pub mem_mhz: Vec<Option<u32>>,
    pub mem_off: Vec<Option<i32>>,
    /// CMFD (1.7.6): do not take the memory lock from the HiveOS OC profile (an explicit `mem_mhz` still applies).
    pub no_hive_mem: bool,
    /// NEB (D-007): a HiveOS memory lock below the card's maximum (e.g. Pearl's 810 MHz) is replaced by a lock at the maximum
    /// while mining (put back on exit); NEB is memory bound and runs ~10x slower at 810 MHz
    pub hive_mem_to_max: bool,
    /// NEB 1.0.11 (LOWP111): memory clock lock while the GPU's current slot runs the pure digest-table path (`--table-mclock`).
    /// The table path is ALU bound (DRAM ~idle): 3070 at 810 MHz -0.5 % rate, -10 W, +8..10 % MA/W. Every other path (plain,
    /// cheap, the full-path trial, table + L=3 attempts, a model's table build) gets the full memory clock back (`table_mem`).
    pub table_mhz: Vec<Option<u32>>,
}

impl ClockOpts {
    /// Options with only the memory policy of a coin set.
    pub fn policy(p: MemPolicy) -> ClockOpts {
        let mut o = ClockOpts::default();
        o.set_policy(p);
        o
    }

    pub fn set_policy(&mut self, p: MemPolicy) {
        (self.no_hive_mem, self.hive_mem_to_max) = match p {
            MemPolicy::HiveProfile => (false, false),
            MemPolicy::NoHiveLock => (true, false),
            MemPolicy::FullClock => (true, true),
        };
    }

    pub fn mem_policy(&self) -> MemPolicy {
        match (self.no_hive_mem, self.hive_mem_to_max) {
            (_, true) => MemPolicy::FullClock,
            (true, false) => MemPolicy::NoHiveLock,
            _ => MemPolicy::HiveProfile,
        }
    }

    pub fn any(&self) -> bool {
        self.core_mhz.iter().any(Option::is_some)
            || self.mem_mhz.iter().any(Option::is_some)
            || self.mem_off.iter().any(Option::is_some)
            || self.table_mhz.iter().any(Option::is_some)
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
    ord: i32, // the device ordinal (GPUn of the log)
    bus: String,
    pos: usize,
    n: usize,
    core_locked: bool,
    core_mhz: u32,
    mem_locked: bool,
    mem_mhz: u32,
    mem_off: bool,
    mem_off_mhz: i32,
    core_prev: Prev,
    mem_prev: Prev,
    /// `--table-mclock` of this GPU (0 = off), whether it is applied now, and whether it was ever applied (restore on exit)
    tab_mhz: u32,
    tab_low: bool,
    tab_used: bool,
}

impl Item {
    /// The memory lock this run wants now: the table lock while the table path runs, else the --lock-mclock / HiveOS one.
    fn mem_want(&self) -> Option<u32> {
        if self.tab_low { Some(self.tab_mhz) } else if self.mem_locked { Some(self.mem_mhz) } else { None }
    }
    /// The memory clock was changed by this run (put back on exit / pause).
    fn mem_touched(&self) -> bool {
        self.mem_locked || self.tab_used
    }
}

struct State {
    nv: Nvml,
    marker: PathBuf,
    items: Vec<Item>,
    suspended: bool, // idle mode: the owner's clocks are back while the PC is in use (suspend / resume)
}

fn state() -> &'static Mutex<Option<State>> {
    static S: Mutex<Option<State>> = Mutex::new(None);
    &S
}

fn lg(m: impl AsRef<str>) {
    println!("clocks: {}", m.as_ref());
}

fn marker_path(dir: &Path) -> PathBuf {
    dir.join(crate::coins::group_file("clocks-restore.json")) // 1.8.0: per group of cards (coins::group_file)
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

/// The HiveOS profile's memory lock to apply, and the one skipped: CMFD (`no_hive_mem`) skips it, because the 810 MHz that Pearl
/// wants makes CMFD about 6x slower (Rig01: 3.7 FW/s instead of ~21).  The core lock still comes from the profile (Rig05, 1.7.3).
fn hive_mem_choice(hive_mem: Option<u32>, no_hive_mem: bool) -> (Option<u32>, Option<u32>) {
    if no_hive_mem { (None, hive_mem) } else { (hive_mem, None) }
}

/// 1.8.0: the memory lock a GPU gets from its HiveOS profile lock `hive_mem` under the policy `p` (no --lock-mclock given):
/// (lock applied, profile lock skipped, lock at the maximum instead).  `max`: the card's maximum memory clock.
pub fn mem_plan(p: MemPolicy, hive_mem: Option<u32>, max: Option<u32>) -> (Option<u32>, Option<u32>, Option<u32>) {
    let o = ClockOpts::policy(p);
    let (apply, skipped) = hive_mem_choice(hive_mem, o.no_hive_mem);
    let to_max = match (skipped, max) {
        (Some(m), Some(x)) if o.hive_mem_to_max && x > m => Some(x),
        _ => None,
    };
    (apply, skipped, to_max)
}

/// Applies the requested clock locks / memory offset to `devices`.  Returns a guard that restores them on drop;
/// `None` if nothing was requested, NVML is missing, or no device could be changed (then mining goes on as is).
/// The lock each clock had before is noted first (HiveOS OC profile / $FFG_RESTORE_* or $FFGP_RESTORE_*) and put back on exit.
pub fn apply(devices: &[i32], o: &ClockOpts, dir: &Path) -> Option<Guard> {
    // 1.7.3: on HiveOS a clock without a --lock-* value gets the OC profile's lock (Rig05: another miner left 4x CMP 50HX unlocked
    // at ~2040 MHz with the profile's +150 offset, LCLOCK 1410; mining started on all four at once and the rig lost power)
    let hive = crate::hiveoc::load();
    if !o.any() && hive.is_none() {
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
        let (core_opt, mem_opt) = (o.core_mhz.get(i).copied().flatten(), o.mem_mhz.get(i).copied().flatten());
        let moff = o.mem_off.get(i).copied().flatten();
        let tab = o.table_mhz.get(i).copied().flatten();
        if core_opt.is_none() && mem_opt.is_none() && moff.is_none() && tab.is_none() && hive.is_none() {
            continue;
        }
        let bus = match bus_id(dev) {
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
        // the HiveOS profile's locks for the clocks without a --lock-* value (the same lock nvidia-oc sets; put back on exit)
        let idx = nv.index(d).map(|x| x as usize);
        let core_hive = core_opt.is_none().then(|| hive.as_ref().zip(idx).and_then(|(h, x)| h.core_lock(x))).flatten();
        let hive_mem = mem_opt.is_none().then(|| hive.as_ref().zip(idx).and_then(|(h, x)| h.mem_lock(x))).flatten();
        let (mem_hive, mem_skipped) = hive_mem_choice(hive_mem, o.no_hive_mem);
        let mut mem_max = None;
        if let Some(m) = mem_skipped {
            let max = nv.max_mem_clock_mhz(d);
            if o.hive_mem_to_max && max.is_some_and(|x| x > m) {
                mem_max = max;
                lg(format!(
                    "GPU{dev} (bus {bus}): WARNING the HiveOS OC profile locks the memory to {m} MHz: NEB needs the full memory clock (at {m} MHz it mines about 10x slower). The memory is locked to its maximum {} MHz while mining and {m} MHz is put back on exit; --keep-hive-mem keeps the profile's {m} MHz, --lock-mclock N sets your own",
                    max.unwrap_or(0)
                ));
            } else {
                lg(format!(
                    "GPU{dev} (bus {bus}): the HiveOS profile's memory lock {m} MHz is not applied (full memory clock needed); give --lock-mclock to force one"
                ));
            }
        }
        let (core, mem) = (core_opt.or(core_hive), mem_opt.or(mem_hive).or(mem_max));
        if core.is_none() && mem.is_none() && moff.is_none() && tab.is_none() {
            continue;
        }
        if core_hive.is_some() || mem_hive.is_some() {
            lg(format!(
                "GPU{dev} (bus {bus}): the HiveOS OC profile locks{}{} are set before mining (no --lock-cclock / --lock-mclock; another program may have left the clocks unlocked)",
                core_hive.map_or(String::new(), |m| format!(" core {m} MHz")),
                mem_hive.map_or(String::new(), |m| format!(" memory {m} MHz"))
            ));
        }
        let mut it = Item { dev: d, ord: dev, bus: bus.clone(), pos: i, n, core_locked: false, core_mhz: 0, mem_locked: false, mem_mhz: 0, mem_off: false, mem_off_mhz: 0, core_prev: Prev::Unknown, mem_prev: Prev::Unknown, tab_mhz: 0, tab_low: false, tab_used: false };
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
                Err(e) if format!("{e:#}").contains("Not Supported") => {
                    lg(format!(
                        "GPU{dev} (bus {bus}): memory clock lock not supported by this GPU / driver (e.g. CMP cards): left as it is (the HiveOS memory setting stays){}",
                        if mem_max.is_some() { "; the memory clock under load is checked after a minute of mining" } else { "; harmless" }
                    ));
                    if let (Some(_), Some(m)) = (mem_max, mem_skipped) {
                        hive_mem_left().lock().unwrap().insert(bus.to_ascii_lowercase(), m);
                    }
                }
                Err(e) => {
                    lg(format!("GPU{dev} (bus {bus}): cannot lock memory clock ({e:#})"));
                    if let (Some(_), Some(m)) = (mem_max, mem_skipped) {
                        hive_mem_left().lock().unwrap().insert(bus.to_ascii_lowercase(), m);
                    }
                }
            }
        }
        if let Some(v) = moff {
            match nv.set_mem_offset(d, v) {
                Ok(()) => {
                    it.mem_off = true;
                    it.mem_off_mhz = v;
                    lg(format!("GPU{dev} (bus {bus}): memory clock offset {v:+} MHz"));
                }
                Err(e) => lg(format!("GPU{dev} (bus {bus}): cannot set memory offset ({e:#}); needs driver 555+ and root/Administrator")),
            }
        }
        if let Some(t) = tab {
            if !it.mem_locked {
                it.mem_prev = prev_lock(&nv, d, Kind::Mem, i, n).0;
            }
            it.tab_mhz = t;
            lg(format!(
                "GPU{dev} (bus {bus}): memory clock {t} MHz while this GPU mines on the pure digest-table path (--table-mclock); the full memory clock{} on every other path and for table builds",
                if it.mem_locked { format!(" ({} MHz)", it.mem_mhz) } else { String::new() }
            ));
        }
        if it.core_locked || it.mem_locked || it.mem_off || it.tab_mhz > 0 {
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
        "gpus": items.iter().map(|it| json!({"bus": it.bus, "pos": it.pos, "n": it.n, "core_locked": it.core_locked, "mem_locked": it.mem_locked || it.tab_mhz > 0,
            "mem_off": it.mem_off, "core_prev": it.core_prev.to_json(), "mem_prev": it.mem_prev.to_json()})).collect::<Vec<_>>(),
    });
    let _ = std::fs::write(&marker, serde_json::to_string_pretty(&doc).unwrap());
    let checks: Vec<(i32, String, u32)> = items.iter().filter(|it| it.mem_locked).map(|it| (devices[it.pos], it.bus.clone(), it.mem_mhz)).collect();
    *state().lock().unwrap() = Some(State { nv, marker, items, suspended: false });
    if !checks.is_empty() {
        spawn_mem_lock_check(checks);
    }
    spawn_lock_keeper();
    Some(Guard)
}

/// A memory clock sample counts only at or above this GPU utilization (an idle card may sit in a low P-state).
const MEM_CHECK_BUSY_PCT: u32 = 70;
/// Busy samples needed (spaced `MEM_CHECK_STEP_S` apart, the first after `MEM_CHECK_FIRST_S`), and how long to wait for them.
const MEM_CHECK_SAMPLES: usize = 3;
const MEM_CHECK_FIRST_S: u64 = 60;
const MEM_CHECK_STEP_S: u64 = 10;
const MEM_CHECK_GIVE_UP_S: u64 = 900;

/// Verdict of the memory-lock check: the memory clock the card really runs at under load when it is steadily off the requested
/// lock, else `None` (holds, too few busy samples, or the samples disagree).  `samples` = (memory MHz, GPU utilization %);
/// idle samples are ignored (the clock may be lower while idling, that is no error).  Tolerance: 5 % of the lock, at least 30 MHz.
pub(crate) fn mem_lock_verdict(requested: u32, samples: &[(u32, u32)]) -> Option<u32> {
    let mut busy: Vec<u32> = samples.iter().filter(|s| s.1 >= MEM_CHECK_BUSY_PCT).map(|s| s.0).collect();
    if requested == 0 || busy.len() < MEM_CHECK_SAMPLES {
        return None;
    }
    let tol = (requested / 20).max(30);
    if !busy.iter().all(|&m| m.abs_diff(requested) > tol) {
        return None;
    }
    let above = busy[0] > requested;
    if busy.iter().any(|&m| (m > requested) != above) {
        return None; // on both sides of the lock: not steady
    }
    busy.sort_unstable();
    Some(busy[busy.len() / 2])
}

/// The warning text for `mem_lock_verdict`; one line per card.
pub(crate) fn mem_lock_message(dev: i32, bus: &str, requested: u32, actual: u32) -> String {
    if actual > requested {
        format!(
            "WARNING: GPU{dev} (bus {bus}): memory clock lock {requested} MHz accepted, but the memory runs at {actual} MHz under load (the lowest this GPU/driver allows while computing); the lock has no effect below {actual}"
        )
    } else {
        format!(
            "WARNING: GPU{dev} (bus {bus}): memory clock lock {requested} MHz accepted, but the memory runs at {actual} MHz under load, below the lock; the lock is not held"
        )
    }
}

/// Once per card, after the first minute of mining: reads the real memory clock through NVML a few times under load and
/// warns when it is steadily not the locked one (RTX 3070, driver 595: `-lmc 405` is accepted, the card computes at 810).  Only
/// a warning: the lock and the restore on exit are untouched; no NVML reading, no load or a pause (idle mode) = no verdict.
fn spawn_mem_lock_check(cards: Vec<(i32, String, u32)>) {
    let _ = std::thread::Builder::new().name("mclk-check".into()).spawn(move || {
        let sleep = |s: u64| std::thread::sleep(std::time::Duration::from_secs(s));
        let mut samples: Vec<Vec<(u32, u32)>> = vec![Vec::new(); cards.len()];
        let mut done = vec![false; cards.len()];
        sleep(MEM_CHECK_FIRST_S);
        let mut waited = MEM_CHECK_FIRST_S;
        while waited <= MEM_CHECK_GIVE_UP_S && done.iter().any(|d| !d) {
            {
                let g = state().lock().unwrap();
                let Some(st) = g.as_ref() else { return }; // restored / shut down
                if !st.suspended {
                    for (i, (dev, bus, req)) in cards.iter().enumerate() {
                        if done[i] {
                            continue;
                        }
                        let Some(it) = st.items.iter().find(|it| it.mem_locked && it.bus == *bus) else {
                            done[i] = true;
                            continue;
                        };
                        let (Some(m), Some(u)) = (st.nv.mem_clock_mhz(it.dev), st.nv.util_gpu(it.dev)) else {
                            done[i] = true; // NVML does not give it: silently skip
                            continue;
                        };
                        samples[i].push((m, u));
                        if samples[i].iter().filter(|s| s.1 >= MEM_CHECK_BUSY_PCT).count() >= MEM_CHECK_SAMPLES {
                            done[i] = true;
                            if let Some(actual) = mem_lock_verdict(*req, &samples[i]) {
                                lg(mem_lock_message(*dev, bus, *req, actual));
                            }
                        }
                    }
                }
            }
            sleep(MEM_CHECK_STEP_S);
            waited += MEM_CHECK_STEP_S;
        }
    });
}

/// Lock keeper (1.7.8, all coins): HiveOS `nvidia-oc` (at a miner start, an OC change in the web UI) and other OC tools reset the
/// clock locks; NVML cannot read a lock back, so the keeper watches the effect: a card that held its lock under load and then runs
/// above it (core) or off it (memory) in `KEEP_NEED` samples in a row has lost it, and the lock of this run is set again (one line in
/// the log).  Quiet otherwise: a sample every `KEEP_POLL_S`, nothing is written to a card that holds, at most `OC_RESETS_MAX` re-sets
/// an hour per card (then it stands down and says so).  The offset has its own keeper in each tuner (and PROX `--uv-offset`).
const KEEP_POLL_S: u64 = 5; // D-016: an OC apply that drops the lock is undone within ~10 s
const KEEP_BUSY_PCT: u32 = 70;
const KEEP_NEED: u32 = 2;

#[derive(Default)]
struct Track {
    armed: bool, // the lock was seen holding under load (a lock that never held, e.g. a clock the driver does not honour, is not chased)
    miss: u32,
    unheld: u32, // 1.0.3: busy core samples in a row above the lock + HOLD_TOL while it was never seen holding
    resets: u32, // 1.0.3: the lock set again while it was never seen holding
}

/// 1.0.3 (RTX 2060 SUPER rig, driver 620: NVML said "locked to 1400 MHz", the card ran at 1470-2055 under load): a core lock that is
/// never seen holding is set again `HOLD_RESETS` times (HiveOS nvidia-oc with an offset-only profile runs `nvtool --setclocks 0`,
/// which drops a lock set before it); still not held -> the lock does not hold on this card: a loud warning, auto-uv off for it.
/// A clock above the lock is never a power / thermal limit (those pull the clock down), so above the lock + 45 MHz (3 bins) = not held.
pub const HOLD_TOL_MIN: u32 = 45; // 3 clock bins
const HOLD_NEED: u32 = 3;
const HOLD_RESETS: u32 = 2;

pub fn hold_tol(requested: u32) -> u32 {
    (requested / 33).max(HOLD_TOL_MIN)
}

#[derive(Debug, PartialEq)]
enum Hold {
    Fine,
    SetAgain(u32),
    Broken(u32),
}

/// One busy core sample of a lock that was never seen holding (`t.armed` false).
fn hold_judge(t: &mut Track, requested: u32, actual: u32, util: u32) -> Hold {
    if requested == 0 || util < KEEP_BUSY_PCT || t.armed {
        return Hold::Fine;
    }
    if actual <= requested + hold_tol(requested) {
        t.unheld = 0;
        return Hold::Fine;
    }
    t.unheld += 1;
    if t.unheld < HOLD_NEED {
        return Hold::Fine;
    }
    t.unheld = 0;
    if t.resets < HOLD_RESETS {
        t.resets += 1;
        Hold::SetAgain(actual)
    } else {
        Hold::Broken(actual)
    }
}

/// Cards whose core lock does not hold (bus -> (lock, clock under load)): auto-uv leaves them (neb/tune.rs), the API and the
/// stats line show it.
fn broken() -> &'static Mutex<std::collections::HashMap<String, (u32, u32)>> {
    static B: std::sync::OnceLock<Mutex<std::collections::HashMap<String, (u32, u32)>>> = std::sync::OnceLock::new();
    B.get_or_init(Default::default)
}

/// 1.0.4 (RTX 2060 SUPER, driver 620: the HiveOS profile locks the memory to 810 MHz and the driver does not let ffgn lock it to
/// its maximum, "memory clock lock not supported"): bus -> the profile's memory lock that stayed.  The mining loop compares the
/// memory clock under load with it (`mark_mem_stuck`).
fn hive_mem_left() -> &'static Mutex<std::collections::HashMap<String, u32>> {
    static M: std::sync::OnceLock<Mutex<std::collections::HashMap<String, u32>>> = std::sync::OnceLock::new();
    M.get_or_init(Default::default)
}

/// The HiveOS memory lock that could not be replaced on `bus` (its MHz).
pub fn hive_mem_kept(bus: &str) -> Option<u32> {
    hive_mem_left().lock().unwrap().get(&bus.to_ascii_lowercase()).copied()
}

fn mem_stuck_map() -> &'static Mutex<std::collections::HashMap<String, u32>> {
    static M: std::sync::OnceLock<Mutex<std::collections::HashMap<String, u32>>> = std::sync::OnceLock::new();
    M.get_or_init(Default::default)
}

/// 1.0.4: the memory of `bus` runs at the low lock of the OC profile (MHz): a loud warning on every stats line, in the API and
/// in HiveOS (`mem_stuck_message`).
pub fn mark_mem_stuck(bus: &str, dev: i32, mhz: u32) {
    if mem_stuck_map().lock().unwrap().insert(bus.to_ascii_lowercase(), mhz).is_none() {
        eprintln!("WARNING GPU{dev}: {}", mem_stuck_message(mhz));
    }
}

pub fn mem_stuck(bus: &str) -> Option<u32> {
    mem_stuck_map().lock().unwrap().get(&bus.to_ascii_lowercase()).copied()
}

/// The owner's text (1.0.4).
pub fn mem_stuck_message(mhz: u32) -> String {
    format!("NEB runs ~6x slower: memory locked to {mhz} MHz by the OC profile — remove the memory lock")
}

/// The memory clock under load `cur` is at the profile's lock `lock` (5 %, at least 30 MHz) and well below the maximum.
pub fn mem_at_lock(cur: u32, lock: u32, max: u32) -> bool {
    cur <= lock + (lock / 20).max(30) && (cur as f64) < max as f64 * 0.6
}

/// The text of the warning (one line per card; h-stats.sh looks for "does not hold").
pub fn not_held_message(dev: i32, lock: u32, actual: u32) -> String {
    format!("WARNING GPU{dev}: the core clock lock {lock} MHz does not hold (runs at {actual} MHz): auto-uv and auto-lock are off for this GPU; use the power limit of the OC profile")
}

fn unheld_devs() -> &'static Mutex<Vec<i32>> {
    static D: std::sync::OnceLock<Mutex<Vec<i32>>> = std::sync::OnceLock::new();
    D.get_or_init(Default::default)
}

/// 1.8.0 (rule 5, every coin): the tag of the stats line for the GPUs whose core lock does not hold ("" when none):
/// `  [LOCK NOT HELD: GPU1,GPU3]` (h-stats.sh shows it next to the version, as FFGN does).
pub fn unheld_tag() -> String {
    let d = unheld_devs().lock().unwrap();
    if d.is_empty() { String::new() } else { format!("  [LOCK NOT HELD: {}]", d.iter().map(|x| format!("GPU{x}")).collect::<Vec<_>>().join(",")) }
}

/// Marks the core lock of `bus` as not holding (once; logged loudly).
pub fn mark_not_held(bus: &str, dev: i32, lock: u32, actual: u32) {
    let first = broken().lock().unwrap().insert(bus.to_ascii_lowercase(), (lock, actual)).is_none();
    if first {
        unheld_devs().lock().unwrap().push(dev);
        eprintln!("{}", not_held_message(dev, lock, actual));
        crate::autolock::note_unheld(bus, dev);
    }
}

/// Some((lock, clock under load)) when the core lock of `bus` was found not to hold.
pub fn not_held(bus: &str) -> Option<(u32, u32)> {
    broken().lock().unwrap().get(&bus.to_ascii_lowercase()).copied()
}

/// Sets this run's core lock of `bus` again (the tuner saw the clock above it); false = no lock of this run on that card.
pub fn set_core_again(bus: &str) -> bool {
    let g = state().lock().unwrap();
    let Some(st) = g.as_ref().filter(|st| !st.suspended) else { return false };
    let Some(it) = st.items.iter().find(|it| it.core_locked && it.bus.eq_ignore_ascii_case(bus)) else { return false };
    st.nv.set_core_locked(it.dev, it.core_mhz).is_ok()
}

/// Tolerance of a clock sample against its lock: core 3 % (at least 30 MHz: the clock moves in 15 MHz bins), memory 5 % (at least 30).
fn keep_tol(k: Kind, requested: u32) -> u32 {
    match k {
        Kind::Core => (requested / 33).max(30),
        Kind::Mem => (requested / 20).max(30),
    }
}

/// One sample of a locked clock.  Returns the clock the card runs at when the lock is judged lost (`KEEP_NEED` busy samples in a
/// row off the lock after it was seen holding), else `None`.  Core: only ABOVE the lock counts (below = power / thermal limit);
/// memory: either side.  Idle samples change nothing.
fn keep_judge(t: &mut Track, k: Kind, requested: u32, actual: u32, util: u32) -> Option<u32> {
    if requested == 0 || util < KEEP_BUSY_PCT {
        return None;
    }
    let tol = keep_tol(k, requested);
    let off = match k {
        Kind::Core => actual > requested + tol,
        Kind::Mem => actual.abs_diff(requested) > tol,
    };
    if !off {
        t.armed = true;
        t.miss = 0;
        return None;
    }
    if !t.armed {
        return None;
    }
    t.miss += 1;
    if t.miss >= KEEP_NEED {
        t.armed = false;
        t.miss = 0;
        return Some(actual);
    }
    None
}

fn spawn_lock_keeper() {
    let _ = std::thread::Builder::new().name("clock-keep".into()).spawn(move || {
        let mut tr: std::collections::HashMap<(String, bool), (Track, crate::hiveoc::ResetLimit, bool)> = Default::default();
        loop {
            std::thread::sleep(std::time::Duration::from_secs(KEEP_POLL_S));
            let g = state().lock().unwrap();
            let Some(st) = g.as_ref() else { return }; // restored / shut down
            if st.suspended {
                continue;
            }
            for it in &st.items {
                let mw = it.mem_want();
                for (k, on, mhz) in [(Kind::Core, it.core_locked, it.core_mhz), (Kind::Mem, mw.is_some(), mw.unwrap_or(0))] {
                    if !on {
                        continue;
                    }
                    let e = tr.entry((it.bus.clone(), matches!(k, Kind::Core))).or_default();
                    if e.2 {
                        continue; // stood down
                    }
                    let (actual, util) = match k {
                        Kind::Core => (st.nv.clock_mhz(it.dev), st.nv.util_gpu(it.dev)),
                        Kind::Mem => (st.nv.mem_clock_mhz(it.dev), st.nv.util_gpu(it.dev)),
                    };
                    let (Some(actual), Some(util)) = (actual, util) else { continue };
                    let judged = keep_judge(&mut e.0, k, mhz, actual, util);
                    if judged.is_none() && matches!(k, Kind::Core) && !e.0.armed {
                        match hold_judge(&mut e.0, mhz, actual, util) {
                            Hold::Fine => {}
                            Hold::SetAgain(now) => {
                                let r = st.nv.set_core_locked(it.dev, mhz);
                                lg(format!(
                                    "GPU{} (bus {}): the core clock lock {mhz} MHz is not held (the card runs at {now} MHz under load): the core clock lock was reset by something else (HiveOS OC apply?): set again{}",
                                    it.ord,
                                    it.bus,
                                    r.err().map_or(String::new(), |e| format!(" (failed: {e:#})"))
                                ));
                                crate::faults::note_foreign_change(&it.bus);
                            }
                            Hold::Broken(now) => {
                                mark_not_held(&it.bus, it.ord, mhz, now);
                                e.2 = true; // nothing more to do on this card: the tuner leaves it (tune::tick)
                            }
                        }
                        continue;
                    }
                    let Some(now) = judged else { continue };
                    let name = if matches!(k, Kind::Core) { "core" } else { "memory" };
                    if !e.1.allow() {
                        e.2 = true;
                        lg(format!(
                            "GPU bus {}: the {name} clock lock {mhz} MHz was lost again (the card runs at {now} MHz): {} times within an hour already, so the miner stands down on this lock (restart the miner once the OC is settled)",
                            it.bus,
                            crate::hiveoc::OC_RESETS_MAX
                        ));
                        continue;
                    }
                    let r = if matches!(k, Kind::Core) { st.nv.set_core_locked(it.dev, mhz) } else { st.nv.set_mem_locked(it.dev, mhz) };
                    match r {
                        Ok(()) => lg(format!("GPU bus {}: the {name} clock lock {mhz} MHz was reset by an OC apply (HiveOS nvidia-oc or another tool; the card ran at {now} MHz under load); set again", it.bus)),
                        Err(e) => lg(format!("GPU bus {}: the {name} clock lock {mhz} MHz was reset (the card runs at {now} MHz) and cannot be set again ({e:#})", it.bus)),
                    }
                    crate::faults::note_foreign_change(&it.bus);
                }
            }
        }
    });
}

/// Core lock (MHz) with the best Pearl TMAC/J on RTX 30 (Ampere).  Source: RTX 3070 frequency sweeps (pearl-miner JOURNAL, night 10):
/// 1530 MHz = -1.9% speed, -5.4% power, +3.6% TMAC/J against 1560; 1515 and below is a plateau.  Only a hint: nothing is set.
pub const AMPERE_PEARL_LOCK: u32 = 1530;

/// PCI bus id of a device ordinal: from the library of the coin that is mining (set by `set_bus_resolver`: proxgpu / noidgpu /
/// nebgpu), else from the Pearl library.  A PROX, NOID or NEB run must not need pearlgpu just to find its cards' NVML handles.
static BUS_RESOLVER: std::sync::OnceLock<fn(i32) -> anyhow::Result<String>> = std::sync::OnceLock::new();

pub fn set_bus_resolver(f: fn(i32) -> anyhow::Result<String>) {
    let _ = BUS_RESOLVER.set(f);
}

fn bus_id(dev: i32) -> anyhow::Result<String> {
    match BUS_RESOLVER.get() {
        Some(f) => f(dev),
        None => gpu::device_pci_bus_id(dev),
    }
}

/// RTX 30 (GA10x, sm_86; a CMP 70HX/90HX is not one) whose core lock is missing or above `AMPERE_PEARL_LOCK`.
pub(crate) fn ampere_hint_needed(name: &str, cc: Option<(i32, i32)>, lock: Option<u32>) -> bool {
    let n = name.to_ascii_uppercase();
    let rtx30 = n.contains("RTX 30") && !n.contains("CMP") && cc.is_none_or(|c| c == (8, 6));
    rtx30 && lock.is_none_or(|l| l > AMPERE_PEARL_LOCK)
}

/// Pearl on RTX 30: one log line when a GPU runs without a core lock or above 1530 MHz (`locks` = what the miner set, by position).
pub fn hint_ampere(devices: &[i32], locks: &[Option<u32>]) {
    let Ok(nv) = Nvml::load() else { return };
    let mut gpus = Vec::new();
    for (i, &dev) in devices.iter().enumerate() {
        let Some(d) = gpu::device_pci_bus_id(dev).ok().and_then(|b| nv.by_bus(&b).ok()) else { continue };
        let name = nv.name(d).unwrap_or_default();
        if ampere_hint_needed(&name, nv.compute_capability(d), locks.get(i).copied().flatten()) {
            gpus.push(format!("GPU{dev}"));
        }
    }
    if !gpus.is_empty() {
        lg(format!(
            "hint: for better TMAC/W on RTX 30 try --lock-cclock {AMPERE_PEARL_LOCK} ({}: no lock or above {AMPERE_PEARL_LOCK} MHz); about +3.6% TMAC/J for -1.9% speed against 1560 (RTX 3070); auto-uv tunes a new lock from the profile of a higher one",
            gpus.join(", ")
        ));
    }
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

/// NEB 1.0.11 (LOWP111, `--table-mclock`): GPU `dev` mines on the pure digest-table path now (`low`) or not (any other path,
/// a model change with its table build). Sets the table memory lock or gives the full memory clock back (the --lock-mclock /
/// HiveOS lock, else the lock from before / default). Acts only on a change; a no-op without --table-mclock or while paused.
/// Returns Some(new state) when it changed the clock.
/// 1.0.11: GPU `dev` (ordinal) runs at the --table-mclock low memory lock right now (set by this run, on the pure table path).
pub fn table_low(dev: i32) -> bool {
    let g = state().lock().unwrap();
    g.as_ref().is_some_and(|st| st.items.iter().any(|it| it.ord == dev && it.tab_mhz > 0 && it.tab_low))
}

pub fn table_mem(dev: i32, low: bool) -> Option<bool> {
    let mut g = state().lock().unwrap();
    let st = g.as_mut().filter(|st| !st.suspended)?;
    let nv = &st.nv;
    let it = st.items.iter_mut().find(|it| it.ord == dev && it.tab_mhz > 0)?;
    if it.tab_low == low {
        return None;
    }
    let r = if low {
        nv.set_mem_locked(it.dev, it.tab_mhz)
    } else if it.mem_locked {
        nv.set_mem_locked(it.dev, it.mem_mhz)
    } else {
        put_back(nv, it.dev, Kind::Mem, it.mem_prev, it.pos, it.n);
        Ok(())
    };
    match r {
        Ok(()) => {
            it.tab_low = low;
            it.tab_used |= low;
            Some(low)
        }
        Err(e) => {
            lg(format!(
                "GPU{dev} (bus {}): --table-mclock: cannot {} ({e:#}); the table memory lock is off for this GPU",
                it.bus,
                if low { format!("lock the memory to {} MHz", it.tab_mhz) } else { "give the full memory clock back".to_string() }
            ));
            it.tab_mhz = 0;
            None
        }
    }
}

/// Puts back every clock setting this run applied (the lock from before, else default), and removes the marker.  Safe to
/// call more than once.
pub fn restore_all() {
    crate::nvml::quiet::set_exiting(); // 1.0.12: a GPU's quiet period after a fault: its time part only now
    // 1.0.12: the --power-limit of this run too (every exit path calls this one)
    crate::powerlimit::restore_all();
    let Some(st) = state().lock().unwrap().take() else { return };
    let mut lines = Vec::new();
    for it in &st.items {
        let mut what = Vec::new();
        if it.core_locked {
            what.push(put_back(&st.nv, it.dev, Kind::Core, it.core_prev, it.pos, it.n));
        }
        if it.mem_touched() {
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

/// Idle mode (1.7.5): the PC is in use, the clocks go back to what they were before the miner (a lock would cap a game); the
/// marker stays, `restore_all` still works.  `resume` sets this run's locks / offset again.
pub fn suspend() {
    crate::powerlimit::suspend();
    let mut g = state().lock().unwrap();
    let Some(st) = g.as_mut() else { return };
    if st.suspended {
        return;
    }
    st.suspended = true;
    for it in &st.items {
        if it.core_locked {
            put_back(&st.nv, it.dev, Kind::Core, it.core_prev, it.pos, it.n);
        }
        if it.mem_touched() {
            put_back(&st.nv, it.dev, Kind::Mem, it.mem_prev, it.pos, it.n);
        }
        if it.mem_off {
            let _ = st.nv.set_mem_offset(it.dev, 0);
        }
    }
}

pub fn resume() {
    crate::powerlimit::resume();
    let mut g = state().lock().unwrap();
    let Some(st) = g.as_mut() else { return };
    if !st.suspended {
        return;
    }
    st.suspended = false;
    for it in &st.items {
        let r = [
            it.core_locked.then(|| st.nv.set_core_locked(it.dev, it.core_mhz)),
            it.mem_want().map(|m| st.nv.set_mem_locked(it.dev, m)),
            it.mem_off.then(|| st.nv.set_mem_offset(it.dev, it.mem_off_mhz)),
        ];
        for e in r.into_iter().flatten().filter_map(|r| r.err()) {
            lg(format!("GPU bus {}: cannot set the clocks again after the pause ({e:#})", it.bus));
        }
    }
}

/// An OC apply was seen on `bus` (its offset was reset): this run's clock locks go back first, before the offset is set again or a
/// check runs at it (1.7.8; the live test on an RTX 3070: a verified +240 re-checked at the unlocked 2055 MHz gave Xid 43 and was
/// stepped back).  Sets the locks of the card unconditionally; false = nothing of this run on that card / paused by idle mode.
pub fn reassert(bus: &str) -> bool {
    reassert_as(bus, "an OC apply was seen")
}

/// 1.0.12 (nvml.rs resume hook): the GPU's NVML calls resume after its fault (a driver reset may have dropped this run's locks).
pub fn reassert_after_fault(bus: &str) -> bool {
    reassert_as(bus, "after the GPU's fault")
}

fn reassert_as(bus: &str, why: &str) -> bool {
    // 1.0.12: this run's --power-limit is set again too when the OC apply changed it
    let pl = crate::powerlimit::reassert(bus);
    let g = state().lock().unwrap();
    let Some(st) = g.as_ref().filter(|st| !st.suspended) else { return pl };
    let mut any = pl;
    for it in st.items.iter().filter(|it| it.bus.eq_ignore_ascii_case(bus)) {
        let r = [
            it.core_locked.then(|| st.nv.set_core_locked(it.dev, it.core_mhz)),
            it.mem_want().map(|m| st.nv.set_mem_locked(it.dev, m)),
            it.mem_off.then(|| st.nv.set_mem_offset(it.dev, it.mem_off_mhz)),
        ];
        any |= r.iter().any(Option::is_some);
        for e in r.into_iter().flatten().filter_map(|r| r.err()) {
            lg(format!("GPU bus {}: cannot set the clocks again ({e:#})", it.bus));
        }
        if it.core_locked || it.mem_want().is_some() {
            lg(format!("GPU bus {}: the clock locks of this run are set again first ({why}), before the offset", it.bus));
        }
    }
    any
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
    let parts = crate::uv::list_parts(raw);
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

/// 1.0.12: the L=3 gain the engine needs with --table-mclock (R111.md section 10: the 810 lock gives the pure table ~+10 % MA/W;
/// the L=3 path runs at the full memory clock): NEBGPU_L3_AUTO_GAIN
pub const TABLE_MCLOCK_L3_GAIN: &str = "0.10";

/// 1.0.12: a --table-mclock value that locks the memory of at least one GPU (a list may leave GPUs out with '-' or 0)
pub fn l3_gain_wanted(v: &str) -> bool {
    v.split(',').any(|x| x.trim().parse::<u32>().is_ok_and(|m| m > 0))
}

#[cfg(test)]
mod tests {
    #[test]
    fn table_mclock_asks_for_the_l3_gain() {
        assert!(l3_gain_wanted("810"));
        assert!(l3_gain_wanted("-,810,-"));
        assert!(!l3_gain_wanted("-,-"));
        assert!(!l3_gain_wanted("0"));
    }

    use super::*;

    #[test]
    fn cmfd_skips_the_hiveos_memory_lock() {
        assert_eq!(hive_mem_choice(Some(810), true), (None, Some(810)));
        assert_eq!(hive_mem_choice(None, true), (None, None));
        assert_eq!(hive_mem_choice(Some(810), false), (Some(810), None)); // Pearl / NOID keep it
        assert!(!ClockOpts { no_hive_mem: true, ..Default::default() }.any());
    }

    #[test]
    fn mem_lock_check_decision() {
        let busy = |v: &[u32]| v.iter().map(|&m| (m, 99)).collect::<Vec<_>>();
        // RTX 3070: lock 405, the card computes at 810 -> warning with the real clock
        assert_eq!(mem_lock_verdict(405, &busy(&[810, 810, 810])), Some(810));
        // holds (within 30 MHz / 5 %)
        assert_eq!(mem_lock_verdict(810, &busy(&[810, 810, 810])), None);
        assert_eq!(mem_lock_verdict(5001, &busy(&[5001, 4990, 5001])), None);
        assert_eq!(mem_lock_verdict(810, &busy(&[835, 810, 790])), None);
        // lower than the lock (5001 -> 4000)
        assert_eq!(mem_lock_verdict(5001, &busy(&[4000, 4000, 4000])), Some(4000));
        // tolerance is 5 % on a big clock
        assert_eq!(mem_lock_verdict(7000, &busy(&[7300, 7300, 7300])), None);
        assert_eq!(mem_lock_verdict(7000, &busy(&[7400, 7400, 7400])), Some(7400));
        // one sample on the lock = not steady
        assert_eq!(mem_lock_verdict(405, &busy(&[810, 405, 810])), None);
        // both sides of the lock = not steady
        assert_eq!(mem_lock_verdict(5001, &busy(&[4000, 7000, 4000])), None);
        // too few samples, no lock
        assert_eq!(mem_lock_verdict(405, &busy(&[810, 810])), None);
        assert_eq!(mem_lock_verdict(0, &busy(&[810, 810, 810])), None);
        // idle samples never count (a low clock while idling is no error)
        let mut s = vec![(405, 0), (405, 3)];
        assert_eq!(mem_lock_verdict(810, &s), None);
        s.extend(busy(&[810, 810, 810]));
        assert_eq!(mem_lock_verdict(810, &s), None);
        assert_eq!(mem_lock_verdict(405, &s), Some(810));
        let idle = vec![(810, 0), (810, 0), (810, 5), (810, 20)];
        assert_eq!(mem_lock_verdict(405, &idle), None);
    }

    #[test]
    fn lock_keeper_judges_a_lost_core_lock() {
        let mut t = Track::default();
        // never seen holding: not chased (the clock above the lock from the start)
        assert_eq!(keep_judge(&mut t, Kind::Core, 1500, 1800, 100), None);
        assert_eq!(keep_judge(&mut t, Kind::Core, 1500, 1800, 100), None);
        // holds (with the 15 MHz bin noise), then lost: two busy samples in a row
        assert_eq!(keep_judge(&mut t, Kind::Core, 1500, 1500, 100), None);
        assert_eq!(keep_judge(&mut t, Kind::Core, 1500, 1515, 100), None);
        assert_eq!(keep_judge(&mut t, Kind::Core, 1500, 1860, 100), None); // first miss
        assert_eq!(keep_judge(&mut t, Kind::Core, 1500, 1500, 100), None); // a hold in between resets the count
        assert_eq!(keep_judge(&mut t, Kind::Core, 1500, 1860, 100), None);
        assert_eq!(keep_judge(&mut t, Kind::Core, 1500, 1860, 100), Some(1860));
        // after a re-set it has to hold again before it is judged again
        assert_eq!(keep_judge(&mut t, Kind::Core, 1500, 1860, 100), None);
        // idle samples and a clock below the lock (power limit) change nothing
        let mut t = Track::default();
        assert_eq!(keep_judge(&mut t, Kind::Core, 1500, 1500, 100), None);
        for _ in 0..5 {
            assert_eq!(keep_judge(&mut t, Kind::Core, 1500, 1900, 10), None);
            assert_eq!(keep_judge(&mut t, Kind::Core, 1500, 1200, 100), None);
        }
    }

    #[test]
    fn lock_keeper_judges_a_lost_memory_lock() {
        let mut t = Track::default();
        assert_eq!(keep_judge(&mut t, Kind::Mem, 810, 810, 99), None);
        assert_eq!(keep_judge(&mut t, Kind::Mem, 810, 7001, 99), None);
        assert_eq!(keep_judge(&mut t, Kind::Mem, 810, 7001, 99), Some(7001));
        // a lock the card never honoured (RTX 3070: 405 accepted, runs 810) is not chased
        let mut t = Track::default();
        for _ in 0..6 {
            assert_eq!(keep_judge(&mut t, Kind::Mem, 405, 810, 99), None);
        }
    }

    #[test]
    fn mem_lock_message_is_a_yellow_warning() {
        let m = mem_lock_message(1, "00000000:04:00.0", 405, 810);
        assert!(m.contains("GPU1") && m.contains("lock 405 MHz accepted") && m.contains("runs at 810 MHz under load"));
        assert_eq!(crate::color::kind(&format!("clocks: {m}")), crate::color::Kind::Warn);
        let m = mem_lock_message(0, "x", 5001, 4000);
        assert!(m.contains("4000 MHz") && m.contains("not held"));
        assert_eq!(crate::color::kind(&format!("clocks: {m}")), crate::color::Kind::Warn);
    }

    #[test]
    fn restore_env_values() {
        assert_eq!(prev_from_env_value("1200", 0, 3), Some(Prev::Locked(1200)));
        assert_eq!(prev_from_env_value("0", 2, 3), Some(Prev::Unlocked));
        assert_eq!(prev_from_env_value("1200,-,1400", 1, 3), None);
        assert_eq!(prev_from_env_value("1200,-,1400", 2, 3), Some(Prev::Locked(1400)));
        assert_eq!(prev_from_env_value("1200,1400", 0, 3), None); // wrong count: not used
    }

    #[test]
    fn memory_policy_per_coin_never_leaks() {
        // a HiveOS profile with Pearl's 810 MHz memory lock on a 3070 (max 7001)
        let (h, max) = (Some(810), Some(7001));
        // Pearl / NOID / PROX: the profile's 810 is applied (Pearl's lock, as before 1.8.0)
        assert_eq!(mem_plan(MemPolicy::HiveProfile, h, max), (Some(810), None, None));
        // NEB: never at 810: the maximum while mining, the 810 put back on exit
        assert_eq!(mem_plan(MemPolicy::FullClock, h, max), (None, Some(810), Some(7001)));
        // CMFD: the 810 skipped, nothing set
        assert_eq!(mem_plan(MemPolicy::NoHiveLock, h, max), (None, Some(810), None));
        // a profile already at the maximum / no profile lock: NEB sets nothing
        assert_eq!(mem_plan(MemPolicy::FullClock, Some(7001), max), (None, Some(7001), None));
        assert_eq!(mem_plan(MemPolicy::FullClock, None, max), (None, None, None));
        assert_eq!(mem_plan(MemPolicy::HiveProfile, None, max), (None, None, None));
        for p in [MemPolicy::HiveProfile, MemPolicy::NoHiveLock, MemPolicy::FullClock] {
            assert_eq!(ClockOpts::policy(p).mem_policy(), p);
            assert!(!ClockOpts::policy(p).any(), "a policy alone changes no clock");
        }
        // the coins' policies (coins.rs): Pearl never gets NEB's settings and NEB never Pearl's 810
        let pearl = ClockOpts::policy(crate::coins::spec(crate::coins::Coin::Pearl).mem);
        assert!(!pearl.hive_mem_to_max && !pearl.no_hive_mem && pearl.table_mhz.is_empty());
        let neb = ClockOpts::policy(crate::coins::spec(crate::coins::Coin::Neb).mem);
        assert!(neb.hive_mem_to_max && neb.no_hive_mem);
        assert_eq!(mem_plan(neb.mem_policy(), h, max).0, None);
    }

    #[test]
    fn ampere_hint_only_for_rtx30_without_or_above_1530() {
        let n = "NVIDIA GeForce RTX 3070";
        assert!(ampere_hint_needed(n, Some((8, 6)), None));
        assert!(ampere_hint_needed(n, Some((8, 6)), Some(1560)));
        assert!(!ampere_hint_needed(n, Some((8, 6)), Some(1530)));
        assert!(!ampere_hint_needed(n, Some((8, 6)), Some(1410)));
        assert!(!ampere_hint_needed("NVIDIA GeForce RTX 4070", Some((8, 9)), None));
        assert!(!ampere_hint_needed("NVIDIA CMP 90HX", Some((8, 6)), None));
        assert!(!ampere_hint_needed(n, Some((8, 9)), None));
    }

    #[test]
    fn a_core_lock_that_never_holds_is_set_again_then_reported() {
        let mut t = Track::default();
        // 1400 locked, the card runs at 1470..2055 under load: 3 samples -> set again, twice; then broken
        let mut seen = Vec::new();
        for _ in 0..9 {
            let h = hold_judge(&mut t, 1400, 1650, 100);
            if h != Hold::Fine {
                seen.push(h);
            }
        }
        assert_eq!(seen, vec![Hold::SetAgain(1650), Hold::SetAgain(1650), Hold::Broken(1650)]);
        // idle samples and samples within 3 bins count nothing
        let mut t = Track::default();
        for _ in 0..10 {
            assert_eq!(hold_judge(&mut t, 1400, 2055, 10), Hold::Fine);
            assert_eq!(hold_judge(&mut t, 1400, 1440, 100), Hold::Fine);
        }
        // a lock seen holding is the keeper's (keep_judge), not this
        let mut t = Track { armed: true, ..Default::default() };
        for _ in 0..10 {
            assert_eq!(hold_judge(&mut t, 1400, 1650, 100), Hold::Fine);
        }
        assert_eq!(hold_tol(1400), 45);
        assert_eq!(hold_tol(2100), 63);
    }

    #[test]
    fn a_memory_lock_left_by_the_oc_profile_is_reported() {
        // RTX 2060 SUPER (max 7001): 810 MHz under load with the profile's 810 lock = stuck; full clock = fine
        assert!(mem_at_lock(810, 810, 7001));
        assert!(mem_at_lock(840, 810, 7001));
        assert!(!mem_at_lock(7001, 810, 7001));
        assert!(!mem_at_lock(6800, 810, 7001));
        assert_eq!(mem_stuck("0000:BB:00.0"), None);
        mark_mem_stuck("0000:BB:00.0", 2, 810);
        assert_eq!(mem_stuck("0000:bb:00.0"), Some(810));
        assert_eq!(mem_stuck_message(810), "NEB runs ~6x slower: memory locked to 810 MHz by the OC profile — remove the memory lock");
    }

    #[test]
    fn a_lock_not_held_is_remembered_per_bus() {
        assert_eq!(not_held("0000:AA:00.0"), None);
        mark_not_held("0000:AA:00.0", 5, 1400, 1470);
        assert_eq!(not_held("0000:aa:00.0"), Some((1400, 1470)));
        assert!(unheld_tag().contains("[LOCK NOT HELD: ") && unheld_tag().contains("GPU5"), "{}", unheld_tag());
        assert!(not_held_message(5, 1400, 1470).contains("WARNING GPU5: the core clock lock 1400 MHz does not hold (runs at 1470 MHz): auto-uv and auto-lock are off for this GPU"));
    }
}
