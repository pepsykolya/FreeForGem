// FreeForGem — see LICENSE
//! FFGN 1.0.13: the safe state of a GPU after a fault of it, BEFORE its new engine (process / context) starts.  Shared by every
//! coin (nothing NEB in here): the coin gives the engine start (`EngineStart`), the GPU side is a trait (`Hw`, NVML in the
//! miner, a mock in the tests).
//!
//! Field case (Rig01, 8x RTX 3070, ffgn-hive 1.0.12, --lock-cclock 1800 --auto-uv --table-mclock 810): an auto-uv step at +365
//! gave Xid 43; 1.0.12's NVML quiet period (no NVML call for that GPU until its NEW engine had mined 3 s) meant that the new
//! engine started while +365 was still applied (the offset can only be lowered through NVML), crashed again (Xid 31), tries
//! 2 and 3 at +365 too, and the healthy card was reported "LOST ... (DEAD) - the rig needs a reboot".  The same for 2 more of
//! its 8 cards.  So, after a fault:
//!  1. wait for the time part of the quiet period (`quiet_min`, the driver's reset);
//!  2. the driver must answer a probe made OUTSIDE this process (`Hw::probe`: a child process does the NVML calls, so a call
//!     into a driver still in its reset can never take the miner down: the Windows 0xC0000005 in nvml.dll of 1.0.12);
//!  3. the quiet period ends now, with a fresh NVML handle (`Hw::resume`), the run's core lock and the coin's memory policy go
//!     back (`Hw::apply_clocks`), then the SAFE offset (`safe_offset`: failed - 2 steps, never above the last confirmed one,
//!     never below the base: the owner's rule 2), read back;
//!  4. only then the engine starts (`SafeState::Ready`).
//!  NVML still not back after `nvml_max`: a GPU whose offset needs no change starts anyway (at its safe offset already); one
//!  that would start at the crashed offset never does: it is reported lost (`SafeState::Lost`, the device cannot be reached).
//! The retry budget (`Budget`) counts only the faults at a SAFE offset (the offset was not lowered for the restart): a fault of
//! an auto-uv step / at a tuned offset is a tuning failure (the offset goes down), not a lost device.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// What the recovery knows about the GPU's offset (MHz).  `crashed`: the offset applied when it faulted (None: this run does
/// not manage its offset: nothing is changed); `confirmed`: the last offset that passed its checks / mined cleanly (the
/// tuner's); `base`: never below (the owner's / profile offset, the tuner's checked start); `step`: the tuner's step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plan {
    pub crashed: Option<i32>,
    pub confirmed: Option<i32>,
    pub base: i32,
    pub step: i32,
}

/// Steps below the crash (the owner's rule 2: "failed - 2 steps", 18.11.2026).
pub const BACK_STEPS: i32 = 2;

/// The offset the GPU restarts at: `crashed - 2 steps`, not above `confirmed`, not below `base`, never above `crashed`.
/// None = leave the offset as it is (not managed by this run).
pub fn safe_offset(p: &Plan) -> Option<i32> {
    let c = p.crashed?;
    if c <= p.base {
        return Some(c);
    }
    let mut t = c - BACK_STEPS * p.step.max(1);
    if let Some(k) = p.confirmed {
        t = t.min(k);
    }
    Some(t.max(p.base).min(c))
}

/// The GPU side of the recovery (NVML in the miner).
pub trait Hw {
    /// Time since the GPU's fault while its NVML calls are paused (None: not paused).
    fn since_fault(&self) -> Option<Duration>;
    /// The driver answers for this GPU (asked outside this process).  Err = not yet.
    fn probe(&self) -> Result<(), String>;
    /// The GPU's NVML calls resume now, with a fresh handle.  Err = no handle for its bus id (the driver lost it).
    fn resume(&self) -> Result<(), String>;
    /// This run's core lock, memory clock policy (the coin's) and power limit, set again.
    fn apply_clocks(&self) -> Result<(), String>;
    fn set_offset(&self, v: i32) -> Result<(), String>;
    fn read_offset(&self) -> Option<i32>;
    fn stopped(&self) -> bool;
    fn sleep(&self, d: Duration) {
        std::thread::sleep(d);
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Opts {
    /// the time part of the quiet period (the driver's reset)
    pub quiet_min: Duration,
    /// a probe again after this long
    pub probe_every: Duration,
    /// NVML back within this long after the fault, else `nvml_max` rules apply
    pub nvml_max: Duration,
}

impl Default for Opts {
    fn default() -> Self {
        Opts { quiet_min: Duration::from_secs(10), probe_every: Duration::from_secs(5), nvml_max: Duration::from_secs(300) }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum SafeState {
    /// start the engine now. `offset`: set and read back (None: left as it was); `lowered`: below the crashed offset (the
    /// fault was a tuning failure); `nvml`: the clocks / offset were set (false: NVML did not come back, nothing needed it)
    Ready { offset: Option<i32>, lowered: bool, nvml: bool, waited: Duration },
    /// the device cannot be reached (no NVML handle by its bus id, or NVML never back while the crashed offset is applied)
    Lost(String),
    /// the miner stops
    Stopped,
}

impl SafeState {
    pub fn lowered(&self) -> bool {
        matches!(self, SafeState::Ready { lowered: true, .. })
    }
}

/// Waits for the GPU's safe state after a fault (see the module text).  `log` gets the lines worth logging.
pub fn recover_after_fault(hw: &dyn Hw, plan: &Plan, o: &Opts, log: &mut dyn FnMut(String)) -> SafeState {
    recover_core(hw, plan, None, o, log)
}

/// FreeForGem: `recover_after_fault`; with `after`, the plan is made again from the offset read right after the GPU's NVML calls
/// resumed (a fault-restart of the whole process: the offset on the card now is known only then).
fn recover_core(hw: &dyn Hw, plan0: &Plan, after: Option<&dyn Fn(Option<i32>) -> Plan>, o: &Opts, log: &mut dyn FnMut(String)) -> SafeState {
    let t0 = Instant::now();
    let mut plan = *plan0;
    let mut target = safe_offset(&plan);
    let mut needs_change = matches!((target, plan.crashed), (Some(t), Some(c)) if t < c);
    // the time since the fault (the quiet period's own clock when there is one: the fault may be older than this call)
    let since = |hw: &dyn Hw| hw.since_fault().unwrap_or_else(|| t0.elapsed());
    // 1. the driver's reset
    while hw.since_fault().is_some_and(|s| s < o.quiet_min) {
        if hw.stopped() {
            return SafeState::Stopped;
        }
        hw.sleep(Duration::from_millis(100).min(o.quiet_min));
    }
    let mut said_wait = false;
    loop {
        if hw.stopped() {
            return SafeState::Stopped;
        }
        // 2. the driver answers a probe outside this process
        let probe = hw.probe();
        if probe.is_ok() {
            // 3. fresh handle, clocks, then the safe offset
            if let Err(e) = hw.resume() {
                return SafeState::Lost(format!("no NVML handle for it after its fault ({e})"));
            }
            if let Err(e) = hw.apply_clocks() {
                log(format!("the clock locks could not all be set again ({e})"));
            }
            if let Some(f) = after {
                plan = f(hw.read_offset());
                target = safe_offset(&plan);
                needs_change = matches!((target, plan.crashed), (Some(t), Some(c)) if t < c);
            }
            let Some(t) = target else {
                return SafeState::Ready { offset: None, lowered: false, nvml: true, waited: t0.elapsed() };
            };
            let mut why = String::new();
            for _ in 0..3 {
                match hw.set_offset(t) {
                    Ok(()) if hw.read_offset() == Some(t) => {
                        return SafeState::Ready { offset: Some(t), lowered: needs_change, nvml: true, waited: t0.elapsed() };
                    }
                    Ok(()) => why = format!("it reads {} after the set", hw.read_offset().map_or("nothing".into(), |v| format!("{v:+}"))),
                    Err(e) => why = e,
                }
                hw.sleep(Duration::from_millis(500));
            }
            if !needs_change {
                // the offset need not change (at the base already): the engine may start, the set is the tuner's again
                log(format!("offset {t:+} not confirmed ({why}); it is the crashed one's own base: the engine starts"));
                return SafeState::Ready { offset: None, lowered: false, nvml: true, waited: t0.elapsed() };
            }
            log(format!("cannot set the safe offset {t:+} yet ({why}): its engine waits"));
        } else if !said_wait {
            said_wait = true;
            log(format!("the driver does not answer for it yet ({}): its engine waits (never started at the crashed offset)", probe.unwrap_err()));
        }
        if since(hw) >= o.nvml_max {
            if !needs_change {
                // nothing to set: its offset is already the safe one (or not managed); the clocks stay as the driver left them
                log(format!("NVML did not come back within {} s; its offset needs no change: the engine starts", o.nvml_max.as_secs()));
                return SafeState::Ready { offset: None, lowered: false, nvml: false, waited: t0.elapsed() };
            }
            return SafeState::Lost(format!(
                "NVML could not reach it for {} s after its fault, and its engine would start at the crashed offset {:+}",
                o.nvml_max.as_secs(),
                plan.crashed.unwrap_or(0)
            ));
        }
        hw.sleep(o.probe_every);
    }
}

/// The coin's engine start (NEB: a new engine process / context of the GPU).  Called only with `SafeState::Ready`.
pub trait EngineStart {
    type Out;
    fn start(&mut self, s: &SafeState) -> Self::Out;
}

impl<T, F: FnMut(&SafeState) -> T> EngineStart for F {
    type Out = T;
    fn start(&mut self, s: &SafeState) -> T {
        self(s)
    }
}

/// The recovery, then the coin's engine start when it is safe (Err: the state that did not allow a start).
pub fn recover_and_start<E: EngineStart>(hw: &dyn Hw, plan: &Plan, o: &Opts, log: &mut dyn FnMut(String), mut e: E) -> Result<E::Out, SafeState> {
    match recover_after_fault(hw, plan, o, log) {
        s @ SafeState::Ready { .. } => Ok(e.start(&s)),
        s => Err(s),
    }
}

/// Faults of one GPU that count against it: only those at a SAFE offset (`lowered` = false), `tries` within `window`.
#[derive(Debug)]
pub struct Budget {
    tries: u32,
    window: Duration,
    at_safe: VecDeque<Instant>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    /// a new engine; `counted`: this fault counts (n of tries)
    Retry { counted: bool, n: u32 },
    /// more than `tries` faults at a safe offset within the window: the card is faulty at it (NOT a lost device)
    Exhausted { n: u32 },
}

impl Budget {
    pub fn new(tries: u32, window: Duration) -> Budget {
        Budget { tries, window, at_safe: VecDeque::new() }
    }
    pub fn tries(&self) -> u32 {
        self.tries
    }
    pub fn note(&mut self, now: Instant, lowered: bool) -> Verdict {
        while self.at_safe.front().is_some_and(|t| now.saturating_duration_since(*t) > self.window) {
            self.at_safe.pop_front();
        }
        if lowered {
            return Verdict::Retry { counted: false, n: self.at_safe.len() as u32 };
        }
        self.at_safe.push_back(now);
        let n = self.at_safe.len() as u32;
        if n > self.tries { Verdict::Exhausted { n } } else { Verdict::Retry { counted: true, n } }
    }
    pub fn clear(&mut self) {
        self.at_safe.clear();
    }
}

/// An error text of a device that cannot be opened at all (the only "LOST / DEAD" a fault may give): no CUDA device, the
/// device count fails, Xid 79 / fallen off the bus, the device unavailable.  Kernel faults (illegal memory access, launch
/// failure, unknown error ...) are not: a new engine at a safe offset mines again.
pub fn device_gone(err: &str) -> bool {
    let e = err.to_ascii_lowercase();
    ["no cuda-capable device", "cudaerrornodevice", "cuda_error_no_device", "cudagetdevicecount", "fallen off the bus", "xid 79",
     "gpu is lost", "devices unavailable", "cudaerrordevicesunavailable", "cuda_error_devices_unavailable", "invalid device ordinal",
     "cudaerrorinvaliddevice", "no nvml handle"]
        .iter()
        .any(|k| e.contains(k))
}

// ---------------------------------------------------------------------------------------------------------------------------------
// the miner's NVML side (nvml.rs quiet period, offset.rs, clocks.rs)
// ---------------------------------------------------------------------------------------------------------------------------------

/// The flag of the probe process: `<exe> --nvml-probe <bus id>` (exit 0 = the driver answered for that GPU).
pub const PROBE_FLAG: &str = "--nvml-probe";

/// The probe process' work: NVML loaded, the handle by the bus id, its temperature and offset read.  Exit code 0 = answered.
pub fn probe_main(bus: &str) -> i32 {
    let Ok(nv) = crate::nvml::Nvml::load() else { return 2 };
    let Ok(d) = nv.by_bus(bus) else { return 3 };
    if nv.temp_c(d).is_none() {
        return 4;
    }
    0
}

/// One GPU of the miner (by its PCI bus id).
pub struct NvmlGpu<'a> {
    pub bus: String,
    pub stop: &'a std::sync::atomic::AtomicBool,
}

impl Hw for NvmlGpu<'_> {
    fn since_fault(&self) -> Option<Duration> {
        crate::nvml::quiet::since_fault(&self.bus)
    }
    fn probe(&self) -> Result<(), String> {
        probe_process(&self.bus)
    }
    fn resume(&self) -> Result<(), String> {
        crate::offset::nvml().ok_or("NVML not loaded")?.resume_after_fault(&self.bus).map_err(|e| format!("{e:#}"))
    }
    fn apply_clocks(&self) -> Result<(), String> {
        crate::clocks::reassert_after_fault(&self.bus);
        Ok(())
    }
    fn set_offset(&self, v: i32) -> Result<(), String> {
        crate::offset::set(&self.bus, v).map_err(|e| format!("{e:#}"))
    }
    fn read_offset(&self) -> Option<i32> {
        crate::offset::read_now(&self.bus)
    }
    fn stopped(&self) -> bool {
        self.stop.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// The probe process for the GPU on `bus` (`<exe> --nvml-probe <bus>`; main.rs real_main runs it before any coin's parser): Ok
/// = the driver answered for it.  A driver still in its reset can only take that process down, never the miner.
pub fn probe_process(bus: &str) -> Result<(), String> {
    {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let mut ch = std::process::Command::new(exe)
            .arg(PROBE_FLAG)
            .arg(bus)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("cannot start the probe process: {e}"))?;
        let t = Instant::now();
        loop {
            match ch.try_wait() {
                Ok(Some(st)) if st.success() => return Ok(()),
                Ok(Some(st)) => return Err(format!("the probe process ended with {st}")),
                Ok(None) if t.elapsed() > Duration::from_secs(15) => {
                    let _ = ch.kill();
                    let _ = ch.wait();
                    return Err("the probe process did not answer within 15 s".into());
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(e) => return Err(e.to_string()),
            }
        }
    }
}

/// The plan of the miner's GPU on `bus`: the offset applied at the fault (offset.rs), the tuner's last confirmed offset, base
/// and step (registered by the coin's tuner: `set_hints`).
pub fn plan_for(bus: &str) -> Plan {
    let crashed = crate::offset::applied(bus);
    let h = HINTS.lock().unwrap_or_else(|e| e.into_inner()).and_then(|f| f(bus));
    let (orig, _, _) = crate::offset::info(bus).unwrap_or((crashed.unwrap_or(0), 0, 0));
    match h {
        Some(h) => Plan { crashed, confirmed: Some(h.confirmed), base: h.base, step: h.step },
        // no tuner: a fixed offset of this run (--uv-offset) is the safe one (never lowered by the miner)
        None => Plan { crashed, confirmed: None, base: crashed.unwrap_or(orig), step: 15 },
    }
}

/// The tuner's view of a GPU (the coin registers it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hint {
    pub confirmed: i32,
    pub base: i32,
    pub step: i32,
}

static HINTS: std::sync::Mutex<Option<fn(&str) -> Option<Hint>>> = std::sync::Mutex::new(None);

pub fn set_hints(f: fn(&str) -> Option<Hint>) {
    *HINTS.lock().unwrap_or_else(|e| e.into_inner()) = Some(f);
}

// ---------------------------------------------------------------------------------------------------------------------------------
// FreeForGem 1.8.0: the same recovery for every coin (Ting: a GPU's engine process; PROX / NOID / Pearl / Ting: the start after a
// fault-restart of the whole process, faults::restart_self)
// ---------------------------------------------------------------------------------------------------------------------------------

/// A GPU of a coin whose offsets another keeper sets (Ting / PROX / NOID: noid/offset.rs; before any keeper: `raw_set` /
/// `raw_read`).  `fault_at`: the fault of an earlier process (a fault-restart: this process has no quiet period for it).
pub struct CoinGpu<'a> {
    pub bus: String,
    pub stop: &'a std::sync::atomic::AtomicBool,
    pub set: fn(&str, i32) -> anyhow::Result<()>,
    pub read: fn(&str) -> Option<i32>,
    pub fault_at: Option<std::time::SystemTime>,
    /// called while the recovery waits (Ting: the worker's stall clock)
    pub beat: Option<&'a (dyn Fn() + Sync)>,
}

impl CoinGpu<'_> {
    fn tick(&self) {
        if let Some(b) = self.beat {
            b();
        }
    }
}

impl Hw for CoinGpu<'_> {
    fn since_fault(&self) -> Option<Duration> {
        crate::nvml::quiet::since_fault(&self.bus).or_else(|| self.fault_at.map(|t| t.elapsed().unwrap_or(Duration::ZERO)))
    }
    fn probe(&self) -> Result<(), String> {
        self.tick();
        let r = probe_process(&self.bus);
        self.tick();
        r
    }
    fn resume(&self) -> Result<(), String> {
        crate::offset::nvml().ok_or("NVML not loaded")?.resume_after_fault(&self.bus).map_err(|e| format!("{e:#}"))
    }
    fn apply_clocks(&self) -> Result<(), String> {
        crate::clocks::reassert_after_fault(&self.bus);
        Ok(())
    }
    fn set_offset(&self, v: i32) -> Result<(), String> {
        (self.set)(&self.bus, v).map_err(|e| format!("{e:#}"))
    }
    fn read_offset(&self) -> Option<i32> {
        (self.read)(&self.bus)
    }
    fn stopped(&self) -> bool {
        self.stop.load(std::sync::atomic::Ordering::Relaxed)
    }
    fn sleep(&self, d: Duration) {
        let t = Instant::now();
        while t.elapsed() < d && !self.stopped() {
            self.tick();
            std::thread::sleep(Duration::from_millis(50).min(d));
        }
    }
}

/// The card's offset straight through NVML (no keeper has it yet: the start of a process).
pub fn raw_read(bus: &str) -> Option<i32> {
    let nv = crate::offset::nvml()?;
    let d = nv.by_bus(bus).ok()?;
    nv.probe_offset(d).ok().map(|x| x.1)
}

pub fn raw_set(bus: &str, v: i32) -> anyhow::Result<()> {
    let nv = crate::offset::nvml().ok_or_else(|| anyhow::anyhow!("NVML not loaded"))?;
    let d = nv.by_bus(bus)?;
    let (mode, _, lo, hi) = nv.probe_offset(d)?;
    nv.set_offset(d, mode, v.clamp(lo, hi))
}

/// The plan of a GPU at the start after a fault-restart: the card should not run above `at_crash - 2 steps` (not above
/// `confirmed`, not below the owner's `base`).  `cur`: its offset now (the old process put the owner's back before it
/// restarted: then nothing changes); above that safe offset (the crashed one is still applied) it goes down to it.
pub fn restart_plan(cur: Option<i32>, at_crash: i32, confirmed: Option<i32>, base: i32, step: i32) -> Plan {
    let cap = safe_offset(&Plan { crashed: Some(at_crash), confirmed, base, step }).unwrap_or(at_crash);
    match cur {
        Some(c) if c > cap => Plan { crashed: Some(c), confirmed: Some(cap), base, step },
        _ => Plan { crashed: None, confirmed, base, step },
    }
}

/// The step the recovery takes for a coin's --uv-step: 2 of them = uv::crash_back (the owner's rule 2, at least 30 MHz).
pub fn coin_step(uv_step: i32) -> i32 {
    crate::uv::crash_back(uv_step) / BACK_STEPS
}

/// The time of this process' fault-restart (`FFG_SELF_RESTARTS`, faults::restart_self), None = a start by the owner / HiveOS.
pub fn restart_time() -> Option<u64> {
    std::env::var("FFG_SELF_RESTARTS").unwrap_or_default().split(',').filter_map(|s| s.trim().parse::<u64>().ok()).max()
}

/// FreeForGem 1.8.0: the start after a fault-restart of the whole process (PROX, NOID, Pearl, Ting): every GPU with a crash
/// recorded for this restart gets its safe state BEFORE this process sets its clocks or opens it: the driver's reset waited
/// for, a probe process answered, then not above its crashed offset - 2 steps (`restart_plan`).  `step`: the coin's
/// --uv-step.  Returns the buses of the GPUs that cannot be reached (`Lost`): they are left out of mining for an hour
/// (faults.rs), the others mine.
pub fn at_restart(buses: &[String], dir: &std::path::Path, step: i32, stop: &std::sync::atomic::AtomicBool, who: &str) -> Vec<String> {
    let Some(t) = restart_time() else { return Vec::new() };
    let step = coin_step(step);
    let mut lost = Vec::new();
    for (uuid, r) in crate::faults::crashed_near(dir, t, 600) {
        let Some(bus) = buses.iter().find(|b| crate::nvml::quiet::same_bus(b, &r.bus)).cloned() else { continue };
        let fault_at = std::time::UNIX_EPOCH + Duration::from_secs(r.last);
        let hw = CoinGpu { bus: bus.clone(), stop, set: raw_set, read: raw_read, fault_at: Some(fault_at), beat: None };
        let short = crate::uv::short_bus(&bus);
        println!(
            "{who}: GPU bus {short}: the miner restarted itself after a fault of this GPU (offset {:+} at the crash): its safe state first (the driver's reset, a probe process, then not above the crash - 2 steps), before this run sets its clocks or opens it",
            r.offset
        );
        let at = r.offset;
        let (bus2, dir2) = (bus.clone(), dir.to_path_buf());
        let after = move |cur: Option<i32>| {
            // the owner's offset: the HiveOS OC profile's Core Clock (else the offset on the card: nothing is lowered)
            let base = crate::offset::nvml()
                .and_then(|nv| {
                    let d = nv.by_bus(&bus2).ok()?;
                    let (_, c, lo, hi) = nv.probe_offset(d).ok()?;
                    Some(crate::profiles::owner_offset(nv, d, &dir2, c, lo, hi, "recover"))
                })
                .unwrap_or(cur.unwrap_or(at));
            restart_plan(cur, at, None, base.min(at), step)
        };
        let p0 = Plan { crashed: Some(at), confirmed: None, base: i32::MIN, step };
        match recover_core(&hw, &p0, Some(&after), &Opts::default(), &mut |l| println!("{who}: GPU bus {short}: {l}")) {
            SafeState::Stopped => return lost,
            SafeState::Lost(why) => {
                let mut r = r.clone();
                r.excluded_until = crate::uv::now() + crate::faults::EXCLUDE_SECS;
                crate::faults::put(dir, &uuid, &r);
                eprintln!(
                    "WARNING {who}: GPU bus {short}: LOST: {why}; this GPU is left out of mining for {} min, the other GPUs mine (a device that cannot be reached: check its riser / power; a reboot of the rig brings it back)",
                    crate::faults::EXCLUDE_SECS / 60
                );
                lost.push(bus);
            }
            SafeState::Ready { offset, lowered, waited, .. } => {
                let what = match offset {
                    Some(o) if lowered => format!("its offset went down to {o:+} (set and read back: the crashed offset was still applied)"),
                    _ => "its offset is not above the safe one (the owner's was put back before the restart)".into(),
                };
                println!("{who}: GPU bus {short}: safe to open after {:.0} s: {what}", waited.as_secs_f64());
            }
        }
    }
    lost
}

#[cfg(test)]
mod restart_tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::sync::atomic::AtomicBool;

    /// A card after a fault-restart: the driver answers after `nvml_back` (since the fault); `cur` changes through NVML only.
    struct Card {
        t: Cell<Duration>,
        nvml_back: Option<Duration>,
        cur: Cell<i32>,
        resumed: Cell<bool>,
        events: RefCell<Vec<String>>,
    }

    impl Card {
        fn new(cur: i32, nvml_back: Option<Duration>) -> Card {
            Card { t: Cell::new(Duration::ZERO), nvml_back, cur: Cell::new(cur), resumed: Cell::new(false), events: RefCell::new(vec![]) }
        }
    }

    impl Hw for Card {
        fn since_fault(&self) -> Option<Duration> {
            (!self.resumed.get()).then(|| self.t.get())
        }
        fn probe(&self) -> Result<(), String> {
            if self.nvml_back.is_some_and(|b| self.t.get() >= b) { Ok(()) } else { Err("busy".into()) }
        }
        fn resume(&self) -> Result<(), String> {
            self.resumed.set(true);
            self.events.borrow_mut().push("resume".into());
            Ok(())
        }
        fn apply_clocks(&self) -> Result<(), String> {
            self.events.borrow_mut().push("clocks".into());
            Ok(())
        }
        fn set_offset(&self, v: i32) -> Result<(), String> {
            assert!(self.resumed.get(), "no offset set before the driver answered");
            self.cur.set(v);
            self.events.borrow_mut().push(format!("offset {v:+}"));
            Ok(())
        }
        fn read_offset(&self) -> Option<i32> {
            self.resumed.get().then(|| self.cur.get())
        }
        fn stopped(&self) -> bool {
            false
        }
        fn sleep(&self, d: Duration) {
            self.t.set(self.t.get() + d);
        }
    }

    fn restart(card: &Card, at_crash: i32, base: i32, step: i32) -> SafeState {
        let after = |cur: Option<i32>| restart_plan(cur, at_crash, None, base, step);
        recover_core(card, &Plan { crashed: Some(at_crash), confirmed: None, base: i32::MIN, step }, Some(&after), &Opts::default(), &mut |_| {})
    }

    #[test]
    fn the_restart_plan_caps_at_the_crash_minus_two_steps_and_never_raises() {
        // the owner's +100 was put back before the restart: nothing changes (never raised to the crash - 2 steps)
        assert_eq!(safe_offset(&restart_plan(Some(100), 300, None, 100, 15)), None);
        // the crashed +300 is still on the card: down to +270
        assert_eq!(safe_offset(&restart_plan(Some(300), 300, None, 100, 15)), Some(270));
        // a crash at the owner's own offset: never below it
        assert_eq!(safe_offset(&restart_plan(Some(100), 100, None, 100, 15)), None);
        // not above the last confirmed one
        assert_eq!(safe_offset(&restart_plan(Some(300), 300, Some(240), 100, 15)), Some(240));
        // the coins' step: 2 of them = uv::crash_back (at least 30 MHz)
        assert_eq!(coin_step(15), 15);
        assert_eq!(2 * coin_step(5), crate::uv::crash_back(5));
    }

    /// PROX / NOID / Pearl / Ting after faults::restart_self: the card faulted at +300, the old process could not put the
    /// owner's +100 back (the crashed offset is still applied); the driver answers 10 s after the fault: no offset set before
    /// it answered, then the clocks, then +270 before this process opens the GPU.
    #[test]
    fn a_fault_restart_lowers_a_crashed_offset_that_is_still_applied_before_the_gpu_opens() {
        let card = Card::new(300, Some(Duration::from_secs(10)));
        let s = restart(&card, 300, 100, 15);
        assert!(matches!(s, SafeState::Ready { offset: Some(270), lowered: true, nvml: true, .. }), "{s:?}");
        assert_eq!(*card.events.borrow(), vec!["resume", "clocks", "offset +270"]);
        assert!(card.t.get() >= Duration::from_secs(10), "the driver's reset is waited for");
    }

    #[test]
    fn a_fault_restart_with_the_owners_offset_back_changes_nothing() {
        let card = Card::new(100, Some(Duration::ZERO));
        let s = restart(&card, 300, 100, 15);
        assert!(matches!(s, SafeState::Ready { offset: None, lowered: false, nvml: true, .. }), "{s:?}");
        assert_eq!(card.cur.get(), 100);
        assert_eq!(*card.events.borrow(), vec!["resume", "clocks"]);
    }

    #[test]
    fn a_fault_restart_whose_driver_never_answers_is_lost() {
        let card = Card::new(300, None);
        assert!(matches!(restart(&card, 300, 100, 15), SafeState::Lost(_)));
        assert_eq!(card.cur.get(), 300);
    }

    /// The real CoinGpu side: its fault clock of an earlier process (`fault_at`) and its wait beat.
    #[test]
    fn a_coin_gpu_takes_the_fault_time_of_the_old_process_and_beats_while_it_waits() {
        let stop = AtomicBool::new(false);
        let beats = std::sync::atomic::AtomicU32::new(0);
        let beat = || {
            beats.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        };
        let hw = CoinGpu {
            bus: "0000:c7:00.0".into(),
            stop: &stop,
            set: raw_set,
            read: |_| None,
            fault_at: Some(std::time::SystemTime::now() - Duration::from_secs(4)),
            beat: Some(&beat),
        };
        let s = hw.since_fault().unwrap();
        assert!(s >= Duration::from_secs(4) && s < Duration::from_secs(60), "{s:?}");
        hw.sleep(Duration::from_millis(120));
        assert!(beats.load(std::sync::atomic::Ordering::Relaxed) >= 2);
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        assert!(hw.stopped());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    /// A GPU whose driver answers after `nvml_back` (since the fault), whose offset `cur` only NVML can change.
    struct Mock {
        t: Cell<Duration>,
        fault_age: Duration,
        nvml_back: Option<Duration>,
        cur: Cell<i32>,
        resumed: Cell<bool>,
        clocks: Cell<u32>,
        calls_in_quiet: Cell<u32>,
        lose_handle: bool,
        events: RefCell<Vec<String>>,
    }

    impl Mock {
        fn new(cur: i32, nvml_back: Option<Duration>) -> Mock {
            Mock { t: Cell::new(Duration::ZERO), fault_age: Duration::ZERO, nvml_back, cur: Cell::new(cur), resumed: Cell::new(false), clocks: Cell::new(0), calls_in_quiet: Cell::new(0), lose_handle: false, events: RefCell::new(vec![]) }
        }
        fn now(&self) -> Duration {
            self.fault_age + self.t.get()
        }
        fn nvml_ok(&self) -> bool {
            self.nvml_back.is_some_and(|b| self.now() >= b)
        }
    }

    impl Hw for Mock {
        fn since_fault(&self) -> Option<Duration> {
            (!self.resumed.get()).then(|| self.now())
        }
        fn probe(&self) -> Result<(), String> {
            if self.nvml_ok() { Ok(()) } else { Err("busy".into()) }
        }
        fn resume(&self) -> Result<(), String> {
            if self.lose_handle {
                return Err("Not Found".into());
            }
            self.resumed.set(true);
            self.events.borrow_mut().push("resume".into());
            Ok(())
        }
        fn apply_clocks(&self) -> Result<(), String> {
            self.clocks.set(self.clocks.get() + 1);
            self.events.borrow_mut().push("clocks".into());
            Ok(())
        }
        fn set_offset(&self, v: i32) -> Result<(), String> {
            if !self.resumed.get() {
                self.calls_in_quiet.set(self.calls_in_quiet.get() + 1);
                return Err("quiet".into());
            }
            self.cur.set(v);
            self.events.borrow_mut().push(format!("offset {v:+}"));
            Ok(())
        }
        fn read_offset(&self) -> Option<i32> {
            self.resumed.get().then(|| self.cur.get())
        }
        fn stopped(&self) -> bool {
            false
        }
        fn sleep(&self, d: Duration) {
            self.t.set(self.t.get() + d);
        }
    }

    fn rig01(crashed: i32) -> Plan {
        // Rig01 GPU1: base +275 (HiveOS profile), step +350 taken (confirmed), +365 faulted
        Plan { crashed: Some(crashed), confirmed: Some(350), base: 275, step: 15 }
    }

    #[test]
    fn the_safe_offset_is_failed_minus_two_steps_never_above_the_confirmed_nor_below_the_base() {
        assert_eq!(safe_offset(&rig01(365)), Some(335));
        // a coarse step (+60 above the confirmed one) faulted: back to the confirmed one, not 2 small steps under the crash
        assert_eq!(safe_offset(&Plan { crashed: Some(410), confirmed: Some(350), base: 275, step: 15 }), Some(350));
        assert_eq!(safe_offset(&Plan { crashed: Some(290), confirmed: Some(290), base: 275, step: 15 }), Some(275));
        assert_eq!(safe_offset(&Plan { crashed: Some(275), confirmed: Some(275), base: 275, step: 15 }), Some(275));
        assert_eq!(safe_offset(&Plan { crashed: None, confirmed: Some(300), base: 275, step: 15 }), None);
    }

    /// The Rig01 sequence: Xid 43 at +365, the driver answers 10 s later; the new engine must start only after the offset is
    /// +335 (1.0.12: it started at +365 three times and the card was "DEAD").
    #[test]
    fn rig01_the_new_engine_starts_only_after_the_offset_went_down() {
        let hw = Mock::new(365, Some(Duration::from_secs(10)));
        let mut lines = vec![];
        let r = recover_and_start(&hw, &rig01(365), &Opts::default(), &mut |l| lines.push(l), |s: &SafeState| {
            // the engine start sees the safe state set already
            assert_eq!(hw.cur.get(), 335, "the engine starts at the safe offset");
            assert!(hw.clocks.get() >= 1, "the core lock / memory policy are set before the engine");
            s.clone()
        });
        let s = r.unwrap();
        assert!(matches!(s, SafeState::Ready { offset: Some(335), lowered: true, nvml: true, .. }), "{s:?}");
        assert_eq!(*hw.events.borrow(), vec!["resume", "clocks", "offset +335"], "fresh handle, clocks, then the offset");
        assert_eq!(hw.calls_in_quiet.get(), 0, "no NVML set in the quiet period");
        assert!(hw.t.get() >= Duration::from_secs(10), "the quiet period's time part is waited for");
    }

    #[test]
    fn a_fault_older_than_the_quiet_period_does_not_wait_again() {
        let mut hw = Mock::new(365, Some(Duration::ZERO));
        hw.fault_age = Duration::from_secs(30);
        let s = recover_after_fault(&hw, &rig01(365), &Opts::default(), &mut |_| {});
        assert!(s.lowered());
        assert!(hw.t.get() < Duration::from_secs(1));
    }

    #[test]
    fn nvml_never_back_never_starts_at_the_crashed_offset() {
        let hw = Mock::new(365, None);
        let mut started = false;
        let r = recover_and_start(&hw, &rig01(365), &Opts::default(), &mut |_| {}, |_: &SafeState| started = true);
        assert!(!started, "never an engine at the crashed offset");
        assert!(matches!(r, Err(SafeState::Lost(ref w)) if w.contains("crashed offset +365")), "{r:?}");
        assert_eq!(hw.cur.get(), 365);
    }

    #[test]
    fn nvml_never_back_but_no_change_needed_starts_the_engine() {
        // a fault at the base (no offset of this run to lower): the engine may start without NVML
        let hw = Mock::new(275, None);
        let s = recover_after_fault(&hw, &rig01(275), &Opts::default(), &mut |_| {});
        assert!(matches!(s, SafeState::Ready { offset: None, lowered: false, nvml: false, .. }), "{s:?}");
    }

    #[test]
    fn nvml_late_still_sets_the_offset_before_the_engine() {
        let hw = Mock::new(365, Some(Duration::from_secs(95)));
        let s = recover_after_fault(&hw, &rig01(365), &Opts::default(), &mut |_| {});
        assert!(matches!(s, SafeState::Ready { offset: Some(335), lowered: true, .. }), "{s:?}");
        assert!(hw.t.get() >= Duration::from_secs(95));
    }

    #[test]
    fn no_handle_by_the_bus_id_is_a_lost_device() {
        let mut hw = Mock::new(365, Some(Duration::ZERO));
        hw.lose_handle = true;
        assert!(matches!(recover_after_fault(&hw, &rig01(365), &Opts::default(), &mut |_| {}), SafeState::Lost(_)));
    }

    #[test]
    fn an_unmanaged_offset_is_left_alone() {
        let hw = Mock::new(100, Some(Duration::ZERO));
        let s = recover_after_fault(&hw, &Plan { crashed: None, confirmed: None, base: 0, step: 15 }, &Opts::default(), &mut |_| {});
        assert!(matches!(s, SafeState::Ready { offset: None, lowered: false, nvml: true, .. }));
        assert_eq!(hw.cur.get(), 100);
        assert_eq!(hw.clocks.get(), 1, "the clocks still go back");
    }

    /// The budget: auto-uv faults (lowered) never count; 3 at a safe offset within 10 min are allowed, the 4th exhausts it.
    #[test]
    fn the_budget_counts_only_faults_at_a_safe_offset() {
        let mut b = Budget::new(3, Duration::from_secs(600));
        let t = Instant::now();
        // the Rig01 card: +365 (tuning step) -> +335 -> +305 -> +275 (each a crash at a tuned offset: lowered)
        for i in 0..3 {
            assert_eq!(b.note(t + Duration::from_secs(i * 20), true), Verdict::Retry { counted: false, n: 0 });
        }
        assert_eq!(b.note(t + Duration::from_secs(100), false), Verdict::Retry { counted: true, n: 1 });
        assert_eq!(b.note(t + Duration::from_secs(200), false), Verdict::Retry { counted: true, n: 2 });
        assert_eq!(b.note(t + Duration::from_secs(300), false), Verdict::Retry { counted: true, n: 3 });
        assert_eq!(b.note(t + Duration::from_secs(400), false), Verdict::Exhausted { n: 4 });
        // after 10 min the old ones fall out
        b.clear();
        assert_eq!(b.note(t + Duration::from_secs(1000), false), Verdict::Retry { counted: true, n: 1 });
        assert_eq!(b.note(t + Duration::from_secs(1700), false), Verdict::Retry { counted: true, n: 1 });
    }

    #[test]
    fn only_an_unopenable_device_is_gone() {
        for e in ["GPU1 scan: cudaEventSynchronize(ctx->ev1[pe]): an illegal memory access was encountered (src/nebgpu.cu:6911) (code -2)",
                  "GPU1 scan: dtable build: an illegal memory access was encountered (code -2)",
                  "cudaEventSynchronize(ctx->ev1[pe]): unknown error",
                  "GPU1: the engine process ended during scan (exit status: 1): a crash of this GPU's engine (CUDA fault, driver)"] {
            assert!(!device_gone(e), "{e}");
        }
        for e in ["cudaGetDeviceCount: no CUDA-capable device is detected", "GPU has fallen off the bus", "Xid 79 reported by the driver",
                  "cudaSetDevice: invalid device ordinal"] {
            assert!(device_gone(e), "{e}");
        }
    }
}
