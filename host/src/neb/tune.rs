// FFGN — see LICENSE
//! NEB undervolt auto-tuner (`--auto-uv`, off unless asked).  The PROX/Ting tuner of FreeForGem 1.7.8 (prox/tune.rs, ting/tune.rs)
//! with the NEB check: the shared ladder (climb.rs: 30 -> 15 after a failure, --uv-coarse), profiles per GPU (UUID) + driver +
//! kernel + core lock (profiles.rs, uv.json section "neb"; a new lock starts from the profile of the lock above), the offset
//! keeper (noid/offset.rs: the owner's offset put back on exit, on a signal and by the next start after a crash).
//! 1.0.10: every step runs on the card's own MINING load (uvmine.rs): the card keeps mining, submitting shares and counting its
//! evaluated MA/s on whatever path the engine chose (digest table / cheap / plain); the power of a step is paired with the best
//! offset on the same load; stable = no Xid of this card, its worker alive, every result CPU-checked (bad 0). All GPUs are tuned
//! at once (no check processes). Before 1.0.10 (and still as the fallback for a card that does not mine, or FFGN_UV_CHILD=1)
//! every step was checked in a child process (`ffgn --neb-uv-check`, the engine's synthetic plain-path bench) with the card's
//! mining paused: 0.000 MA/s on every card under tuning for the whole tuning.
use super::gpu;
use super::uvmine;
use crate::climb;
use crate::offset;
use crate::nvml::{Dev, Nvml};
use crate::profiles::{self, Profile, Store};
use crate::uv::{self, signed};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const JOBS: usize = 8;
/// a check launch has about CHECK_COUNT / 2^CHECK_K candidates (first BlockId word <= the target word 0 = 2^(32-K) - 1)
const CHECK_K: u32 = 12;
const CHECK_COUNT: u32 = 1 << 22;
const DEF_MAX: i32 = 495;
const DISABLE_SECS: u64 = 24 * 3600;
/// 1.0.12: `Entry::pause_why` of a pause after a real verdict (every offset down to 0 failed on this GPU's own checks)
const PAUSE_VERDICT: &str = "verdict";

/// 1.0.12: a saved 24 h pause counts only when it was written after a verdict (see `Entry::pause_why`)
fn pause_honored(e: &Entry) -> bool {
    e.pause_why == PAUSE_VERDICT
}

/// 1.0.12 (the 12x RTX 2060 SUPER rig, LAS-rig5.md, 21.11.2026): after a reboot auto-uv resumed GPU1/2/3 straight at their saved
/// +375, they crashed there again and on the steps below (+345, +330, +315: an Xid 31 storm), GPU1 then fell off the bus (Xid 79)
/// and GPU2 could not reopen its device. A "confirmed" offset of a card that crashed within the last day is not trusted after a
/// restart: a card with a crash record (`Entry::crash_log`) younger than RECENT_CRASH_SECS, or any card of a rig that saw an Xid 79
/// that recently, resumes at a margin below its saved offset (`resume_margin`) and climbs back from there under the usual rules,
/// never above the saved offset. One day: the crashes of that rig came within hours of each other, and a card that ran a day
/// without a crash gets its saved offset back at once.
pub const RECENT_CRASH_SECS: u64 = 24 * 3600;
/// 1.0.12: an offset that mined this long without a fault of its card is the card's "stable" offset (`Entry::stable`): the resume
/// margin after a crash. One hour: on the 2060S rig the "confirmed" +375 crashed 11-50 min after the start.
pub const STABLE_SECS: u64 = 3600;
/// 1.0.12: crash records kept per profile (the newest; older than RECENT_CRASH_SECS are dropped anyway)
const CRASH_LOG_MAX: usize = 16;
/// 1.0.12: two records of the same offset this close are one crash (the crash record of the main loop and the tuner's own)
const CRASH_SAME_SECS: u64 = 120;
/// 1.0.12: after an Xid 79 (a GPU fell off the bus) the whole rig is cautious this long (uv-xid79.json, kept across restarts):
/// no climbing, one GPU at a time for any tuning or re-check. Six hours: it covers the reboot and the first hours after it (the
/// 2060S rig crashed 11 min to 1 h after its reboot), and a rig with a fixed riser still tunes again the same day.
pub const XID79_CAUTIOUS_SECS: u64 = 6 * 3600;
/// 1.0.12: the default auto-uv climb cap of GeForce Turing (CUDA CC 7.5: RTX 20xx / GTX 16xx; not CMP, see `default_max`) when
/// --uv-max is not given. The 12x RTX 2060 SUPER rig (LAS-rig5.md): the cards that stayed stable mine at +75..+255, the crashes
/// came from +165 up to +450 (+165, +270, +315 x3, +345, +375 x3, +450), and the cards above +240 saved only a few W against
/// +255. +240 keeps every stable result of that rig but one step (+255 -> +240) and cuts off the zone of all its crashes but
/// the +165 ones. The data is one rig of one model (12 cards on gen1/gen2 x1 risers): a default, not a limit; --uv-max N
/// overrides it.
pub const TURING_UV_MAX: i32 = 240;

/// 1.0.12: the default --uv-max of a card: `TURING_UV_MAX` for GeForce Turing (CC 7.5, not CMP: the CMP 50HX of Rig05 run
/// healthy above it), else DEF_MAX; and where it comes from (for the log)
fn default_max(cc: Option<(i32, i32)>, name: &str) -> (i32, &'static str) {
    if cc == Some((7, 5)) && !name.contains("CMP") {
        (TURING_UV_MAX, "default for Turing CC 7.5 (RTX 20xx / GTX 16xx); --uv-max overrides it")
    } else {
        (DEF_MAX, "default")
    }
}

/// 1.0.12: the climb cap of a card: --uv-max (`given`) when set, else `default_max`; never above HARD_CAP nor the driver's
/// range `hi`. Returns the cap and where it comes from.
fn card_max(given: Option<i32>, cc: Option<(i32, i32)>, name: &str, hi: i32) -> (i32, &'static str) {
    let (def, src) = default_max(cc, name);
    let (v, src) = match given {
        Some(v) => (v, "--uv-max"),
        None => (def, src),
    };
    (v.min(uv::HARD_CAP).min(hi), src)
}

/// 1.0.12: GPUs tuned at a time: one while the rig is cautious after an Xid 79
fn par_limit(limit: usize, cautious: bool) -> usize {
    if cautious { 1 } else { limit.max(1) }
}

/// 1.0.12: the cap of a climb from `at`: no room above `at` while the rig is cautious after an Xid 79
fn climb_cap(cap: i32, at: i32, cautious: bool) -> i32 {
    if cautious { cap.min(at) } else { cap }
}

/// 1.0.12: the crash records of a profile younger than RECENT_CRASH_SECS (unix s, offset)
fn recent_crashes(e: &Entry, now: u64) -> Vec<(u64, i32)> {
    e.crash_log.iter().copied().filter(|&(t, _)| now.saturating_sub(t) < RECENT_CRASH_SECS).collect()
}

/// 1.0.12: a crash / failure of this card at offset `at` (unix s `t`) into its profile: the stable offset goes when it is not
/// below the crash; records older than RECENT_CRASH_SECS go, at most CRASH_LOG_MAX are kept
fn note_crash(e: &mut Entry, t: u64, at: i32) {
    if !e.crash_log.iter().any(|&(x, a)| a == at && x.abs_diff(t) <= CRASH_SAME_SECS) {
        e.crash_log.push((t, at));
    }
    let newest = e.crash_log.iter().map(|x| x.0).max().unwrap_or(t);
    e.crash_log.retain(|&(x, _)| newest.saturating_sub(x) < RECENT_CRASH_SECS);
    e.crash_log.sort_unstable();
    let n = e.crash_log.len();
    if n > CRASH_LOG_MAX {
        e.crash_log.drain(..n - CRASH_LOG_MAX);
    }
    if e.stable.is_some_and(|s| s >= at) {
        e.stable = None;
        e.stable_at = 0;
    }
}

/// 1.0.12: a profile written anew (a climb, a resume) keeps the crash history and the stable offset of the one it replaces
fn merge_history(new: &mut Entry, old: &Entry) {
    for &(t, at) in &old.crash_log {
        note_crash(new, t, at);
    }
    if new.stable.is_none() {
        if let Some(s) = old.stable.filter(|&s| !new.crash_log.iter().any(|&(t, a)| a <= s && t >= old.stable_at)) {
            new.stable = Some(s);
            new.stable_at = old.stable_at;
        }
    }
}

/// 1.0.12: where a card with recent crashes resumes instead of its saved offset `saved`: the offset that mined STABLE_SECS without
/// a fault (`stable`, when it is below `saved` and not below the base `floor`), else the base plus half the gap to `saved`, on the
/// step grid of the base. None: no margin (nothing above the base, or the stable offset is the saved one).
fn resume_margin(saved: i32, floor: i32, step: i32, stable: Option<i32>) -> Option<(i32, String)> {
    if saved <= floor {
        return None;
    }
    if let Some(s) = stable.filter(|&s| s >= floor) {
        return (s < saved).then(|| (s, format!("the last offset that mined {} h without a fault", STABLE_SECS / 3600)));
    }
    let step = step.max(1);
    let at = floor + (saved - floor) / 2 / step * step;
    (at < saved).then(|| (at, format!("half way from the base {} to the saved {}", signed(floor), signed(saved))))
}

/// 1.0.12: the rig's Xid 79 (uv-xid79.json next to uv.json): when, and which GPU
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Xid79 {
    pub at: u64,
    pub gpu: String,
}

impl Xid79 {
    fn file(dir: &std::path::Path) -> std::path::PathBuf {
        dir.join("uv-xid79.json")
    }
    pub fn load(dir: &std::path::Path) -> Option<Xid79> {
        let v: Value = serde_json::from_str(&std::fs::read_to_string(Self::file(dir)).ok()?).ok()?;
        Some(Xid79 { at: v["at"].as_u64()?, gpu: v["gpu"].as_str().unwrap_or("a GPU").into() })
    }
    fn save(&self, dir: &std::path::Path) {
        let _ = crate::uv::write_atomic(&Self::file(dir), &json!({"at": self.at, "gpu": self.gpu}).to_string());
    }
    pub fn reset(dir: &std::path::Path) -> bool {
        std::fs::remove_file(Self::file(dir)).is_ok()
    }
    /// no climbing, one GPU at a time
    pub fn cautious(&self, now: u64) -> bool {
        self.at > 0 && now < self.at + XID79_CAUTIOUS_SECS
    }
    /// every GPU of the rig resumes below its saved offset
    pub fn recent(&self, now: u64) -> bool {
        self.at > 0 && now < self.at + RECENT_CRASH_SECS
    }
}

/// 1.0.12: the rig's last Xid 79 known to this process (unix s, 0 = none): uv-xid79.json at the start, then `note_xid79`
static XID79_AT: AtomicU64 = AtomicU64::new(0);
/// 1.0.12: re-checks running now (one at a time while the rig is cautious)
static RECHECKS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn rig_xid79() -> Xid79 {
    Xid79 { at: XID79_AT.load(Ordering::Relaxed), gpu: String::new() }
}

/// 1.0.12: the rig is cautious after an Xid 79 (XID79_CAUTIOUS_SECS): no climbing, one GPU at a time
pub fn rig_cautious(now: u64) -> bool {
    rig_xid79().cautious(now)
}

fn cautious_line(gpu: &str, left_s: u64) -> String {
    format!(
        "CAUTIOUS after the Xid 79 of {gpu} (it fell off the bus: check its riser / power cable; the rig needs a reboot): for {:.1} h more auto-uv climbs nothing and tunes / re-checks one GPU at a time; for {} h after the Xid every GPU resumes below its saved offset",
        left_s as f64 / 3600.0,
        RECENT_CRASH_SECS / 3600
    )
}

/// 1.0.12: an Xid 79 of `label` (neb/mod.rs fault lines, a tuning step): the rig is cautious for XID79_CAUTIOUS_SECS, kept in
/// uv-xid79.json for the next starts; `tuning`: auto-uv runs in this process (the warning on the stats line and in the API)
pub fn note_xid79(dir: &std::path::Path, label: &str, tuning: bool) {
    let now = uv::now();
    Xid79 { at: now, gpu: label.to_string() }.save(dir);
    XID79_AT.store(now, Ordering::Relaxed);
    if tuning {
        warn_set("CAUTIOUS", cautious_line(label, XID79_CAUTIOUS_SECS));
    }
}
/// NEB check lengths (as PROX: 30 / 15 / 60, quick 15; the defaults of uv::Opts are Pearl's 180 / 30 / 120 s)
const WARMUP: u32 = 30;
/// 1.0.5: 25 s (was 15): the power is averaged only after the first 10 s of load (a steady clock and temperature); RTX 2060
/// SUPER, 1.0.4, 4-5 launches per check: +285 read 76.7 W, then 67.4 W at the same offset
const STEP_SECS: u32 = 25;
/// 1.0.5: the power of a check is the mean of the NVML samples after this long under load (at most 2/5 of the check)
const SETTLE_SECS: f64 = 10.0;
const CONFIRM: u32 = 60;
const QUICK: u32 = 15;

/// FFGN 1.0.10: the tuning runs on the MINING load (uvmine.rs): the card keeps mining and counting its MA/s through every step.
/// The check process of 1.0.3-1.0.9 (the synthetic plain-path bench, the card's mining paused) is only the fallback for a card
/// whose worker does not mine within LOAD_WAIT (no work yet, pool down) and for FFGN_UV_CHILD=1.
const LOAD_WAIT: Duration = Duration::from_secs(240);
/// the base check on the mining load (the card is warm already: it mines)
const MINING_BASE: u32 = 10;
/// the coarse step on the mining load when --uv-coarse is not given (fine steps near a known edge and after a failure)
const MINING_COARSE_STEPS: i32 = 4;
/// after a fault: clean mining this long before the next window, given up after RECOVER_MAX (s)
const RECOVER_CALM: f64 = 3.0;
const RECOVER_MAX: f64 = 60.0;
/// the same while mining after the tuning (tick), in whole seconds
const RECOVER_CALM_TICK: u64 = 10;

fn force_child() -> bool {
    std::env::var("FFGN_UV_CHILD").is_ok_and(|v| v == "1")
}

/// 1.0.10: the counters a GPU's mining worker shares with the tuner (neb/mod.rs GpuStat).
pub struct Flags {
    pub bad: Arc<AtomicU64>,
    /// 1.0.11: crashes of the GPU's own engine process (iso.rs)
    pub crashes: Arc<AtomicU64>,
    pub pause: Arc<AtomicBool>,
    pub paused: Arc<AtomicBool>,
    pub attempts: Arc<AtomicU64>,
    pub checked: Arc<AtomicU64>,
    pub load_key: Arc<AtomicU64>,
    pub alive: Arc<AtomicBool>,
}

impl Default for Flags {
    fn default() -> Flags {
        Flags {
            bad: Default::default(),
            crashes: Default::default(),
            pause: Default::default(),
            paused: Default::default(),
            attempts: Default::default(),
            checked: Default::default(),
            load_key: Default::default(),
            alive: Arc::new(AtomicBool::new(true)),
        }
    }
}

fn lg(m: impl AsRef<str>) {
    println!("auto-uv: {}", m.as_ref());
}

/// FFGN 1.0.5: how many GPUs are tuned at the same time. A check process is a whole engine at full load (its own CUDA
/// context, the dataset tables, the reference on the CPU); the power supply and the risers are invisible to software
/// (hive2060SSS: 12 GPUs on a 2-thread Pentium, 1.0.4 tuned all 12 at once and the rig hung). So: start small (1 GPU on
/// rigs of 6+ GPUs, else 2), grow 1 -> 2 -> 4 after GROW_AFTER GPUs tuned without trouble, never above the free CPU threads
/// nor the RAM a check needs (CHECK_MB each); any trouble (a GPU lost from the bus, a check that hangs, all GPUs stalled at
/// once, a mining GPU computing wrong) -> 1 at once, and remembered for the rig (later starts stay at 1 until
/// --uv-reset-all / --uv-parallel-reset). `--uv-parallel N` is fixed.
pub const GROW_AFTER: usize = 2;
pub const PAR_MAX: usize = 4;
/// RAM of one check process (MB) for the limit
pub const CHECK_MB: u64 = 1200;

/// The start: (GPUs at a time, why).
pub fn par_start(given: Option<usize>, gpus: usize, cautious: Option<&str>) -> (usize, String) {
    let gpus = gpus.max(1);
    if let Some(n) = given {
        return (n.clamp(1, gpus), format!("--uv-parallel {n}"));
    }
    if let Some(why) = cautious {
        return (1, format!("auto, cautious on this rig since {why} (--uv-parallel-reset / --uv-reset-all to grow again)"));
    }
    let n = if gpus >= 6 { 1 } else { 2 };
    (n.min(gpus), format!("auto start: {} on {gpus} GPU(s), up to {PAR_MAX} after {GROW_AFTER} GPUs tuned without trouble", if gpus >= 6 { "1" } else { "2" }))
}

/// The ceiling of the CPU and the RAM: free CPU threads (all but one) and the available RAM / CHECK_MB.
pub fn par_ceiling(cores: usize, ram_avail_mb: Option<u64>) -> usize {
    let c = cores.saturating_sub(1).max(1);
    let r = ram_avail_mb.map_or(usize::MAX, |m| ((m / CHECK_MB) as usize).max(1));
    c.min(r).min(PAR_MAX)
}

/// After a GPU's tuning: the new number of GPUs at a time (None = unchanged) and why.
pub fn par_next(cur: usize, ok_in_row: usize, trouble: Option<&str>, auto: bool, cautious: bool, ceiling: usize) -> Option<(usize, String)> {
    if let Some(t) = trouble {
        return (cur != 1 || !cautious).then(|| (1, format!("{t}: 1 GPU at a time from now on (remembered for this rig)")));
    }
    if !auto || cautious || ok_in_row < GROW_AFTER {
        return None;
    }
    let n = (cur * 2).min(PAR_MAX).min(ceiling.max(1));
    (n > cur).then(|| (n, format!("{ok_in_row} GPUs tuned without trouble: {n} at a time (ceiling {ceiling}: free CPU threads / RAM per check)")))
}

/// the rig's memory of trouble while tuning (next to uv.json)
fn caution_file(dir: &std::path::Path) -> std::path::PathBuf {
    dir.join("uv-parallel.json")
}
pub fn caution_get(dir: &std::path::Path) -> Option<String> {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(caution_file(dir)).ok()?).ok()?;
    v["cautious"].as_bool().filter(|&b| b)?;
    Some(v["why"].as_str().unwrap_or("an earlier failure").to_string())
}
fn caution_set(dir: &std::path::Path, why: &str) {
    let _ = std::fs::write(caution_file(dir), json!({"cautious": true, "why": why, "at": uv::now()}).to_string());
}
pub fn caution_reset(dir: &std::path::Path) -> bool {
    std::fs::remove_file(caution_file(dir)).is_ok()
}

/// FFGN 1.0.5 (the owner: "it must never turn into endless tuning and reboots"): the guards of auto-uv, kept in
/// uv-guard.json next to uv.json.
///  * a GPU with CARD_FAILS failures in a row (a crash while mining or in a check at its tuned offset, counted at each start
///    that finds one) gets no auto-uv for 24 h: it mines at its own offset (+0 of the tuner);
///  * the rig: a start that finds the "tuning in progress" mark (the miner ended or the rig rebooted during a tuning, also
///    ffgn's own restarts) is an event; RIG_EVENTS of them within an hour pause auto-uv on the rig until --uv-unpause /
///    --uv-reset-all or a new miner version (mining goes on with the saved results of this version, else at +0);
///  * after one such event the tuning waits DELAY_SECS of mining first and goes on only if the rig stayed calm;
///  * a run spends at most CARD_BUDGET_SECS per GPU and RIG_BUDGET_SECS in all on tuning: the rest goes on at the next start.
pub const CARD_FAILS: u32 = 3;
pub const CARD_OFF_SECS: u64 = 24 * 3600;
pub const RIG_EVENTS: usize = 2;
pub const DELAY_SECS: u64 = 7 * 60;
pub const CARD_BUDGET_SECS: u64 = 10 * 60;
pub const RIG_BUDGET_SECS: u64 = 60 * 60;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Guard {
    pub version: String,
    /// starts that found the tuning mark (unix seconds), the last hour
    pub events: Vec<u64>,
    pub paused: bool,
    /// GPUs being tuned now (uuid): the mark
    pub tuning: Vec<String>,
    /// uuid -> (failures in a row, auto-uv off until)
    pub cards: std::collections::BTreeMap<String, (u32, u64)>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RigStart {
    Go,
    /// the last run ended during a tuning: mine first, then tune if the rig stays calm
    Delay,
    Paused,
}

impl Guard {
    fn file(dir: &std::path::Path) -> std::path::PathBuf {
        dir.join("uv-guard.json")
    }
    pub fn load(dir: &std::path::Path) -> Guard {
        let Ok(t) = std::fs::read_to_string(Self::file(dir)) else { return Guard::default() };
        let Ok(v) = serde_json::from_str::<Value>(&t) else { return Guard::default() };
        let mut g = Guard {
            version: v["version"].as_str().unwrap_or("").into(),
            events: v["events"].as_array().map(|a| a.iter().filter_map(|x| x.as_u64()).collect()).unwrap_or_default(),
            paused: v["paused"].as_bool().unwrap_or(false),
            tuning: v["tuning"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default(),
            cards: Default::default(),
        };
        if let Some(o) = v["cards"].as_object() {
            for (k, x) in o {
                g.cards.insert(k.clone(), (x["fails"].as_u64().unwrap_or(0) as u32, x["off_until"].as_u64().unwrap_or(0)));
            }
        }
        g
    }
    pub fn save(&self, dir: &std::path::Path) {
        let cards: serde_json::Map<String, Value> = self.cards.iter().map(|(k, (f, u))| (k.clone(), json!({"fails": f, "off_until": u}))).collect();
        let v = json!({"version": self.version, "events": self.events, "paused": self.paused, "tuning": self.tuning, "cards": cards});
        let tmp = Self::file(dir).with_extension("json.tmp");
        if std::fs::write(&tmp, v.to_string()).is_ok() {
            let _ = std::fs::rename(&tmp, Self::file(dir));
        }
    }
    /// At the start of a run (`version` of this miner): the event of an unclean end during a tuning, the rig's pause.
    pub fn start(&mut self, now: u64, version: &str) -> RigStart {
        if self.version != version {
            // a new miner version: its own chance (the per-GPU counters stay)
            self.events.clear();
            self.paused = false;
            self.version = version.into();
        }
        let had = !self.tuning.is_empty();
        self.tuning.clear();
        if had {
            self.events.push(now);
        }
        self.events.retain(|&t| now.saturating_sub(t) < 3600);
        if self.events.len() >= RIG_EVENTS {
            self.paused = true;
        }
        if self.paused {
            RigStart::Paused
        } else if had {
            RigStart::Delay
        } else {
            RigStart::Go
        }
    }
    pub fn unpause(&mut self) {
        self.paused = false;
        self.events.clear();
        self.cards.clear();
    }
    /// auto-uv of this GPU is off until (unix seconds), if it is
    pub fn card_off(&self, uuid: &str, now: u64) -> Option<u64> {
        self.cards.get(uuid).map(|x| x.1).filter(|&u| u > now)
    }
    /// After a GPU's start: `failed` = it began with a failure at its tuned offset. Returns the time auto-uv is off until when
    /// this made CARD_FAILS in a row.
    pub fn card_result(&mut self, uuid: &str, failed: bool, now: u64) -> Option<u64> {
        let e = self.cards.entry(uuid.to_string()).or_insert((0, 0));
        if !failed {
            e.0 = 0;
            return None;
        }
        e.0 += 1;
        if e.0 >= CARD_FAILS {
            e.0 = 0;
            e.1 = now + CARD_OFF_SECS;
            return Some(e.1);
        }
        None
    }
}

/// 1.0.5: the loud lines of auto-uv for the stats line, the API and HiveOS (a GPU with auto-uv off, the rig paused)
pub static UV_WARN: Mutex<Vec<String>> = Mutex::new(Vec::new());
fn warn(m: String) {
    eprintln!("WARNING auto-uv: {m}");
    UV_WARN.lock().unwrap().push(m);
}
/// 1.0.12: a warning of the kind `prefix` (it starts with it) replaces the earlier one of that kind
fn warn_set(prefix: &str, m: String) {
    eprintln!("WARNING auto-uv: {m}");
    let mut w = UV_WARN.lock().unwrap();
    w.retain(|x| !x.starts_with(prefix));
    w.push(m);
}
pub fn warnings() -> Vec<String> {
    UV_WARN.lock().unwrap().clone()
}
/// short tags of the warnings for the stats line / HiveOS: "PAUSED", "GPU3 OFF 24h", "NOT CALM"
pub fn tags() -> Vec<String> {
    warnings()
        .iter()
        .map(|w| {
            if w.starts_with("PAUSED") {
                "PAUSED".to_string()
            } else if w.starts_with("CAUTIOUS") {
                // 1.0.12: the rig after an Xid 79
                "CAUTIOUS Xid 79".to_string()
            } else if w.contains("auto-uv OFF") {
                format!("{} OFF 24h", w.split_whitespace().next().unwrap_or("GPU"))
            } else {
                "NOT CALM".to_string()
            }
        })
        .collect()
}

/// FFGN 1.0.5: rig-level trouble seen anywhere in the process (all GPUs stalled at once: neb/mod.rs)
pub static RIG_TROUBLE: AtomicU64 = AtomicU64::new(0);

/// Total and available RAM in MB (Linux /proc/meminfo, Windows GlobalMemoryStatusEx); None if unknown.
pub fn ram_mb() -> Option<(u64, u64)> {
    #[cfg(target_os = "linux")]
    {
        let s = std::fs::read_to_string("/proc/meminfo").ok()?;
        let get = |k: &str| s.lines().find(|l| l.starts_with(k)).and_then(|l| l.split_whitespace().nth(1)).and_then(|v| v.parse::<u64>().ok());
        Some((get("MemTotal:")? / 1024, get("MemAvailable:").or_else(|| get("MemFree:"))? / 1024))
    }
    #[cfg(windows)]
    {
        #[repr(C)]
        struct Ms {
            len: u32,
            load: u32,
            total: u64,
            avail: u64,
            pf_total: u64,
            pf_avail: u64,
            v_total: u64,
            v_avail: u64,
            v_ext: u64,
        }
        unsafe extern "system" {
            fn GlobalMemoryStatusEx(m: *mut Ms) -> i32;
        }
        let mut m = Ms { len: std::mem::size_of::<Ms>() as u32, load: 0, total: 0, avail: 0, pf_total: 0, pf_avail: 0, v_total: 0, v_avail: 0, v_ext: 0 };
        if unsafe { GlobalMemoryStatusEx(&mut m) } == 0 {
            return None;
        }
        Some((m.total >> 20, m.avail >> 20))
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        None
    }
}

/// FFGN 1.0.5: check processes start at least this far apart (no 12 CUDA contexts and 12 power steps at the same moment)
const CHILD_GAP: Duration = Duration::from_secs(3);
/// ... and only with this much RAM available (waits up to CHILD_RAM_WAIT, then starts anyway)
const CHILD_RAM_MB: u64 = 700;
const CHILD_RAM_WAIT: Duration = Duration::from_secs(90);
static CHILD_GATE: Mutex<Option<Instant>> = Mutex::new(None);

/// Waits for the start gap and for free RAM before a check process starts (one start at a time).
fn child_gate(stop: &AtomicBool, label: &str) {
    let mut last = CHILD_GATE.lock().unwrap();
    if let Some(t) = *last {
        let since = t.elapsed();
        if since < CHILD_GAP {
            std::thread::sleep(CHILD_GAP - since);
        }
    }
    let t0 = Instant::now();
    let mut told = false;
    while let Some((_, avail)) = ram_mb() {
        if avail >= CHILD_RAM_MB || t0.elapsed() >= CHILD_RAM_WAIT || stop.load(Ordering::Relaxed) {
            if told && avail < CHILD_RAM_MB {
                lg(format!("{label}: still only {avail} MB of RAM available after {} s: the check starts anyway", CHILD_RAM_WAIT.as_secs()));
            }
            break;
        }
        if !told {
            lg(format!("{label}: {avail} MB of RAM available (want {CHILD_RAM_MB}): the check waits for RAM"));
            told = true;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    *last = Some(Instant::now());
}

fn kernel_id() -> String {
    format!("ffgn{}/neb", env!("CARGO_PKG_VERSION"))
}

/// 1.0.5: the mark of a profile saved in the middle of a climb (every taken step is saved at once: hive2060SSS hung during
/// the tuning of 1.0.4 and lost all of it). Such a profile is not "this version's result": after a restart it gets the check of
/// its offset and the climb goes on upward from it (Plan::Continue); a failure or a crash in the middle of a check (the trying
/// marker) still counts as failed and resumes 2 steps below it, never above the last taken step.
fn climbing_id() -> String {
    format!("{}/climbing", kernel_id())
}

// ---------------------------------------------------------------------------------------------------------------------
// the check (child process)
// ---------------------------------------------------------------------------------------------------------------------

/// `ffgn --neb-uv-check --devices D --uv-secs S`: prints `NEB_LOAD`, then `NEB_CHECK ok launches N ghs R counts - cpu K` (R in MA/s)
/// or `NEB_CHECK FAIL reason`; exit code 0 = ok.  The engine's bench on its built-in synthetic models of both networks at full
/// load, each round with the engine's canary (GPU tickets vs the C reference inside the library): one wrong ticket fails the step.
pub fn check_child(dev: i32, secs: u32, _refs: &str, _backend: Option<gpu::Backend>) -> i32 {
    let parent = parent_id();
    std::thread::spawn(move || {
        let t = Instant::now();
        loop {
            std::thread::sleep(Duration::from_millis(500));
            if parent.is_some() && parent_id() != parent {
                std::process::exit(6); // the tuner died: do not keep loading its GPU
            }
            if t.elapsed() > Duration::from_secs(secs as u64 + 60) {
                println!("NEB_CHECK FAIL the check hung (no result after {} s)", secs + 60);
                std::process::exit(5);
            }
        }
    });
    let fail = |why: String| {
        println!("NEB_CHECK FAIL {why}");
        1
    };
    if let Err(e) = gpu::load(Some(gpu::Backend::Cuda), 1) {
        return fail(format!("{e:#}"));
    }
    let mut g = match gpu::open_bare(dev) {
        Ok(g) => g,
        Err(e) => return fail(format!("{e:#}")),
    };
    println!("NEB_LOAD");
    let t0 = Instant::now();
    let (mut launches, mut cpu, mut att, mut ms) = (0u64, 0u64, 0.0f64, 0.0f64);
    let mut arch = 1u32;
    while t0.elapsed().as_secs_f64() < secs as f64 || launches < 2 {
        let t = Instant::now();
        match g.bench_arch(arch, 2000) {
            Ok(r) => {
                att += r * t.elapsed().as_secs_f64();
                ms += t.elapsed().as_secs_f64();
            }
            Err(e) => return fail(format!("{e:#}")),
        }
        // the bench runs the engine's canary (GPU tickets of the synthetic model vs its C reference) in every launch: a wrong
        // ticket returns E_COMPUTE, i.e. the bench call above fails
        cpu += 2;
        launches += 1;
        arch = 3 - arch;
        if t0.elapsed().as_secs_f64() > secs as f64 + 30.0 {
            break;
        }
    }
    println!("NEB_CHECK ok launches {launches} ghs {:.3} counts - cpu {cpu}", att / ms.max(1e-9) / 1e6);
    0
}

#[cfg(unix)]
fn parent_id() -> Option<u32> {
    Some(std::os::unix::process::parent_id())
}
#[cfg(not(unix))]
fn parent_id() -> Option<u32> {
    None
}

// ---------------------------------------------------------------------------------------------------------------------
// profiles (uv.json section "neb")
// ---------------------------------------------------------------------------------------------------------------------

#[derive(Clone, Default, Debug)]
struct Entry {
    name: String,
    bus: String,
    driver: String,
    kernel: String,
    core_lock: u32,
    clk: u32,
    orig: i32,
    base: i32,
    offset: i32,
    step: i32,
    edge: i32,
    w_base: f64,
    w: f64,
    ghs: f64,
    refs: String,
    tested_at: u64,
    verified_at: u64,
    disabled_until: u64,
    stepped_back: u32,
    /// 1.0.12: why the profile is paused ("verdict": every offset from the start down to 0 failed on this GPU's own checks).
    /// A pause without it (written before 1.0.12) is not honored: 1.0.10 / 1.0.11 also paused a GPU for 24 h at +0 when its base
    /// check had no verdict (its worker stopped after ANOTHER GPU's fault; Rig01 21.11.2026, FIELD111.md anomaly 1).
    pause_why: String,
    /// 1.0.12: the recent crashes / failures of this card at an offset it mined at or resumed at (unix s, offset; at most
    /// CRASH_LOG_MAX, none older than RECENT_CRASH_SECS); "crash_log" in uv.json, absent in profiles written before 1.0.12
    crash_log: Vec<(u64, i32)>,
    /// 1.0.12: the last offset that mined STABLE_SECS without a fault of this card, and when ("stable_offset", "stable_at")
    stable: Option<i32>,
    stable_at: u64,
}

impl Profile for Entry {
    fn from_json(v: &Value) -> Option<Entry> {
        let u = |k: &str| v[k].as_u64().unwrap_or(0);
        let i = |k: &str| v[k].as_i64().unwrap_or(0) as i32;
        let f = |k: &str| v[k].as_f64().unwrap_or(0.0);
        let s = |k: &str| v[k].as_str().unwrap_or("").to_string();
        Some(Entry {
            name: s("name"),
            bus: s("bus"),
            driver: s("driver"),
            kernel: v["kernel"].as_str()?.into(),
            core_lock: u("core_lock") as u32,
            clk: u("clk_mhz") as u32,
            orig: i("orig_offset"),
            base: v["base_offset"].as_i64()? as i32,
            offset: v["offset"].as_i64()? as i32,
            step: i("step"),
            edge: i("edge"),
            w_base: f("watts_base"),
            w: f("watts"),
            ghs: f("ghs"),
            refs: s("refs"),
            tested_at: u("tested_at"),
            verified_at: u("verified_at"),
            disabled_until: u("disabled_until"),
            stepped_back: u("stepped_back_in_mining") as u32,
            pause_why: s("pause_why"),
            crash_log: v["crash_log"]
                .as_array()
                .map(|a| a.iter().filter_map(|x| Some((x["t"].as_u64()?, x["at"].as_i64()? as i32))).collect())
                .unwrap_or_default(),
            stable: v["stable_offset"].as_i64().map(|x| x as i32),
            stable_at: u("stable_at"),
        })
    }
    fn to_json(&self) -> Value {
        json!({"name": self.name, "bus": self.bus, "driver": self.driver, "kernel": self.kernel, "core_lock": self.core_lock, "clk_mhz": self.clk,
               "orig_offset": self.orig, "base_offset": self.base, "offset": self.offset, "step": self.step, "edge": self.edge,
               "watts_base": self.w_base, "watts": self.w, "ghs": self.ghs, "refs": self.refs, "tested_at": self.tested_at,
               "verified_at": self.verified_at, "disabled_until": self.disabled_until, "stepped_back_in_mining": self.stepped_back,
               "pause_why": self.pause_why,
               "crash_log": self.crash_log.iter().map(|&(t, at)| json!({"t": t, "at": at})).collect::<Vec<_>>(),
               "stable_offset": self.stable, "stable_at": self.stable_at})
    }
    fn same_clocks(&self, o: &Entry) -> bool {
        self.driver == o.driver
            && match (self.core_lock, o.core_lock) {
                (0, 0) => (self.clk as i64 - o.clk as i64).abs() <= 45,
                (a, b) => a == b,
            }
    }
}

/// 1.0.3: after a failure a card resumes this many steps below the failed offset (the owner: "not from zero, 2 steps lower")
const RESUME_BACK: i32 = 2;
/// the check at a resumed offset: shorter than the confirmation of a climb (CONFIRM), longer than the quick check of a saved profile
const RESUME: u32 = 30;

/// What prepare_card knows of the saved profile of this card / lock.
#[derive(Clone, Copy, Debug)]
struct SavedProf {
    offset: i32,
    base: i32,
    same_kernel: bool, // saved by this miner version (else: another version, its offset is only an upper bound)
    /// 1.0.3: its check power was below the base (an older version saved "+0 -> +60, 64.4 W -> 87.8 W": not usable)
    lowered: bool,
}

/// 1.0.3: a tolerance of a check's mean power (W): 1.5 % of it, at least 1 W.  1.0.4: only a sanity guard against a power that
/// clearly ROSE (a saved profile or a resumed offset that draws more than its base: the 1.0.2 overclock of a 2060 SUPER); it is
/// never a threshold of a saving any more.
fn tol_w(w: f64) -> f64 {
    (w * 0.015).max(1.0)
}

/// 1.0.4 (the owner, 18.11.2026: "1 W per step from a card is very good"; RTX 2060 SUPER: +195 -0.6 W, +225 -0.9 W, and 1.0.3
/// stopped at +165): a step is taken when its power is lower than the best (lowest) power of the climb by more than this.  No
/// percentages anywhere.  Measured 18.11.2026 on Shahtyor GPU1 (RTX 3070 @1650, 15 s checks): 10 checks in a row at one offset
/// 160.2-161.3 W after the warm-up (sd 0.4 W); the owner: 0.3-0.5 W on a 3070 and a 2060 SUPER.
const NOISE_W: f64 = 0.5;
/// Between checks at different offsets the noise is larger (the same sweep 0..+270 up and down: the power falls ~1-1.5 W per
/// 15 MHz down to 128 W at +270, but single readings move by 1-2 W, +30 -> +60 even read +2.0 W): one step that does not beat the
/// best does not stop the climb; PATIENCE of them in a row do (the power stopped falling).  1.0.4 test: stopping at the first
/// flat step left an RTX 3070 at +90 (153 W in the confirmation) where +165 had drawn 139 W.
const PATIENCE: u32 = 3;
/// a rise above the best this big (more than the noise between checks, 2 W) ends the climb at once
const RISE_W: f64 = 3.0;
/// at the power limit the power cannot fall: the rate decides, with this noise (share of the rate)
const RATE_NOISE: f64 = 0.005;
/// a step that did not lower the power while the card warmed by this much (C) is measured once more (a warming card draws more
/// at the same voltage: the saving hides)
const WARM_C: i32 = 2;

/// At the power limit (within 3 %): the power cannot fall there, the clock (and the rate) rises instead.
fn at_pl(w: f64, pl: Option<f64>) -> bool {
    pl.is_some_and(|p| p > 0.0 && w >= p * 0.97)
}

/// 1.0.3: one measured check (offset, mean power, MA/s of the check, card temperature).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Meas {
    at: i32,
    w: Option<f64>,
    ghs: f64,
    t: Option<i32>,
}

/// 1.0.4: what a passed step did to the power against the best step so far.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Verdict {
    /// lower by more than NOISE_W: taken, the climb goes on
    Lower,
    /// not lower beyond the noise (within it, or a rise up to RISE_W): not taken; PATIENCE of them in a row stop the climb
    Flat,
    /// higher by more than RISE_W: the climb stops at once
    Rose,
}

/// 1.0.4: the verdict of `now` against `best` (same clock, a lower voltage); both at the power limit: the rate (`RATE_NOISE`).
/// Unknown power: taken (as before).
fn step_verdict(best: &Meas, now: &Meas, pl: Option<f64>) -> Verdict {
    let (Some(p), Some(w)) = (best.w, now.w) else { return Verdict::Lower };
    if at_pl(p, pl) && at_pl(w, pl) {
        if best.ghs <= 0.0 || now.ghs <= 0.0 {
            return Verdict::Flat;
        }
        let g = now.ghs / best.ghs - 1.0;
        return if g > RATE_NOISE { Verdict::Lower } else { Verdict::Flat };
    }
    if p - w > NOISE_W {
        Verdict::Lower
    } else if w - p > RISE_W {
        Verdict::Rose
    } else {
        Verdict::Flat
    }
}

/// the card warmed by WARM_C or more between the two measurements
fn warmed(prev: &Meas, now: &Meas) -> bool {
    matches!((prev.t, now.t), (Some(a), Some(b)) if b - a >= WARM_C)
}

/// 1.0.4: the judge of the climb.  Every passed step is compared with the best (lowest-power) step so far: lower by more than
/// NOISE_W -> taken, the new best.  Higher by more than RISE_W -> stop.  Otherwise not taken, and the climb goes on: the power of a
/// card falls in noisy stairs (RTX 3070 @1650, Shahtyor 18.11.2026: +90 142.1 W, +120 142.6 W, +150 141.8 W, and +270 ~129 W);
/// PATIENCE such steps in a row = the power stopped falling.  The result is `best`: the highest offset that still lowered the power.
#[derive(Clone, Copy, Debug)]
struct Judge {
    best: Meas,
    held: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Decision {
    /// lower than `best` beyond the noise: the new best, the climb goes on
    Take,
    /// not lower than `best`: not taken, the climb goes on (n of PATIENCE)
    Hold(u32),
    /// a clear rise, or PATIENCE steps in a row that did not lower the power: the climb ends at `best`
    Stop(Verdict),
}

impl Judge {
    fn new(origin: Meas) -> Judge {
        Judge { best: origin, held: 0 }
    }
    fn judge(&mut self, now: Meas, pl: Option<f64>) -> Decision {
        match step_verdict(&self.best, &now, pl) {
            Verdict::Lower => {
                self.best = now;
                self.held = 0;
                Decision::Take
            }
            Verdict::Rose => Decision::Stop(Verdict::Rose),
            Verdict::Flat => {
                self.held += 1;
                if self.held >= PATIENCE { Decision::Stop(Verdict::Flat) } else { Decision::Hold(self.held) }
            }
        }
    }
}

fn fw(w: Option<f64>) -> String {
    w.map_or("?".into(), |w| format!("{w:.1} W"))
}

/// 1.0.3: what the clock under a check's load says about the core lock `lock` at offset `at`.
#[derive(Debug, PartialEq)]
enum ClockVerdict {
    Fine,
    /// at a positive offset the clock runs above the lock: the offset overclocks (GeForce Turing: the V/F offset moves the locked
    /// point up), the step is not taken
    OffsetRaises(u32),
    /// at +0 (or below) the clock runs above the lock: the lock was reset (set it again) or does not hold
    LockOff(u32),
}

/// `start`: the offset the card starts from (the owner's / HiveOS offset, or the checked base): at or below it a clock above the lock
/// is the lock's fault, above it the offset's.
fn clock_verdict(lock: Option<u32>, at: i32, start: i32, clk_med: Option<u32>) -> ClockVerdict {
    let (Some(l), Some(m)) = (lock, clk_med) else { return ClockVerdict::Fine };
    if m <= l + crate::clocks::hold_tol(l) {
        ClockVerdict::Fine
    } else if at > start {
        ClockVerdict::OffsetRaises(m)
    } else {
        ClockVerdict::LockOff(m)
    }
}

#[derive(Debug, PartialEq)]
enum Plan {
    /// the old way: a base check and the climb (a first tuning, a bystander of another card's fault, nothing known to resume at)
    Tune,
    /// check the offsets of `ladder` in turn (`secs` each), mine at the first that passes
    Resume { failed: Option<i32>, why: String, ladder: Vec<i32>, secs: u32 },
    /// 1.0.4: a profile of an earlier version (1.0.3 stopped early or fell back by its 3 % rule): a RESUME check at its offset,
    /// then the climb goes on upward from there with the 1.0.4 rule (each step must lower the power beyond the noise)
    Continue { from: i32 },
}

enum Resumed {
    Mining,
    Stopped,
    /// nothing of the ladder passed; the lowest offset that failed
    Exhausted(i32),
}

/// `first`, then RESUME_BACK steps lower each time, while above `floor` (`first` itself also at the floor when `incl` is set).
fn ladder(first: i32, floor: i32, step: i32, incl: bool) -> Vec<i32> {
    let back = RESUME_BACK * step.max(1);
    let mut v: Vec<i32> = Vec::new();
    if incl && first == floor {
        v.push(first);
    }
    let mut at = first;
    while at > floor {
        v.push(at);
        at -= back;
    }
    v
}

/// 1.0.3: where a card starts after a restart.  A failed offset (`crash`: a crash while mining = `mined_crash`, or a fault in a
/// check) with a confirmed one (the saved profile, else the steps that had passed) -> resume at (failed - 2 steps), not above the
/// confirmed one; a saved profile and nothing failed -> its offset (a quick check when this version saved it); else the climb.
fn plan(crash: Option<profiles::Crash>, prof: Option<SavedProf>, floor: i32, cap: i32, step: i32, mined_crash: Option<i32>) -> Plan {
    let step = step.max(1);
    let back = RESUME_BACK * step;
    let usable = prof.filter(|p| p.offset >= p.base && p.lowered);
    match crash {
        Some(cr) if cr.failed() => {
            let Some(conf) = usable.map(|p| p.offset).or(cr.good) else { return Plan::Tune };
            let what = if mined_crash == Some(cr.at) { "the crash while mining at" } else { "the failure at" };
            let first = (cr.at - back).min(conf).min(cap);
            let why = if first < cr.at - back {
                format!("{what} {} was {RESUME_BACK} steps above {}; not above the last confirmed {}", signed(cr.at), signed(cr.at - back), signed(first))
            } else {
                format!("{RESUME_BACK} steps below {what} {}", signed(cr.at))
            };
            Plan::Resume { failed: Some(cr.at), why, ladder: ladder(first, floor, step, false), secs: RESUME }
        }
        Some(_) => Plan::Tune,
        None => match usable {
            Some(p) if p.same_kernel => Plan::Resume { failed: None, why: "the saved profile".into(), ladder: ladder(p.offset.min(cap), floor, step, true), secs: QUICK },
            // 1.0.4: an earlier version's profile is not a ceiling any more: checked, then tuned on upward from it (not from +0)
            Some(p) => Plan::Continue { from: p.offset.min(cap).max(floor) },
            None => Plan::Tune,
        },
    }
}

/// 1.0.12: a resume below the saved offset (LAS-rig5.md): check at `at`, then climb back up to `ceiling` at most.
#[derive(Debug, PartialEq)]
struct Margin {
    at: i32,
    saved: i32,
    ceiling: i32,
    why: String,
}

/// 1.0.12: the margin of a start that would resume at a saved offset (`Plan::Resume` of a saved profile, nothing failed in the
/// previous run; `Plan::Continue`) when the profile `e` has crashes younger than RECENT_CRASH_SECS or the rig had an Xid 79 that
/// recently (`rig79`). A start after a failure of the previous run keeps the owner's rule (`plan`: failed - 2 steps, never above
/// the last confirmed offset) and gets no margin on top. The climb from the margin goes up to the saved offset at most (Continue:
/// up to `cap`, as before).
fn margin_plan(p: &Plan, e: &Entry, floor: i32, step: i32, cap: i32, now: u64, rig79: bool) -> Option<Margin> {
    let (saved, ceiling) = match p {
        Plan::Resume { failed: None, ladder, .. } => (*ladder.first()?, *ladder.first()?),
        Plan::Continue { from } => (*from, cap),
        _ => return None,
    };
    let recent = recent_crashes(e, now);
    let cause = match recent.iter().max() {
        Some(&(t, at)) => format!(
            "{} crash(es) / failure(s) of this GPU in the last {} h (the latest at {}, {:.1} h ago)",
            recent.len(),
            RECENT_CRASH_SECS / 3600,
            signed(at),
            now.saturating_sub(t) as f64 / 3600.0
        ),
        None if rig79 => format!("an Xid 79 (a GPU fell off the bus) on this rig in the last {} h", RECENT_CRASH_SECS / 3600),
        None => return None,
    };
    let (at, how) = resume_margin(saved, floor, step, e.stable)?;
    Some(Margin { at, saved, ceiling: ceiling.max(at), why: format!("{cause}: not straight at the saved {}, at {how}", signed(saved)) })
}

/// A saved profile is usable unless its offset clearly RAISED the check power against its base (1.0.2 on a 2060 SUPER: +0 -> +60,
/// 64.4 W -> 87.8 W).  1.0.4: no minimum saving (1.0.3 asked for 3 %); unknown power: usable, as before.
fn profile_lowered(e: &Entry) -> bool {
    !(e.w_base > 0.0 && e.w > 0.0 && e.offset > e.base && e.w > e.w_base + tol_w(e.w_base))
}

/// "1500" (the core lock) or "~1905" (no lock: the clock under load at the base check)
fn pkey(e: &Entry) -> String {
    if e.core_lock != 0 { format!("{}", e.core_lock) } else { format!("~{}", (e.clk + 7) / 15 * 15) }
}

// ---------------------------------------------------------------------------------------------------------------------
// one GPU
// ---------------------------------------------------------------------------------------------------------------------

pub struct CardRef {
    pub dev: i32,
    pub bus: String,
    pub name: String,
    pub core_lock: Option<u32>,
    pub pause: Arc<AtomicBool>,
    pub paused: Arc<AtomicBool>,
    pub bad: Arc<AtomicU64>,
    /// 1.0.11 (iso.rs): crashes of this GPU's own engine process (its worker started a new one; the other GPUs mined on)
    pub crashes: Arc<AtomicU64>,
    /// 1.0.10: the worker's evaluated attempts, CPU-checked results, load key and alive flag (uvmine.rs)
    pub attempts: Arc<AtomicU64>,
    pub checked: Arc<AtomicU64>,
    pub load_key: Arc<AtomicU64>,
    pub alive: Arc<AtomicBool>,
}

struct Card {
    r: CardRef,
    uuid: String,
    nv: Dev,
    orig: i32,
    lo: i32,
    cap: i32,
    base: AtomicI32,  // the checked start offset: step-backs stop here
    cur: AtomicI32,
    key: Mutex<String>,
    refs: Mutex<String>,
    owned: AtomicBool, // the tuner watches this GPU (offset above its base)
    busy: AtomicBool,
    bad_seen: AtomicU64,
    /// 1.0.11: the crashes of the card's engine process seen by `tick`
    crash_seen: AtomicU64,
    next_reval: AtomicU64,
    next_readback: AtomicU64,
    resets: crate::hiveoc::ResetLimit, // OC applies (HiveOS nvidia-oc) answered with a re-apply, per hour
    unheld: AtomicBool, // 1.0.3: left because its core lock does not hold
    /// 1.0.5: this start found a failure at its tuned offset (the guard's count); the tuning deadline (unix seconds)
    failed_start: AtomicBool,
    deadline: AtomicU64,
    /// 1.0.10: the checks of this card run on its mining load (else in a check process with its mining paused)
    mining: AtomicBool,
    /// 1.0.10: a check found no mining load for a long time (no work): not a verdict, nothing disabled
    no_load: AtomicBool,
    /// 1.0.12: the last base check ended without a verdict (`infra`: its worker stopped after a fault, interrupted, the lock
    /// does not hold): give_up then pauses nothing
    no_verdict: AtomicBool,
    /// 1.0.10 (live test): the last window on the mining load ended with a fault / wrong result: the next one first waits until
    /// the card mines cleanly again (uvmine::wait_healthy); the faulting offset and when (unix s), for the crash record
    recover: AtomicBool,
    fault_at: AtomicI32,
    fault_t: AtomicU64,
    /// the recovery seen from `tick` (mining after the tuning): attempts at the last tick, the last unclean moment (unix s)
    rec_att: AtomicU64,
    rec_since: AtomicU64,
    rec_t: AtomicU64,
    /// 1.0.12: where `cap` comes from (--uv-max or the card's default, `default_max`)
    cap_src: &'static str,
    /// 1.0.12: the offset mined since (unix s) without a fault seen by `tick`, and whether it was saved as the stable offset
    stable_cur: AtomicI32,
    stable_since: AtomicU64,
    stable_saved: AtomicBool,
}

impl Card {
    fn label(&self) -> String {
        format!("GPU{} (bus {}, {}{})", self.r.dev, uv::short_bus(&self.r.bus), self.r.name.replace("NVIDIA GeForce ", ""), self.r.core_lock.map_or(String::new(), |l| format!(", lock {l}")))
    }
}

#[derive(Default)]
struct Res {
    ok: bool,
    fault: bool, // driver fault / hang: no refining
    infra: bool, // not a GPU verdict (stop, no executable)
    reason: String,
    refs: String,
    launches: u64,
    ghs: f64,
    w: Option<f64>,
    /// 1.0.5: standard deviation of the power samples of the window (W) and their number
    w_sd: Option<f64>,
    w_n: usize,
    clk: Option<u32>,
    /// 1.0.3: median core clock under the check load
    clk_med: Option<u32>,
    /// 1.0.3: the step was refused because the clock ran above the lock (MHz)
    clk_over: Option<u32>,
    t: Option<i32>,
    /// 1.0.10: measured on the mining load: the paired power against the reference offset, CPU-checked results, no load
    mining: bool,
    pair: Option<uvmine::Pair>,
    reference: Option<i32>,
    checked: u64,
    no_load: bool,
    secs: f64,
}

pub struct Params {
    pub step: i32,
    pub coarse: i32,
    pub keep_last: bool,
    pub reval_h: f64,
    pub warmup: u32,
    pub step_secs: u32,
    pub confirm: u32,
    /// 1.0.10: the coarse step on the mining load (--uv-coarse, else MINING_COARSE_STEPS fine steps)
    pub coarse_mining: i32,
    pub min: uv::PerGpu,
    pub reset: bool,
}

impl Params {
    /// From the shared --uv-* options; check lengths left at the Pearl defaults get the NEB ones.
    pub fn from(o: &uv::Opts) -> Params {
        let d = uv::Opts::default();
        let pick = |v: u32, pearl: u32, prox: u32| if v == pearl { prox } else { v };
        Params {
            step: o.step,
            coarse: o.coarse_step(),
            keep_last: o.keep_last_good,
            reval_h: o.reval_hours,
            warmup: pick(o.warmup_secs, d.warmup_secs, WARMUP),
            step_secs: pick(o.step_secs, d.step_secs, STEP_SECS),
            confirm: pick(o.confirm_secs, d.confirm_secs, CONFIRM),
            coarse_mining: if o.coarse.is_some() { o.coarse_step() } else { (MINING_COARSE_STEPS * o.step).max(o.step) },
            min: o.min_offset.clone(),
            reset: o.reset,
        }
    }
}

pub struct Live {
    nv: &'static Nvml,
    db: Store,
    p: Params,
    driver: String,
    cards: Vec<Arc<Card>>,
    stop: Arc<AtomicBool>,
    /// 1.0.5: the guards (uv-guard.json) and the tuning deadline of this run (unix seconds)
    guard: Mutex<Guard>,
    rig_deadline: u64,
}

impl Live {
    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    /// A check at `at` with (1.0.3) the core lock watched under its load (`clock_verdict`):
    ///  * a positive offset that runs the clock above the lock: (1.0.4) the lock is set again and the step checked once more;
    ///    still above -> not taken (a failed step, no refining: `clk_over`);
    ///  * at +0 above the lock: the lock is set again and checked once more; still above -> the lock does not hold on this card
    ///    (clocks::mark_not_held, a loud warning): an `infra` result, the tuner leaves the card.
    fn check(&self, c: &Card, at: i32, secs: u32, refs: &str) -> Res {
        self.check_ref(c, at, secs, refs, None)
    }

    /// `check` with (1.0.10, on the mining load) the power paired against `reference` (the best offset so far, or the base).
    fn check_ref(&self, c: &Card, at: i32, secs: u32, refs: &str, reference: Option<i32>) -> Res {
        let r = self.check_ref_inner(c, at, secs, refs, reference);
        if r.no_load {
            c.no_load.store(true, Ordering::Relaxed);
        }
        r
    }

    fn check_ref_inner(&self, c: &Card, at: i32, secs: u32, refs: &str, reference: Option<i32>) -> Res {
        if let Some((l, m)) = crate::clocks::not_held(&c.r.bus) {
            return Res { infra: true, reason: format!("the core clock lock {l} MHz does not hold (runs at {m} MHz)"), ..Default::default() };
        }
        let r = self.check_once(c, at, secs, refs, reference);
        // 1.0.5: rig-level trouble for --uv-parallel (an Xid of the tuned card at a high offset is the edge, not the rig's)
        if r.fault && (r.reason.contains("Xid 79") || r.reason.starts_with("timeout")) {
            RIG_TROUBLE.fetch_add(1, Ordering::Relaxed);
        }
        if r.fault && r.reason.contains("Xid 79") {
            note_xid79(&self.db.dir, &c.label(), true);
        }
        if r.infra {
            return self.held_after(c, r);
        }
        let l = c.r.core_lock.unwrap_or(0);
        let start = c.base.load(Ordering::Relaxed);
        match clock_verdict(c.r.core_lock, at, start, r.clk_med) {
            ClockVerdict::Fine => self.held_after(c, r),
            ClockVerdict::OffsetRaises(m) => {
                // 1.0.4: the lock may have been reset meanwhile (HiveOS nvidia-oc at the miner's start): set it again and check the
                // step once more; only a clock above the lock with the lock just set again refuses the step
                let again = crate::clocks::set_core_again(&c.r.bus);
                lg(format!(
                    "{} step {}: the core clock ran at {m} MHz, above the lock {l} MHz: {}",
                    c.label(),
                    signed(at),
                    if again { "the lock is set again and the step checked once more" } else { "the lock cannot be set again (not set by this run / no permission)" }
                ));
                let r = if again { self.check_once(c, at, secs, refs, reference) } else { r };
                if r.infra {
                    return self.held_after(c, r);
                }
                let m = match clock_verdict(c.r.core_lock, at, start, r.clk_med) {
                    ClockVerdict::Fine => return self.held_after(c, r),
                    ClockVerdict::OffsetRaises(m2) | ClockVerdict::LockOff(m2) => m2,
                };
                let mut r = r;
                r.ok = false;
                r.fault = true; // no refining inside the interval: higher offsets raise the clock even more
                r.clk_over = Some(m);
                r.reason = format!("the offset raises the core clock above the lock ({m} MHz at lock {l}): not taken");
                self.held_after(c, r)
            }
            ClockVerdict::LockOff(m) => {
                let again = crate::clocks::set_core_again(&c.r.bus);
                lg(format!(
                    "{} the core clock lock {l} MHz is not held in the check at {} (the card ran at {m} MHz): the core clock lock was reset by something else (HiveOS OC apply?): {}",
                    c.label(),
                    signed(at),
                    if again { "set again, checking once more" } else { "it cannot be set again (not set by this run / no permission)" }
                ));
                let r2 = if again { self.check_once(c, at, secs, refs, reference) } else { r };
                if let ClockVerdict::LockOff(m2) = clock_verdict(c.r.core_lock, at, start, r2.clk_med) {
                    if !r2.infra {
                        crate::clocks::mark_not_held(&c.r.bus, c.r.dev, l, m2);
                        return Res { infra: true, reason: format!("the core clock lock {l} MHz does not hold"), ..Default::default() };
                    }
                }
                self.held_after(c, r2)
            }
        }
    }

    /// the clock keeper (clocks.rs) may have found the lock not holding during the check
    fn held_after(&self, c: &Card, r: Res) -> Res {
        match crate::clocks::not_held(&c.r.bus) {
            Some((l, m)) => Res { infra: true, reason: format!("the core clock lock {l} MHz does not hold (runs at {m} MHz)"), ..Default::default() },
            None => r,
        }
    }

    /// 1.0.3: the card's core lock does not hold: the offset back to +0 (within the card's range), auto-uv leaves this card.
    fn leave_unheld(&self, c: &Card) -> bool {
        let Some((l, m)) = crate::clocks::not_held(&c.r.bus) else { return false };
        if c.unheld.swap(true, Ordering::Relaxed) {
            return true;
        }
        let z = 0.clamp(c.lo, c.cap);
        let _ = offset::set(&c.r.bus, z);
        c.cur.store(z, Ordering::Relaxed);
        c.base.store(z, Ordering::Relaxed);
        c.owned.store(false, Ordering::Relaxed);
        lg(format!("{} the core clock lock {l} MHz does not hold (runs at {m} MHz): auto-uv is off for this GPU, offset {}; nothing saved", c.label(), signed(z)));
        true
    }

    fn check_once(&self, c: &Card, at: i32, secs: u32, refs: &str, reference: Option<i32>) -> Res {
        // 1.0.12 (nvml.rs): no check while the GPU is in its NVML quiet period after a fault (no power / clock readings: a window
        // there would judge a step on no power at all, and its offset sets are deferred): wait for its end and the resume hooks
        if let Err(why) = self.wait_nvml(c) {
            lg(format!("{} {why}: no verdict on {}", c.label(), signed(at)));
            return Res { infra: true, reason: format!("{why}: no verdict on {}", signed(at)), ..Default::default() };
        }
        let faults = crate::nvml::quiet::faults(&c.r.bus);
        let mut r = if c.mining.load(Ordering::Relaxed) { self.check_mining(c, at, secs, reference) } else { self.check_child(c, at, secs, refs) };
        // a fault of this GPU during the check that the check itself did not see (its NVML readings stopped): no verdict
        if r.ok && crate::nvml::quiet::faults(&c.r.bus) != faults {
            r.ok = false;
            r.infra = true;
            r.reason = format!("a fault of this GPU during the check (its NVML calls paused): no verdict on {}", signed(at));
            lg(format!("{} {}", c.label(), r.reason));
        }
        r
    }

    /// 1.0.12: waits while the GPU's NVML calls pause after a fault (nvml.rs quiet period) and its clocks / offset are set again.
    fn wait_nvml(&self, c: &Card) -> Result<(), String> {
        if !crate::nvml::quiet::settling(&c.r.bus) {
            return Ok(());
        }
        let t = Instant::now();
        let max = crate::nvml::quiet::QUIET_MAX + Duration::from_secs(15);
        lg(format!("{} waits for the end of its NVML quiet period after a fault (no power readings now)", c.label()));
        while crate::nvml::quiet::settling(&c.r.bus) {
            if self.stopped() {
                return Err("interrupted".into());
            }
            if t.elapsed() > max {
                return Err(format!("the NVML calls of this GPU did not resume within {} s of its fault", max.as_secs()));
            }
            // a call ends the quiet period when it is due (the fresh handle, the resume hooks)
            let _ = self.nv.temp_c(c.nv);
            std::thread::sleep(Duration::from_millis(250));
        }
        Ok(())
    }

    /// 1.0.10: a check on the card's own mining load (uvmine::window): it mines and counts all the time.
    fn check_mining(&self, c: &Card, at: i32, secs: u32, reference: Option<i32>) -> Res {
        let mut res = Res { mining: true, reference, ..Default::default() };
        if self.stopped() {
            res.infra = true;
            res.reason = "interrupted".into();
            return res;
        }
        let mut p = CardProbe { l: self, c, t0: Instant::now() };
        // 1.0.10 (live test, Shahtyor GPU1): after a fault the worker re-opens its CUDA context; its errors and its stop are not a
        // verdict on the next offset. Wait (at the offset the fault left: the last good one) until it mines cleanly again.
        if c.recover.load(Ordering::Relaxed) {
            match uvmine::wait_healthy(&mut p, RECOVER_CALM, RECOVER_MAX, 0.5) {
                Ok(w) => {
                    if w > RECOVER_CALM + 1.0 {
                        lg(format!("{} mines cleanly again {:.0} s after the fault: the tuning goes on", c.label(), w));
                    }
                    c.recover.store(false, Ordering::Relaxed);
                    c.fault_t.store(0, Ordering::Relaxed);
                    self.db.clear_trying(&c.uuid);
                }
                Err(e) => {
                    // the crash marker of the faulting offset stays: a restart resumes 2 steps below IT, not below this one
                    res.infra = true;
                    res.reason = format!("{e}: no verdict on {}", signed(at));
                    lg(format!("{} {}: the tuning stops here (the miner's fault handling takes over)", c.label(), res.reason));
                    return res;
                }
            }
        }
        if at != c.base.load(Ordering::Relaxed) {
            self.mark_trying(c, at);
        }
        let _ = self.nv.take_xids(c.nv);
        let o = uvmine::window(&mut p, at, reference, secs as f64, &uvmine::Cfg { noise_w: NOISE_W, ..Default::default() });
        if o.fault || o.bad > 0 {
            c.recover.store(true, Ordering::Relaxed);
            c.rec_since.store(uv::now(), Ordering::Relaxed);
        }
        // a wrong result counts too: on Shahtyor GPU1 the Xid 31 of +300 showed first as a wrong result, then killed the worker
        if !o.ok && !o.infra {
            // kept until the card mines cleanly again: a process restart (sticky CUDA error) finds the offset that faulted
            c.fault_at.store(at, Ordering::Relaxed);
            c.fault_t.store(uv::now(), Ordering::Relaxed);
            if o.hang {
                // a hung kernel does not come back by itself: the crash marker of `at` stays and the miner restarts (the
                // restart resumes 2 steps below it, not above the last good one)
                // 1.0.11 (iso.rs): with one engine process per GPU only this GPU's process is stopped (its worker starts a new
                // one); one process: the miner restarts
                if super::iso::enabled() && super::iso::kill(c.r.dev) > 0 {
                    lg(format!("{} step {}: {}: its engine process is stopped and started again (only this GPU)", c.label(), signed(at), o.reason));
                } else {
                    lg(format!("{} step {}: {}: the miner restarts to free the GPU", c.label(), signed(at), o.reason));
                    super::request_restart(&format!("GPU{} hung during an auto-uv step at {}", c.r.dev, signed(at)));
                }
            }
        } else {
            self.db.clear_trying(&c.uuid);
        }
        res.ok = o.ok;
        res.fault = o.fault;
        res.infra = o.infra;
        res.no_load = o.no_load;
        res.reason = o.reason;
        res.ghs = o.mas;
        res.w = o.w;
        res.w_sd = o.w_sd;
        res.w_n = o.w_n;
        res.clk = o.clk;
        res.clk_med = o.clk_med;
        res.t = o.t;
        res.pair = o.pair;
        res.checked = o.checked;
        res.secs = o.secs;
        res.refs = "mining".into();
        test_hooks(at, &mut res);
        res
    }

    /// The crash marker of a check, with (1.0.10) the core lock it ran at: a marker of another lock is kept for that lock.
    fn mark_trying(&self, c: &Card, at: i32) {
        self.db.mark_trying_tag(&c.uuid, at, c.r.core_lock.map(|l| ("lock", json!(l))));
    }

    /// 1.0.10: does the card's worker mine now (its evaluated attempts grow)? Waits up to `max`.
    fn wait_load(&self, c: &Card, max: Duration) -> bool {
        let t = Instant::now();
        let mut last = c.r.attempts.load(Ordering::Relaxed);
        let mut grew = 0;
        while t.elapsed() < max && !self.stopped() {
            std::thread::sleep(Duration::from_millis(500));
            let a = c.r.attempts.load(Ordering::Relaxed);
            if a > last && c.r.alive.load(Ordering::Relaxed) && !c.r.pause.load(Ordering::Relaxed) {
                grew += 1;
                if grew >= 3 {
                    return true;
                }
            }
            last = a;
        }
        false
    }

    fn check_child(&self, c: &Card, at: i32, secs: u32, refs: &str) -> Res {
        let mut res = Res::default();
        if self.stopped() {
            res.infra = true;
            res.reason = "interrupted".into();
            return res;
        }
        if let Err(e) = offset::set(&c.r.bus, at) {
            res.infra = true;
            res.reason = format!("cannot set offset {}: {e:#}", signed(at));
            return res;
        }
        c.cur.store(at, Ordering::Relaxed);
        if at != c.base.load(Ordering::Relaxed) {
            self.mark_trying(c, at);
        }
        let _ = self.nv.take_xids(c.nv);
        let proc_exe = std::path::Path::new("/proc/self/exe");
        let exe = if cfg!(target_os = "linux") && proc_exe.exists() { proc_exe.to_path_buf() } else { std::env::current_exe().unwrap_or_else(|_| "ffgn".into()) };
        let mut args = vec!["--neb-uv-check".to_string(), "--devices".into(), c.r.dev.to_string(), "--uv-secs".into(), secs.to_string()];
        if !refs.is_empty() {
            args.push("--uv-ref".into());
            args.push(refs.into());
        }
        child_gate(&self.stop, &c.label());
        let mut child = match Command::new(exe).args(&args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn() {
            Ok(ch) => ch,
            Err(e) => {
                res.infra = true;
                res.reason = format!("cannot start the check process ({e})");
                self.db.clear_trying(&c.uuid);
                return res;
            }
        };
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        for r in [Box::new(child.stdout.take().unwrap()) as Box<dyn std::io::Read + Send>, Box::new(child.stderr.take().unwrap())] {
            let tx = tx.clone();
            std::thread::spawn(move || {
                for l in BufReader::new(r).lines().map_while(|l| l.ok()) {
                    let _ = tx.send(l);
                }
            });
        }
        drop(tx);
        let t0 = Instant::now();
        let (mut load_at, mut result, mut tail): (Option<Instant>, Option<String>, Vec<String>) = (None, None, Vec::new());
        let mut samples: Vec<(f64, u32, u32)> = Vec::new();
        let mut last = Instant::now();
        let limit = Duration::from_secs(secs as u64 + 90);
        let settle = SETTLE_SECS.min(secs as f64 * 0.4).max(4.0);
        let take = |l: String, load_at: &mut Option<Instant>, result: &mut Option<String>, tail: &mut Vec<String>| {
            if l.starts_with("NEB_LOAD") {
                *load_at = Some(Instant::now());
            } else if l.starts_with("NEB_CHECK") {
                *result = Some(l);
            } else if !l.trim().is_empty() {
                tail.push(l);
                if tail.len() > 3 {
                    tail.remove(0);
                }
            }
        };
        loop {
            if !matches!(child.try_wait(), Ok(None)) {
                break;
            }
            while let Ok(l) = rx.try_recv() {
                take(l, &mut load_at, &mut result, &mut tail);
            }
            if self.stopped() {
                let _ = child.kill();
                let _ = child.wait();
                self.db.clear_trying(&c.uuid);
                res.infra = true;
                res.reason = "interrupted".into();
                return res;
            }
            if let Some(x) = self.nv.take_xids(c.nv).first() {
                let _ = child.kill();
                let _ = child.wait();
                res.fault = true;
                res.reason = format!("Xid {x} reported by the driver");
                return res;
            }
            if t0.elapsed() > limit {
                let _ = child.kill();
                let _ = child.wait();
                res.fault = true;
                res.reason = "timeout (GPU hang?)".into();
                return res;
            }
            if result.is_none() && load_at.is_some_and(|t| t.elapsed().as_secs_f64() > settle) && last.elapsed() >= Duration::from_millis(500) {
                last = Instant::now();
                if let Some(p) = self.nv.power_w(c.nv) {
                    samples.push((p, self.nv.clock_mhz(c.nv).unwrap_or(0), self.nv.temp_c(c.nv).unwrap_or(0)));
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        while let Ok(l) = rx.recv_timeout(Duration::from_millis(500)) {
            take(l, &mut load_at, &mut result, &mut tail);
        }
        if let Some(x) = self.nv.take_xids(c.nv).first() {
            res.fault = true;
            res.reason = format!("Xid {x} reported by the driver");
            return res;
        }
        if result.is_none() && self.stopped() {
            self.db.clear_trying(&c.uuid);
            res.infra = true;
            res.reason = "interrupted".into();
            return res;
        }
        self.db.clear_trying(&c.uuid);
        if !samples.is_empty() {
            let n = samples.len() as f64;
            let mean = samples.iter().map(|s| s.0).sum::<f64>() / n;
            res.w = Some(mean);
            res.w_n = samples.len();
            res.w_sd = (samples.len() >= 2).then(|| (samples.iter().map(|s| (s.0 - mean).powi(2)).sum::<f64>() / (n - 1.0)).sqrt());
            res.clk = samples.last().map(|s| s.1).filter(|&x| x > 0);
            let mut ck: Vec<u32> = samples.iter().map(|s| s.1).filter(|&x| x > 0).collect();
            ck.sort_unstable();
            res.clk_med = ck.get(ck.len() / 2).copied();
            res.t = samples.last().map(|s| s.2 as i32).filter(|&x| x > 0);
        }
        res.secs = t0.elapsed().as_secs_f64();
        match result {
            Some(l) if l.starts_with("NEB_CHECK ok ") => {
                res.ok = true;
                let mut it = l["NEB_CHECK ok ".len()..].split_whitespace();
                while let (Some(k), Some(v)) = (it.next(), it.next()) {
                    match k {
                        "launches" => res.launches = v.parse().unwrap_or(0),
                        "ghs" => res.ghs = v.parse().unwrap_or(0.0),
                        "counts" => res.refs = v.to_string(),
                        _ => {}
                    }
                }
            }
            Some(l) => res.reason = l.trim_start_matches("NEB_CHECK FAIL ").to_string(),
            None => {
                res.fault = true;
                res.reason = format!("the check process ended without a result{}", tail.last().map_or(String::new(), |t| format!(" (last line: {t})")));
            }
        }
        test_hooks(at, &mut res);
        res
    }

    fn line(&self, c: &Card, what: &str, at: i32, r: &Res) -> String {
        let w = r.w.map_or("? W".into(), |w| format!("{w:.1} W{}", r.w_sd.map_or(String::new(), |s| format!(" (sd {s:.1} W, {} samples)", r.w_n))));
        if r.ok && r.mining {
            let pair = match (r.pair, r.reference) {
                (Some(p), Some(a)) => format!(", paired with {} on the same load: {:.1} -> {:.1} W ({:+.1} W, se {:.2}, {} cycles)", signed(a), p.wa, p.wb, p.wb - p.wa, p.se, p.n),
                (None, Some(a)) => format!(", not paired with {} (the load changed in every cycle)", signed(a)),
                _ => String::new(),
            };
            format!(
                "{} {what} {}: ok while mining ({:.3} MA/s counted, {} results CPU-checked, bad 0, no Xid, {:.0} s), {w}{pair}, {} MHz, {} C",
                c.label(),
                signed(at),
                r.ghs,
                r.checked,
                r.secs,
                r.clk.unwrap_or(0),
                r.t.unwrap_or(0)
            )
        } else if r.ok {
            format!("{} {what} {}: ok ({} launches, {:.3} MA/s in the check), {w}, {} MHz, {} C", c.label(), signed(at), r.launches, r.ghs, r.clk.unwrap_or(0), r.t.unwrap_or(0))
        } else {
            format!("{} {what} {}: FAILED ({})", c.label(), signed(at), r.reason)
        }
    }

    fn entry(&self, c: &Card, clk: u32) -> Entry {
        Entry {
            name: c.r.name.clone(),
            bus: c.r.bus.clone(),
            driver: self.driver.clone(),
            kernel: kernel_id(),
            core_lock: c.r.core_lock.unwrap_or(0),
            clk,
            orig: c.orig,
            step: self.p.step,
            ..Default::default()
        }
    }

    /// Base check at `base` (falling 30 MHz at a time to 0 like Pearl when it fails); Some((base, result)) or None (nothing passes).
    fn base_check(&self, c: &Card, base: i32) -> Option<(i32, Res)> {
        c.no_verdict.store(false, Ordering::Relaxed);
        let r = self.check(c, base, self.base_secs(c), "");
        lg(self.line(c, "base", base, &r));
        if r.ok {
            return Some((base, r));
        }
        if r.infra {
            c.no_verdict.store(true, Ordering::Relaxed);
            return None;
        }
        // 1.7.8 (1.7.7 plan, point 4): a failing start is checked once more before the search goes lower (one failure can be a
        // transient one); not after a driver fault
        if !r.fault && !self.stopped() {
            lg(format!("{} the start {} failed once: checking it again before going lower", c.label(), signed(base)));
            let r = self.check(c, base, self.base_secs(c), "");
            lg(self.line(c, "base (again)", base, &r));
            if r.ok {
                return Some((base, r));
            }
            if r.infra {
                c.no_verdict.store(true, Ordering::Relaxed);
                return None;
            }
        }
        for b in climb::fall_steps(base, c.lo.max(0)) {
            let r = self.check(c, b, self.base_secs(c), "");
            lg(self.line(c, "base", b, &r));
            if r.ok {
                return Some((b, r));
            }
            if r.infra {
                c.no_verdict.store(true, Ordering::Relaxed);
                return None;
            }
        }
        None
    }

    /// the base check: 1.0.10 on the mining load MINING_BASE s (the card is warm, it mines), else the warm-up of a check process
    fn base_secs(&self, c: &Card) -> u32 {
        if c.mining.load(Ordering::Relaxed) { MINING_BASE.min(self.p.warmup) } else { self.p.warmup }
    }

    fn tune(&self, c: &Card, base: i32, br: Res, cap: i32, crash: Option<profiles::Crash>) {
        let w = br.w;
        self.climb_from(c, base, w, base, br, cap, crash, 0)
    }

    /// The climb from `at` (its check `ar`) for the profile of base `base` (its power `base_w`): a new tuning starts at the base
    /// (`at` = `base`); 1.0.4 goes on from the offset of an earlier version's profile (`at` above it, `edge_in` its failed
    /// offset).  1.0.4 rule (the owner, 18.11.2026): a passed step is taken while its power is lower than the step before by more
    /// than the noise (`Judge`: a step that does not beat the best is not taken, PATIENCE in a row or a clear rise stop); the
    /// result is the highest offset that passed and still lowered the power (no % thresholds).  Unchanged: the clock against the lock, the
    /// bit-exact check, a failure -> one fine step below the last good, --uv-max, the confirmation.
    #[allow(clippy::too_many_arguments)]
    fn climb_from(&self, c: &Card, base: i32, base_w: Option<f64>, at: i32, ar: Res, mut cap: i32, crash: Option<profiles::Crash>, edge_in: i32) {
        let step = self.p.step;
        let mut from = at;
        if let Some(cr) = crash {
            if cr.failed() {
                cap = cap.min(cr.at - step);
            }
            if let Some(g) = cr.good.filter(|&g| g > at && g <= cap) {
                from = g;
                lg(format!("{} steps up to {} passed before the crash{}: not climbing them again", c.label(), signed(g), if cr.failed() { format!(" at {}", signed(cr.at)) } else { " of another GPU".into() }));
            } else if cr.failed() {
                lg(format!("{} the previous run crashed at {}: tuning up to {}", c.label(), signed(cr.at), signed(cap)));
            }
        }
        // 1.0.12: no climbing while the rig is cautious after an Xid 79: the card mines at `at` (saved below)
        if climb_cap(cap, at, rig_cautious(uv::now())) < cap {
            lg(format!("{} the rig is cautious after an Xid 79: no climb above {} for now (it goes on at a start after the caution)", c.label(), signed(at)));
            cap = at;
        }
        let refs = ar.refs.clone();
        let mining = c.mining.load(Ordering::Relaxed);
        let coarse = if mining { self.p.coarse_mining } else { self.p.coarse };
        let pl = self.nv.power_limit_w(c.nv);
        let origin = Meas { at, w: ar.w, ghs: ar.ghs, t: ar.t };
        let mut passed: Vec<Meas> = vec![origin];
        let mut power_stop = false;
        let mut e = self.entry(c, ar.clk.unwrap_or(0));
        e.base = base;
        e.w_base = base_w.unwrap_or(0.0);
        e.edge = edge_in;
        if from + step > cap {
            lg(format!("{} no room above {} (cap {}): mining at {}", c.label(), signed(at), signed(cap), signed(at)));
            e.offset = at;
            e.w = ar.w.unwrap_or(0.0);
            e.ghs = ar.ghs;
            e.refs = refs.clone();
            e.tested_at = uv::now();
            e.verified_at = e.tested_at;
            self.save(c, &e);
            return;
        }
        if mining {
            lg(format!(
                "{} tuning {}..{} WHILE MINING (+{step} MHz steps, coarse +{coarse}, each step up to {} s on the mining load with its power paired against the best offset, {} s confirmation at the result; a step is taken while it lowers the power by more than {NOISE_W} W)",
                c.label(),
                signed(from),
                signed(cap),
                self.p.step_secs,
                self.p.confirm
            ));
        } else {
            lg(format!(
                "{} tuning {}..{} (+{step} MHz steps, coarse +{coarse}, {} s each, {} s confirmation; a step is taken while it lowers the power by more than {NOISE_W} W); this GPU mines after it",
                c.label(),
                signed(from),
                signed(cap),
                self.p.step_secs,
                self.p.confirm
            ));
        }
        let abort = climb::StepOut { abort: true, ..Default::default() };
        let mut judge = Judge::new(origin);
        let mut over_budget = false;
        let mut no_load = false;
        let s = climb::climb_ex(from, ar.w, ar.t, cap, step, coarse, false, |cand, good, _mhz, _refine| {
            // 1.0.5: the time of this GPU / of the run is used up: mine at the last taken step (saved), climb on next time
            if uv::now() >= c.deadline.load(Ordering::Relaxed).min(self.rig_deadline) {
                over_budget = true;
                lg(format!("{} the tuning time of this run is used up ({} min per GPU, {} min per run): it mines at the last taken step {}, the climb goes on at the next start", c.label(), CARD_BUDGET_SECS / 60, RIG_BUDGET_SECS / 60, signed(judge.best.at)));
                return abort;
            }
            // 1.0.12: an Xid 79 on the rig during the climb: it stops at the last taken step
            if rig_cautious(uv::now()) {
                over_budget = true;
                lg(format!("{} an Xid 79 on the rig: the climb stops, it mines at the last taken step {}", c.label(), signed(judge.best.at)));
                return abort;
            }
            self.db.set_good(&c.uuid, Some(good));
            // 1.0.10: on the mining load the step is paired with the best offset so far (same loads, same minutes)
            let mut r = self.check_ref(c, cand, self.p.step_secs, &refs, Some(judge.best.at));
            if r.infra {
                no_load |= r.no_load;
                return abort;
            }
            lg(self.line(c, "step", cand, &r));
            if r.ok {
                let best = judge.best;
                let mut now = Meas { at: cand, w: r.w, ghs: r.ghs, t: r.t };
                let paired = apply_pair(&mut judge, &mut now, &r);
                if !paired && step_verdict(&best, &now, pl) != Verdict::Lower && warmed(&best, &now) {
                    lg(format!(
                        "{} step {}: {} against {} at {} while the card warmed {} -> {} C: measured once more",
                        c.label(),
                        signed(cand),
                        fw(now.w),
                        fw(best.w),
                        signed(best.at),
                        best.t.unwrap_or(0),
                        now.t.unwrap_or(0)
                    ));
                    r = self.check(c, cand, self.p.step_secs, &refs);
                    if r.infra {
                        return abort;
                    }
                    lg(self.line(c, "step (again)", cand, &r));
                    now = Meas { at: cand, w: r.w, ghs: r.ghs, t: r.t };
                }
                // 1.0.5: a step that did not clearly lower the power (Flat) is measured once more against the best, in the
                // order A-B-A-B (best, step, best, step): a drift of the card (warming, the clock settling) shifts both alike
                if r.ok && !paired && step_verdict(&judge.best, &now, pl) == Verdict::Flat {
                    let a2 = self.check(c, judge.best.at, self.p.step_secs, &refs);
                    if a2.infra {
                        return abort;
                    }
                    lg(self.line(c, "A-B-A: best again", judge.best.at, &a2));
                    let b2 = self.check(c, cand, self.p.step_secs, &refs);
                    if b2.infra {
                        return abort;
                    }
                    lg(self.line(c, "A-B-A: step again", cand, &b2));
                    if !b2.ok {
                        r = b2;
                    } else if let (true, Some(wa1), Some(wa2), Some(wb1), Some(wb2)) = (a2.ok, judge.best.w, a2.w, now.w, b2.w) {
                        let (wa, wb) = ((wa1 + wa2) / 2.0, (wb1 + wb2) / 2.0);
                        lg(format!(
                            "{} step {}: A-B-A {} {:.1}/{:.1} W -> {wa:.1} W, step {:.1}/{:.1} W -> {wb:.1} W ({:+.1} W)",
                            c.label(),
                            signed(cand),
                            signed(judge.best.at),
                            wa1,
                            wa2,
                            wb1,
                            wb2,
                            wb - wa
                        ));
                        judge.best.w = Some(wa);
                        judge.best.t = a2.t.or(judge.best.t);
                        now = Meas { at: cand, w: Some(wb), ghs: (now.ghs + b2.ghs) / 2.0, t: b2.t.or(now.t) };
                    }
                }
                let best = judge.best;
                if r.ok {
                    let d = match (best.w, now.w) {
                        (Some(a), Some(b)) => format!("{} at {} -> {}, {:+.1} W", fw(Some(a)), signed(best.at), fw(Some(b)), b - a),
                        _ => "power unknown".into(),
                    };
                    // both at the power limit the rate decides (step_verdict): say so, "the power falls (+0.1 W)" read as a bug
                    let at_limit = matches!((best.w, now.w), (Some(a), Some(b)) if at_pl(a, pl) && at_pl(b, pl));
                    let (g0, g1) = (best.ghs, now.ghs);
                    match judge.judge(now, pl) {
                        Decision::Take => {
                            if at_limit {
                                lg(format!(
                                    "{} step {}: at the power limit ({d}) the rate rises ({g0:.3} -> {g1:.3} MA/s on the same load): taken, saved (tuning in progress)",
                                    c.label(),
                                    signed(cand)
                                ));
                            } else {
                                lg(format!("{} step {}: the power falls ({d}): taken, saved (tuning in progress)", c.label(), signed(cand)));
                            }
                            passed.push(now);
                            // 1.0.5: saved at once; the final result replaces it
                            let mut ip = e.clone();
                            ip.kernel = climbing_id();
                            ip.offset = cand;
                            ip.w = now.w.unwrap_or(0.0);
                            ip.ghs = now.ghs;
                            ip.refs = if r.refs.is_empty() || r.refs == "-" { refs.clone() } else { r.refs.clone() };
                            ip.tested_at = uv::now();
                            self.save(c, &ip);
                        }
                        Decision::Hold(k) => {
                            lg(format!(
                                "{} step {}: not lower than the best ({d}; noise {NOISE_W} W): not taken, the climb goes on ({k} of {PATIENCE} in a row stop it)",
                                c.label(),
                                signed(cand)
                            ));
                        }
                        Decision::Stop(v) => {
                            power_stop = true;
                            lg(format!(
                                "{} step {}: {} ({d}; noise {NOISE_W} W): not taken, the climb stops at {}",
                                c.label(),
                                signed(cand),
                                if v == Verdict::Rose { format!("the power rose by more than {RISE_W} W") } else { format!("the power stopped falling ({PATIENCE} steps in a row did not lower it)") },
                                signed(judge.best.at)
                            ));
                            // a "fault" ends the climb without refining; the result is judge.best
                            return climb::StepOut { ok: false, fault: true, w: r.w, t: r.t, abort: false };
                        }
                    }
                }
            }
            if r.clk_over.is_some() {
                power_stop = true; // the climb stops at the step before (kept, no step back below it)
            }
            climb::StepOut { ok: r.ok, fault: r.fault, w: r.w, t: r.t, abort: false }
        });
        self.db.set_good(&c.uuid, None);
        if s.aborted {
            // 1.0.10: no mining load (no work) is not a verdict: the card mines at the last taken step, the climb goes on next time
            if no_load && !self.stopped() {
                lg(format!("{} no mining load for a long time: it mines at the last taken step {}, the climb goes on at the next start", c.label(), signed(judge.best.at)));
            }
            // 1.0.10: a card that did not come back after a fault stays at the last good step (the fault left it there), not at the start
            let lost = c.recover.load(Ordering::Relaxed);
            let back = if (over_budget || no_load || lost) && !self.stopped() { judge.best.at } else { at };
            let _ = offset::set(&c.r.bus, back);
            c.cur.store(back, Ordering::Relaxed);
            self.leave_unheld(c);
            return;
        }
        let mut fin = if s.failed && !self.p.keep_last && !power_stop { (s.good - step).max(at) } else { s.good };
        // 1.0.4: never above the last step that lowered the power (a step within the noise is not taken)
        if judge.best.at < fin {
            lg(format!("{} the last step that lowered the power: {} ({})", c.label(), signed(judge.best.at), fw(judge.best.w)));
            fin = judge.best.at.max(at);
        }
        let (mut conf_w, mut conf_ghs) = (ar.w, ar.ghs);
        let mut conf_base: Option<f64> = None;
        while fin > at {
            // 1.0.10: once, at the result; on the mining load paired with the base (the saving on the same loads goes in the profile)
            let r = self.check_ref(c, fin, self.p.confirm, &refs, (base < fin).then_some(base));
            lg(self.line(c, "confirmation", fin, &r));
            if r.infra {
                self.leave_unheld(c);
                return;
            }
            if r.ok {
                conf_w = r.pair.map(|p| p.wb).or(r.w);
                conf_base = r.pair.map(|p| p.wa);
                conf_ghs = r.ghs;
                break;
            }
            fin = (fin - step).max(at);
        }
        if fin == at {
            (conf_w, conf_ghs) = (ar.w, ar.ghs);
            conf_base = None;
        }
        let _ = offset::set(&c.r.bus, fin);
        c.cur.store(fin, Ordering::Relaxed);
        e.offset = fin;
        if s.failed && !power_stop {
            e.edge = s.fail_at;
        }
        e.w = conf_w.unwrap_or(0.0);
        if let Some(b) = conf_base {
            e.w_base = b;
        }
        e.ghs = conf_ghs;
        e.refs = refs;
        e.tested_at = uv::now();
        e.verified_at = e.tested_at;
        self.save(c, &e);
        let steps: Vec<String> = passed.iter().map(|m| format!("{} {}", signed(m.at), fw(m.w))).collect();
        let power = match conf_base {
            Some(b) => format!("mining power at the base {} {} -> {} at {} (paired on the same load, {:+.1} W)", signed(base), fw(Some(b)), fw(conf_w), signed(fin), conf_w.unwrap_or(b) - b),
            None => format!("check power {} -> {}", fw(ar.w), fw(conf_w)),
        };
        lg(format!(
            "{} RESULT: offset {} -> {}, {power} (steps taken: {}){}, saved",
            c.label(),
            signed(at),
            signed(fin),
            steps.join(", "),
            if at != base && conf_base.is_none() { format!("; the profile's base {} {}", signed(base), fw(base_w)) } else { String::new() }
        ));
    }

    fn save(&self, c: &Card, e: &Entry) {
        let k = pkey(e);
        *c.key.lock().unwrap() = k.clone();
        *c.refs.lock().unwrap() = e.refs.clone();
        c.base.store(e.base, Ordering::Relaxed);
        // 1.0.12: the crash history and the stable offset of the profile stay (a climb / resume writes a new entry)
        let mut e = e.clone();
        if let Some(old) = self.db.get::<Entry>(&c.uuid, &k) {
            merge_history(&mut e, &old);
        }
        self.db.save(&c.uuid, "", &k, &e);
    }

    /// 1.0.12: a crash / failure of this card at `at` into the profile `k` (its crash history)
    fn note_fail(&self, c: &Card, k: &str, at: i32) {
        if k.is_empty() {
            return;
        }
        if let Some(mut e) = self.db.get::<Entry>(&c.uuid, k) {
            note_crash(&mut e, uv::now(), at);
            self.db.save(&c.uuid, "", k, &e);
        }
    }

    /// 1.0.12 (`tick`): the card mined at its offset STABLE_SECS without a fault: saved as its stable offset (the resume margin)
    fn note_stable(&self, c: &Card, t: u64) {
        let cur = c.cur.load(Ordering::Relaxed);
        if c.stable_cur.swap(cur, Ordering::Relaxed) != cur || c.stable_since.load(Ordering::Relaxed) == 0 {
            c.stable_since.store(t, Ordering::Relaxed);
            c.stable_saved.store(false, Ordering::Relaxed);
            return;
        }
        if t.saturating_sub(c.stable_since.load(Ordering::Relaxed)) < STABLE_SECS || c.stable_saved.swap(true, Ordering::Relaxed) {
            return;
        }
        let k = c.key.lock().unwrap().clone();
        if let Some(mut e) = self.db.get::<Entry>(&c.uuid, &k).filter(|e| e.offset == cur) {
            e.stable = Some(cur);
            e.stable_at = t;
            self.db.save(&c.uuid, "", &k, &e);
            lg(format!("{} mined {} h at {} without a fault: saved as its stable offset (where it resumes after a crash)", c.label(), STABLE_SECS / 3600, signed(cur)));
        }
    }

    /// One card before mining: a crash marker, a saved profile (quick check) or a new tuning.
    fn prepare_card(&self, c: &Card) {
        let start = self.p.min.get(c_pos(self, c)).unwrap_or(c.orig).clamp(c.lo, c.cap);
        let marker = self.db.peek_trying(&c.uuid);
        let mut crash = self.db.take_trying(&c.uuid);
        if let Some(k) = crash {
            // 1.7.8: a CUDA fault on one card ends the whole process; only the card with the Xid has a failed offset
            if let Some(by) = crate::faults::bystander(&self.db.dir, &c.uuid) {
                lg(format!("{} the previous run died while this GPU checked offset {}, but the fault was on {by}: this GPU did not fail, its offset is not counted as failed{}", c.label(), signed(k.at), k.good.map_or(String::new(), |g| format!("; the steps up to {} that passed are kept", signed(g)))));
                crash = k.bystander();
            }
        }
        // 1.0.10: the state is per core lock: a failure found at another lock (--lock-cclock changed since) is kept for that lock
        // (it resumes 2 steps below it when that lock comes back) and does not count at this one; a failure kept earlier for this
        // lock counts now
        let mlock = marker.as_ref().and_then(|v| v["lock"].as_u64()).map(|x| x as u32);
        match lock_split(crash, mlock, c.r.core_lock) {
            LockSplit::Here(k) => crash = k,
            LockSplit::Other(m, k) => {
                pending_put(&self.db.dir, &c.uuid, m, k);
                lg(format!("{} the previous run failed at {} with core lock {m} MHz; this run has lock {} MHz: kept for lock {m}, not counted here", c.label(), signed(k.at), c.r.core_lock.unwrap_or(0)));
                crash = None;
            }
        }
        if crash.is_none() {
            if let Some(k) = c.r.core_lock.and_then(|l| pending_take(&self.db.dir, &c.uuid, l)) {
                lg(format!("{} a failure at {} with this core lock {} MHz from an earlier run: it counts now", c.label(), signed(k.at), c.r.core_lock.unwrap_or(0)));
                crash = Some(k);
            }
        }
        // a crash while mining at a tuned offset (faults.rs): that offset counts as failed
        let mut mined_crash: Option<i32> = None;
        let mut mined_t: Option<u64> = None;
        if let Some(mut r) = crate::faults::get(&self.db.dir, &c.uuid) {
            if r.n >= 1 && r.offset > start {
                mined_crash = Some(r.offset);
                mined_t = (r.last > 0).then_some(r.last);
                lg(format!("{} crashed while mining at offset {} in the previous run (Xid {}): that offset counts as failed", c.label(), signed(r.offset), r.xid));
                r.n = 0;
                crate::faults::put(&self.db.dir, &c.uuid, &r);
                crash = Some(crash.map_or(profiles::Crash { at: r.offset, good: None }, |x| profiles::Crash { at: x.at.min(r.offset), good: x.good }));
            }
        }
        c.failed_start.store(crash.is_some_and(|k| k.failed()), Ordering::Relaxed);
        let lock = c.r.core_lock;
        let profs: Vec<(String, Entry)> = self.db.list::<Entry>(&c.uuid, "").into_iter().filter(|(_, e)| e.driver == self.driver).collect();
        if self.p.reset {
            let keys: Vec<String> = profs.iter().filter(|(_, e)| lock.is_some_and(|l| e.core_lock == l) || lock.is_none()).map(|(k, _)| k.clone()).collect();
            if !keys.is_empty() && self.db.forget(&c.uuid, &keys) {
                lg(format!("{} --uv-reset: profile(s) {keys:?} forgotten", c.label()));
            }
        }
        let profs: Vec<(String, Entry)> = self.db.list::<Entry>(&c.uuid, "").into_iter().filter(|(_, e)| e.driver == self.driver).collect();
        // the profile of this lock; without a lock the base check measures the clock under load first
        let mut base_res: Option<(i32, Res)> = None;
        let mut sel = match lock {
            Some(l) => profs.iter().find(|(_, e)| e.core_lock == l).cloned(),
            None => {
                let Some(b) = self.base_check(c, start) else { return self.give_up(c, start) };
                let clk = b.1.clk.unwrap_or(0);
                base_res = Some(b);
                profs.iter().filter(|(_, e)| e.core_lock == 0 && (e.clk as i64 - clk as i64).abs() <= 45).max_by_key(|(_, e)| e.verified_at).cloned()
            }
        };
        let mut drop_sel = false;
        if crash.is_none() {
            if let Some((k, e)) = &sel {
                if e.disabled_until > uv::now() && !pause_honored(e) {
                    drop_sel = true;
                    lg(format!(
                        "{} profile {k}: the 24 h pause at {} written by {} is not honored (before 1.0.12 a base check without a verdict, e.g. this GPU's worker stopped by another GPU's fault, also paused it): the base is checked again",
                        c.label(),
                        signed(e.base),
                        if e.kernel.is_empty() { "an earlier version" } else { e.kernel.as_str() }
                    ));
                } else if e.disabled_until > uv::now() {
                    lg(format!("{} profile {k}: auto-uv paused until the next 24 h (the base failed earlier); mining at {}", c.label(), signed(e.base)));
                    let _ = offset::set(&c.r.bus, e.base);
                    c.cur.store(e.base, Ordering::Relaxed);
                    return;
                }
            }
        }
        if drop_sel {
            // the pause entry holds no tuning (offset = base = the fallback): a new tuning from the start offset
            sel = None;
        }
        // 1.0.3 (the owner, 18.11.2026: "not from zero, 2 steps lower"): a saved result or a known failed offset -> a short check at
        // (failed - 2 steps), never above the last confirmed offset, and the card mines; another failure -> 2 steps lower, down to the base
        let prof = sel.as_ref().map(|(_, e)| SavedProf {
            offset: e.offset,
            base: e.base,
            same_kernel: e.kernel == kernel_id() && !e.refs.is_empty(),
            lowered: profile_lowered(e),
        });
        if let (Some((k, e)), Some(p)) = (&sel, prof) {
            if !p.lowered {
                lg(format!("{} profile {k}: its offset {} did not lower the check power ({:.1} W -> {:.1} W): not used, tuned again", c.label(), signed(e.offset), e.w_base, e.w));
            }
        }
        // 1.0.12: the crash of the previous run goes into the profile's crash history (the resume margins of the next starts)
        if let (Some(at), Some((k, e))) = (mined_crash, sel.as_mut()) {
            note_crash(e, mined_t.unwrap_or_else(uv::now), at);
            self.db.save(&c.uuid, "", k, e);
        }
        let pl = plan(crash, prof, start, c.cap, self.p.step, mined_crash);
        // 1.0.12 (LAS-rig5.md): a card with recent crashes (or any card after a recent Xid 79 on the rig) does not resume at its
        // saved offset: a margin below it, then the climb back (never above the saved offset; no climb while the rig is cautious)
        let now = uv::now();
        if let Some((k, e)) = &sel {
            if let Some(m) = margin_plan(&pl, e, start, self.p.step, c.cap, now, rig_xid79().recent(now)) {
                self.db.use_key(&c.uuid, k);
                let Some(lowest) = self.margin_resume(c, k, e, &m, start) else { return };
                let b = match base_res.take() {
                    Some(b) => b,
                    None => match self.base_check(c, start) {
                        Some(b) => b,
                        None => return self.give_up(c, start),
                    },
                };
                return self.tune(c, b.0, b.1, c.cap.min(lowest - RESUME_BACK * self.p.step).min(b.0), None);
            }
        }
        match pl {
            Plan::Tune => {
                // 1.0.10: a new core lock with no profile of its own starts 2 steps below the profile of the nearest higher lock
                // (climb.rs hint_start: the edge offset grows as the lock goes down), checked there first
                if let (Some(l), None, None) = (lock, &sel, crash) {
                    if let Some((h, from)) = lock_hint(&profs, l, start, c.cap, RESUME_BACK * self.p.step.max(1), uv::now()) {
                        lg(format!("{} {}", c.label(), climb::hint_line(&from, RESUME_BACK * self.p.step.max(1), h, start)));
                        return self.start_from_hint(c, h, start, base_res.take());
                    }
                }
            }
            Plan::Continue { from } => {
                let Some((k, e)) = &sel else { unreachable!("Plan::Continue comes from a saved profile") };
                self.db.use_key(&c.uuid, k);
                lg(format!(
                    "{} profile {k}: offset {} saved by {} (base {} {:.1} W): a {RESUME} s check there, then the climb goes on upward from it with the {} rule (each step must lower the power by more than {NOISE_W} W)",
                    c.label(),
                    signed(from),
                    if e.kernel.is_empty() { "an earlier version".to_string() } else { e.kernel.clone() },
                    signed(e.base),
                    e.w_base,
                    env!("CARGO_PKG_VERSION")
                ));
                let r = self.check(c, from, RESUME, &e.refs);
                if r.infra {
                    self.leave_unheld(c);
                    return;
                }
                lg(self.line(c, "check of the saved offset", from, &r));
                let lowest = if r.ok {
                    let mut cap = c.cap;
                    if e.edge > from {
                        cap = cap.min(e.edge - self.p.step);
                    }
                    // a crash while mining that this GPU had above the saved offset (gpu-faults.json): not climbed into again
                    if let Some(x) = crate::faults::get(&self.db.dir, &c.uuid).filter(|x| x.offset > from) {
                        cap = cap.min(x.offset - self.p.step);
                        lg(format!("{} it crashed while mining at {} (Xid {}): the climb stays below it (up to {})", c.label(), signed(x.offset), x.xid, signed(cap)));
                    }
                    let mut r = r;
                    if r.refs.is_empty() || r.refs == "-" {
                        r.refs = e.refs.clone();
                    }
                    let edge = if e.edge > from { e.edge } else { 0 };
                    return self.climb_from(c, e.base.min(from), (e.w_base > 0.0).then_some(e.w_base), from, r, cap, None, edge);
                } else {
                    // the saved offset failed: 2 steps lower each time (the rule after a failure), no climb
                    let back = RESUME_BACK * self.p.step.max(1);
                    match self.resume(c, Some(e), Some(from), &format!("{RESUME_BACK} steps below the failed saved {}", signed(from)), &ladder(from - back, start, self.p.step, false), RESUME, None, start) {
                        Resumed::Mining | Resumed::Stopped => return,
                        Resumed::Exhausted(l) => l,
                    }
                };
                let b = match base_res.take() {
                    Some(b) => b,
                    None => match self.base_check(c, start) {
                        Some(b) => b,
                        None => return self.give_up(c, start),
                    },
                };
                return self.tune(c, b.0, b.1, c.cap.min(lowest - RESUME_BACK * self.p.step).min(b.0), None);
            }
            Plan::Resume { failed, why, ladder, secs } => {
                if let Some((k, _)) = &sel {
                    self.db.use_key(&c.uuid, k);
                }
                let lowest = match self.resume(c, sel.as_ref().map(|x| &x.1), failed, &why, &ladder, secs, base_res.as_ref().and_then(|b| b.1.clk), start) {
                    Resumed::Mining | Resumed::Stopped => return,
                    Resumed::Exhausted(l) => l,
                };
                // nothing above the base passed: the base (its check, falling to 0 like Pearl), no new climb
                let b = match base_res.take() {
                    Some(b) => b,
                    None => match self.base_check(c, start) {
                        Some(b) => b,
                        None => return self.give_up(c, start),
                    },
                };
                return self.tune(c, b.0, b.1, c.cap.min(lowest - RESUME_BACK * self.p.step).min(b.0), None);
            }
        }
        if let (Some((k, e)), None) = (&sel, crash) {
            if e.offset < e.base {
                lg(format!("{} profile {k} has no usable offset: a new reference at its base, then the climb", c.label()));
            }
        }
        let b = match base_res.take() {
            Some(b) => b,
            None => match self.base_check(c, start) {
                Some(b) => b,
                None => return self.give_up(c, start),
            },
        };
        self.tune(c, b.0, b.1, c.cap, crash);
    }

    /// 1.0.12: the resume at the margin `m` below the saved offset of profile `k` (`e`): a RESUME check there, then the climb back
    /// up (each step must pass its checks and lower the power, never above `m.ceiling`); while the rig is cautious after an Xid 79
    /// the card only resumes there (no climb). A failure at the margin: the owner's rule, 2 steps lower each time, no climb.
    /// None: done (mining / stopped); Some(lowest failed): nothing above the base passed.
    fn margin_resume(&self, c: &Card, k: &str, e: &Entry, m: &Margin, floor: i32) -> Option<i32> {
        let cautious = rig_cautious(uv::now());
        lg(format!(
            "{} profile {k}: {}: resuming at {} instead of {} (a {RESUME} s check), then {}",
            c.label(),
            m.why,
            signed(m.at),
            signed(m.saved),
            if cautious { "no climb (the rig is cautious after an Xid 79)".to_string() } else { format!("the climb back up to {} at most, under the usual rules", signed(m.ceiling)) }
        ));
        if cautious {
            return match self.resume(c, Some(e), None, &m.why, &ladder(m.at, floor, self.p.step, true), RESUME, None, floor) {
                Resumed::Mining | Resumed::Stopped => None,
                Resumed::Exhausted(l) => Some(l),
            };
        }
        let r = self.check(c, m.at, RESUME, &e.refs);
        if r.infra {
            self.leave_unheld(c);
            return None;
        }
        lg(self.line(c, "check of the resume margin", m.at, &r));
        if r.ok {
            let mut cap = m.ceiling;
            if e.edge > m.at {
                cap = cap.min(e.edge - self.p.step);
            }
            if let Some(x) = crate::faults::get(&self.db.dir, &c.uuid).filter(|x| x.offset > m.at && x.n > 0) {
                cap = cap.min(x.offset - self.p.step);
            }
            let mut r = r;
            if r.refs.is_empty() || r.refs == "-" {
                r.refs = e.refs.clone();
            }
            let edge = if e.edge > m.at { e.edge } else { 0 };
            self.climb_from(c, e.base.min(m.at), (e.w_base > 0.0).then_some(e.w_base), m.at, r, cap.max(m.at), None, edge);
            return None;
        }
        // failed at the margin: the owner's rule after a failure (2 steps lower each time), no climb
        self.note_fail(c, k, m.at);
        let back = RESUME_BACK * self.p.step.max(1);
        match self.resume(c, Some(e), Some(m.at), &format!("{RESUME_BACK} steps below the failed resume margin {}", signed(m.at)), &ladder(m.at - back, floor, self.p.step, false), RESUME, None, floor) {
            Resumed::Mining | Resumed::Stopped => None,
            Resumed::Exhausted(l) => Some(l),
        }
    }

    /// 1.0.10: a new lock from the hint of a higher lock: a RESUME check at `from`, then the climb upward from it; failing there,
    /// 2 steps lower each time (the rule after a failure), then the base.
    fn start_from_hint(&self, c: &Card, from: i32, start: i32, base_res: Option<(i32, Res)>) {
        let r = self.check(c, from, RESUME, "");
        if r.infra {
            self.leave_unheld(c);
            return;
        }
        lg(self.line(c, "check of the start from the higher lock", from, &r));
        let lowest = if r.ok {
            return self.climb_from(c, start, None, from, r, c.cap, None, 0);
        } else {
            let mut e = self.entry(c, r.clk.unwrap_or(0));
            e.base = start;
            e.tested_at = uv::now();
            let back = RESUME_BACK * self.p.step.max(1);
            match self.resume(c, Some(&e), Some(from), &format!("{RESUME_BACK} steps below the failed start {}", signed(from)), &ladder(from - back, start, self.p.step, false), RESUME, None, start) {
                Resumed::Mining | Resumed::Stopped => return,
                Resumed::Exhausted(l) => l,
            }
        };
        let b = match base_res {
            Some(b) => b,
            None => match self.base_check(c, start) {
                Some(b) => b,
                None => return self.give_up(c, start),
            },
        };
        self.tune(c, b.0, b.1, c.cap.min(lowest - RESUME_BACK * self.p.step).min(b.0), None);
    }

    /// 1.0.3: the ladder of `plan` (each offset a `secs` check, the first that passes is mined at and saved).
    #[allow(clippy::too_many_arguments)]
    fn resume(&self, c: &Card, prof: Option<&Entry>, failed: Option<i32>, why: &str, ladder: &[i32], secs: u32, clk: Option<u32>, floor: i32) -> Resumed {
        let mut lowest = failed.unwrap_or(i32::MAX);
        // 1.0.12: the rungs that failed go into the crash history of the profile
        let mut fails: Vec<(u64, i32)> = Vec::new();
        for (i, &at) in ladder.iter().enumerate() {
            if i == 0 {
                lg(format!("{} resuming at {} ({why}): a {secs} s check, then this GPU mines (no new climb from the base)", c.label(), signed(at)));
            } else {
                lg(format!("{} resuming at {} ({} steps below the failed {})", c.label(), signed(at), RESUME_BACK, signed(lowest)));
            }
            let refs = prof.map_or(String::new(), |e| e.refs.clone());
            let r = self.check_ref(c, at, secs, &refs, prof.map(|p| p.base).filter(|&b| b < at));
            if r.infra {
                self.leave_unheld(c);
                return Resumed::Stopped;
            }
            lg(self.line(c, "resume check", at, &r));
            if !r.ok {
                lowest = at;
                fails.push((uv::now(), at));
                continue;
            }
            // 1.0.3: a resumed offset whose power is above the profile's base power does not lower it: the next rung down
            // 1.0.10: on the mining load against the base on the same loads (the saved base power is of other loads / minutes)
            let (pb, pw) = match r.pair {
                Some(pp) => (Some(pp.wa), Some(pp.wb)),
                None => (prof.map(|p| p.w_base).filter(|&b| b > 0.0), r.w),
            };
            if let (Some(p), Some(w)) = (pb, pw) {
                if w > p + tol_w(p) && !at_pl(w, self.nv.power_limit_w(c.nv)) {
                    lg(format!("{} resume at {}: the power {w:.1} W is above the base {p:.1} W of the profile: not taken", c.label(), signed(at)));
                    lowest = at;
                    continue;
                }
            }
            let mut e = match prof {
                Some(p) => p.clone(),
                None => {
                    let mut e = self.entry(c, r.clk.or(clk).unwrap_or(0));
                    e.tested_at = uv::now();
                    e.base = floor;
                    e
                }
            };
            if lowest != i32::MAX {
                e.edge = lowest;
            }
            for &(t, x) in &fails {
                note_crash(&mut e, t, x);
            }
            e.offset = at;
            e.kernel = kernel_id();
            e.driver = self.driver.clone();
            if !r.refs.is_empty() {
                e.refs = r.refs.clone();
            }
            if e.refs.is_empty() {
                e.refs = "-".into();
            }
            if r.ghs > 0.0 {
                e.ghs = r.ghs;
            }
            if let Some(w) = pw {
                e.w = w;
            }
            if let Some(pp) = r.pair {
                e.w_base = pp.wa;
            }
            e.verified_at = uv::now();
            e.disabled_until = 0;
            let _ = offset::set(&c.r.bus, at);
            c.cur.store(at, Ordering::Relaxed);
            self.save(c, &e);
            if at != ladder[0] || failed.is_some() {
                lg(format!("{} RESULT: offset {} (resumed{}), saved", c.label(), signed(at), if lowest != i32::MAX { format!(", {} failed", signed(lowest)) } else { String::new() }));
            }
            return Resumed::Mining;
        }
        if let Some(p) = prof {
            let k = pkey(p);
            for &(_, x) in &fails {
                self.note_fail(c, &k, x);
            }
        }
        if ladder.is_empty() {
            lg(format!("{} {why}: nothing left above the base {}: mining at the base", c.label(), signed(floor)));
        } else {
            lg(format!("{} no resume offset above the base passed (lowest failed {}): mining at the base", c.label(), signed(lowest)));
        }
        Resumed::Exhausted(lowest)
    }

    fn give_up(&self, c: &Card, start: i32) {
        if self.stopped() || self.leave_unheld(c) {
            return;
        }
        if c.no_load.load(Ordering::Relaxed) {
            lg(format!("{} no mining load for a long time (no work / idle / pool down): not a verdict, nothing disabled; mining at {}, tuned at the next start", c.label(), signed(start)));
            let _ = offset::set(&c.r.bus, start);
            c.cur.store(start, Ordering::Relaxed);
            return;
        }
        if c.no_verdict.load(Ordering::Relaxed) {
            // 1.0.12 (Rig01 21.11.2026): a base check without a verdict is no failure of this GPU: nothing is paused or saved, it
            // mines at its start offset (the OC profile's), the tuning runs again at the next start
            lg(format!("{} the base check of {} ended without a verdict (see above): nothing paused, nothing saved; mining at {}, tuned at the next start", c.label(), signed(start), signed(start)));
            let _ = offset::set(&c.r.bus, start);
            c.cur.store(start, Ordering::Relaxed);
            return;
        }
        let at = c.orig.min(0);
        lg(format!("{} no offset from {} down to 0 passes the check: auto-uv paused for 24 h, mining at {}", c.label(), signed(start), signed(at)));
        let _ = offset::set(&c.r.bus, at);
        c.cur.store(at, Ordering::Relaxed);
        let mut e = self.entry(c, 0);
        e.base = at;
        e.offset = at;
        e.disabled_until = uv::now() + DISABLE_SECS;
        e.pause_why = PAUSE_VERDICT.into();
        if let Some(l) = c.r.core_lock {
            e.core_lock = l;
            self.db.save(&c.uuid, "", &pkey(&e), &e);
        }
    }
}

/// 1.0.10: a mining-load step paired with the best offset: the judge compares the two powers of the same loads (the best's
/// absolute power is of other minutes / models). Returns whether it was paired.
fn apply_pair(judge: &mut Judge, now: &mut Meas, r: &Res) -> bool {
    let Some(p) = r.pair.filter(|_| r.mining && r.reference == Some(judge.best.at)) else { return false };
    judge.best.w = Some(p.wa);
    if p.ga > 0.0 {
        judge.best.ghs = p.ga;
    }
    now.w = Some(p.wb);
    if p.gb > 0.0 {
        now.ghs = p.gb;
    }
    true
}

#[derive(Debug, PartialEq)]
enum LockSplit {
    /// the marker counts for this run's lock (or there is none)
    Here(Option<profiles::Crash>),
    /// a failed offset found at another lock
    Other(u32, profiles::Crash),
}

/// 1.0.10: whose is a crash marker: the lock it was written at (`mlock`, None = before 1.0.10) against this run's lock.
fn lock_split(crash: Option<profiles::Crash>, mlock: Option<u32>, lock: Option<u32>) -> LockSplit {
    match (crash, mlock, lock) {
        (Some(k), Some(m), Some(l)) if m != l && k.failed() => LockSplit::Other(m, k),
        (Some(_), Some(m), Some(l)) if m != l => LockSplit::Here(None), // a bystander marker of another lock: its passed steps are of that lock
        (k, _, _) => LockSplit::Here(k),
    }
}

fn pending_file(dir: &std::path::Path) -> std::path::PathBuf {
    dir.join("neb-uv-locks.json")
}

/// 1.0.10: a failure of `uuid` at core lock `lock` kept for that lock (neb-uv-locks.json); the lower offset wins.
fn pending_put(dir: &std::path::Path, uuid: &str, lock: u32, k: profiles::Crash) {
    let p = pending_file(dir);
    let mut v: Value = std::fs::read_to_string(&p).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_else(|| json!({}));
    if !v.is_object() {
        v = json!({});
    }
    let old = profiles::parse_marker(&v[uuid][lock.to_string()]);
    let k = match old {
        Some(o) if o.at <= k.at => o,
        _ => k,
    };
    if !v[uuid].is_object() {
        v[uuid] = json!({});
    }
    v[uuid][lock.to_string()] = match k.good {
        Some(g) => json!({"offset": k.at, "good": g}),
        None => json!({"offset": k.at}),
    };
    let _ = std::fs::write(&p, serde_json::to_string_pretty(&v).unwrap_or_default());
}

/// The failure kept for `uuid` at `lock`, removed.
fn pending_take(dir: &std::path::Path, uuid: &str, lock: u32) -> Option<profiles::Crash> {
    let p = pending_file(dir);
    let mut v: Value = serde_json::from_str(&std::fs::read_to_string(&p).ok()?).ok()?;
    let k = profiles::parse_marker(&v[uuid][lock.to_string()])?;
    if let Some(m) = v[uuid].as_object_mut() {
        m.remove(&lock.to_string());
    }
    let _ = std::fs::write(&p, serde_json::to_string_pretty(&v).unwrap_or_default());
    Some(k)
}

/// 1.0.10: the start of a new core lock `lock` from the usable profile of the nearest higher lock (`back` MHz below it).
fn lock_hint(profs: &[(String, Entry)], lock: u32, start: i32, cap: i32, back: i32, now: u64) -> Option<(i32, String)> {
    let hs: Vec<climb::HintProfile> = profs
        .iter()
        .filter(|(_, e)| e.core_lock != 0)
        .map(|(_, e)| climb::HintProfile {
            clk: e.core_lock,
            mlock: 0,
            offset: e.offset,
            usable: e.offset >= e.base && profile_lowered(e) && e.disabled_until <= now && !e.refs.is_empty(),
            label: format!("lock {}", e.core_lock),
            verified_at: e.verified_at,
        })
        .collect();
    climb::hint_start(&hs, lock, 0, start, cap, back)
}

/// test hooks (tests only): FFG_TEST_CLOCK_ADD=N adds N MHz to the clock a check measures (1.0.3: a lock that does not hold),
/// FFG_TEST_CLOCK_ADD_ABOVE=N only at offsets above N (an offset that overclocks); FFG_TEST_UV_FAIL_ABOVE=N fails every check
/// above offset N (with FFG_TEST_OFFSET_SIM)
fn test_hooks(at: i32, res: &mut Res) {
    let add = std::env::var("FFG_TEST_CLOCK_ADD").ok().and_then(|v| v.parse::<u32>().ok());
    let above = std::env::var("FFG_TEST_CLOCK_ADD_ABOVE").ok().and_then(|v| v.parse::<i32>().ok()).unwrap_or(i32::MIN);
    if let (Some(add), true) = (add, at > above) {
        res.clk_med = res.clk_med.map(|x| x + add);
    }
    if res.ok && std::env::var("FFG_TEST_UV_FAIL_ABOVE").ok().and_then(|v| v.parse::<i32>().ok()).is_some_and(|n| at > n) {
        res.ok = false;
        res.reason = "injected test failure (FFG_TEST_UV_FAIL_ABOVE)".into();
    }
}

/// 1.0.10: the card as uvmine sees it (NVML of this card, the counters of its mining worker).
struct CardProbe<'a> {
    l: &'a Live,
    c: &'a Card,
    t0: Instant,
}

impl uvmine::Probe for CardProbe<'_> {
    fn set_offset(&mut self, at: i32) -> Result<(), String> {
        offset::set(&self.c.r.bus, at).map_err(|e| format!("{e:#}"))?;
        self.c.cur.store(at, Ordering::Relaxed);
        Ok(())
    }
    fn sample(&mut self) -> uvmine::Sample {
        let (nv, d, r) = (self.l.nv, self.c.nv, &self.c.r);
        uvmine::Sample {
            time: self.t0.elapsed().as_secs_f64(),
            w: nv.power_w(d),
            clk: nv.clock_mhz(d),
            temp: nv.temp_c(d).map(|x| x as i32),
            attempts: r.attempts.load(Ordering::Relaxed),
            // 1.0.11: a crash of the card's engine process fails the window like a wrong result (one process: the worker's
            // retry counted in `bad`)
            bad: r.bad.load(Ordering::Relaxed) + r.crashes.load(Ordering::Relaxed),
            crashes: r.crashes.load(Ordering::Relaxed),
            checked: r.checked.load(Ordering::Relaxed),
            key: r.load_key.load(Ordering::Relaxed),
            alive: r.alive.load(Ordering::Relaxed) && !r.pause.load(Ordering::Relaxed),
            xid: nv.take_xids(d).first().copied(),
            util: nv.util_gpu(d),
        }
    }
    fn sleep(&mut self, secs: f64) {
        std::thread::sleep(Duration::from_secs_f64(secs));
    }
    fn stopped(&self) -> bool {
        self.l.stopped()
    }
}

fn c_pos(l: &Live, c: &Card) -> usize {
    l.cards.iter().position(|x| std::ptr::eq(x.as_ref(), c)).unwrap_or(0)
}

/// Before mining: every GPU in parallel (one check child each); returns the state the miner keeps for `tick`.  `nv` from
/// offset::begin (the owner's offsets recorded).
pub fn prepare(nv: &'static Nvml, cards: Vec<CardRef>, o: &uv::Opts, dir: &std::path::Path, stop: &Arc<AtomicBool>) -> Arc<Live> {
    let db = Store::load(dir, profiles::NEB, |m| lg(m), |_| 0);
    if o.reset_all && db.forget_all() {
        lg("--uv-reset-all: every NEB profile forgotten");
    }
    let mut v = Vec::new();
    for (i, r) in cards.into_iter().enumerate() {
        let Ok(d) = nv.by_bus(&r.bus) else {
            lg(format!("GPU{}: no NVML handle; not tuned", r.dev));
            continue;
        };
        let Some((orig, lo, hi)) = offset::info(&r.bus) else {
            lg(format!("GPU{} (bus {}): the core offset cannot be read or set (root / Administrator needed?); not tuned", r.dev, uv::short_bus(&r.bus)));
            continue;
        };
        if let Err(e) = offset::set(&r.bus, orig) {
            lg(format!("GPU{} (bus {}): the core offset cannot be set ({e:#}); not tuned (Linux: root, Windows: Run as administrator)", r.dev, uv::short_bus(&r.bus)));
            continue;
        }
        // 1.0.12: a per-architecture default when --uv-max is not given (TURING_UV_MAX for GeForce CC 7.5)
        let (max, cap_src) = card_max(o.max_offset.get(i), nv.compute_capability(d), &r.name, hi);
        nv.watch_xid(d);
        let uuid = nv.uuid(d).unwrap_or_else(|_| r.bus.clone());
        v.push(Arc::new(Card {
            uuid,
            nv: d,
            orig,
            lo,
            cap: max,
            base: AtomicI32::new(orig),
            cur: AtomicI32::new(orig),
            key: Mutex::new(String::new()),
            refs: Mutex::new(String::new()),
            owned: AtomicBool::new(false),
            busy: AtomicBool::new(false),
            bad_seen: AtomicU64::new(r.bad.load(Ordering::Relaxed)),
            crash_seen: AtomicU64::new(r.crashes.load(Ordering::Relaxed)),
            next_reval: AtomicU64::new(0),
            next_readback: AtomicU64::new(0),
            resets: Default::default(),
            unheld: AtomicBool::new(false),
            failed_start: AtomicBool::new(false),
            deadline: AtomicU64::new(u64::MAX),
            mining: AtomicBool::new(false),
            no_load: AtomicBool::new(false),
            no_verdict: AtomicBool::new(false),
            recover: AtomicBool::new(false),
            fault_at: AtomicI32::new(0),
            fault_t: AtomicU64::new(0),
            rec_att: AtomicU64::new(0),
            rec_since: AtomicU64::new(0),
            rec_t: AtomicU64::new(0),
            cap_src,
            stable_cur: AtomicI32::new(i32::MIN),
            stable_since: AtomicU64::new(0),
            stable_saved: AtomicBool::new(false),
            r,
        }));
    }
    let mut guard = Guard::load(dir);
    if o.parallel_reset || o.reset_all {
        guard.unpause();
        // 1.0.12: also the rig's caution after an Xid 79
        if Xid79::reset(dir) {
            lg("the rig's caution after an Xid 79 is forgotten (--uv-unpause / --uv-reset-all)");
        }
    }
    // 1.0.12: an Xid 79 of an earlier run (uv-xid79.json): margins for a day, the rig cautious for XID79_CAUTIOUS_SECS
    let now = uv::now();
    if let Some(x) = Xid79::load(dir).filter(|x| x.recent(now)) {
        XID79_AT.fetch_max(x.at, Ordering::Relaxed);
        if x.cautious(now) {
            warn_set("CAUTIOUS", cautious_line(&x.gpu, x.at + XID79_CAUTIOUS_SECS - now));
        } else {
            lg(format!("an Xid 79 of {} {:.1} h ago: every GPU resumes below its saved offset until {} h after it", x.gpu, (now - x.at) as f64 / 3600.0, RECENT_CRASH_SECS / 3600));
        }
    }
    let rig = guard.start(uv::now(), env!("CARGO_PKG_VERSION"));
    guard.save(dir);
    let live = Arc::new(Live { nv, db, p: Params::from(o), driver: nv.driver_version(), cards: v, stop: stop.clone(), guard: Mutex::new(guard), rig_deadline: uv::now() + RIG_BUDGET_SECS });
    if rig == RigStart::Paused {
        // mining at the results of this version that are saved and complete, else at the GPU's own offset
        warn(format!("PAUSED on this rig: the miner ended during a tuning {RIG_EVENTS} times within an hour (restarts or reboots); mining goes on with the saved results, nothing is tuned until --uv-unpause / --uv-reset-all or a new version"));
        for c in &live.cards {
            let fin = live.db.list::<Entry>(&c.uuid, "").into_iter().find(|(_, e)| e.kernel == kernel_id() && e.driver == live.driver && c.r.core_lock.is_none_or(|l| e.core_lock == l));
            let at = fin.as_ref().map_or(c.orig, |(_, e)| e.offset);
            if offset::set(&c.r.bus, at).is_ok() {
                c.cur.store(at, Ordering::Relaxed);
            }
            lg(format!("{} auto-uv paused: mining at {} ({})", c.label(), signed(at), if fin.is_some() { "the saved result" } else { "its own offset" }));
        }
        return live;
    }
    let n = live.cards.len();
    if n == 0 {
        return live;
    }
    if o.parallel_reset && caution_reset(dir) {
        lg("--uv-parallel-reset: the rig's caution from earlier tunings is forgotten");
    }
    if o.reset_all && caution_reset(dir) {
        lg("--uv-reset-all: the rig's caution from earlier tunings is forgotten");
    }
    let cautious = caution_get(dir).or_else(|| rig_cautious(uv::now()).then(|| "an Xid 79 (a GPU fell off the bus), for some hours".to_string()));
    let (par, why) = if o.parallel.is_none() && cautious.is_none() && !force_child() {
        // 1.0.10: on the mining load a step starts no check process and adds no load (the card mines anyway): all at once
        (n, "all at once: each GPU is tuned on its own mining load, no check processes".to_string())
    } else {
        par_start(o.parallel, n, cautious.as_deref())
    };
    let delay = rig == RigStart::Delay;
    if delay {
        lg(format!("the previous run ended during a tuning (a restart or a reboot): {} min of mining first, the tuning goes on only if the rig stays calm", DELAY_SECS / 60));
    }
    if par >= n && o.parallel.is_some() && !delay && force_child() {
        // --uv-parallel N >= the GPUs: every GPU at once, before mining (as 1.0.4; the check processes start 3 s apart)
        lg(format!("tuning {n} GPU(s) at the same time ({why}) before mining"));
        std::thread::scope(|s| {
            for c in &live.cards {
                let l = live.clone();
                s.spawn(move || l.tune_card(c));
            }
        });
        for c in &live.cards {
            live.card_done(c);
        }
        return live;
    }
    // FFGN 1.0.5: a few GPUs at a time while the others mine at their current offset; a GPU's mining pauses while it is tuned
    lg(format!("tuning {par} of {n} GPU(s) at a time ({why}); every GPU keeps mining (a GPU without mining load within {} s is checked in a check process, its mining paused)", LOAD_WAIT.as_secs()));
    for c in &live.cards {
        c.busy.store(true, Ordering::Relaxed); // tick leaves it alone until its turn is over
    }
    struct Sched {
        queue: std::collections::VecDeque<usize>,
        limit: usize,
        running: usize,
        ok_in_row: usize,
        cautious: bool,
    }
    let sched = Arc::new(Mutex::new(Sched { queue: (0..n).collect(), limit: par, running: 0, ok_in_row: 0, cautious: cautious.is_some() }));
    let auto = o.parallel.is_none();
    let dir = dir.to_path_buf();
    let calm = Arc::new(AtomicBool::new(true));
    let gate = Arc::new(Mutex::new(!delay)); // false until the delay is over
    if delay {
        let (l, calm, gate) = (live.clone(), calm.clone(), gate.clone());
        let _ = std::thread::Builder::new().name("uv-delay".into()).spawn(move || {
            let t0 = RIG_TROUBLE.load(Ordering::Relaxed);
            let b0: u64 = l.cards.iter().map(|x| x.r.bad.load(Ordering::Relaxed)).sum();
            let t = Instant::now();
            while t.elapsed() < Duration::from_secs(DELAY_SECS) && !l.stopped() {
                std::thread::sleep(Duration::from_millis(500));
            }
            let b1: u64 = l.cards.iter().map(|x| x.r.bad.load(Ordering::Relaxed)).sum();
            if RIG_TROUBLE.load(Ordering::Relaxed) > t0 || b1 > b0 {
                calm.store(false, Ordering::Relaxed);
                warn(format!("the rig was not calm in the first {} min (all GPUs stalled / wrong results): no tuning in this run", DELAY_SECS / 60));
            } else if !l.stopped() {
                lg("the rig stayed calm: the tuning goes on");
            }
            *gate.lock().unwrap() = true;
        });
    }
    for k in 0..PAR_MAX.max(par).min(n) {
        let (l, sc, dir, calm, gate) = (live.clone(), sched.clone(), dir.clone(), calm.clone(), gate.clone());
        let _ = std::thread::Builder::new().name(format!("uv-tune{k}")).spawn(move || {
            while !*gate.lock().unwrap() && !l.stopped() {
                std::thread::sleep(Duration::from_millis(500));
            }
            loop {
                // a free place under the current limit, or the queue is empty
                let i = loop {
                    let mut g = sc.lock().unwrap();
                    // the rig's budget of this run, or a rig that was not calm after an unclean end: the rest at the next start
                    if !g.queue.is_empty() && (uv::now() >= l.rig_deadline || !calm.load(Ordering::Relaxed)) {
                        if uv::now() >= l.rig_deadline {
                            lg(format!("the tuning time of this run ({} min) is used up: the {} GPU(s) left mine at their offsets and are tuned at the next start", RIG_BUDGET_SECS / 60, g.queue.len()));
                        }
                        for &i in &g.queue {
                            l.cards[i].busy.store(false, Ordering::Relaxed);
                        }
                        g.queue.clear();
                    }
                    if g.queue.is_empty() || l.stopped() {
                        let rest: Vec<usize> = g.queue.drain(..).collect();
                        drop(g);
                        for i in rest {
                            l.cards[i].busy.store(false, Ordering::Relaxed);
                        }
                        return;
                    }
                    // 1.0.12: one GPU at a time while the rig is cautious after an Xid 79 (also one seen during this run)
                    let limit = par_limit(g.limit, rig_cautious(uv::now()));
                    if g.running < limit {
                        g.running += 1;
                        break g.queue.pop_front().unwrap();
                    }
                    drop(g);
                    std::thread::sleep(Duration::from_millis(200));
                };
                let c = l.cards[i].clone();
                let trouble0 = RIG_TROUBLE.load(Ordering::Relaxed);
                let bad0: u64 = l.cards.iter().filter(|x| x.r.dev != c.r.dev).map(|x| x.r.bad.load(Ordering::Relaxed)).sum();
                // 1.0.10: the GPU keeps mining (tune_card pauses it only for the check-process fallback)
                l.tune_card(&c);
                l.card_done(&c);
                c.busy.store(false, Ordering::Relaxed);
                // trouble while it was tuned: Xid 79 / a hanging check (RIG_TROUBLE), all GPUs stalled (mod.rs), a mining GPU
                // that computed wrong (its `bad` counter)
                let bad1: u64 = l.cards.iter().filter(|x| x.r.dev != c.r.dev).map(|x| x.r.bad.load(Ordering::Relaxed)).sum();
                let trouble = if RIG_TROUBLE.load(Ordering::Relaxed) > trouble0 {
                    Some(format!("trouble on the rig while {} was tuned (a GPU lost from the bus, a check that hung or all GPUs stalled at once)", c.label()))
                } else if bad1 > bad0 && !c.mining.load(Ordering::Relaxed) {
                    // (1.0.10: on the mining load every GPU is tuned at once and judged by its own results: a wrong result of
                    // another GPU is that GPU's failed step, not trouble caused by this one)
                    Some(format!("a mining GPU computed wrong results while {} was tuned", c.label()))
                } else {
                    None
                };
                let mut g = sc.lock().unwrap();
                g.running -= 1;
                g.ok_in_row = if trouble.is_some() { 0 } else { g.ok_in_row + 1 };
                let ceiling = par_ceiling(std::thread::available_parallelism().map_or(1, |x| x.get()), ram_mb().map(|x| x.1));
                if let Some((nl, why)) = par_next(g.limit, g.ok_in_row, trouble.as_deref(), auto, g.cautious, ceiling) {
                    if trouble.is_some() {
                        g.cautious = true;
                        caution_set(&dir, trouble.as_deref().unwrap_or(""));
                    }
                    lg(format!("parallel tuning {} -> {nl}: {why}", g.limit));
                    g.limit = nl;
                    g.ok_in_row = 0;
                } else if trouble.is_some() && !g.cautious {
                    g.cautious = true;
                    caution_set(&dir, trouble.as_deref().unwrap_or(""));
                }
            }
        });
    }
    live
}

impl Live {
    fn tune_card(&self, c: &Card) {
        let now = uv::now();
        if let Some(u) = self.guard.lock().unwrap().card_off(&c.uuid, now) {
            warn(format!("{}: auto-uv OFF for {:.1} h more ({CARD_FAILS} failures in a row): it mines at its own offset {}", c.label(), (u - now) as f64 / 3600.0, signed(c.orig)));
            let _ = offset::set(&c.r.bus, c.orig);
            c.cur.store(c.orig, Ordering::Relaxed);
            return;
        }
        lg(format!("{}: current offset {}, range {}..{} (max: {})", c.label(), signed(c.orig), signed(c.lo), signed(c.cap), c.cap_src));
        // the mark "tuning in progress" before the first check, off when the GPU mines (an end in between is an event)
        {
            let mut g = self.guard.lock().unwrap();
            g.tuning.push(c.uuid.clone());
            g.save(&self.db.dir);
        }
        // 1.0.10: the card is tuned WHILE IT MINES (its worker's load, the engine's own path per model); only a card that does
        // not mine within LOAD_WAIT (or FFGN_UV_CHILD=1) is checked in a check process with its mining paused, as before
        let mining = !force_child() && self.wait_load(c, LOAD_WAIT);
        c.mining.store(mining, Ordering::Relaxed);
        c.no_load.store(false, Ordering::Relaxed);
        // the tuning time of this GPU counts from here (not the wait for its first mining load)
        c.deadline.store(uv::now() + CARD_BUDGET_SECS, Ordering::Relaxed);
        if mining {
            lg(format!(
                "{} tuned while it mines: every step on its mining load (the path the engine chose per model), its MA/s counted and its shares sent all the time; power paired with the best offset on the same load; stable = no Xid of this GPU and every result CPU-checked (bad 0)",
                c.label()
            ));
        } else {
            lg(format!(
                "{} {}: the checks run in a check process (the engine's synthetic bench); mining on this GPU pauses while it is tuned",
                c.label(),
                if force_child() { "FFGN_UV_CHILD=1".to_string() } else { format!("its worker did not mine within {} s", LOAD_WAIT.as_secs()) }
            ));
            c.r.pause.store(true, Ordering::SeqCst);
            let t = Instant::now();
            while !c.r.paused.load(Ordering::SeqCst) && t.elapsed() < Duration::from_secs(60) && !self.stopped() {
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        self.prepare_card(c);
        self.leave_unheld(c);
        if !mining {
            c.r.pause.store(false, Ordering::SeqCst);
        }
        let mut g = self.guard.lock().unwrap();
        g.tuning.retain(|u| u != &c.uuid);
        if let Some(u) = g.card_result(&c.uuid, c.failed_start.load(Ordering::Relaxed), uv::now()) {
            let _ = u;
            warn(format!("{}: {CARD_FAILS} failures in a row at its tuned offsets: auto-uv OFF for this GPU for 24 h (now at {})", c.label(), signed(c.cur.load(Ordering::Relaxed))));
        }
        g.save(&self.db.dir);
    }

    /// After a GPU's tuning: the tuner watches it from now on.
    fn card_done(&self, c: &Card) {
        let t = uv::now();
        let cur = c.cur.load(Ordering::Relaxed);
        c.owned.store(cur > c.base.load(Ordering::Relaxed) && !self.stopped(), Ordering::Relaxed);
        c.bad_seen.store(c.r.bad.load(Ordering::Relaxed), Ordering::Relaxed);
        c.next_reval.store(t + (self.p.reval_h * 3600.0) as u64, Ordering::Relaxed);
        c.next_readback.store(t + 30, Ordering::Relaxed);
        if !self.stopped() {
            lg(format!("{} mining at offset {}", c.label(), signed(cur)));
        }
    }
}

impl Live {
    /// One step down (never below the checked base), saved.
    fn step_back(&self, c: &Card, why: &str) {
        let cur = c.cur.load(Ordering::Relaxed);
        let base = c.base.load(Ordering::Relaxed);
        let nw = (cur - self.p.step).max(base);
        if nw == cur {
            return;
        }
        if let Err(e) = offset::set(&c.r.bus, nw) {
            lg(format!("{} {why}: cannot step back ({e:#}); auto-uv stops watching this GPU", c.label()));
            c.owned.store(false, Ordering::Relaxed);
            return;
        }
        c.cur.store(nw, Ordering::Relaxed);
        lg(format!("{} {why}: offset {} -> {}, saved", c.label(), signed(cur), signed(nw)));
        let k = c.key.lock().unwrap().clone();
        if let Some(mut e) = self.db.get::<Entry>(&c.uuid, &k) {
            e.offset = nw;
            e.stepped_back += 1;
            note_crash(&mut e, uv::now(), cur); // 1.0.12: the crash history (resume margins)
            self.db.save(&c.uuid, "", &k, &e);
        }
        if nw <= base {
            c.owned.store(false, Ordering::Relaxed);
        }
    }

    /// 1.0.11 (iso.rs): the card's engine process crashed while it mined at its tuned offset (only this GPU restarted): the
    /// offset goes RESUME_BACK steps below the crash (never below the base), saved; the crash is this offset's (fault_offset:
    /// the main loop's crash record names it, so a restart resumes 2 steps below it too).
    fn crash_back(&self, c: &Card) {
        let at = c.cur.load(Ordering::Relaxed);
        let nw = crash_target(at, c.base.load(Ordering::Relaxed), self.p.step);
        c.fault_at.store(at, Ordering::Relaxed);
        c.fault_t.store(uv::now(), Ordering::Relaxed);
        if nw == at {
            lg(format!("{} crashed while mining at {} (its own engine process; only this GPU restarts): already at its base, the offset stays", c.label(), signed(at)));
            let k = c.key.lock().unwrap().clone();
            self.note_fail(c, &k, at);
            return;
        }
        if let Err(e) = offset::set(&c.r.bus, nw) {
            lg(format!("{} crashed while mining at {}: cannot step back ({e:#}); auto-uv stops watching this GPU", c.label(), signed(at)));
            c.owned.store(false, Ordering::Relaxed);
            return;
        }
        c.cur.store(nw, Ordering::Relaxed);
        lg(format!("{} crashed while mining at {} (its own engine process; only this GPU restarts): offset {} -> {} ({RESUME_BACK} steps below the crash), saved", c.label(), signed(at), signed(at), signed(nw)));
        let k = c.key.lock().unwrap().clone();
        if let Some(mut e) = self.db.get::<Entry>(&c.uuid, &k) {
            e.offset = nw;
            e.stepped_back += 1;
            note_crash(&mut e, uv::now(), at); // 1.0.12: the crash history (resume margins)
            self.db.save(&c.uuid, "", &k, &e);
        }
        c.bad_seen.store(c.r.bad.load(Ordering::Relaxed), Ordering::Relaxed);
        if nw <= c.base.load(Ordering::Relaxed) {
            c.owned.store(false, Ordering::Relaxed);
        }
    }

    fn revalidate(&self, c: &Card) {
        // 1.0.12: while the rig is cautious after an Xid 79, one re-check at a time (the others wait for their turn)
        struct Turn;
        impl Drop for Turn {
            fn drop(&mut self) {
                RECHECKS.fetch_sub(1, Ordering::SeqCst);
            }
        }
        while rig_cautious(uv::now()) && RECHECKS.load(Ordering::SeqCst) > 0 && !self.stopped() {
            std::thread::sleep(Duration::from_millis(500));
        }
        RECHECKS.fetch_add(1, Ordering::SeqCst);
        let _turn = Turn;
        let refs = c.refs.lock().unwrap().clone();
        // 1.0.10: on the mining load (it keeps mining); the check process only when the GPU does not mine now
        let mining = !force_child() && self.wait_load(c, Duration::from_secs(10));
        c.mining.store(mining, Ordering::Relaxed);
        if mining {
            lg(format!("{} re-check at {} on its mining load (~{} s, it keeps mining)", c.label(), signed(c.cur.load(Ordering::Relaxed)), QUICK));
            self.recheck_loop(c, &refs);
            let _ = offset::set(&c.r.bus, c.cur.load(Ordering::Relaxed));
            return;
        }
        lg(format!("{} re-check at {} (mining on this GPU pauses for ~{} s)", c.label(), signed(c.cur.load(Ordering::Relaxed)), QUICK + 5));
        c.r.pause.store(true, Ordering::SeqCst);
        let t = Instant::now();
        while !c.r.paused.load(Ordering::SeqCst) && t.elapsed() < Duration::from_secs(30) && !self.stopped() {
            std::thread::sleep(Duration::from_millis(50));
        }
        if c.r.paused.load(Ordering::SeqCst) {
            self.recheck_loop(c, &refs);
            let _ = offset::set(&c.r.bus, c.cur.load(Ordering::Relaxed));
        } else {
            lg(format!("{} re-check skipped (the mining thread did not pause)", c.label()));
        }
        c.r.pause.store(false, Ordering::SeqCst);
    }

    /// The re-check at the current offset; a failure steps back (never below the base) and checks again.
    fn recheck_loop(&self, c: &Card, refs: &str) {
        {
            while c.owned.load(Ordering::Relaxed) && !self.stopped() {
                let cur = c.cur.load(Ordering::Relaxed);
                let r = self.check(c, cur, QUICK, refs);
                lg(self.line(c, "re-check", cur, &r));
                if r.ok || r.infra {
                    if r.ok {
                        let k = c.key.lock().unwrap().clone();
                        if let Some(mut e) = self.db.get::<Entry>(&c.uuid, &k) {
                            e.verified_at = uv::now();
                            self.db.save(&c.uuid, "", &k, &e);
                        }
                    }
                    break;
                }
                self.step_back(c, "re-check failed");
            }
        }
    }
}

/// 1.0.11: the offset after a crash at `at`: RESUME_BACK steps lower, never below the base.
fn crash_target(at: i32, base: i32, step: i32) -> i32 {
    (at - RESUME_BACK * step.max(1)).max(base).min(at)
}

/// The offset the tuner keeps on `dev` (faults.rs: a crash at a tuned offset counts against that offset).
/// 1.0.10: the offset of a tuning step that faulted on this card a moment ago (its worker has not mined cleanly since): a crash of
/// the worker now belongs to that offset, not to the one the fault handling left on the card (the last good one).
pub fn fault_offset(live: &Live, dev: i32) -> Option<i32> {
    let c = live.cards.iter().find(|c| c.r.dev == dev)?;
    let t = c.fault_t.load(Ordering::Relaxed);
    (t > 0 && uv::now().saturating_sub(t) <= RECOVER_MAX as u64 + 60).then(|| c.fault_at.load(Ordering::Relaxed))
}

pub fn expected_offset(live: &Live, dev: i32) -> Option<i32> {
    live.cards.iter().find(|c| c.r.dev == dev).map(|c| c.cur.load(Ordering::Relaxed))
}

/// From the miner's main loop: wrong GPU results -> one step back (+ a re-check in a minute); someone else changed the offset ->
/// leave the GPU alone; the periodic re-check.
pub fn tick(live: &Arc<Live>) {
    let t = uv::now();
    for c in &live.cards {
        // 1.0.11 (iso.rs): a crash of the card's own engine process (seen here whatever the card's state: a crash during a tuning
        // step is that step's verdict, uvmine.rs)
        let k = c.r.crashes.load(Ordering::Relaxed);
        let crashed = k > c.crash_seen.swap(k, Ordering::Relaxed);
        // 1.0.3: the clock keeper found the core lock not holding while mining: back to +0, auto-uv leaves the card
        if c.owned.load(Ordering::Relaxed) && !c.busy.load(Ordering::Relaxed) && crate::clocks::not_held(&c.r.bus).is_some() {
            live.leave_unheld(c);
            continue;
        }
        if !c.owned.load(Ordering::Relaxed) || c.busy.load(Ordering::Relaxed) {
            continue;
        }
        if t >= c.next_readback.load(Ordering::Relaxed) {
            c.next_readback.store(t + 30, Ordering::Relaxed);
            if let Some(v) = offset::current(&c.r.bus) {
                let want = c.cur.load(Ordering::Relaxed);
                if v != want {
                    crate::faults::note_foreign_change(&c.r.bus);
                    match crate::hiveoc::reset_kind(live.nv, c.nv, v, c.orig) {
                        Some(why) if c.resets.allow() => {
                            // 1.7.8: HiveOS nvidia-oc (a miner start, an OC change in the web UI) wrote its own value: the verified
                            // offset goes back after a quick check (mining on this GPU pauses for ~QUICK s), as for Pearl / NOID
                            lg(format!(
                                "{} the offset was reset to {} by an OC apply ({why}); re-applying the verified {} after a quick check (mining on this GPU pauses for ~{} s)",
                                c.label(),
                                signed(v),
                                signed(want),
                                QUICK + 5
                            ));
                            c.busy.store(true, Ordering::Relaxed);
                            crate::clocks::reassert(&c.r.bus); // the lock first: a check at the verified offset on an unlocked clock crashes
                            let (l, c) = (live.clone(), c.clone());
                            std::thread::spawn(move || {
                                if !c.refs.lock().unwrap().is_empty() {
                                    l.revalidate(&c);
                                }
                                // revalidate sets the offset itself when it could pause the GPU; otherwise (or after a step back) once more here
                                if c.owned.load(Ordering::Relaxed) {
                                    let cur = c.cur.load(Ordering::Relaxed);
                                    if offset::current(&c.r.bus) != Some(cur) {
                                        let _ = offset::set(&c.r.bus, cur);
                                    }
                                }
                                c.bad_seen.store(c.r.bad.load(Ordering::Relaxed), Ordering::Relaxed);
                                c.next_readback.store(uv::now() + 30, Ordering::Relaxed);
                                c.busy.store(false, Ordering::Relaxed);
                            });
                            continue;
                        }
                        Some(why) => {
                            lg(format!(
                                "{} the offset was reset to {} by an OC apply ({why}) again: {} times within an hour already, so auto-uv stands down on this GPU and leaves it as is (restart the miner once the OC is settled)",
                                c.label(),
                                signed(v),
                                crate::hiveoc::OC_RESETS_MAX
                            ));
                            c.owned.store(false, Ordering::Relaxed);
                            continue;
                        }
                        None => {
                            lg(format!("{} the offset was changed by someone else ({} now, {} expected): auto-uv leaves this GPU as it is", c.label(), signed(v), signed(want)));
                            c.owned.store(false, Ordering::Relaxed);
                            continue;
                        }
                    }
                }
            }
        }
        let bad = c.r.bad.load(Ordering::Relaxed);
        let seen = c.bad_seen.swap(bad, Ordering::Relaxed);
        // 1.0.10 (live test, Shahtyor GPU1): the last tuning step faulted and the worker re-opens its CUDA context: its errors are
        // that step's, not the offset's the fault handling put back (they had stepped +240 -> +225 -> +210, saved). Wait until the
        // card mines cleanly for RECOVER_CALM_TICK s; a real fault at this offset after that steps back as before.
        if c.recover.load(Ordering::Relaxed) {
            if bad > seen || crashed {
                c.rec_since.store(t, Ordering::Relaxed);
            }
            // the main loop ticks every ~20 ms: the attempts are compared every 2 s (a scan call returns every 0.1-0.5 s)
            if t < c.rec_t.load(Ordering::Relaxed) + 2 {
                continue;
            }
            c.rec_t.store(t, Ordering::Relaxed);
            let att = c.r.attempts.load(Ordering::Relaxed);
            let grew = att > c.rec_att.swap(att, Ordering::Relaxed);
            if !grew || !c.r.alive.load(Ordering::Relaxed) {
                c.rec_since.store(t, Ordering::Relaxed);
            } else if t.saturating_sub(c.rec_since.load(Ordering::Relaxed)) >= RECOVER_CALM_TICK {
                c.recover.store(false, Ordering::Relaxed);
                c.fault_t.store(0, Ordering::Relaxed);
                live.db.clear_trying(&c.uuid);
                lg(format!("{} mines cleanly again after the fault of the tuning: at {}", c.label(), signed(c.cur.load(Ordering::Relaxed))));
            }
            continue;
        }
        if crashed {
            // the owner's rule (18.11.2026): after a crash, 2 steps below the offset that crashed, not one
            live.crash_back(c);
            c.next_reval.store(t + 60, Ordering::Relaxed);
            continue;
        }
        if bad > seen {
            live.step_back(c, &format!("{} candidate(s) failed the CPU check while mining", bad - seen));
            c.next_reval.store(t + 60, Ordering::Relaxed);
            continue;
        }
        // 1.0.12: an offset mined STABLE_SECS without a fault is the card's stable offset (its resume margin after a crash)
        live.note_stable(c, t);
        if live.p.reval_h > 0.0 && t >= c.next_reval.load(Ordering::Relaxed) && !c.refs.lock().unwrap().is_empty() {
            c.busy.store(true, Ordering::Relaxed);
            let (l, c) = (live.clone(), c.clone());
            std::thread::spawn(move || {
                l.revalidate(&c);
                c.bad_seen.store(c.r.bad.load(Ordering::Relaxed), Ordering::Relaxed);
                c.next_reval.store(uv::now() + (l.p.reval_h * 3600.0) as u64, Ordering::Relaxed);
                c.busy.store(false, Ordering::Relaxed);
            });
        }
    }
}

/// `--uv-offset N` without `--auto-uv` (1.7.8): the fixed offsets are kept like the tuned ones: when an OC apply (HiveOS nvidia-oc)
/// writes its own value over them, the offset of this run is set again (one log line, at most OC_RESETS_MAX an hour per GPU).
pub struct Fixed {
    cards: Mutex<Vec<FixedCard>>,
}

struct FixedCard {
    dev: i32,
    bus: String,
    nv: Dev,
    orig: i32,
    want: i32,
    owned: bool,
    resets: crate::hiveoc::ResetLimit,
    next: Instant,
}

/// `items` = (device, bus id, offset that was set); only offsets that were set count.
pub fn fixed_prepare(nv: &'static Nvml, items: &[(i32, String, i32)]) -> Option<(&'static Nvml, Fixed)> {
    let cards: Vec<FixedCard> = items
        .iter()
        .filter_map(|(dev, bus, want)| {
            let d = nv.by_bus(bus).ok()?;
            let (orig, _, _) = offset::info(bus)?;
            Some(FixedCard { dev: *dev, bus: bus.clone(), nv: d, orig, want: *want, owned: true, resets: Default::default(), next: Instant::now() + Duration::from_secs(30) })
        })
        .collect();
    (!cards.is_empty()).then(|| (nv, Fixed { cards: Mutex::new(cards) }))
}

pub fn fixed_tick(nv: &Nvml, f: &Fixed) {
    let mut g = f.cards.lock().unwrap();
    for c in g.iter_mut() {
        if !c.owned || Instant::now() < c.next {
            continue;
        }
        c.next = Instant::now() + Duration::from_secs(30);
        let Some(v) = offset::current(&c.bus) else { continue };
        if v == c.want {
            continue;
        }
        let label = format!("GPU{} (bus {})", c.dev, uv::short_bus(&c.bus));
        crate::faults::note_foreign_change(&c.bus);
        match crate::hiveoc::reset_kind(nv, c.nv, v, c.orig) {
            Some(why) if c.resets.allow() => match {
                crate::clocks::reassert(&c.bus); // the lock first: the offset on an unlocked clock is too much
                offset::set(&c.bus, c.want)
            } {
                Ok(()) => lg(format!("{label} the offset was reset to {} by an OC apply ({why}); set back to {} (--uv-offset)", signed(v), signed(c.want))),
                Err(e) => {
                    lg(format!("{label} the offset was reset to {} by an OC apply ({why}); cannot set {} again ({e:#}): left as it is", signed(v), signed(c.want)));
                    c.owned = false;
                }
            },
            Some(why) => {
                lg(format!(
                    "{label} the offset was reset to {} by an OC apply ({why}) again: {} times within an hour already, so the miner stands down on this GPU and leaves it as is (restart the miner once the OC is settled)",
                    signed(v),
                    crate::hiveoc::OC_RESETS_MAX
                ));
                c.owned = false;
            }
            None => {
                lg(format!("{label} the offset was changed by someone else ({} now, {} expected): the miner leaves this GPU as it is", signed(v), signed(c.want)));
                c.owned = false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// 1.0.12 (Rig01 21.11.2026, FIELD111.md anomaly 1): 1.0.10 paused 6 of 8 GPUs for 24 h at +0 (OC profile +275, +60..70 W
    /// each) because their base check had no verdict: their workers stopped with GPU5's Xid 43 in the one-process engine. Such a
    /// pause (no `pause_why`) is not honored; a pause written after a real verdict is, and survives the JSON round trip.
    #[test]
    fn a_pause_without_a_verdict_is_not_honored() {
        let rig01 = serde_json::json!({"base_offset": 0, "bus": "0000:05:00.0", "clk_mhz": 0, "core_lock": 1800,
            "disabled_until": 1795335196u64, "driver": "580.82.07", "edge": 0, "ghs": 0.0, "kernel": "ffgn1.0.10/neb",
            "name": "NVIDIA GeForce RTX 3070", "offset": 0, "orig_offset": 275, "refs": "", "step": 15,
            "stepped_back_in_mining": 0, "tested_at": 0, "verified_at": 0, "watts": 0.0, "watts_base": 0.0});
        let e = Entry::from_json(&rig01).unwrap();
        assert!(e.disabled_until > 0 && e.pause_why.is_empty());
        assert!(!pause_honored(&e));
        let v = Entry { pause_why: PAUSE_VERDICT.into(), ..e.clone() };
        assert!(pause_honored(&v));
        let back = Entry::from_json(&v.to_json()).unwrap();
        assert!(pause_honored(&back));
        assert_eq!(back.disabled_until, 1795335196);
    }
    /// a saved profile of this version (LAS-rig5.md GPU1: confirmed +375, base +0) with crash records at `crashes` (age in s)
    fn las_gpu1(now: u64, crashes: &[(u64, i32)]) -> Entry {
        let mut e = Entry { kernel: kernel_id(), driver: "580".into(), core_lock: 1305, base: 0, offset: 375, edge: 450, refs: "a".into(), ..Default::default() };
        for &(age, at) in crashes {
            note_crash(&mut e, now - age, at);
        }
        e
    }
    fn sp(e: &Entry) -> Option<SavedProf> {
        Some(SavedProf { offset: e.offset, base: e.base, same_kernel: e.kernel == kernel_id() && !e.refs.is_empty(), lowered: profile_lowered(e) })
    }

    /// 1.0.12 (LAS-rig5.md): after the reboot GPU1/2/3 resumed straight at the saved +375 and crashed there and below. A card with
    /// crashes in the last 24 h resumes at a margin: the offset that mined 1 h without a fault, else half way from the base.
    #[test]
    fn a_restart_after_recent_crashes_resumes_at_a_margin() {
        let now = 1_800_000_000u64;
        let e = las_gpu1(now, &[(3 * 3600, 450), (2 * 3600, 495)]);
        let p = plan(None, sp(&e), 0, 495, 15, None);
        assert_eq!(lad(&p).0[0], 375); // without the margin: straight at +375 (1.0.11)
        let m = margin_plan(&p, &e, 0, 15, 495, now, false).expect("a margin");
        assert_eq!((m.at, m.saved, m.ceiling), (180, 375, 375)); // 0 + (375 / 2 = 187 -> the step grid) = +180, back up to +375 at most
        assert!(m.why.contains("2 crash(es)") && m.why.contains("half way from the base +0"), "{}", m.why);
        // the grid is the base's: --uv-min 120 -> 120 + 255 / 2 = 247 -> 120 + 120 = +240
        assert_eq!(margin_plan(&p, &e, 120, 15, 495, now, false).unwrap().at, 240);
        // the offset that mined 1 h without a fault is the margin when it is known
        let mut s = e.clone();
        s.stable = Some(255);
        s.stable_at = now - 1800;
        assert_eq!(margin_plan(&p, &s, 0, 15, 495, now, false).unwrap().at, 255);
        // ... but not when it is the saved offset itself (no crash at or below it since)
        s.stable = Some(375);
        assert_eq!(margin_plan(&p, &s, 0, 15, 495, now, false), None);
        // an earlier version's profile (Plan::Continue): the margin too, the climb then goes up to the cap as before
        let old = Entry { kernel: "ffgn1.0.10/neb".into(), ..e.clone() };
        let p = plan(None, sp(&old), 0, 495, 15, None);
        assert_eq!(p, Plan::Continue { from: 375 });
        let m = margin_plan(&p, &old, 0, 15, 495, now, false).unwrap();
        assert_eq!((m.at, m.ceiling), (180, 495));
        // nothing above the base: no margin
        let p = plan(None, prof(0, true), 0, 495, 15, None);
        assert_eq!(margin_plan(&p, &e, 0, 15, 495, now, false), None);
    }

    #[test]
    fn a_restart_after_old_crashes_resumes_at_the_saved_offset() {
        let now = 1_800_000_000u64;
        let e = las_gpu1(now, &[(RECENT_CRASH_SECS + 60, 450), (RECENT_CRASH_SECS + 1, 375)]);
        assert!(recent_crashes(&e, now).is_empty());
        let p = plan(None, sp(&e), 0, 495, 15, None);
        assert_eq!(margin_plan(&p, &e, 0, 15, 495, now, false), None);
        assert_eq!(lad(&p), (vec![375, 345, 315, 285, 255, 225, 195, 165, 135, 105, 75, 45, 15], QUICK));
        // no crash at all: the saved offset as before
        let clean = las_gpu1(now, &[]);
        assert_eq!(margin_plan(&plan(None, sp(&clean), 0, 495, 15, None), &clean, 0, 15, 495, now, false), None);
        // but a recent Xid 79 on the rig: every card gets the margin
        let m = margin_plan(&plan(None, sp(&clean), 0, 495, 15, None), &clean, 0, 15, 495, now, true).unwrap();
        assert_eq!(m.at, 180);
        assert!(m.why.contains("Xid 79"), "{}", m.why);
    }

    /// the owner's rule (18.11.2026) stays for every NEW crash: failed - 2 steps, never above the last confirmed offset; a start
    /// after a failure of the previous run gets no margin on top of it
    #[test]
    fn a_new_crash_after_the_margin_resume_goes_two_steps_below_it() {
        let now = 1_800_000_000u64;
        // resumed at the margin +180, climbed back to +255, crashed there while mining (crash_back in this run)
        assert_eq!(crash_target(255, 0, 15), 225);
        let mut e = las_gpu1(now, &[(3600, 375)]);
        e.offset = 225;
        note_crash(&mut e, now, 255);
        // the next start finds the crash record: 2 steps below it, no margin
        let p = plan(crash(255, None), sp(&e), 0, 495, 15, Some(255));
        assert_eq!(lad(&p).0[0], 225);
        assert_eq!(margin_plan(&p, &e, 0, 15, 495, now, false), None);
        // a crash far above the confirmed offset: never above the last confirmed
        let p = plan(crash(375, None), sp(&e), 0, 495, 15, Some(375));
        assert_eq!(lad(&p).0[0], 225);
        // the margin's climb never goes above the saved offset
        let e = las_gpu1(now, &[(600, 450)]);
        let m = margin_plan(&plan(None, sp(&e), 0, 495, 15, None), &e, 0, 15, 495, now, false).unwrap();
        assert!(m.ceiling <= e.offset);
    }

    #[test]
    fn the_crash_history_of_a_profile() {
        let now = 1_800_000_000u64;
        let mut e = Entry { stable: Some(300), stable_at: now - 7200, ..Default::default() };
        // a crash above the stable offset keeps it, one at or below it drops it
        note_crash(&mut e, now - 100, 345);
        assert_eq!(e.stable, Some(300));
        note_crash(&mut e, now - 50, 300);
        assert_eq!(e.stable, None);
        // the main loop's record and the tuner's of the same crash are one
        note_crash(&mut e, now - 40, 300);
        assert_eq!(e.crash_log.len(), 2);
        // at most CRASH_LOG_MAX, older than a day dropped
        for i in 0..40u64 {
            note_crash(&mut e, now + i * 600, 100 + i as i32);
        }
        assert_eq!(e.crash_log.len(), CRASH_LOG_MAX);
        assert_eq!(e.crash_log.last(), Some(&(now + 39 * 600, 139)));
        note_crash(&mut e, now + RECENT_CRASH_SECS + 39 * 600, 90);
        assert_eq!(e.crash_log, vec![(now + RECENT_CRASH_SECS + 39 * 600, 90)]);
        // a new entry (a climb / a resume) keeps the history and the stable offset of the old one
        let old = Entry { crash_log: vec![(now - 600, 375)], stable: Some(240), stable_at: now - 300, ..Default::default() };
        let mut new = Entry { offset: 255, ..Default::default() };
        merge_history(&mut new, &old);
        assert_eq!((new.crash_log.clone(), new.stable), (vec![(now - 600, 375)], Some(240)));
        // the JSON round trip
        let back = Entry::from_json(&Entry { kernel: kernel_id(), ..new.clone() }.to_json()).unwrap();
        assert_eq!((back.crash_log, back.stable, back.stable_at), (vec![(now - 600, 375)], Some(240), now - 300));
    }

    /// 1.0.12: a uv.json written before 1.0.12 (no crash_log / stable_offset / stable_at) loads: no history, no stable offset
    #[test]
    fn an_old_profile_without_the_crash_history_loads() {
        let v = serde_json::json!({"base_offset": 0, "bus": "0000:02:00.0", "clk_mhz": 0, "core_lock": 1305, "disabled_until": 0,
            "driver": "580.82.07", "edge": 450, "ghs": 10.1, "kernel": "ffgn1.0.11/neb", "name": "NVIDIA GeForce RTX 2060 SUPER",
            "offset": 375, "orig_offset": 0, "refs": "x", "step": 15, "stepped_back_in_mining": 0, "tested_at": 1, "verified_at": 1,
            "watts": 70.0, "watts_base": 80.0});
        let e = Entry::from_json(&v).unwrap();
        assert!(e.crash_log.is_empty() && e.stable.is_none() && e.stable_at == 0);
        assert_eq!((e.offset, e.edge), (375, 450));
        // a 1.0.12 file with a broken record keeps the good ones
        let mut v = v;
        v["crash_log"] = serde_json::json!([{"t": 5, "at": 375}, {"t": "x"}, 7]);
        v["stable_offset"] = serde_json::Value::Null;
        let e = Entry::from_json(&v).unwrap();
        assert_eq!((e.crash_log, e.stable), (vec![(5, 375)], None));
    }

    /// 1.0.12: an Xid 79 makes the rig cautious for XID79_CAUTIOUS_SECS (no climb, one GPU at a time), kept in uv-xid79.json for
    /// the next start; every GPU resumes below its saved offset for RECENT_CRASH_SECS; both expire
    #[test]
    fn an_xid79_makes_the_rig_cautious_for_some_hours() {
        let t = 1_800_000_000u64;
        let x = Xid79 { at: t, gpu: "GPU1 (bus 02, RTX 2060 SUPER)".into() };
        assert!(x.cautious(t + 1) && x.cautious(t + XID79_CAUTIOUS_SECS - 1) && !x.cautious(t + XID79_CAUTIOUS_SECS));
        assert!(x.recent(t + XID79_CAUTIOUS_SECS) && !x.recent(t + RECENT_CRASH_SECS));
        assert!(!Xid79::default().cautious(t) && !Xid79::default().recent(t));
        // cautious: one GPU at a time, no room above the current offset
        assert_eq!(par_limit(4, true), 1);
        assert_eq!(par_limit(4, false), 4);
        assert_eq!(climb_cap(495, 180, true), 180);
        assert_eq!(climb_cap(495, 180, false), 495);
        // across a restart (the file), and the reset
        let d = std::env::temp_dir().join(format!("ffgn-xid79-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&d);
        assert_eq!(Xid79::load(&d), None);
        x.save(&d);
        assert_eq!(Xid79::load(&d), Some(x.clone()));
        assert!(Xid79::reset(&d));
        assert_eq!(Xid79::load(&d), None);
        // note_xid79: the file, the process state, the loud warning and its stats-line tag
        note_xid79(&d, "GPU1 (bus 02, RTX 2060 SUPER)", true);
        let back = Xid79::load(&d).unwrap();
        assert_eq!(back.gpu, "GPU1 (bus 02, RTX 2060 SUPER)");
        assert!(back.cautious(uv::now()) && rig_cautious(uv::now()));
        assert!(warnings().iter().any(|w| w.starts_with("CAUTIOUS") && w.contains("GPU1 (bus 02") && w.contains("riser / power cable")), "{:?}", warnings());
        assert!(tags().iter().any(|t| t == "CAUTIOUS Xid 79"), "{:?}", tags());
        note_xid79(&d, "GPU1 (bus 02, RTX 2060 SUPER)", true);
        assert_eq!(warnings().iter().filter(|w| w.starts_with("CAUTIOUS")).count(), 1);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 1.0.12: GeForce Turing (CC 7.5) climbs to TURING_UV_MAX by default; --uv-max overrides it; CMP and other archs keep DEF_MAX
    #[test]
    fn turing_has_its_own_default_cap() {
        assert_eq!(card_max(None, Some((7, 5)), "NVIDIA GeForce RTX 2060 SUPER", 1000).0, TURING_UV_MAX);
        assert_eq!(card_max(None, Some((7, 5)), "NVIDIA GeForce GTX 1660 Ti", 1000).0, 240);
        assert_eq!(card_max(None, Some((7, 5)), "NVIDIA CMP 50HX", 1000).0, DEF_MAX);
        assert_eq!(card_max(None, Some((8, 6)), "NVIDIA GeForce RTX 3070", 1000).0, DEF_MAX);
        assert_eq!(card_max(None, None, "NVIDIA GeForce RTX 2060 SUPER", 1000).0, DEF_MAX); // CC unknown: as before
        let (v, src) = card_max(Some(375), Some((7, 5)), "NVIDIA GeForce RTX 2060 SUPER", 1000);
        assert_eq!((v, src), (375, "--uv-max"));
        assert_eq!(card_max(Some(150), Some((7, 5)), "NVIDIA GeForce RTX 2060 SUPER", 1000).0, 150);
        assert_eq!(card_max(None, Some((7, 5)), "NVIDIA GeForce RTX 2060 SUPER", 200).0, 200); // the driver's range
        // a saved +375 of a 2060 SUPER resumes at the cap
        assert_eq!(lad(&plan(None, prof(375, true), 0, TURING_UV_MAX, 15, None)).0[0], 240);
    }

    #[test]
    fn a_crash_while_mining_goes_two_steps_below_it() {
        // 1.0.11 (iso.rs): the owner's rule after a crash: "the failed offset - 2 steps", never below the base
        assert_eq!(crash_target(165, 0, 15), 135);
        assert_eq!(crash_target(30, 0, 15), 0);
        assert_eq!(crash_target(20, 10, 15), 10);
        assert_eq!(crash_target(0, 0, 15), 0);
    }

    #[test]
    fn guards_simulated_restarts() {
        let v = "1.0.5";
        let t = 1_000_000u64;
        let mut g = Guard::default();
        // a clean start: go
        assert_eq!(g.start(t, v), RigStart::Go);
        // the miner dies during a tuning (the mark stays): the next start waits (mining first)
        g.tuning.push("gpu-a".into());
        assert_eq!(g.start(t + 600, v), RigStart::Delay);
        assert!(g.tuning.is_empty());
        // again within the hour: paused, also on later clean starts
        g.tuning.push("gpu-a".into());
        assert_eq!(g.start(t + 1200, v), RigStart::Paused);
        assert_eq!(g.start(t + 9000, v), RigStart::Paused);
        // a new version: tuning again
        assert_eq!(g.start(t + 9600, "1.0.6"), RigStart::Go);
        // two events more than an hour apart: no pause
        let mut g = Guard::default();
        g.tuning.push("x".into());
        assert_eq!(g.start(t, v), RigStart::Delay);
        g.tuning.push("x".into());
        assert_eq!(g.start(t + 4000, v), RigStart::Delay);
        // the manual reset
        g.tuning.push("x".into());
        assert_eq!(g.start(t + 4100, v), RigStart::Paused);
        g.unpause();
        assert_eq!(g.start(t + 4200, v), RigStart::Go);
        // a GPU: 3 failed starts in a row -> off 24 h; a good start in between resets the count
        let mut g = Guard::default();
        assert_eq!(g.card_result("a", true, t), None);
        assert_eq!(g.card_result("a", true, t), None);
        assert_eq!(g.card_result("a", false, t), None);
        assert_eq!(g.card_result("a", true, t), None);
        assert_eq!(g.card_result("a", true, t), None);
        assert_eq!(g.card_result("a", true, t), Some(t + CARD_OFF_SECS));
        assert_eq!(g.card_off("a", t + 100), Some(t + CARD_OFF_SECS));
        assert_eq!(g.card_off("a", t + CARD_OFF_SECS + 1), None);
        assert_eq!(g.card_off("b", t), None);
        // the file round trip
        let d = std::env::temp_dir().join(format!("ffgn-guard-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&d);
        g.tuning.push("a".into());
        g.events.push(t);
        g.save(&d);
        assert_eq!(Guard::load(&d), g);
        let _ = std::fs::remove_dir_all(&d);
    }
    #[test]
    fn in_progress_profile() {
        // a profile saved in the middle of a climb is another "kernel": the climb goes on from it after a restart
        let ip = Entry { kernel: climbing_id(), core_lock: 1350, base: 165, offset: 225, w_base: 80.0, w: 76.0, refs: "a".into(), ..Default::default() };
        let p = SavedProf { offset: ip.offset, base: ip.base, same_kernel: ip.kernel == kernel_id() && !ip.refs.is_empty(), lowered: profile_lowered(&ip) };
        assert_eq!(plan(None, Some(p), 165, 330, 15, None), Plan::Continue { from: 225 });
        // a crash in the middle of the check of +255 (the trying marker): 2 steps below it, not above the last taken +225
        let cr = profiles::Crash { at: 255, good: Some(240) };
        match plan(Some(cr), Some(p), 165, 330, 15, None) {
            Plan::Resume { failed, ladder, .. } => assert_eq!((failed, ladder[0]), (Some(255), 225)),
            x => panic!("{x:?}"),
        }
        // a crash at +300 far above it: still not above the last taken step
        let cr = profiles::Crash { at: 300, good: None };
        match plan(Some(cr), Some(p), 165, 330, 15, None) {
            Plan::Resume { ladder, .. } => assert_eq!(ladder[0], 225),
            x => panic!("{x:?}"),
        }
    }
    /// 1.0.12 (PC_AORUS RTX 3080 Ti, --lock-cclock 1680 --pl 260): +300 "taken, saved (tuning in progress)", then the card faulted
    /// ("unknown error", now a crash of its engine process: faults::sticky).  The owner's rule: failed - 2 steps, never above the
    /// last confirmed (here: the last taken) offset.
    #[test]
    fn a_crash_right_after_a_taken_step() {
        let ip = Entry { kernel: climbing_id(), core_lock: 1680, base: 0, offset: 300, w_base: 300.0, w: 259.5, refs: "a".into(), ..Default::default() };
        let p = SavedProf { offset: ip.offset, base: ip.base, same_kernel: ip.kernel == kernel_id() && !ip.refs.is_empty(), lowered: profile_lowered(&ip) };
        // the fault in the next paired window (+360 against +300): the trying marker / fault_offset name +360; a restart resumes
        // 2 steps below it but not above the last taken +300
        match plan(Some(profiles::Crash { at: 360, good: Some(300) }), Some(p), 0, 495, 15, None) {
            Plan::Resume { failed, ladder, .. } => assert_eq!((failed, ladder[0], ladder.get(1).copied()), (Some(360), 300, Some(270))),
            x => panic!("{x:?}"),
        }
        // the fault at the taken step itself (its confirmation, or mining at it): +300 is the crash offset -> +270
        match plan(Some(profiles::Crash { at: 300, good: Some(240) }), Some(p), 0, 495, 15, None) {
            Plan::Resume { failed, ladder, .. } => assert_eq!((failed, ladder[0]), (Some(300), 270)),
            x => panic!("{x:?}"),
        }
        // the miner survives now: a crash while mining at the taken +300 steps it back in the run (tick -> crash_back): +270
        assert_eq!(crash_target(300, 0, 15), 270);
        // and again: 2 steps lower each time, down to the base
        assert_eq!(crash_target(270, 0, 15), 240);
        assert_eq!(crash_target(15, 0, 15), 0);
    }

    #[test]
    fn parallel_tuning() {
        // start: 1 on 6+ GPUs, else 2 (never more than the GPUs); cautious rig: 1; --uv-parallel: as given
        assert_eq!(par_start(None, 12, None).0, 1);
        assert_eq!(par_start(None, 4, None).0, 2);
        assert_eq!(par_start(None, 1, None).0, 1);
        assert_eq!(par_start(None, 4, Some("x")).0, 1);
        assert_eq!(par_start(Some(3), 12, Some("x")).0, 3);
        assert_eq!(par_start(Some(8), 2, None).0, 2);
        // ceiling: free CPU threads (all but one) and RAM / CHECK_MB, at most PAR_MAX
        assert_eq!(par_ceiling(2, Some(7000)), 1); // hive2060SSS: 2 threads
        assert_eq!(par_ceiling(8, Some(3000)), 2); // RAM: 2 checks
        assert_eq!(par_ceiling(24, Some(30000)), PAR_MAX);
        assert_eq!(par_ceiling(24, None), PAR_MAX);
        // growth 1 -> 2 -> 4 after GROW_AFTER GPUs without trouble, capped by the ceiling
        assert_eq!(par_next(1, 1, None, true, false, 4), None);
        assert_eq!(par_next(1, 2, None, true, false, 4).map(|x| x.0), Some(2));
        assert_eq!(par_next(2, 2, None, true, false, 4).map(|x| x.0), Some(4));
        assert_eq!(par_next(4, 2, None, true, false, 4), None);
        assert_eq!(par_next(2, 2, None, true, false, 1), None); // the ceiling stops it (no growth below)
        assert_eq!(par_next(1, 5, None, true, false, 1), None);
        // never with --uv-parallel or on a cautious rig
        assert_eq!(par_next(1, 9, None, false, false, 4), None);
        assert_eq!(par_next(1, 9, None, true, true, 4), None);
        // trouble: 1 at once
        assert_eq!(par_next(4, 0, Some("Xid 79"), true, false, 4).map(|x| x.0), Some(1));
        assert_eq!(par_next(2, 0, Some("stall"), false, false, 4).map(|x| x.0), Some(1));
        assert_eq!(par_next(1, 0, Some("stall"), true, true, 4), None); // already 1 and remembered
        // the rig's memory
        let d = std::env::temp_dir().join(format!("ffgn-par-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&d);
        assert_eq!(caution_get(&d), None);
        caution_set(&d, "Xid 79 on GPU3");
        assert_eq!(caution_get(&d).as_deref(), Some("Xid 79 on GPU3"));
        assert!(caution_reset(&d));
        assert_eq!(caution_get(&d), None);
        let _ = std::fs::remove_dir_all(&d);
    }
    #[test]
    fn keys_and_json() {
        let mut e = Entry { kernel: kernel_id(), driver: "580".into(), core_lock: 1500, offset: 210, base: 0, refs: "a:b:c".into(), ..Default::default() };
        assert_eq!(pkey(&e), "1500");
        let back = Entry::from_json(&e.to_json()).unwrap();
        assert_eq!((back.offset, back.refs.as_str(), back.core_lock), (210, "a:b:c", 1500));
        e.core_lock = 0;
        e.clk = 1903;
        assert_eq!(pkey(&e), "~1905");
        let f = Entry { clk: 1897, ..e.clone() };
        assert!(e.same_clocks(&f));
    }

    fn prof(offset: i32, same_kernel: bool) -> Option<SavedProf> {
        Some(SavedProf { offset, base: 0, same_kernel, lowered: true })
    }
    fn crash(at: i32, good: Option<i32>) -> Option<profiles::Crash> {
        Some(profiles::Crash { at, good })
    }
    fn lad(p: &Plan) -> (Vec<i32>, u32) {
        match p {
            Plan::Resume { ladder, secs, .. } => (ladder.clone(), *secs),
            _ => panic!("expected a resume, got {p:?}"),
        }
    }

    #[test]
    fn resume_after_a_crash_while_mining() {
        // GPU1 of Shahtyor 18.11.2026: tuned +225, stepped back to +180 by the CPU check, Xid 31 at +180 while mining
        let p = plan(crash(180, None), prof(180, true), 0, 495, 15, Some(180));
        assert_eq!(lad(&p), (vec![150, 120, 90, 60, 30], RESUME));
        let Plan::Resume { why, failed, .. } = &p else { unreachable!() };
        assert_eq!(*failed, Some(180));
        assert_eq!(why, "2 steps below the crash while mining at +180");
        // never above the last confirmed offset (the crash was in a check above the saved one)
        let p = plan(crash(300, Some(255)), prof(150, true), 0, 495, 15, None);
        assert_eq!(lad(&p).0[0], 150);
        let Plan::Resume { why, .. } = &p else { unreachable!() };
        assert!(why.contains("not above the last confirmed +150"), "{why}");
        // nor above --uv-max
        assert_eq!(lad(&plan(crash(300, None), prof(300, true), 0, 200, 15, Some(300))).0[0], 200);
    }

    #[test]
    fn repeated_crashes_go_down_to_the_base() {
        // each crash start saves the resumed offset; the next crash there goes 2 steps lower, at the base the ladder is empty
        let (mut saved, mut seen) = (180, Vec::new());
        loop {
            let p = plan(crash(saved, None), prof(saved, true), 0, 495, 15, Some(saved));
            let (l, _) = lad(&p);
            match l.first() {
                Some(&at) => {
                    assert_eq!(at, saved - 30);
                    seen.push(at);
                    saved = at;
                }
                None => break,
            }
        }
        assert_eq!(seen, vec![150, 120, 90, 60, 30]);
        // a crash at +30 leaves nothing above the base 0: mining at the base (no climb)
        assert_eq!(lad(&plan(crash(30, None), prof(30, true), 0, 495, 15, Some(30))).0, Vec::<i32>::new());
        // a raised base (--uv-min 60): nothing at or below it
        assert_eq!(lad(&plan(crash(120, None), prof(120, true), 60, 495, 15, Some(120))).0, vec![90]);
    }

    #[test]
    fn saved_profiles_and_first_tuning() {
        // nothing failed: the saved offset, quick check; failing it -> 2 steps lower each time
        assert_eq!(lad(&plan(None, prof(225, true), 0, 495, 15, None)), (vec![225, 195, 165, 135, 105, 75, 45, 15], QUICK));
        assert_eq!(lad(&plan(None, prof(0, true), 0, 495, 15, None)).0, vec![0]);
        // 1.0.4: another miner version saved it: checked, then the climb goes on upward from its offset (not from 0)
        assert_eq!(plan(None, prof(210, false), 0, 495, 15, None), Plan::Continue { from: 210 });
        // a crash in a check of the first tuning: the steps that had passed are the confirmed part
        assert_eq!(lad(&plan(crash(270, Some(255)), None, 0, 495, 15, None)).0[0], 240);
        assert_eq!(lad(&plan(crash(270, Some(225)), None, 0, 495, 15, None)).0[0], 225);
        // nothing confirmed (first step crashed, no profile), a bystander of another card's fault, nothing saved: the old climb
        assert_eq!(plan(crash(270, None), None, 0, 495, 15, None), Plan::Tune);
        assert_eq!(plan(crash(profiles::NO_FAIL, Some(150)), prof(150, true), 0, 495, 15, None), Plan::Tune);
        assert_eq!(plan(None, None, 0, 495, 15, None), Plan::Tune);
        // a profile without a usable offset
        assert_eq!(plan(None, Some(SavedProf { offset: -15, base: 0, same_kernel: true, lowered: true }), 0, 495, 15, None), Plan::Tune);
    }

    fn m(at: i32, w: f64, ghs: f64) -> Meas {
        Meas { at, w: Some(w), ghs, t: None }
    }

    #[test]
    fn a_step_is_taken_while_it_lowers_the_power_beyond_the_noise() {
        // RTX 2060 SUPER (a friend's rig, 18.11.2026, 1.0.3 stopped at +165): +195 -0.6 W and +225 -0.9 W are real savings
        let pl = Some(130.0);
        assert_eq!(step_verdict(&m(165, 89.2, 10.9), &m(195, 88.6, 10.9), pl), Verdict::Lower);
        assert_eq!(step_verdict(&m(195, 88.6, 10.9), &m(225, 87.7, 10.9), pl), Verdict::Lower);
        // within the noise, or a rise within the noise between checks (2 W): not lower
        assert_eq!(step_verdict(&m(225, 87.7, 10.9), &m(255, 87.4, 10.9), pl), Verdict::Flat);
        assert_eq!(step_verdict(&m(225, 87.7, 10.9), &m(255, 87.7, 10.9), pl), Verdict::Flat);
        assert_eq!(step_verdict(&m(30, 149.4, 12.5), &m(60, 151.4, 12.5), Some(200.0)), Verdict::Flat);
        // a clear rise: the 1.0.2 overclock (+0 64.4 W -> +60 87.8 W), or more than 3 W
        assert_eq!(step_verdict(&m(0, 64.4, 5.0), &m(30, 76.0, 5.0), pl), Verdict::Rose);
        assert_eq!(step_verdict(&m(225, 87.7, 10.9), &m(255, 91.0, 10.9), pl), Verdict::Rose);
        // no percentages: a big card saves the same 0.6 W per step as a small one
        assert_eq!(step_verdict(&m(150, 143.9, 20.0), &m(180, 143.3, 20.0), Some(250.0)), Verdict::Lower);
        // both at the power limit: the rate decides
        assert_eq!(step_verdict(&m(0, 129.0, 5.0), &m(30, 130.0, 5.2), pl), Verdict::Lower);
        assert_eq!(step_verdict(&m(0, 129.0, 5.0), &m(30, 130.0, 5.01), pl), Verdict::Flat);
        assert_eq!(step_verdict(&m(0, 129.0, 5.0), &m(30, 130.0, 4.8), pl), Verdict::Flat);
        // no power reading: taken as before
        assert_eq!(step_verdict(&Meas { at: 0, w: None, ghs: 0.0, t: None }, &m(30, 99.0, 1.0), pl), Verdict::Lower);
        // a warming card is measured once more before a step counts as not lowering
        let (a, b) = (Meas { t: Some(60), ..m(195, 88.6, 10.9) }, Meas { t: Some(62), ..m(225, 88.7, 10.9) });
        assert!(warmed(&a, &b));
        assert!(!warmed(&a, &Meas { t: Some(61), ..b }));
        assert!(!warmed(&m(195, 88.6, 10.9), &b));
    }

    /// The climb of the tuner (climb_from) on a simulated card: `w(offset)` the check power, `fail` the first failing offset.
    fn sim(from: i32, cap: i32, w: impl Fn(i32) -> f64, fail: i32) -> (i32, Vec<i32>) {
        let mut judge = Judge::new(m(from, w(from), 10.0));
        let mut stop = false;
        let mut tried = Vec::new();
        let s = climb::climb_ex(from, Some(w(from)), None, cap, 15, 30, false, |cand, _good, _, _| {
            tried.push(cand);
            if cand >= fail {
                return climb::StepOut { ok: false, ..Default::default() };
            }
            let now = m(cand, w(cand), 10.0);
            match judge.judge(now, Some(300.0)) {
                Decision::Stop(_) => {
                    stop = true;
                    climb::StepOut { ok: false, fault: true, w: now.w, ..Default::default() }
                }
                Decision::Take | Decision::Hold(_) => climb::StepOut { ok: true, w: now.w, ..Default::default() },
            }
        });
        let fin = if s.failed && !stop { (s.good - 15).max(from) } else { s.good };
        (fin.min(judge.best.at).max(from), tried)
    }

    #[test]
    fn the_climb_goes_on_while_every_step_lowers_the_power() {
        // the 2060 SUPER: +165 89.2, +195 88.6, +225 87.7, then flat: +225 (1.0.3: +165)
        let w2060 = |at: i32| match at {
            165 => 89.2,
            195 => 88.6,
            225 => 87.7,
            _ => 87.6,
        };
        assert_eq!(sim(165, 495, w2060, 999), (225, vec![195, 225, 255, 285, 315]));
        // no step lowers it (3 in a row within the noise): the start stays
        assert_eq!(sim(165, 495, |at| if at == 165 { 89.2 } else { 89.0 }, 999), (165, vec![195, 225, 255]));
        // the power rises clearly at +255: +225 at once
        assert_eq!(sim(165, 495, |at| if at <= 225 { w2060(at) } else { 92.0 }, 999), (225, vec![195, 225, 255]));
        // 1 W per step all the way to the edge: the failure decides (refined, one fine step below the last good), not a %
        assert_eq!(sim(0, 495, |at| 150.0 - at as f64 / 30.0, 300).0, 270);
        // --uv-max (cap) is a hard ceiling
        assert_eq!(sim(0, 120, |at| 150.0 - at as f64 / 30.0, 999).0, 120);
    }

    #[test]
    fn noisy_stairs_do_not_stop_the_climb() {
        // RTX 3070 @1650 (Shahtyor GPU1, 18.11.2026), a sweep of 15 s checks: +60 read 2 W above +30, and the power fell on to
        // +270 (129.4 W); +300 failed the check.  1.0.4 climbs to the edge: +270 (refined +285 in the noise)
        let sweep = |at: i32| match at {
            0 => 152.05,
            30 => 149.42,
            60 => 151.40,
            90 => 147.41,
            120 => 144.53,
            150 => 143.15,
            180 => 139.35,
            210 => 134.35,
            240 => 132.20,
            270 => 129.35,
            _ => 128.95,
        };
        assert_eq!(sim(0, 495, sweep, 300), (270, vec![30, 60, 90, 120, 150, 180, 210, 240, 270, 300, 285]));
        // the tuning run of the same card: +90 142.1, +120 142.6, +150 141.8 (both within the noise), +180 139.4: on to +180
        let mut j = Judge::new(m(90, 142.1, 12.5));
        assert_eq!(j.judge(m(120, 142.6, 12.5), None), Decision::Hold(1));
        assert_eq!(j.judge(m(150, 141.8, 12.5), None), Decision::Hold(2));
        assert_eq!(j.judge(m(180, 139.4, 12.5), None), Decision::Take);
        assert_eq!(j.best.at, 180);
        // three in a row that do not lower it: the power stopped falling, the result is the best (+180)
        assert_eq!(j.judge(m(210, 139.3, 12.5), None), Decision::Hold(1));
        assert_eq!(j.judge(m(240, 139.5, 12.5), None), Decision::Hold(2));
        assert_eq!(j.judge(m(270, 139.2, 12.5), None), Decision::Stop(Verdict::Flat));
        assert_eq!(j.best.at, 180);
        // a clear rise stops at once
        let mut j = Judge::new(m(90, 142.7, 12.5));
        assert_eq!(j.judge(m(120, 146.0, 12.5), None), Decision::Stop(Verdict::Rose));
        // a failure after steps that did not lower the power: the best, not the step below the failure
        let w = |at: i32| match at {
            0 => 158.9,
            30 => 150.8,
            60 => 148.1,
            90 => 142.1,
            120 => 142.6,
            _ => 142.4,
        };
        assert_eq!(sim(0, 495, w, 150).0, 90);
    }

    #[test]
    fn a_profile_of_an_earlier_version_is_tuned_on_upward() {
        // Rig05 GPU0 (CMP 50HX): 1.0.3 saved its base +150 (the +270 of 1.0.2 saved 1.7 %, under its 3 %): 1.0.4 checks +150
        // and climbs on from there, not from +0
        let old = Some(SavedProf { offset: 150, base: 150, same_kernel: false, lowered: true });
        assert_eq!(plan(None, old, 0, 495, 15, None), Plan::Continue { from: 150 });
        // the 2060 SUPER: 1.0.3 stopped early at +165
        assert_eq!(plan(None, Some(SavedProf { offset: 165, base: 165, same_kernel: false, lowered: true }), 165, 495, 15, None), Plan::Continue { from: 165 });
        // never above --uv-max
        assert_eq!(plan(None, Some(SavedProf { offset: 300, base: 0, same_kernel: false, lowered: true }), 0, 240, 15, None), Plan::Continue { from: 240 });
        // the climb from there: the steps of the 2060 SUPER
        assert_eq!(sim(165, 495, |at| [89.2, 88.6, 87.7, 87.6][((at - 165) / 30).clamp(0, 3) as usize], 999).0, 225);
        // this version's own profile: a quick check, no climb
        assert_eq!(lad(&plan(None, Some(SavedProf { offset: 225, base: 165, same_kernel: true, lowered: true }), 165, 495, 15, None)).0[0], 225);
        // a crash still wins: 2 steps below it ("the failed - 2 steps" rule), no climb
        assert_eq!(lad(&plan(crash(180, None), old, 0, 495, 15, Some(180))).0[0], 150);
    }

    #[test]
    fn a_saved_profile_that_raised_the_power_is_not_resumed() {
        let bad = Entry { kernel: "ffgn1.0.2/neb".into(), core_lock: 1400, base: 0, offset: 60, w_base: 64.4, w: 87.8, refs: "-".into(), ..Default::default() };
        assert!(!profile_lowered(&bad));
        let good = Entry { w: 58.0, ..bad.clone() };
        assert!(profile_lowered(&good));
        // 1.0.4: no minimum saving (Rig05 GPU0: +150 -> +270, 143.9 W -> 141.5 W, 1.7 %: usable)
        assert!(profile_lowered(&Entry { base: 150, offset: 270, w_base: 143.9, w: 141.5, ..bad.clone() }));
        // nor a power within the tolerance of its base (a longer, warmer confirmation)
        assert!(profile_lowered(&Entry { base: 150, offset: 270, w_base: 143.9, w: 144.5, ..bad.clone() }));
        let unknown = Entry { w: 0.0, ..bad.clone() };
        assert!(profile_lowered(&unknown));
        let p = Some(SavedProf { offset: 60, base: 0, same_kernel: false, lowered: false });
        assert_eq!(plan(None, p, 0, 495, 15, None), Plan::Tune);
        // the owner's rule stays: a crash while mining resumes 2 steps below (a usable profile)
        let p = Some(SavedProf { offset: 180, base: 0, same_kernel: true, lowered: true });
        assert_eq!(lad(&plan(crash(180, None), p, 0, 495, 15, Some(180))).0[0], 150);
    }

    #[test]
    fn the_clock_under_load_is_checked_against_the_lock() {
        // lock 1400: 1410 holds; 1470 at +60 = the offset overclocks (GeForce Turing); 1470 at +0 = the lock is off
        assert_eq!(clock_verdict(Some(1400), 60, 0, Some(1410)), ClockVerdict::Fine);
        assert_eq!(clock_verdict(Some(1400), 60, 0, Some(1445)), ClockVerdict::Fine);
        assert_eq!(clock_verdict(Some(1400), 60, 0, Some(1470)), ClockVerdict::OffsetRaises(1470));
        assert_eq!(clock_verdict(Some(1400), 0, 0, Some(1470)), ClockVerdict::LockOff(1470));
        // below the lock (power limit): fine; no lock / no reading: fine
        assert_eq!(clock_verdict(Some(1400), 60, 0, Some(1140)), ClockVerdict::Fine);
        assert_eq!(clock_verdict(None, 60, 0, Some(2055)), ClockVerdict::Fine);
        assert_eq!(clock_verdict(Some(1400), 60, 0, None), ClockVerdict::Fine);
        // the start offset of the card (HiveOS +150 on a CMP, +58 on a laptop): there the lock is at fault, not the offset
        assert_eq!(clock_verdict(Some(1650), 58, 58, Some(2280)), ClockVerdict::LockOff(2280));
        assert_eq!(clock_verdict(Some(1560), 330, 150, Some(1560)), ClockVerdict::Fine);
    }

    /// 1.0.10: the climb on a simulated MINING card (uvmine::sim): every step a paired window against the best offset, the
    /// blocks alternate between two models 7 W apart (Kepler / TESS) every 9 s, power noise +-0.5 W.  The real power falls
    /// 1.2 W per 15 MHz up to +300 and is flat above; +405 gives an Xid after 3 s.
    fn sim_mining(from: i32, cap: i32, coarse: i32, xid_at: i32) -> (i32, Vec<i32>, f64) {
        use super::uvmine::{self, sim::Card};
        let w = |o: i32, k: u64| 160.0 - 1.2 * (o.min(300) as f64 / 15.0) + if k == 2 { 7.0 } else { 0.0 };
        let mut card = Card::new(w);
        card.load = Box::new(|t| if (t / 9.0) as u64 % 2 == 0 { 1 } else { 2 });
        card.noise = 0.5;
        card.xid_at = Some((xid_at, 3.0));
        let cfg = uvmine::Cfg { noise_w: NOISE_W, ..Default::default() };
        let base = uvmine::window(&mut card, from, None, MINING_BASE as f64, &cfg);
        let mut judge = Judge::new(Meas { at: from, w: base.w, ghs: base.mas, t: base.t });
        let (mut stop, mut tried, mut mined) = (false, Vec::new(), 0.0f64);
        let s = climb::climb_ex(from, base.w, base.t, cap, 15, coarse, false, |cand, _good, _, _| {
            tried.push(cand);
            let o = uvmine::window(&mut card, cand, Some(judge.best.at), STEP_SECS as f64, &cfg);
            mined = mined.max(o.mas);
            let r = Res { ok: o.ok, fault: o.fault, mining: true, pair: o.pair, reference: Some(judge.best.at), w: o.w, ghs: o.mas, ..Default::default() };
            if !r.ok {
                return climb::StepOut { ok: false, fault: r.fault, ..Default::default() };
            }
            let mut now = Meas { at: cand, w: r.w, ghs: r.ghs, t: None };
            assert!(apply_pair(&mut judge, &mut now, &r), "paired");
            match judge.judge(now, None) {
                Decision::Stop(_) => {
                    stop = true;
                    climb::StepOut { ok: false, fault: true, w: now.w, ..Default::default() }
                }
                _ => climb::StepOut { ok: true, w: now.w, ..Default::default() },
            }
        });
        let fin = if s.failed && !stop { (s.good - 15).max(from) } else { s.good };
        (fin.min(judge.best.at).max(from), tried, mined)
    }

    #[test]
    fn the_climb_on_the_mining_load_pairs_each_step_and_keeps_the_rules() {
        // coarse +60 from +0: the power falls 4.8 W per step to +300, then +360 is flat: 3 flat steps stop it at +300
        let (fin, tried, mined) = sim_mining(0, 495, 60, 999);
        assert_eq!(fin, 300, "{tried:?}");
        assert_eq!(tried, vec![60, 120, 180, 240, 300, 360, 420, 480]);
        assert!(mined > 199.0, "the card mined and was counted in every step: {mined}");
        // an Xid at +405: a driver fault on the coarse step +420 is not refined; one fine step below the last good... but the
        // power stopped falling above +300 anyway: the result is the last step that lowered the power
        let (fin, tried, _) = sim_mining(240, 495, 60, 405);
        assert_eq!(fin, 300, "{tried:?}");
        // the rule "climb while each step lowers the power by more than 0.5 W" with +15 fine steps from +255: every step -1.2 W
        let (fin, _, _) = sim_mining(255, 300, 15, 999);
        assert_eq!(fin, 300);
    }

    #[test]
    fn the_absolute_power_of_alternating_models_would_mislead_the_climb() {
        // why the pairing: the same card judged on absolute window means (models 7 W apart, 9 s blocks) does not end at +300
        use super::uvmine::{self, sim::Card};
        let w = |o: i32, k: u64| 160.0 - 1.2 * (o.min(300) as f64 / 15.0) + if k == 2 { 7.0 } else { 0.0 };
        let mut card = Card::new(w);
        card.load = Box::new(|t| if (t / 9.0) as u64 % 2 == 0 { 1 } else { 2 });
        let cfg = uvmine::Cfg::default();
        let mut diffs = Vec::new();
        let mut prev = uvmine::window(&mut card, 0, None, 25.0, &cfg).w.unwrap();
        for at in [60, 120, 180] {
            let o = uvmine::window(&mut card, at, None, 25.0, &cfg);
            diffs.push(o.w.unwrap() - prev);
            prev = o.w.unwrap();
        }
        // the true change is -4.8 W per step; the absolute means are off by up to several W
        assert!(diffs.iter().any(|d| (d + 4.8).abs() > 1.0), "{diffs:?}");
        let mut card = Card::new(w);
        card.load = Box::new(|t| if (t / 9.0) as u64 % 2 == 0 { 1 } else { 2 });
        let o = uvmine::window(&mut card, 60, Some(0), 25.0, &cfg);
        let p = o.pair.unwrap();
        assert!((p.wb - p.wa + 4.8).abs() < 0.3, "{p:?}");
    }

    #[test]
    fn the_state_is_kept_per_core_lock() {
        let k = |at: i32, good: Option<i32>| profiles::Crash { at, good };
        // a marker of this lock, or of a version before 1.0.10 (no lock in it): counts here
        assert_eq!(lock_split(Some(k(395, Some(365))), Some(1650), Some(1650)), LockSplit::Here(Some(k(395, Some(365)))));
        assert_eq!(lock_split(Some(k(395, None)), None, Some(1500)), LockSplit::Here(Some(k(395, None))));
        // a failure at another lock: kept for that lock, not counted at this one
        assert_eq!(lock_split(Some(k(395, Some(365))), Some(1650), Some(1500)), LockSplit::Other(1650, k(395, Some(365))));
        // a bystander marker of another lock: nothing of it here (its passed steps are of the other lock)
        assert_eq!(lock_split(Some(k(profiles::NO_FAIL, Some(300))), Some(1650), Some(1500)), LockSplit::Here(None));
        // kept in neb-uv-locks.json until that lock comes back (the lower failure wins)
        let d = std::env::temp_dir().join(format!("ffgn-locks-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&d);
        let _ = std::fs::remove_file(pending_file(&d));
        pending_put(&d, "U", 1650, k(395, Some(365)));
        pending_put(&d, "U", 1650, k(425, Some(395)));
        assert_eq!(pending_take(&d, "U", 1500), None);
        assert_eq!(pending_take(&d, "U", 1650), Some(k(395, Some(365))));
        assert_eq!(pending_take(&d, "U", 1650), None);
        let _ = std::fs::remove_dir_all(&d);
        // and plan() then resumes 2 steps below it (not above the confirmed one)
        let p = plan(Some(k(395, Some(365))), None, 0, 495, 15, None);
        assert_eq!(lad(&p).0.first().copied(), Some(365));
        // a new lower lock starts 2 steps below the profile of the nearest higher lock; a higher lock gets no hint
        let e = |lock: u32, off: i32| {
            (format!("{lock}"), Entry { core_lock: lock, offset: off, base: 0, refs: "mining".into(), kernel: kernel_id(), verified_at: 1, ..Default::default() })
        };
        let profs = vec![e(1650, 300), e(1800, 240)];
        assert_eq!(lock_hint(&profs, 1500, 0, 495, 30, 10).map(|x| x.0), Some(270));
        assert_eq!(lock_hint(&profs, 1700, 0, 495, 30, 10).map(|x| x.0), Some(210));
        assert_eq!(lock_hint(&profs, 1900, 0, 495, 30, 10), None);
        // never above --uv-max, never below the start
        assert_eq!(lock_hint(&profs, 1500, 0, 200, 30, 10).map(|x| x.0), Some(200));
        assert_eq!(lock_hint(&profs, 1500, 280, 495, 30, 10), None);
    }
}
