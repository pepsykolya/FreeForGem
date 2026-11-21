// FreeForGem — see LICENSE
//! FFGN 1.0.12 (FreeForGem 1.8.0 takes it with the NEB module): lost-GPU recovery (generic host part: no algorithm in here; the NEB host feeds it from neb/mod.rs every stats window).
//!
//! Why (LAS-rig5.md, a friend's 12x RTX 2060 SUPER HiveOS rig): one card fell off the PCIe bus (Xid 79); the NVIDIA driver gets
//! such a card back only after a reboot of the rig, and the rig was found the next morning not mining at all.  Two answers:
//!
//! 1. `--reboot-on-lost-gpu MIN` (opt-in, default off): a GPU that is LOST - an Xid 79 ("fallen off the bus") and no attempts
//!    since, its worker given up because the engine cannot open or reach the device (DEAD; 1.0.13: never for kernel faults), or NVML no longer sees
//!    it (the handle query fails / "GPU is lost") - and stays lost for MIN minutes makes the miner ask the OS for a reboot:
//!    HiveOS `sreboot` when present, else `systemctl reboot`; Windows `shutdown /r /t 60 /c "FreeForGem: GPU lost"`.
//!    Safety: at most one reboot per REBOOT_EVERY_SECS (the state file STATE_FILE in the miner's state dir, kept across reboots
//!    and restarts), never in the first START_GUARD_SECS of the miner, a loud WARNING (GPU, bus, how long lost, the command) in
//!    the log and the API before it; never with `--single-process` (debugging) or with FFG_NO_REBOOT=1 (FFGN_NO_REBOOT=1 too).
//! 2. Never stand still: every GPU down or failing for ALL_DOWN_SECS -> the miner exits with EXIT_ALL_DOWN, so that the HiveOS
//!    miner restart / watchdog (or start.ps1, systemd) takes over; and a lost GPU reports 0 (stats line, API, h-stats), so the
//!    HiveOS Watchdog's minimal hashrate rule sees the true total.
//!
//! The OS command is run through an injectable runner (`Watch::tick`'s `run`): the tests never run it.

use anyhow::{Result, bail};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

/// never a reboot in the first 10 min after the miner start
pub const START_GUARD_SECS: u64 = 600;
/// at most one reboot per 60 min (STATE_FILE)
pub const REBOOT_EVERY_SECS: u64 = 3600;
/// every GPU down / failing this long -> the miner exits with EXIT_ALL_DOWN
pub const ALL_DOWN_SECS: u64 = 300;
/// the exit code when every GPU was down for ALL_DOWN_SECS (HiveOS / start.ps1 / systemd restart the miner)
pub const EXIT_ALL_DOWN: i32 = 3;
/// FFG_NO_REBOOT=1: --reboot-on-lost-gpu never reboots (it logs what it would do); FFGN_NO_REBOOT=1 (a rig moved from FFGN) too
pub const ENV_NO_REBOOT: &str = "FFG_NO_REBOOT";
pub const ENV_NO_REBOOT_FFGN: &str = "FFGN_NO_REBOOT";
/// the time of the last reboot asked by the miner (unix s), in the state dir next to uv.json
pub const STATE_FILE: &str = "reboot-on-lost-gpu.json";
/// HiveOS' reboot command ("sreboot": sync + an immediate reboot that does not hang on a GPU the driver lost)
pub const SREBOOT_PATHS: [&str; 3] = ["/hive/sbin/sreboot", "/hive/bin/sreboot", "/usr/sbin/sreboot"];

/// --reboot-on-lost-gpu MIN (0 = off), set once by main
static MINUTES: AtomicU32 = AtomicU32::new(0);

pub fn set_minutes(m: u32) {
    MINUTES.store(m, Ordering::Relaxed);
}

pub fn minutes() -> u32 {
    MINUTES.load(Ordering::Relaxed)
}

/// `--reboot-on-lost-gpu MIN`: 1 .. 1440 minutes (0 / off = off)
pub fn parse_minutes(s: &str) -> Result<u32> {
    let s = s.trim();
    if s.eq_ignore_ascii_case("off") {
        return Ok(0);
    }
    let Ok(m) = s.parse::<u32>() else { bail!("--reboot-on-lost-gpu needs a number of minutes (1..1440, 0 or off = off)") };
    if m > 1440 {
        bail!("--reboot-on-lost-gpu {m}: at most 1440 minutes");
    }
    Ok(m)
}

/// why the miner does not reboot even with --reboot-on-lost-gpu (None = it may)
pub fn skip_reason(single_process: bool, no_reboot_env: Option<&str>) -> Option<String> {
    if single_process {
        Some("every GPU runs in the miner process (--single-process / FFG_SINGLE_PROCESS=1, debugging): no reboot".into())
    } else if no_reboot_env.is_some_and(|v| v.trim() == "1") {
        Some(format!("{ENV_NO_REBOOT}=1 is set: no reboot"))
    } else {
        None
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Os {
    Linux,
    Windows,
    Other,
}

pub fn this_os() -> Os {
    if cfg!(windows) {
        Os::Windows
    } else if cfg!(target_os = "linux") {
        Os::Linux
    } else {
        Os::Other
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Cmd {
    pub prog: String,
    pub args: Vec<String>,
}

impl std::fmt::Display for Cmd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.prog)?;
        for a in &self.args {
            if a.contains(' ') { write!(f, " \"{a}\"")? } else { write!(f, " {a}")? }
        }
        Ok(())
    }
}

/// The reboot command of the platform: HiveOS `sreboot` when one of SREBOOT_PATHS exists (`exists`), else `systemctl reboot`;
/// Windows `shutdown /r /t 60 /c "FreeForGem: GPU lost"` (60 s: the log and the API show it, `shutdown /a` cancels it).
pub fn command(os: Os, exists: &dyn Fn(&str) -> bool) -> Option<Cmd> {
    let c = |p: &str, a: &[&str]| Some(Cmd { prog: p.into(), args: a.iter().map(|s| s.to_string()).collect() });
    match os {
        Os::Linux => match SREBOOT_PATHS.iter().find(|p| exists(p)) {
            Some(p) => c(p, &[]),
            None => c("systemctl", &["reboot"]),
        },
        Os::Windows => c("shutdown", &["/r", "/t", "60", "/c", "FreeForGem: GPU lost"]),
        Os::Other => None,
    }
}

/// The real runner: starts the command (does not wait: sreboot does not come back).
pub fn run_os(c: &Cmd) -> Result<()> {
    std::process::Command::new(&c.prog).args(&c.args).stdin(std::process::Stdio::null()).spawn()?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------------
// Xid 79 per GPU (fed by the fault handling; a lost GPU = an Xid 79 and no attempts since)
// ---------------------------------------------------------------------------------------------------------------------

static XID79: Mutex<BTreeSet<i32>> = Mutex::new(BTreeSet::new());

/// an Xid 79 of GPU `dev` (fault handling)
pub fn note_xid79(dev: i32) {
    XID79.lock().unwrap_or_else(|e| e.into_inner()).insert(dev);
}

pub fn xid79_seen(dev: i32) -> bool {
    XID79.lock().unwrap_or_else(|e| e.into_inner()).contains(&dev)
}

/// Why a GPU counts as lost in this stats window (None = it is not). `moved`: it made attempts in the window; `nvml_lost`: NVML
/// had a handle for it at the start and no longer sees it. A GPU that still makes attempts is never lost.
pub fn lost_why(alive: bool, moved: bool, xid79: bool, nvml_lost: bool) -> Option<&'static str> {
    if !alive {
        Some("its engine cannot open or reach the device (DEAD: no device / device count fails / Xid 79 / no NVML handle; kernel faults never give this)")
    } else if moved {
        None
    } else if xid79 {
        Some("Xid 79: fallen off the PCIe bus")
    } else if nvml_lost {
        Some("NVML no longer sees it (GPU is lost)")
    } else {
        None
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// the reboot watch
// ---------------------------------------------------------------------------------------------------------------------

/// One GPU in a stats window.
#[derive(Clone, Debug)]
pub struct Obs {
    pub dev: i32,
    /// short bus ("02")
    pub bus: String,
    /// Some(why) = lost now
    pub why: Option<String>,
}

struct Lost {
    since: u64,
    bus: String,
    why: String,
}

pub struct Watch {
    minutes: u32,
    skip: Option<String>,
    dir: PathBuf,
    cmd: Option<Cmd>,
    lost: BTreeMap<i32, Lost>,
    requested: bool,
    rebooting: bool,
    said: BTreeSet<&'static str>,
}

fn state_path(dir: &Path) -> PathBuf {
    dir.join(STATE_FILE)
}

/// the last reboot asked by the miner (unix s, 0 = none)
pub fn last_reboot(dir: &Path) -> u64 {
    std::fs::read_to_string(state_path(dir)).ok().and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()).and_then(|v| v["at"].as_u64()).unwrap_or(0)
}

fn mins(s: u64) -> String {
    format!("{:.1} min", s as f64 / 60.0)
}

impl Watch {
    /// `minutes` 0 = off (the lost GPUs are still tracked for the tags); `skip`: skip_reason; `cmd`: command(this_os(), ..)
    pub fn new(minutes: u32, skip: Option<String>, dir: &Path, cmd: Option<Cmd>) -> Watch {
        Watch { minutes, skip, dir: dir.to_path_buf(), cmd, lost: BTreeMap::new(), requested: false, rebooting: false, said: BTreeSet::new() }
    }

    /// the miner's own setup: --reboot-on-lost-gpu, --single-process / FFG_NO_REBOOT, this platform's command
    pub fn for_this_rig(single_process: bool, dir: &Path) -> Watch {
        let env = std::env::var(ENV_NO_REBOOT).ok().filter(|v| v.trim() == "1").or_else(|| std::env::var(ENV_NO_REBOOT_FFGN).ok());
        let skip = skip_reason(single_process, env.as_deref());
        Watch::new(minutes(), skip, dir, command(this_os(), &|p| Path::new(p).exists()))
    }

    /// the start line (None when off)
    pub fn start_line(&self) -> Option<String> {
        if self.minutes == 0 {
            return None;
        }
        Some(match (&self.skip, &self.cmd) {
            (Some(s), _) => format!("reboot-on-lost-gpu: {} min, but {s}", self.minutes),
            (None, None) => format!("reboot-on-lost-gpu: {} min, but this OS has no reboot command known to the miner: no reboot", self.minutes),
            (None, Some(c)) => format!(
                "reboot-on-lost-gpu: a GPU lost (Xid 79 / DEAD / gone from NVML) for {} min reboots the rig with `{c}` (not in the first {} min of the miner, at most once per {} min; {ENV_NO_REBOOT}=1 turns it off)",
                self.minutes,
                START_GUARD_SECS / 60,
                REBOOT_EVERY_SECS / 60
            ),
        })
    }

    /// the GPUs lost now, for the stats line / API / HiveOS: "GPU LOST: GPU1 (bus 02) 12.0 min: Xid 79 ..."
    pub fn tags(&self, mono: u64) -> Vec<String> {
        let mut v: Vec<String> =
            self.lost.iter().map(|(d, l)| format!("GPU LOST: GPU{d} (bus {}) {}: {}", l.bus, mins(mono.saturating_sub(l.since)), l.why)).collect();
        if self.rebooting {
            v.push("REBOOTING: a GPU was lost (--reboot-on-lost-gpu)".into());
        }
        v
    }

    /// the short tags of the stats line (HiveOS h-stats): "GPU LOST: GPU1 (bus 02) 12.0 min", "REBOOTING"
    pub fn stats_tags(&self, mono: u64) -> Vec<String> {
        let mut v: Vec<String> = self.lost.iter().map(|(d, l)| format!("GPU LOST: GPU{d} (bus {}) {}", l.bus, mins(mono.saturating_sub(l.since)))).collect();
        if self.rebooting {
            v.push("REBOOTING".into());
        }
        v
    }

    fn once(&mut self, key: &'static str, line: String, emit: &mut dyn FnMut(String)) {
        if self.said.insert(key) {
            emit(line);
        }
    }

    /// One stats window. `mono`: seconds since the miner start; `unix`: the clock (the state file). `emit`: a log line (printed
    /// and flushed before `run`, so it is in the log before the reboot); `run`: the OS command (tests: a fake). True when the
    /// reboot was asked in this call.
    pub fn tick(&mut self, obs: &[Obs], mono: u64, unix: u64, emit: &mut dyn FnMut(String), run: &mut dyn FnMut(&Cmd) -> Result<()>) -> bool {
        for o in obs {
            match &o.why {
                Some(w) => {
                    if !self.lost.contains_key(&o.dev) {
                        emit(format!("WARNING GPU{} (bus {}): LOST: {w}{}", o.dev, o.bus, if self.minutes > 0 && self.skip.is_none() {
                            format!(" - the rig reboots if it stays lost for {} min (--reboot-on-lost-gpu)", self.minutes)
                        } else {
                            " - the rig needs a reboot to get it back".to_string()
                        }));
                        self.lost.insert(o.dev, Lost { since: mono, bus: o.bus.clone(), why: w.clone() });
                    }
                }
                None => {
                    if self.lost.remove(&o.dev).is_some() {
                        emit(format!("GPU{} (bus {}): mining again, no longer lost", o.dev, o.bus));
                    }
                }
            }
        }
        if self.minutes == 0 || self.requested {
            return false;
        }
        let Some((&dev, l)) = self.lost.iter().min_by_key(|(_, l)| l.since) else { return false };
        let lost_for = mono.saturating_sub(l.since);
        if lost_for < self.minutes as u64 * 60 {
            return false;
        }
        let what = format!("GPU{dev} (bus {}) lost for {} ({})", l.bus, mins(lost_for), l.why);
        if let Some(s) = self.skip.clone() {
            self.requested = true;
            emit(format!("WARNING: {what}: --reboot-on-lost-gpu {} would reboot the rig now, but {s}", self.minutes));
            return false;
        }
        let Some(cmd) = self.cmd.clone() else {
            self.requested = true;
            emit(format!("WARNING: {what}: --reboot-on-lost-gpu: no reboot command on this OS; reboot the rig by hand"));
            return false;
        };
        if mono < START_GUARD_SECS {
            self.once("guard", format!("WARNING: {what}: no reboot in the first {} min after the miner start; waiting", START_GUARD_SECS / 60), emit);
            return false;
        }
        let last = last_reboot(&self.dir);
        if last > 0 && unix < last + REBOOT_EVERY_SECS {
            self.once(
                "limit",
                format!(
                    "WARNING: {what}: the miner asked for a reboot {} ago: at most one reboot per {} min, no reboot now (reboot the rig by hand, check the GPU's riser / power)",
                    mins(unix.saturating_sub(last)),
                    REBOOT_EVERY_SECS / 60
                ),
                emit,
            );
            return false;
        }
        // the state file first: a reboot that comes back with the same lost GPU waits an hour before the next one
        let saved = serde_json::json!({"at": unix, "gpu": format!("GPU{dev} (bus {})", l.bus), "why": l.why}).to_string();
        if let Err(e) = crate::uv::write_atomic(&state_path(&self.dir), &saved) {
            emit(format!("WARNING: --reboot-on-lost-gpu: cannot write {}: {e:#}: no reboot (it could not be rate limited)", state_path(&self.dir).display()));
            self.requested = true;
            return false;
        }
        self.requested = true;
        self.rebooting = true;
        emit(format!("WARNING: REBOOTING THE RIG: {what}; --reboot-on-lost-gpu {}: running `{cmd}`", self.minutes));
        if let Err(e) = run(&cmd) {
            emit(format!("ERROR: --reboot-on-lost-gpu: `{cmd}` failed: {e:#}; reboot the rig by hand"));
        }
        true
    }
}

/// the warnings for the API (set by the NEB host every stats window)
static TAGS: Mutex<Vec<String>> = Mutex::new(Vec::new());

pub fn publish_tags(v: Vec<String>) {
    *TAGS.lock().unwrap_or_else(|e| e.into_inner()) = v;
}

pub fn tags() -> Vec<String> {
    TAGS.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

// ---------------------------------------------------------------------------------------------------------------------
// never stand still
// ---------------------------------------------------------------------------------------------------------------------

/// Every GPU down or failing: since when (seconds since the start).
#[derive(Default)]
pub struct AllDown {
    since: Option<u64>,
}

impl AllDown {
    /// `failing` of `n` GPUs fail in this window; Some(seconds) once all of them have failed for ALL_DOWN_SECS in a row
    pub fn tick(&mut self, failing: usize, n: usize, mono: u64) -> Option<u64> {
        if n == 0 || failing < n {
            self.since = None;
            return None;
        }
        let t0 = *self.since.get_or_insert(mono);
        (mono - t0 >= ALL_DOWN_SECS).then_some(mono - t0)
    }
}

/// An exit of the miner with a code of its own (main.rs: `std::process::exit(code)`).
#[derive(Debug)]
pub struct Exit {
    pub code: i32,
    pub msg: String,
}

impl std::fmt::Display for Exit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (exit code {})", self.msg, self.code)
    }
}

impl std::error::Error for Exit {}

/// the exit code of a run's error: an Exit's own, else 1
pub fn exit_code(e: &anyhow::Error) -> i32 {
    e.downcast_ref::<Exit>().map_or(1, |x| x.code)
}

/// the line and the error when every GPU was down for `secs`
pub fn all_down_exit(n: usize, secs: u64) -> Exit {
    Exit {
        code: EXIT_ALL_DOWN,
        msg: format!(
            "all {n} GPU(s) down or not mining for {} although there is work: the miner exits so that HiveOS (its miner restart / Watchdog), start.ps1 or systemd takes over",
            mins(secs)
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ffg-lostgpu-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn cmd() -> Cmd {
        Cmd { prog: "fake-reboot".into(), args: vec![] }
    }

    fn lost(dev: i32) -> Obs {
        Obs { dev, bus: format!("{:02x}", dev + 1), why: Some("Xid 79: fallen off the PCIe bus".into()) }
    }

    fn ok(dev: i32) -> Obs {
        Obs { dev, bus: format!("{:02x}", dev + 1), why: None }
    }

    /// runs `w` over [from, to) every 10 s with `obs`; returns (reboots asked, commands run, lines)
    fn drive(w: &mut Watch, obs: &[Obs], from: u64, to: u64, unix0: u64) -> (usize, usize, Vec<String>) {
        let (mut asked, mut ran, mut lines) = (0, 0, Vec::new());
        let mut t = from;
        while t < to {
            if w.tick(obs, t, unix0 + t, &mut |l| lines.push(l), &mut |_| {
                ran += 1;
                Ok(())
            }) {
                asked += 1;
            }
            t += 10;
        }
        (asked, ran, lines)
    }

    #[test]
    fn a_lost_gpu_reboots_the_rig_once_after_min_minutes() {
        let d = tmp("timer");
        let mut w = Watch::new(10, None, &d, Some(cmd()));
        // mining for 20 min, then GPU1 lost at 1200 s
        assert_eq!(drive(&mut w, &[ok(0), ok(1)], 0, 1200, 1_000_000).0, 0);
        let (a, r, lines) = drive(&mut w, &[ok(0), lost(1)], 1200, 1800, 1_000_000);
        assert_eq!((a, r), (0, 0), "{lines:?}");
        assert!(lines[0].contains("WARNING GPU1 (bus 02): LOST: Xid 79") && lines[0].contains("10 min"), "{lines:?}");
        assert!(w.tags(1790)[0].starts_with("GPU LOST: GPU1 (bus 02) 9.8 min"), "{:?}", w.tags(1790));
        // 10 min lost (at 1800 s): the reboot, once; the WARNING line names the GPU, the bus, how long and the command
        let (a, r, lines) = drive(&mut w, &[ok(0), lost(1)], 1800, 4000, 1_000_000);
        assert_eq!((a, r), (1, 1), "{lines:?}");
        assert!(lines.iter().any(|l| l.contains("REBOOTING THE RIG: GPU1 (bus 02) lost for 10.0 min (Xid 79") && l.contains("`fake-reboot`")), "{lines:?}");
        assert!(w.tags(4000).iter().any(|t| t.starts_with("REBOOTING")));
        assert_eq!(last_reboot(&d), 1_000_000 + 1800);
        // a GPU back before MIN minutes: no reboot, the timer starts over
        let d2 = tmp("timer2");
        let mut w = Watch::new(10, None, &d2, Some(cmd()));
        assert_eq!(drive(&mut w, &[lost(0)], 700, 1200, 0).0, 0);
        let (_, _, l) = drive(&mut w, &[ok(0)], 1200, 1210, 0);
        assert!(l[0].contains("mining again"), "{l:?}");
        assert_eq!(drive(&mut w, &[lost(0)], 1210, 1800, 0).0, 0);
        assert_eq!(drive(&mut w, &[lost(0)], 1800, 1820, 0).0, 1);
        // off (0): the lost GPU is tracked and tagged, never a reboot
        let mut w = Watch::new(0, None, &d2, Some(cmd()));
        let (a, r, l) = drive(&mut w, &[lost(0)], 0, 20000, 10_000_000);
        assert_eq!((a, r), (0, 0));
        assert!(l[0].contains("needs a reboot") && w.tags(20000)[0].starts_with("GPU LOST: GPU0"), "{l:?}");
    }

    #[test]
    fn one_reboot_per_hour_across_restarts() {
        let d = tmp("limit");
        let u0 = 5_000_000;
        let mut w = Watch::new(1, None, &d, Some(cmd()));
        assert_eq!(drive(&mut w, &[lost(0)], 600, 700, u0).1, 1);
        let at = last_reboot(&d);
        assert_eq!(at, u0 + 660);
        // the next start (after the reboot) with the GPU still lost: no reboot within the hour of the state file
        let mut w = Watch::new(1, None, &d, Some(cmd()));
        let (a, r, lines) = drive(&mut w, &[lost(0)], 0, 2000, at + 100);
        assert_eq!((a, r), (0, 0));
        assert_eq!(lines.iter().filter(|l| l.contains("at most one reboot per 60 min")).count(), 1, "{lines:?}");
        // an hour after the last one: the next reboot
        let mut w = Watch::new(1, None, &d, Some(cmd()));
        assert_eq!(drive(&mut w, &[lost(0)], 600, 700, at + REBOOT_EVERY_SECS - 600).1, 1);
        assert!(last_reboot(&d) >= at + REBOOT_EVERY_SECS);
    }

    #[test]
    fn no_reboot_in_the_first_ten_minutes() {
        let d = tmp("guard");
        let mut w = Watch::new(1, None, &d, Some(cmd()));
        // lost from 0 s, 1 min: waits up to 600 s (one line), then reboots
        let (a, r, lines) = drive(&mut w, &[lost(0)], 0, 600, 1_000_000);
        assert_eq!((a, r), (0, 0));
        assert_eq!(lines.iter().filter(|l| l.contains("first 10 min")).count(), 1, "{lines:?}");
        assert_eq!(drive(&mut w, &[lost(0)], 600, 610, 1_000_000).1, 1);
    }

    #[test]
    fn single_process_and_ffgn_no_reboot_skip_the_reboot() {
        assert!(skip_reason(true, None).unwrap().contains("--single-process"));
        assert!(skip_reason(false, Some("1")).unwrap().contains("FFG_NO_REBOOT=1"));
        assert!(skip_reason(false, Some("0")).is_none() && skip_reason(false, None).is_none());
        for skip in [skip_reason(true, None), skip_reason(false, Some("1"))] {
            let d = tmp("skip");
            let mut w = Watch::new(1, skip, &d, Some(cmd()));
            assert!(w.start_line().unwrap().contains("no reboot"));
            let (a, r, lines) = drive(&mut w, &[lost(0)], 0, 5000, 1_000_000);
            assert_eq!((a, r), (0, 0));
            assert_eq!(lines.iter().filter(|l| l.contains("would reboot the rig now")).count(), 1, "{lines:?}");
            assert_eq!(last_reboot(&d), 0);
            assert!(!w.tags(5000).iter().any(|t| t.starts_with("REBOOTING")));
        }
    }

    #[test]
    fn the_reboot_command_of_each_platform() {
        let hive = |p: &str| p == "/hive/sbin/sreboot";
        assert_eq!(command(Os::Linux, &hive), Some(Cmd { prog: "/hive/sbin/sreboot".into(), args: vec![] }));
        let other_hive = |p: &str| p == "/hive/bin/sreboot";
        assert_eq!(command(Os::Linux, &other_hive).unwrap().prog, "/hive/bin/sreboot");
        let none = |_: &str| false;
        assert_eq!(command(Os::Linux, &none).unwrap().to_string(), "systemctl reboot");
        let w = command(Os::Windows, &hive).unwrap();
        assert_eq!(w.to_string(), "shutdown /r /t 60 /c \"FreeForGem: GPU lost\"");
        assert_eq!(w.args, ["/r", "/t", "60", "/c", "FreeForGem: GPU lost"]);
        assert_eq!(command(Os::Other, &hive), None);
        assert_eq!(this_os(), if cfg!(windows) { Os::Windows } else if cfg!(target_os = "linux") { Os::Linux } else { Os::Other });
        // a failing command is logged, not retried
        let d = tmp("fail");
        let mut w = Watch::new(1, None, &d, Some(cmd()));
        let mut lines = Vec::new();
        let mut n = 0;
        for t in (600..700).step_by(10) {
            w.tick(&[lost(0)], t, 1_000_000 + t, &mut |l| lines.push(l), &mut |_| {
                n += 1;
                bail!("permission denied")
            });
        }
        assert_eq!(n, 1);
        assert!(lines.iter().any(|l| l.contains("failed: permission denied")), "{lines:?}");
    }

    #[test]
    fn parse_and_lost_reasons() {
        assert_eq!(parse_minutes("10").unwrap(), 10);
        assert_eq!(parse_minutes("off").unwrap(), 0);
        assert!(parse_minutes("x").is_err() && parse_minutes("2000").is_err());
        assert!(lost_why(false, false, false, false).unwrap().contains("DEAD"));
        assert!(lost_why(true, false, true, false).unwrap().contains("Xid 79"));
        assert!(lost_why(true, false, false, true).unwrap().contains("NVML"));
        // a GPU that makes attempts is never lost
        assert!(lost_why(true, true, true, true).is_none() && lost_why(true, false, false, false).is_none());
        note_xid79(41);
        assert!(xid79_seen(41) && !xid79_seen(42));
    }

    #[test]
    fn every_gpu_down_for_five_minutes_exits_with_its_code() {
        let mut a = AllDown::default();
        assert_eq!(a.tick(2, 3, 0), None);
        assert_eq!(a.tick(3, 3, 10), None);
        assert_eq!(a.tick(3, 3, 300), None);
        // one GPU back: starts over
        assert_eq!(a.tick(2, 3, 305), None);
        assert_eq!(a.tick(3, 3, 310), None);
        assert_eq!(a.tick(3, 3, 609), None);
        assert_eq!(a.tick(3, 3, 610), Some(300));
        assert_eq!(a.tick(0, 0, 10_000), None);
        let e: anyhow::Error = all_down_exit(3, 300).into();
        assert_eq!(exit_code(&e), EXIT_ALL_DOWN);
        assert!(e.to_string().contains("all 3 GPU(s) down") && e.to_string().contains("exit code 3"));
        assert_eq!(exit_code(&anyhow::anyhow!("other")), 1);
    }
}
