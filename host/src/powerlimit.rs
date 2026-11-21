// FreeForGem — see LICENSE
//! Optional GPU power limit via NVML (`--power-limit N`, alias `--pl N`; FFGN 1.0.12, shared host code: FreeForGem 1.8.0 takes it
//! with the NEB module).  One value for all mining GPUs or one per GPU in --devices order (`--pl 140,115,140`; `-` = leave that
//! GPU's limit as it is, e.g. the HiveOS one), like --lock-cclock / --uv-max.  Needs root/Administrator, like the clock locks.
//!  * the value is clamped to the card's range (nvmlDeviceGetPowerManagementLimitConstraints); the log says what was set and
//!    whether it was clamped;
//!  * the limit the card had before is written to a restore file per GPU (`power-limit-restore-<bus>.json` in the state folder,
//!    next to clocks-restore.json) BEFORE it is changed, and put back on exit, on a signal and by the next start after a crash;
//!    one file per GPU, so two miner processes on different GPUs of one rig (FreeForGem split use) and the per-GPU engine
//!    processes never put back each other's limits.  On HiveOS the OC profile's power limit (PLIMIT) is put back when it has one
//!    (HiveOS may have applied a new OC meanwhile);
//!  * a keeper reads the limit back every `KEEP_POLL_S` s (NVML can read a power limit, unlike a clock lock): when an OC apply
//!    (HiveOS nvidia-oc, another tool) changed it, it is set again (one log line), at most `hiveoc::OC_RESETS_MAX` times an hour
//!    per GPU (then it stands down and says so);
//!  * the same keeper notes, one line per change of state, when the limit binds under load (the driver holds the clocks down for
//!    power: nvmlClocksThrottleReasonSwPowerCap): auto-uv measures with the limit in force (it is set before the tuner starts).
use crate::nvml::{Dev, Nvml};
use crate::uv::PerGpu;
use anyhow::Result;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// nvmlClocksThrottleReasonSwPowerCap (= nvmlClocksEventReasonSwPowerCap): the clocks are held down by the power limit.
pub const SW_POWER_CAP: u64 = 0x4;
/// The highest value the option takes (W); the card's own range clamps it further.
pub const MAX_W: i32 = 2000;
/// A read-back limit within this of ours is ours (NVML keeps the mW it was given; 0.5 W for drivers that round to 1 W).
const TOL_MW: u32 = 500;
const KEEP_POLL_S: u64 = 5;
/// A power-cap sample counts at or above this GPU utilization (an idle card never hits its limit; idle samples change nothing).
const CAP_BUSY_PCT: u32 = 70;
/// Busy samples in a row (`KEEP_POLL_S` apart) that change the "binds" state.
const CAP_NEED: u32 = 3;

/// The NVML calls this module uses: a trait so the decisions are tested with a mock.
pub trait Hw {
    fn limit_mw(&self, d: Dev) -> Option<u32>;
    fn constraints_mw(&self, d: Dev) -> Option<(u32, u32)>;
    fn set_limit_mw(&self, d: Dev, mw: u32) -> Result<()>;
    fn reasons(&self, d: Dev) -> Option<u64>;
    fn util(&self, d: Dev) -> Option<u32>;
    /// The HiveOS OC profile's power limit of the GPU (W), `None` = not HiveOS / none in the profile.
    fn hive_w(&self, d: Dev) -> Option<u32>;
}

impl Hw for Nvml {
    fn limit_mw(&self, d: Dev) -> Option<u32> {
        self.pm_limit_mw(d)
    }
    fn constraints_mw(&self, d: Dev) -> Option<(u32, u32)> {
        self.pm_constraints_mw(d)
    }
    fn set_limit_mw(&self, d: Dev, mw: u32) -> Result<()> {
        self.set_pm_limit_mw(d, mw)
    }
    fn reasons(&self, d: Dev) -> Option<u64> {
        self.throttle_reasons(d)
    }
    fn util(&self, d: Dev) -> Option<u32> {
        self.util_gpu(d)
    }
    fn hive_w(&self, d: Dev) -> Option<u32> {
        let h = crate::hiveoc::load()?;
        h.power_limit(self.index(d)? as usize)
    }
}

/// Parses `--power-limit` / `--pl`: one value for all GPUs, or a list with one value per GPU in --devices order (`-` / empty =
/// leave that GPU alone); the count is checked like --uv-max lists (`check_count`, once the GPUs are known).
pub fn parse(raw: &str, flag: &str) -> Result<PerGpu> {
    let p = PerGpu::parse(raw, flag).map_err(|e| anyhow::anyhow!("{}", format!("{e:#}").replace("whole number of MHz", "whole number of watts")))?;
    for v in p.values() {
        anyhow::ensure!((1..=MAX_W).contains(&v), "{flag} {v}: a power limit in watts, 1..{MAX_W} (the card's own range clamps it further)");
    }
    Ok(p)
}

fn w(mw: u32) -> String {
    let x = mw as f64 / 1000.0;
    if mw % 1000 == 0 { format!("{x:.0} W") } else { format!("{x:.1} W") }
}

/// The limit to set for `want_w` on a card with the range `cons` (mW), and the range when it had to be clamped.
pub fn clamp_mw(want_w: u32, cons: Option<(u32, u32)>) -> (u32, Option<(u32, u32)>) {
    let want = want_w.saturating_mul(1000);
    match cons {
        Some((lo, hi)) if want < lo => (lo, Some((lo, hi))),
        Some((lo, hi)) if want > hi => (hi, Some((lo, hi))),
        _ => (want, None),
    }
}

/// The text of a clamp (`clamp_mw` gave a range).
fn clamp_text(want_w: u32, set_mw: u32, (lo, hi): (u32, u32)) -> String {
    if want_w.saturating_mul(1000) < lo {
        format!("{want_w} W is below the card's minimum {}: set to {} (clamped)", w(lo), w(set_mw))
    } else {
        format!("{want_w} W is above the card's maximum {}: set to {} (clamped)", w(hi), w(set_mw))
    }
}

/// The limit to put back: the HiveOS OC profile's (re-read now, clamped to the card's range) when it has one, else the limit the
/// card had before this run (`stored`).
pub fn restore_target(stored_mw: u32, hive_w: Option<u32>, cons: Option<(u32, u32)>) -> (u32, &'static str) {
    match hive_w {
        Some(h) => (clamp_mw(h, cons).0, "HiveOS OC profile"),
        None => (stored_mw, "the limit before"),
    }
}

/// The restore file of one GPU (by its PCI bus id; ':' is not allowed in Windows file names).
pub fn restore_path(dir: &Path, bus: &str) -> PathBuf {
    let b: String = bus.to_ascii_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' { c } else { '-' }).collect();
    dir.join(format!("power-limit-restore-{b}.json"))
}

/// What a restore file holds.
#[derive(Clone, Debug, PartialEq)]
pub struct Rec {
    pub bus: String,
    pub prev_mw: u32,
    pub set_mw: u32,
}

impl Rec {
    fn to_json(&self) -> String {
        serde_json::to_string_pretty(&json!({"pid": std::process::id(), "bus": self.bus, "prev_mw": self.prev_mw, "set_mw": self.set_mw})).unwrap()
    }
    fn from_json(txt: &str) -> Option<Rec> {
        let v: serde_json::Value = serde_json::from_str(txt).ok()?;
        Some(Rec {
            bus: v.get("bus")?.as_str()?.to_string(),
            prev_mw: u32::try_from(v.get("prev_mw")?.as_u64()?).ok().filter(|&m| m > 0)?,
            set_mw: v.get("set_mw").and_then(|x| x.as_u64()).and_then(|x| u32::try_from(x).ok()).unwrap_or(0),
        })
    }
}

pub fn write_rec(dir: &Path, r: &Rec) -> Result<()> {
    crate::uv::write_atomic(&restore_path(dir, &r.bus), &r.to_json())
}

pub fn read_rec(dir: &Path, bus: &str) -> Option<Rec> {
    Rec::from_json(&std::fs::read_to_string(restore_path(dir, bus)).ok()?)
}

/// Is the read-back limit not ours (an OC apply changed it)?  No reading = no verdict.
pub fn differs(ours_mw: u32, readback: Option<u32>) -> bool {
    readback.is_some_and(|r| r.abs_diff(ours_mw) > TOL_MW)
}

#[derive(Debug, PartialEq)]
pub enum KeepAct {
    Holds,
    SetAgain(u32),
    StandDown(u32),
}

/// One read-back of the keeper: ours holds, set it again (`allow`: the hourly re-set budget, asked only when needed), or stand down.
pub fn keep_judge(ours_mw: u32, readback: Option<u32>, allow: impl FnOnce() -> bool) -> KeepAct {
    match readback {
        Some(r) if differs(ours_mw, Some(r)) => {
            if allow() {
                KeepAct::SetAgain(r)
            } else {
                KeepAct::StandDown(r)
            }
        }
        _ => KeepAct::Holds,
    }
}

/// "The power limit binds" (SW power cap under load), debounced: `CAP_NEED` busy samples in a row change the state; idle
/// samples and samples without a reading change nothing.  `sample` returns the new state on a change (one log line each).
#[derive(Default, Debug)]
pub struct CapNote {
    pub binding: bool,
    streak: u32,
}

impl CapNote {
    pub fn sample(&mut self, reasons: Option<u64>, util: Option<u32>) -> Option<bool> {
        let (Some(r), Some(u)) = (reasons, util) else { return None };
        if u < CAP_BUSY_PCT {
            return None;
        }
        let cap = r & SW_POWER_CAP != 0;
        if cap == self.binding {
            self.streak = 0;
            return None;
        }
        self.streak += 1;
        if self.streak < CAP_NEED {
            return None;
        }
        self.streak = 0;
        self.binding = cap;
        Some(cap)
    }
}

pub fn cap_message(ord: i32, bus: &str, set_mw: u32, binding: bool) -> String {
    if binding {
        format!("GPU{ord} (bus {bus}): the power limit {} binds under load (the driver holds the clocks down for power, SW power cap); auto-uv measures with this limit in force", w(set_mw))
    } else {
        format!("GPU{ord} (bus {bus}): the power limit {} no longer binds (the card runs below it)", w(set_mw))
    }
}

struct Card {
    dev: Dev,
    ord: i32,
    bus: String,
    prev_mw: u32,
    set_mw: u32,
    resets: crate::hiveoc::ResetLimit,
    stood_down: bool,
    cap: CapNote,
}

struct State {
    nv: Nvml,
    dir: PathBuf,
    cards: Vec<Card>,
    suspended: bool,
}

fn state() -> &'static Mutex<Option<State>> {
    static S: Mutex<Option<State>> = Mutex::new(None);
    &S
}

fn lg(m: impl AsRef<str>) {
    println!("power-limit: {}", m.as_ref());
}

/// Puts one card's limit back (the HiveOS profile's, else the one before); returns the log text.
fn put_back(hw: &dyn Hw, d: Dev, stored_mw: u32) -> String {
    let (target, src) = restore_target(stored_mw, hw.hive_w(d), hw.constraints_mw(d));
    match hw.set_limit_mw(d, target) {
        Ok(()) => format!("power limit {} put back ({src})", w(target)),
        Err(e) => format!("power limit: cannot put {} back ({e:#})", w(target)),
    }
}

/// Puts back the limits a run that did not exit cleanly left on `buses` (their restore files), then deletes those files.
/// Only the files of these GPUs: another miner process on the other GPUs of the rig keeps its own.
pub fn recover(dir: &Path, buses: &[String]) {
    let todo: Vec<(String, Option<Rec>)> =
        buses.iter().filter(|b| restore_path(dir, b).is_file()).map(|b| (b.clone(), read_rec(dir, b))).collect();
    if todo.is_empty() {
        return;
    }
    let Ok(nv) = Nvml::load() else { return }; // the files stay for a start that has NVML
    for (bus, rec) in todo {
        if let Some(r) = rec {
            if let Ok(d) = nv.by_bus(&bus) {
                lg(format!("GPU bus {bus}: left at {} by an earlier run that did not exit cleanly: {}", w(r.set_mw), put_back(&nv, d, r.prev_mw)));
            }
        }
        let _ = std::fs::remove_file(restore_path(dir, &bus));
    }
}

/// Sets the requested power limits on `devices` (PCI bus ids `buses`, same order).  Returns a guard that puts them back on drop;
/// `None` when nothing was asked, NVML is missing, or no card could be changed (mining goes on as is).
pub fn apply(devices: &[i32], buses: &[String], limits: &PerGpu, dir: &Path) -> Option<Guard> {
    if !limits.is_set() {
        return None;
    }
    let nv = match Nvml::load() {
        Ok(n) => n,
        Err(e) => {
            lg(format!("NVML not available ({e:#}); --power-limit skipped"));
            return None;
        }
    };
    let mut cards = Vec::new();
    for (pos, (&ord, bus)) in devices.iter().zip(buses).enumerate() {
        let Some(want) = limits.get(pos).and_then(|v| u32::try_from(v).ok()) else { continue };
        let d = match nv.by_bus(bus) {
            Ok(d) => d,
            Err(e) => {
                lg(format!("GPU{ord} (bus {bus}): NVML handle failed ({e:#}); its power limit is left alone"));
                continue;
            }
        };
        if let Some(c) = apply_one(&nv, d, ord, bus, want, dir) {
            cards.push(c);
        }
    }
    if cards.is_empty() {
        return None;
    }
    *state().lock().unwrap() = Some(State { nv, dir: dir.to_path_buf(), cards, suspended: false });
    spawn_keeper();
    Some(Guard)
}

fn apply_one(hw: &dyn Hw, d: Dev, ord: i32, bus: &str, want: u32, dir: &Path) -> Option<Card> {
    let Some(prev) = hw.limit_mw(d) else {
        lg(format!("GPU{ord} (bus {bus}): the power limit is not readable (NVML); left alone (it could not be put back on exit)"));
        return None;
    };
    let cons = hw.constraints_mw(d);
    let (set, clamped) = clamp_mw(want, cons);
    // the file first: a crash right after the set still gets the old limit back at the next start
    let rec = Rec { bus: bus.to_string(), prev_mw: prev, set_mw: set };
    if let Err(e) = write_rec(dir, &rec) {
        lg(format!("GPU{ord} (bus {bus}): cannot write {} ({e:#}); the limit is still put back on a normal exit", restore_path(dir, bus).display()));
    }
    if let Err(e) = hw.set_limit_mw(d, set) {
        let _ = std::fs::remove_file(restore_path(dir, bus));
        lg(format!("GPU{ord} (bus {bus}): cannot set the power limit {} ({e:#}); needs root/Administrator and a card that allows it", w(set)));
        return None;
    }
    let now = hw.limit_mw(d);
    lg(format!(
        "GPU{ord} (bus {bus}): power limit {}{}{}; was {}, put back on exit{}",
        w(set),
        clamped.map_or(String::new(), |c| format!(" ({})", clamp_text(want, set, c))),
        cons.map_or(String::new(), |(lo, hi)| format!(", the card allows {}..{}", w(lo), w(hi))),
        w(prev),
        if differs(set, now) { format!("; WARNING the card reads back {}", w(now.unwrap_or(0))) } else { String::new() }
    ));
    Some(Card { dev: d, ord, bus: bus.to_string(), prev_mw: prev, set_mw: set, resets: Default::default(), stood_down: false, cap: CapNote::default() })
}

/// One keeper pass over the cards (the lock of the state is held by the caller); returns the log lines.
fn keep_pass(hw: &dyn Hw, cards: &mut [Card]) -> Vec<String> {
    let mut out = Vec::new();
    for c in cards.iter_mut() {
        if !c.stood_down {
            match keep_judge(c.set_mw, hw.limit_mw(c.dev), || c.resets.allow()) {
                KeepAct::Holds => {}
                KeepAct::SetAgain(found) => {
                    let r = hw.set_limit_mw(c.dev, c.set_mw);
                    out.push(format!(
                        "GPU{} (bus {}): the power limit {} was changed to {} by an OC apply (HiveOS nvidia-oc or another tool): set again{}",
                        c.ord,
                        c.bus,
                        w(c.set_mw),
                        w(found),
                        r.err().map_or(String::new(), |e| format!(" (failed: {e:#})"))
                    ));
                    crate::faults::note_foreign_change(&c.bus);
                }
                KeepAct::StandDown(found) => {
                    c.stood_down = true;
                    out.push(format!(
                        "GPU{} (bus {}): the power limit {} was changed again (now {}): {} times within an hour already, so the miner stands down on it (restart the miner once the OC is settled)",
                        c.ord,
                        c.bus,
                        w(c.set_mw),
                        w(found),
                        crate::hiveoc::OC_RESETS_MAX
                    ));
                }
            }
        }
        if let Some(b) = c.cap.sample(hw.reasons(c.dev), hw.util(c.dev)) {
            out.push(cap_message(c.ord, &c.bus, c.set_mw, b));
        }
    }
    out
}

fn spawn_keeper() {
    let _ = std::thread::Builder::new().name("pl-keep".into()).spawn(|| {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(KEEP_POLL_S));
            let lines = {
                let mut g = state().lock().unwrap();
                let Some(st) = g.as_mut() else { return }; // restored / shut down
                if st.suspended {
                    continue;
                }
                let State { nv, cards, .. } = st;
                keep_pass(nv, cards)
            };
            for l in lines {
                lg(l);
            }
        }
    });
}

/// This run's power limit on `bus` (W) and whether it binds now: the stats line and the API.
pub fn info(bus: &str) -> Option<(f64, bool)> {
    let g = state().lock().unwrap();
    let c = g.as_ref()?.cards.iter().find(|c| c.bus.eq_ignore_ascii_case(bus))?;
    Some((c.set_mw as f64 / 1000.0, c.cap.binding))
}

/// Puts back every limit this run set and removes the restore files.  Safe to call more than once.
pub fn restore_all() {
    crate::nvml::quiet::set_exiting(); // 1.0.12: a GPU's quiet period after a fault: its time part only now
    let Some(st) = state().lock().unwrap().take() else { return };
    let mut lines = Vec::new();
    for c in &st.cards {
        lines.push(format!("GPU bus {}: {}", c.bus, put_back(&st.nv, c.dev, c.prev_mw)));
        let _ = std::fs::remove_file(restore_path(&st.dir, &c.bus));
    }
    // logged only after every card is put back (the log pipe may be gone at shutdown)
    for l in lines {
        lg(l);
    }
}

/// Idle mode: the owner's limits are back while the PC is in use (the restore files stay); `resume` sets this run's again.
pub fn suspend() {
    let mut g = state().lock().unwrap();
    let Some(st) = g.as_mut().filter(|st| !st.suspended) else { return };
    st.suspended = true;
    for c in &st.cards {
        put_back(&st.nv, c.dev, c.prev_mw);
    }
}

pub fn resume() {
    let mut g = state().lock().unwrap();
    let Some(st) = g.as_mut().filter(|st| st.suspended) else { return };
    st.suspended = false;
    for c in &st.cards {
        if let Err(e) = st.nv.set_limit_mw(c.dev, c.set_mw) {
            lg(format!("GPU bus {}: cannot set the power limit again after the pause ({e:#})", c.bus));
        }
    }
}

/// An OC apply was seen on `bus` (clocks::reassert): this run's limit is set again at once when it was changed.
pub fn reassert(bus: &str) -> bool {
    let g = state().lock().unwrap();
    let Some(st) = g.as_ref().filter(|st| !st.suspended) else { return false };
    let Some(c) = st.cards.iter().find(|c| c.bus.eq_ignore_ascii_case(bus) && !c.stood_down) else { return false };
    if differs(c.set_mw, st.nv.limit_mw(c.dev)) {
        if let Err(e) = st.nv.set_limit_mw(c.dev, c.set_mw) {
            lg(format!("GPU bus {}: cannot set the power limit {} again ({e:#})", c.bus, w(c.set_mw)));
        } else {
            lg(format!("GPU bus {}: the power limit {} is set again (an OC apply was seen)", c.bus, w(c.set_mw)));
        }
    }
    true
}

/// Puts the limits back when dropped (normal shutdown); signal handlers call `restore_all` (through clocks::restore_all).
pub struct Guard;
impl Drop for Guard {
    fn drop(&mut self) {
        restore_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// NVML stand-in: one card.
    struct Mock {
        limit: RefCell<Option<u32>>,
        cons: Option<(u32, u32)>,
        fail_set: bool,
        sets: RefCell<Vec<u32>>,
        reasons: RefCell<Option<u64>>,
        util: RefCell<Option<u32>>,
        hive: Option<u32>,
    }

    impl Mock {
        fn new(limit: u32, cons: (u32, u32)) -> Mock {
            Mock { limit: RefCell::new(Some(limit)), cons: Some(cons), fail_set: false, sets: RefCell::new(vec![]), reasons: RefCell::new(Some(0)), util: RefCell::new(Some(100)), hive: None }
        }
    }

    impl Hw for Mock {
        fn limit_mw(&self, _: Dev) -> Option<u32> {
            *self.limit.borrow()
        }
        fn constraints_mw(&self, _: Dev) -> Option<(u32, u32)> {
            self.cons
        }
        fn set_limit_mw(&self, _: Dev, mw: u32) -> Result<()> {
            if self.fail_set {
                anyhow::bail!("nvmlDeviceSetPowerManagementLimit: Insufficient Permissions (code 4)");
            }
            self.sets.borrow_mut().push(mw);
            *self.limit.borrow_mut() = Some(mw);
            Ok(())
        }
        fn reasons(&self, _: Dev) -> Option<u64> {
            *self.reasons.borrow()
        }
        fn util(&self, _: Dev) -> Option<u32> {
            *self.util.borrow()
        }
        fn hive_w(&self, _: Dev) -> Option<u32> {
            self.hive
        }
    }

    fn dev() -> Dev {
        Dev(std::ptr::null_mut())
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ffg-pl-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn parses_like_the_per_gpu_lists() {
        assert_eq!(parse("140", "--pl").unwrap(), PerGpu::All(140));
        assert_eq!(parse(" 140 ", "--power-limit").unwrap(), PerGpu::All(140));
        assert_eq!(parse("-", "--pl").unwrap(), PerGpu::Unset);
        let p = parse("140,115,-,140", "--pl").unwrap();
        assert_eq!(p, PerGpu::List(vec![Some(140), Some(115), None, Some(140)]));
        assert_eq!((p.get(0), p.get(1), p.get(2), p.get(3)), (Some(140), Some(115), None, Some(140)));
        assert_eq!(parse("140 115", "--pl").unwrap(), PerGpu::List(vec![Some(140), Some(115)])); // HiveOS Extra config without quotes
        assert_eq!(parse("140,,115", "--pl").unwrap(), PerGpu::List(vec![Some(140), None, Some(115)]));
        // one value applies to every GPU
        let all = parse("150", "--pl").unwrap();
        assert!((0..8).all(|i| all.get(i) == Some(150)));
        assert!(all.check_count(8, "--pl").is_ok());
        // a list must name every mining GPU, like --uv-max: shorter or longer is an error, never extended
        let l = parse("140,115", "--pl").unwrap();
        assert!(l.check_count(2, "--pl").is_ok());
        assert!(l.check_count(3, "--pl").is_err());
        assert!(l.check_count(1, "--pl").is_err());
        // bad values
        assert!(parse("abc", "--pl").is_err());
        assert!(parse("0", "--pl").is_err());
        assert!(parse("-50", "--pl").is_err());
        assert!(parse("140,x", "--pl").is_err());
        assert!(parse("99999", "--pl").is_err());
        assert!(format!("{:#}", parse("1x", "--pl").unwrap_err()).contains("watts"));
    }

    #[test]
    fn clamps_to_the_card_range() {
        let c = Some((100_000, 240_000));
        assert_eq!(clamp_mw(140, c), (140_000, None));
        assert_eq!(clamp_mw(100, c), (100_000, None));
        assert_eq!(clamp_mw(240, c), (240_000, None));
        assert_eq!(clamp_mw(300, c), (240_000, Some((100_000, 240_000))));
        assert_eq!(clamp_mw(50, c), (100_000, Some((100_000, 240_000))));
        assert_eq!(clamp_mw(140, None), (140_000, None)); // range not readable: the driver refuses what it does not allow
        assert!(clamp_text(300, 240_000, (100_000, 240_000)).contains("above the card's maximum 240 W: set to 240 W"));
        assert!(clamp_text(50, 100_000, (100_000, 240_000)).contains("below the card's minimum 100 W: set to 100 W"));
        assert_eq!(w(115_500), "115.5 W");
    }

    #[test]
    fn apply_one_writes_the_file_first_and_clamps() {
        let dir = tmp("apply");
        let m = Mock::new(220_000, (100_000, 240_000));
        let c = apply_one(&m, dev(), 0, "00000000:01:00.0", 300, &dir).unwrap();
        assert_eq!((c.prev_mw, c.set_mw), (220_000, 240_000));
        assert_eq!(*m.sets.borrow(), vec![240_000]);
        assert_eq!(read_rec(&dir, "00000000:01:00.0"), Some(Rec { bus: "00000000:01:00.0".into(), prev_mw: 220_000, set_mw: 240_000 }));
        // the set fails (no root): no file left behind, no card
        let mut m = Mock::new(220_000, (100_000, 240_000));
        m.fail_set = true;
        assert!(apply_one(&m, dev(), 1, "00000000:02:00.0", 140, &dir).is_none());
        assert!(!restore_path(&dir, "00000000:02:00.0").exists());
        // the limit is not readable: nothing to put back, left alone
        let m = Mock::new(0, (100_000, 240_000));
        *m.limit.borrow_mut() = None;
        assert!(apply_one(&m, dev(), 2, "00000000:03:00.0", 140, &dir).is_none());
        assert!(m.sets.borrow().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn restore_file_round_trip() {
        let dir = tmp("rec");
        let r = Rec { bus: "00000000:0D:00.0".into(), prev_mw: 220_000, set_mw: 140_000 };
        write_rec(&dir, &r).unwrap();
        let p = restore_path(&dir, "00000000:0D:00.0");
        assert_eq!(p.file_name().unwrap().to_str().unwrap(), "power-limit-restore-00000000-0d-00.0.json");
        assert_eq!(restore_path(&dir, "00000000:0d:00.0"), p); // the bus id in either case
        assert_eq!(read_rec(&dir, "00000000:0D:00.0"), Some(r));
        // one file per GPU: another GPU's file is its own
        assert_eq!(read_rec(&dir, "00000000:0E:00.0"), None);
        assert_ne!(restore_path(&dir, "00000000:0E:00.0"), p);
        // a broken file gives nothing
        std::fs::write(&p, "{not json").unwrap();
        assert_eq!(read_rec(&dir, "00000000:0D:00.0"), None);
        assert_eq!(Rec::from_json(r#"{"bus":"x","prev_mw":0}"#), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn put_back_takes_the_hiveos_profile_first() {
        assert_eq!(restore_target(220_000, None, Some((100_000, 240_000))), (220_000, "the limit before"));
        assert_eq!(restore_target(220_000, Some(150), Some((100_000, 240_000))), (150_000, "HiveOS OC profile"));
        assert_eq!(restore_target(220_000, Some(400), Some((100_000, 240_000))).0, 240_000);
        let mut m = Mock::new(140_000, (100_000, 240_000));
        assert!(put_back(&m, dev(), 220_000).contains("220 W put back"));
        m.hive = Some(130);
        assert!(put_back(&m, dev(), 220_000).contains("130 W put back (HiveOS OC profile)"));
        assert_eq!(*m.sets.borrow(), vec![220_000, 130_000]);
    }

    #[test]
    fn reapplies_when_the_read_back_differs() {
        assert_eq!(keep_judge(140_000, Some(140_000), || panic!("not asked when it holds")), KeepAct::Holds);
        assert_eq!(keep_judge(140_000, Some(140_400), || true), KeepAct::Holds); // within 0.5 W
        assert_eq!(keep_judge(140_000, None, || true), KeepAct::Holds); // no reading: no verdict
        assert_eq!(keep_judge(140_000, Some(220_000), || true), KeepAct::SetAgain(220_000));
        assert_eq!(keep_judge(140_000, Some(220_000), || false), KeepAct::StandDown(220_000));
        // the keeper pass: HiveOS nvidia-oc put 220 W back -> 140 W again, one line; then quiet
        let m = Mock::new(140_000, (100_000, 240_000));
        let mut cards = vec![Card { dev: dev(), ord: 3, bus: "b".into(), prev_mw: 220_000, set_mw: 140_000, resets: Default::default(), stood_down: false, cap: CapNote::default() }];
        assert!(keep_pass(&m, &mut cards).is_empty());
        *m.limit.borrow_mut() = Some(220_000);
        let l = keep_pass(&m, &mut cards);
        assert_eq!(l.len(), 1);
        assert!(l[0].contains("GPU3") && l[0].contains("changed to 220 W") && l[0].contains("set again"));
        assert_eq!(*m.limit.borrow(), Some(140_000));
        assert!(keep_pass(&m, &mut cards).is_empty());
        // at most OC_RESETS_MAX re-sets an hour, then it stands down (one line) and leaves the card alone
        for _ in 1..crate::hiveoc::OC_RESETS_MAX {
            *m.limit.borrow_mut() = Some(220_000);
            assert_eq!(keep_pass(&m, &mut cards).len(), 1);
        }
        *m.limit.borrow_mut() = Some(220_000);
        let l = keep_pass(&m, &mut cards);
        assert!(l.len() == 1 && l[0].contains("stands down"));
        assert!(keep_pass(&m, &mut cards).is_empty());
        assert_eq!(*m.limit.borrow(), Some(220_000));
    }

    #[test]
    fn power_cap_note_changes_state_once() {
        let mut n = CapNote::default();
        let cap = Some(SW_POWER_CAP | 0x1);
        // two busy capped samples: not yet; the third: binds
        assert_eq!(n.sample(cap, Some(99)), None);
        assert_eq!(n.sample(cap, Some(99)), None);
        assert_eq!(n.sample(cap, Some(99)), Some(true));
        // stays: no more lines (no spam)
        for _ in 0..10 {
            assert_eq!(n.sample(cap, Some(99)), None);
        }
        // idle samples and no reading change nothing
        for _ in 0..10 {
            assert_eq!(n.sample(Some(0), Some(5)), None);
            assert_eq!(n.sample(None, Some(99)), None);
            assert_eq!(n.sample(Some(0), None), None);
        }
        assert!(n.binding);
        // a single free sample between capped ones does not flip it
        assert_eq!(n.sample(Some(0), Some(99)), None);
        assert_eq!(n.sample(cap, Some(99)), None);
        assert_eq!(n.sample(Some(0), Some(99)), None);
        assert_eq!(n.sample(Some(0), Some(99)), None);
        assert_eq!(n.sample(Some(0), Some(99)), Some(false));
        assert!(!n.binding);
        // other reasons (thermal 0x20, HW slowdown 0x8) are not the power limit
        for _ in 0..5 {
            assert_eq!(n.sample(Some(0x20 | 0x8), Some(99)), None);
        }
        // the keeper's line, once per change
        let m = Mock::new(140_000, (100_000, 240_000));
        *m.reasons.borrow_mut() = cap;
        let mut cards = vec![Card { dev: dev(), ord: 0, bus: "b".into(), prev_mw: 220_000, set_mw: 140_000, resets: Default::default(), stood_down: false, cap: CapNote::default() }];
        let lines: Vec<String> = (0..10).flat_map(|_| keep_pass(&m, &mut cards)).collect();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("power limit 140 W binds under load") && lines[0].contains("auto-uv measures with this limit"));
        assert_eq!(crate::color::kind(&format!("power-limit: {}", lines[0])), crate::color::Kind::Plain);
    }
}
