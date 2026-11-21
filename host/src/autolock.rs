// FreeForGem — see LICENSE
//! `--auto-lock` (1.7.8; off unless asked): the core clock lock of each NVIDIA GPU chosen by measurement.  Ported from our SASEUL
//! miner ffgs 0.5.4 (src/autolock.h, same option names and the same choice rule), adapted to the four coins of FreeForGem.
//!
//! Per GPU a set of locks is tried, highest first (default 1800 .. 1200 step 100, nothing above the card's maximum graphics
//! clock; `--auto-lock-range`).  At each lock auto-uv finds / reuses the undervolt profile of that lock (one profile per lock in
//! uv.json, as always), then the GPU MINES at it: a settle time, then a measured time (hash rate from the miner's own stats line,
//! NVML watts and the median core clock).  Shares found meanwhile go to the pool as usual.  Chosen: the best efficiency (TH/W,
//! MH/W, GH/W) among the locks within `--auto-lock-max-loss` % (10) of the highest hash rate measured (`--auto-lock-goal eff`),
//! or the highest hash rate (`speed`).  Locks below a point that is already below that cut are not measured.
//!
//! How (FreeForGem has four coins with four tuners; none of them is changed): the miner started with --auto-lock is the
//! supervisor.  It runs each round of the sweep as a child `freeforgem` with the same arguments and a per-GPU --lock-cclock (the
//! GPU under test at its next lock, the other GPUs at their chosen / own lock), forwards the child's log unchanged (HiveOS
//! h-stats reads it), reads the per-GPU hash rate and the auto-uv tags from the periodic stats line (the format h-stats already
//! depends on) and samples watts and clocks through NVML itself.  A round ends when every GPU in it has its point; the child is
//! asked to stop (a stop file it watches: the clocks and offsets go back as on any stop), the next round starts.  When every GPU
//! has its lock, this process mines at them (`--lock-cclock` per GPU) as if they had been given on the command line.
//!
//! Saved per GPU (coin / mode, PCI bus, board, driver, memory lock, mining kernel) in `autolock.json` next to uv.json: the next
//! start applies the choice at once, no child, no sweep.  A changed goal / max loss is re-chosen from the saved points; a wider
//! range measures only the new locks; `--auto-lock-reset` forgets the points (the auto-uv profiles stay).  A new driver, another
//! card or kernel = a new key = a new sweep.  If the miner dies while a lock is measured (Xid, freeze, power), the next start marks
//! that lock `crashed` and never tries it again (until --auto-lock-reset).
//!
//! `--auto-lock-goal profit` (profit.rs): the most US$ per day (income - electricity, --power-price) among the measured locks;
//! every lock of the range is measured (default 2000..1200), the price comes from public tickers (or --coin-price), the network
//! data from the pool's jobs (Pearl, NOID: the round's / mining process prints `auto-lock: network ...`) or a public API (PROX;
//! Pearl without a height).  With --auto-lock-reprice-hours (6) this process then supervises the mining process and re-chooses
//! from the measured points every N hours (a switch needs > --auto-lock-hysteresis %, 3).
//!
//! Without --auto-lock nothing of this runs (the hook in each coin returns at once): 1.7.8 behaves exactly as before.
use crate::nvml::{Dev, Nvml};
use crate::profit::{Coin, Econ, Net, Tariff};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::time::{Duration, Instant};

const CHILD_ENV: &str = "FFG_AUTOLOCK_CHILD";
const STOP_ENV: &str = "FFG_AUTOLOCK_STOP";
const PARENT_ENV: &str = "FFG_AUTOLOCK_PARENT";
const NET_ENV: &str = "FFG_AUTOLOCK_NET"; // goal profit: the child reports the network data of its pool's jobs
const FILE: &str = "autolock.json";
const MAX_LOCKS: usize = 24;

fn lg(m: impl AsRef<str>) {
    println!("auto-lock: {}", m.as_ref());
}

// ---------------------------------------------------------------------------------------------------------------------
// options
// ---------------------------------------------------------------------------------------------------------------------

/// --auto-lock-goal
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Goal {
    #[default]
    Eff,
    Speed,
    Profit,
}

impl Goal {
    pub fn name(self) -> &'static str {
        match self {
            Goal::Eff => "eff",
            Goal::Speed => "speed",
            Goal::Profit => "profit",
        }
    }
    pub fn from_name(s: &str) -> Option<Goal> {
        match s.trim().to_ascii_lowercase().as_str() {
            "eff" | "efficiency" => Some(Goal::Eff),
            "speed" | "hashrate" => Some(Goal::Speed),
            "profit" => Some(Goal::Profit),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Opts {
    pub on: bool,
    pub reset: bool,
    pub goal: Goal,
    pub tariff: Option<Tariff>,  // --power-price ($/kWh; day,night@FROM-TO)
    pub coin_price: Option<f64>, // --coin-price (US$; no exchange is asked)
    pub pool_fee: Option<f64>,   // --pool-fee %
    pub reprice_h: f64,          // --auto-lock-reprice-hours (6; 0 = never while mining)
    pub hyst: f64,               // --auto-lock-hysteresis % (3)
    pub sub: bool,      // some --auto-lock-* option given (a note when --auto-lock is not)
    pub list: Vec<u32>, // --auto-lock-range, highest first; empty = the defaults of the card
    pub loss: f64,      // --auto-lock-max-loss, %
    pub settle: u32,    // seconds of mining before the measurement (FFG_AL_TIMES=settle,measure[,max_wait] for tests)
    pub measure: u32,   // seconds measured
    pub max_wait: u32,  // seconds a GPU may take to mine steadily at a new lock (auto-uv tuning of that lock included)
    pub add_uv: bool,   // --auto-uv was turned on by --auto-lock (the child processes need it on their command line)
    pub deadline: Option<Instant>, // --minutes N counted from the start of this process: the sweep stops there too
}

impl Default for Opts {
    fn default() -> Opts {
        Opts {
            on: false,
            reset: false,
            goal: Goal::Eff,
            tariff: None,
            coin_price: None,
            pool_fee: None,
            reprice_h: 6.0,
            hyst: 3.0,
            sub: false,
            list: Vec::new(),
            loss: 10.0,
            settle: 45,
            measure: 120,
            max_wait: 3600,
            add_uv: false,
            deadline: None,
        }
    }
}

static OPTS: OnceLock<Opts> = OnceLock::new();
static ARGV: OnceLock<Vec<String>> = OnceLock::new();
static START: OnceLock<Instant> = OnceLock::new();

/// main(): the options and the command line (list values joined) the child processes of the sweep are built from.
pub fn set(mut o: Opts, argv: Vec<String>) {
    let _ = START.set(Instant::now());
    o.deadline = minutes_arg(&argv).and_then(|m| Duration::try_from_secs_f64(m * 60.0).ok()).map(|d| Instant::now() + d);
    let _ = OPTS.set(o);
    let _ = ARGV.set(argv);
}

/// The value of --minutes on the command line (the sweep's child processes do not get it: this process keeps the time).
pub fn minutes_arg(argv: &[String]) -> Option<f64> {
    argv.iter().position(|a| a == "--minutes").and_then(|i| argv.get(i + 1)).and_then(|v| v.parse::<f64>().ok()).filter(|m| *m > 0.0)
}

/// --minutes counts from the start of the process, the sweep included: what is left for the mining that follows a sweep
/// (at least 1 s so that the miner's own limit still ends the run cleanly).
pub fn remaining_minutes(m: Option<f64>) -> Option<f64> {
    let spent = START.get().map_or(0.0, |t| t.elapsed().as_secs_f64() / 60.0);
    m.map(|m| (m - spent).max(1.0 / 60.0))
}

fn time_up(deadline: Option<Instant>) -> bool {
    deadline.is_some_and(|d| Instant::now() >= d)
}

fn opts() -> Opts {
    OPTS.get().cloned().unwrap_or_default()
}

/// --auto-lock given (this process is the supervisor of a sweep or applies the saved locks)
pub fn enabled() -> bool {
    OPTS.get().is_some_and(|o| o.on)
}

/// This process mines one round of a sweep for its parent.
pub fn is_child() -> bool {
    std::env::var_os(CHILD_ENV).is_some()
}

/// The coin's hook has something to do (else it is not called at all: behaviour as without --auto-lock).
pub fn active() -> bool {
    enabled() || is_child()
}

/// "1200-1700:100", "1700-1200" (step 100), "1300,1450,1600" -> locks, highest first.
pub fn parse_range(s: &str) -> Result<Vec<u32>, String> {
    let bad = || format!("--auto-lock-range: '{s}' is not FROM-TO[:STEP] (1200-1700:100) or a list (1300,1450,1600)");
    let mut v: Vec<i64> = Vec::new();
    let t = s.trim();
    if t.contains('-') {
        let (range, step) = match t.split_once(':') {
            Some((r, st)) => (r, st.trim().parse::<i64>().map_err(|_| bad())?),
            None => (t, 100),
        };
        let (a, b) = range.split_once('-').ok_or_else(bad)?;
        let (mut a, mut b) = (a.trim().parse::<i64>().map_err(|_| bad())?, b.trim().parse::<i64>().map_err(|_| bad())?);
        if !(15..=1000).contains(&step) {
            return Err(format!("--auto-lock-range: step {step} MHz, allowed 15..1000"));
        }
        if a > b {
            std::mem::swap(&mut a, &mut b);
        }
        if b - a > step * MAX_LOCKS as i64 * 2 {
            return Err(format!("--auto-lock-range: more than {MAX_LOCKS} locks, at most {MAX_LOCKS} (a larger step)"));
        }
        let mut l = b;
        while l >= a {
            v.push(l);
            l -= step;
        }
    } else {
        for x in t.split(',') {
            v.push(x.trim().parse::<i64>().map_err(|_| bad())?);
        }
    }
    if let Some(l) = v.iter().find(|&&l| !(300..=4000).contains(&l)) {
        return Err(format!("--auto-lock-range: {l} MHz is outside 300..4000"));
    }
    let mut v: Vec<u32> = v.into_iter().map(|l| l as u32).collect();
    v.sort_unstable_by(|a, b| b.cmp(a));
    v.dedup();
    if v.is_empty() {
        return Err(bad());
    }
    if v.len() > MAX_LOCKS {
        return Err(format!("--auto-lock-range: {} locks, at most {MAX_LOCKS} (a larger step)", v.len()));
    }
    Ok(v)
}

/// main(): one --auto-lock* argument; Ok(false) = not ours.
pub fn arg(o: &mut Opts, a: &str, val: &mut dyn FnMut() -> anyhow::Result<String>) -> anyhow::Result<bool> {
    match a {
        "--auto-lock" => o.on = true,
        "--auto-lock-reset" => {
            o.reset = true;
            o.sub = true;
        }
        "--auto-lock-range" => {
            o.list = parse_range(&val()?).map_err(anyhow::Error::msg)?;
            o.sub = true;
        }
        "--auto-lock-max-loss" => {
            let v = val()?;
            o.loss = v.trim().trim_end_matches('%').parse::<f64>().ok().filter(|x| (0.0..=90.0).contains(x)).ok_or_else(|| anyhow::anyhow!("--auto-lock-max-loss: '{v}' is not a percentage 0..90"))?;
            o.sub = true;
        }
        "--auto-lock-goal" => {
            let v = val()?;
            o.goal = Goal::from_name(&v).ok_or_else(|| anyhow::anyhow!("--auto-lock-goal must be eff, speed or profit"))?;
            o.sub = true;
        }
        "--power-price" => {
            o.tariff = Some(Tariff::parse(&val()?).map_err(anyhow::Error::msg)?);
            o.sub = true;
        }
        "--coin-price" => {
            let v = val()?;
            o.coin_price = Some(v.trim().trim_start_matches('$').parse::<f64>().ok().filter(|x| x.is_finite() && *x > 0.0).ok_or_else(|| anyhow::anyhow!("--coin-price: '{v}' is not a price in US$ above 0"))?);
            o.sub = true;
        }
        "--pool-fee" => {
            let v = val()?;
            o.pool_fee = Some(v.trim().trim_end_matches('%').parse::<f64>().ok().filter(|x| (0.0..=50.0).contains(x)).ok_or_else(|| anyhow::anyhow!("--pool-fee: '{v}' is not a percentage 0..50"))?);
            o.sub = true;
        }
        "--auto-lock-reprice-hours" => {
            let v = val()?;
            o.reprice_h = v.trim().parse::<f64>().ok().filter(|x| (0.0..=720.0).contains(x)).ok_or_else(|| anyhow::anyhow!("--auto-lock-reprice-hours: '{v}' is not 0..720 (0 = never)"))?;
            o.sub = true;
        }
        "--auto-lock-hysteresis" => {
            let v = val()?;
            o.hyst = v.trim().trim_end_matches('%').parse::<f64>().ok().filter(|x| (0.0..=50.0).contains(x)).ok_or_else(|| anyhow::anyhow!("--auto-lock-hysteresis: '{v}' is not a percentage 0..50"))?;
            o.sub = true;
        }
        _ => return Ok(false),
    }
    Ok(true)
}

/// FFG_AL_TIMES=settle,measure[,max_wait] (seconds; tests and short live checks)
pub fn times_from_env(o: &mut Opts) {
    if let Ok(t) = std::env::var("FFG_AL_TIMES") {
        let v: Vec<u32> = t.split(',').filter_map(|x| x.trim().parse().ok()).collect();
        if let Some(&s) = v.first() {
            o.settle = s;
        }
        if let Some(&m) = v.get(1) {
            o.measure = m.max(10);
        }
        if let Some(&w) = v.get(2) {
            o.max_wait = w.max(60);
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// points and the choice (no GPU: unit-tested)
// ---------------------------------------------------------------------------------------------------------------------

pub const OK: i32 = 0;
pub const THROTTLED: i32 = 1;
pub const UNSTABLE: i32 = 2;
pub const FAILED: i32 = 3;
pub const CRASHED: i32 = 4;
pub const SKIPPED: i32 = 5;
pub const NOT_HELD: i32 = 6;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Point {
    pub lock: u32,
    pub offset: i32,
    pub rate: f64, // the coin's unit (Rec::unit)
    pub w: f64,
    pub clk: u32,
    pub state: i32,
}

impl Point {
    pub fn eff(&self) -> f64 {
        if self.w > 0.0 { self.rate / self.w } else { 0.0 }
    }
    /// the efficiency in the unit shown (eff_unit; the same number as eff: rate per W)
    pub fn eff_shown(&self, _unit: &str) -> f64 {
        self.eff()
    }
}

/// One GPU (key = coin|bus|board|driver|mem<lock>|kernel).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rec {
    pub key: String,
    pub unit: String, // TH/s, MH/s, GH/s
    pub range: Vec<u32>,
    pub goal: Goal,
    pub loss: f64,
    pub choice: u32,
    pub done: bool,
    pub unsupported: bool,
    /// 1.8.0 (FFGN 1.0.4): the core lock does not hold on this GPU (the clock runs above it): auto-lock is off for it, like `unsupported`
    pub unheld: bool,
    pub trying: u32,
    pub time: u64,
    pub pts: Vec<Point>, // highest lock first
}

impl Rec {
    pub fn at(&self, lock: u32) -> Option<&Point> {
        self.pts.iter().find(|p| p.lock == lock)
    }
    pub fn put(&mut self, p: Point) {
        match self.pts.iter_mut().find(|x| x.lock == p.lock) {
            Some(x) => *x = p,
            None => self.pts.push(p),
        }
        self.pts.sort_by(|a, b| b.lock.cmp(&a.lock));
    }
    /// The previous run died while this lock was measured / tuned: it counts as crashed, never tried again (--auto-lock-reset).
    pub fn recover_trying(&mut self) -> Option<u32> {
        let t = self.trying;
        if t == 0 {
            return None;
        }
        self.put(Point { lock: t, state: CRASHED, ..Default::default() });
        self.trying = 0;
        Some(t)
    }
}

fn top_rate(r: &Rec, cands: &[u32]) -> f64 {
    r.pts.iter().filter(|p| p.state == OK && cands.contains(&p.lock)).map(|p| p.rate).fold(0.0, f64::max)
}

fn cut(top: f64, loss: f64) -> f64 {
    top * (1.0 - loss / 100.0) - 1e-9
}

/// A lock below a measured point that is already below the cut: a lower clock only gives a lower hash rate, not measured.
pub fn below_cut(r: &Rec, cands: &[u32], lock: u32, loss: f64) -> bool {
    let c = cut(top_rate(r, cands), loss);
    r.pts.iter().any(|p| p.state == OK && cands.contains(&p.lock) && p.lock > lock && p.rate < c)
}

/// The next lock to measure (None = none): highest first, not measured / failed yet, not below the cut.
pub fn next_lock(r: &Rec, cands: &[u32], loss: f64) -> Option<u32> {
    cands.iter().copied().find(|&l| r.at(l).is_none_or(|p| p.state == SKIPPED) && !below_cut(r, cands, l, loss))
}

/// The loss that cuts the sweep: profit measures every lock of the range (a dearer or cheaper coin moves the best one).
pub fn cut_loss(o: &Opts) -> f64 {
    if o.goal == Goal::Profit { 100.0 } else { o.loss }
}

/// profit: the most US$ per day (income - electricity) among the valid points (equal: the better efficiency).  0 = none.
pub fn choose_profit(r: &Rec, cands: &[u32], e: &Econ) -> u32 {
    let mut best: Option<(&Point, f64)> = None;
    for p in r.pts.iter().filter(|p| p.state == OK && cands.contains(&p.lock) && p.rate > 0.0) {
        let pr = e.profit(p.rate, &r.unit, p.w);
        let better = match best {
            None => true,
            Some((b, bp)) => pr > bp + 1e-9 || ((pr - bp).abs() <= 1e-9 && p.eff() > b.eff()),
        };
        if better {
            best = Some((p, pr));
        }
    }
    best.map_or(0, |(p, _)| p.lock)
}

/// eff: the best efficiency among the valid points within `loss` % of the top hash rate (equal efficiency: more hash rate);
/// speed: the top hash rate (equal: the better efficiency); profit: choose_profit (without the money data: as eff).
/// 0 = no valid point.
pub fn choose(r: &Rec, cands: &[u32], loss: f64, goal: Goal, econ: Option<&Econ>) -> u32 {
    if goal == Goal::Profit && !r.unit.is_empty() {
        if let Some(e) = econ {
            return choose_profit(r, cands, e);
        }
    }
    let speed = goal == Goal::Speed;
    let c = cut(top_rate(r, cands), loss);
    let mut best: Option<&Point> = None;
    for p in r.pts.iter().filter(|p| p.state == OK && cands.contains(&p.lock) && p.rate >= c && p.rate > 0.0) {
        let better = match best {
            None => true,
            Some(b) if speed => p.rate > b.rate + 1e-9 || ((p.rate - b.rate).abs() <= 1e-9 && p.eff() > b.eff()),
            Some(b) => p.eff() > b.eff() + 1e-12 || ((p.eff() - b.eff()).abs() <= 1e-12 && p.rate > b.rate),
        };
        if better {
            best = Some(p);
        }
    }
    best.map_or(0, |p| p.lock)
}

/// The default locks: 1800 .. 1200 step 100, nothing above the card's maximum graphics clock (`maxclk`, 0 = unknown); a card that
/// cannot reach 1600 gets the 5 locks below its maximum.
pub fn defaults(maxclk: u32) -> Vec<u32> {
    defaults_for(maxclk, "", Goal::Eff)
}

/// Cards whose top lock is known to run into their power limit get a lower default top.
/// - Mining-only cards (CMP 30HX .. 170HX): 1500 MHz.  The first step at 2000 MHz on a CMP 50HX ran into the limit and the rig drew
///   894 W (Rig05, 4 cards); 4x CMP 50HX: 1300 = 67 TH/s, 1500 = 77 TH/s Pearl at 166-176 W.
/// - GeForce RTX 30 (and the Ampere RTX A workstation cards, the same chips): 1800 MHz.  On an RTX 3080 Ti (350 W limit) the steps at
///   2000 and 1900 MHz sat at 341 W, 150-170 MHz under their lock (about 9 min of tuning each, wasted); the recommended locks of the
///   30 series are 1560-1650 (MANUAL section 7), so 1800 still leaves room above them.
/// None = no card-specific top (the general default of the goal).  An explicit --auto-lock-range always wins.
pub fn safe_top(name: &str) -> Option<u32> {
    let n = name.to_ascii_uppercase();
    if n.contains("CMP") {
        return Some(1500);
    }
    let after = |key: &str| n.find(key).map(|i| n[i + key.len()..].chars().next());
    // "RTX 3080 Ti", "RTX 3070 Laptop GPU", "RTX A4000" (not "RTX 4090" / "RTX 5090")
    let rtx30 = after("RTX 30").flatten().is_some_and(|c| c.is_ascii_digit());
    let rtx_a = after("RTX A").flatten().is_some_and(|c| c.is_ascii_digit());
    (rtx30 || rtx_a).then_some(1800)
}

/// The default locks of a card: the goal's default top (1800 efficiency / 2000 profit), capped by the card's maximum clock and by
/// the safe top of its type; 5 locks, step 100, down to 1200 (or 400 below the top for a card that cannot reach 1600).
pub fn defaults_for(maxclk: u32, name: &str, goal: Goal) -> Vec<u32> {
    let general = if goal == Goal::Profit { 2000 } else { 1800 };
    let general = safe_top(name).map_or(general, |t| general.min(t));
    let top = if maxclk > 0 { general.min(maxclk / 100 * 100) } else { general };
    let bot = 1200.min(top.saturating_sub(400));
    let mut v = Vec::new();
    let mut l = top;
    while l >= bot && l >= 300 {
        v.push(l);
        l -= 100;
    }
    v
}

/// The default locks of goal profit: 2000 .. 1200 step 100, nothing above the card's maximum (a cheap tariff can make a high,
/// less efficient lock the most profitable one: it is measured, not guessed).
pub fn defaults_profit(maxclk: u32) -> Vec<u32> {
    defaults_for(maxclk, "", Goal::Profit)
}

/// The locks of this GPU: --auto-lock-range without the ones above `maxclk` (returned second), or the defaults of the goal.
pub fn candidates(list: &[u32], maxclk: u32, goal: Goal) -> (Vec<u32>, Vec<u32>) {
    candidates_for(list, maxclk, goal, "")
}

/// Like `candidates`, with the card's name for its safe default top (an explicit --auto-lock-range always wins).
pub fn candidates_for(list: &[u32], maxclk: u32, goal: Goal, name: &str) -> (Vec<u32>, Vec<u32>) {
    if list.is_empty() {
        return (defaults_for(maxclk, name, goal), Vec::new());
    }
    list.iter().partition(|&&l| maxclk == 0 || l <= maxclk)
}

fn join(v: &[u32]) -> String {
    v.iter().map(u32::to_string).collect::<Vec<_>>().join(",")
}

fn state_text(s: i32) -> &'static str {
    match s {
        THROTTLED => "not valid: the clock did not hold the lock (power / thermal limit)",
        UNSTABLE => "not valid: unsteady mining (auto-uv re-check, temperature limit, no job)",
        FAILED => "FAILED: lock refused / the GPU stopped / no steady mining",
        CRASHED => "crashed: the previous run died at this lock",
        SKIPPED => "skipped: below the cut, not measured",
        NOT_HELD => "not valid: the clock ran above the lock (the lock was reset by another tool / OC apply)",
        _ => "",
    }
}

/// Efficiency unit of a hash rate unit: TH/W (Pearl), MH/W (NOID), GH/W (PROX).
pub fn eff_unit(unit: &str) -> String {
    match unit {
        "GH/s" => "GH/W".into(),
        u => u.replace("/s", "/W"),
    }
}

/// The result table for the log: one line per lock of the sweep, the chosen one marked; with the money data (goal profit)
/// income, electricity and profit in US$ per day.
pub fn table(r: &Rec, cands: &[u32], choice: u32, econ: Option<&Econ>) -> Vec<String> {
    let (u, e) = (if r.unit.is_empty() { "rate".to_string() } else { r.unit.clone() }, eff_unit(&r.unit));
    let econ = econ.filter(|_| !r.unit.is_empty());
    let money = |p: &Point| econ.map_or(String::new(), |m| format!("  {:7.3}  {:6.3}  {:7.3}", m.income(p.rate, &r.unit), m.elec(p.w), m.profit(p.rate, &r.unit, p.w)));
    let mhead = if econ.is_some() { format!("  {:>7}  {:>6}  {:>7} $/day", "income", "power", "profit") } else { String::new() };
    let mut out = vec![format!("  lock  offset  {u:>8}       W  {e:>8}{mhead}")];
    for &l in cands {
        let Some(p) = r.at(l) else {
            out.push(format!("  {l:4}       -         -       -         -  not measured"));
            continue;
        };
        let off = if p.state == SKIPPED || p.state == CRASHED || (p.state == FAILED && p.rate == 0.0) { "-".to_string() } else { crate::uv::signed(p.offset) };
        if p.state == OK || p.state == THROTTLED || p.state == UNSTABLE || p.state == NOT_HELD {
            out.push(format!(
                "  {l:4}  {off:>6}  {:8.3}  {:6.1}  {:8.4}{}{}{}",
                p.rate,
                p.w,
                p.eff_shown(&r.unit),
                if p.state == OK { money(p) } else { String::new() },
                if l == choice { "  \u{2605} chosen" } else { "" },
                if p.state != OK { format!("  {}", state_text(p.state)) } else { String::new() }
            ));
        } else {
            out.push(format!("  {l:4}  {off:>6}         -       -         -  {}", state_text(p.state)));
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------------------------------
// the saved results (autolock.json next to uv.json)
// ---------------------------------------------------------------------------------------------------------------------

pub fn db_path(dir: &Path) -> PathBuf {
    dir.join(FILE)
}

fn rec_json(r: &Rec) -> Value {
    json!({
        "unit": r.unit, "range": r.range, "goal": r.goal.name(), "loss": r.loss, "choice": r.choice,
        "done": r.done, "unsupported": r.unsupported, "unheld": r.unheld, "trying": r.trying, "time": r.time,
        "pts": r.pts.iter().map(|p| json!({"lock": p.lock, "offset": p.offset, "rate": p.rate, "w": p.w, "clk": p.clk, "state": p.state})).collect::<Vec<_>>(),
    })
}

fn rec_from(key: &str, v: &Value) -> Rec {
    let u = |k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(0);
    let mut r = Rec {
        key: key.to_string(),
        unit: v.get("unit").and_then(Value::as_str).unwrap_or("").to_string(),
        range: v.get("range").and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_u64().map(|x| x as u32)).collect()).unwrap_or_default(),
        goal: v.get("goal").and_then(Value::as_str).and_then(Goal::from_name).unwrap_or_default(),
        loss: v.get("loss").and_then(Value::as_f64).unwrap_or(10.0),
        choice: u("choice") as u32,
        done: v.get("done").and_then(Value::as_bool).unwrap_or(false),
        unsupported: v.get("unsupported").and_then(Value::as_bool).unwrap_or(false),
        unheld: v.get("unheld").and_then(Value::as_bool).unwrap_or(false),
        trying: u("trying") as u32,
        time: u("time"),
        pts: Vec::new(),
    };
    for p in v.get("pts").and_then(Value::as_array).into_iter().flatten() {
        let f = |k: &str| p.get(k).and_then(Value::as_f64).unwrap_or(0.0);
        let lock = p.get("lock").and_then(Value::as_u64).unwrap_or(0) as u32;
        if lock > 0 {
            r.put(Point { lock, offset: f("offset") as i32, rate: f("rate"), w: f("w"), clk: f("clk") as u32, state: p.get("state").and_then(Value::as_i64).unwrap_or(FAILED as i64) as i32 });
        }
    }
    r
}

pub fn load_db(path: &Path) -> BTreeMap<String, Rec> {
    let mut db = BTreeMap::new();
    let Ok(s) = std::fs::read_to_string(path) else { return db };
    let Ok(v) = serde_json::from_str::<Value>(&s) else {
        lg(format!("{} is not valid JSON: ignored (a new sweep; the file is replaced when a point is saved)", path.display()));
        return db;
    };
    if let Some(g) = v.get("gpus").and_then(Value::as_object) {
        for (k, x) in g {
            db.insert(k.clone(), rec_from(k, x));
        }
    }
    db
}

/// Writes one record into the file (the others are kept as they are: other GPUs, coins, drivers).
pub fn save_rec(path: &Path, r: &Rec) {
    let _f = crate::uv::FileLock::take(path); // 1.8.0: per-group processes share the file
    let mut doc = std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()).filter(Value::is_object).unwrap_or_else(|| json!({}));
    doc["version"] = json!(1);
    if !doc.get("gpus").is_some_and(Value::is_object) {
        doc["gpus"] = json!({});
    }
    doc["gpus"][&r.key] = rec_json(r);
    if let Some(d) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        let _ = std::fs::create_dir_all(d); // a fresh state directory (--uv-dir / $FFGP_STATE_DIR not made yet)
    }
    let tmp = path.with_extension(format!("json.tmp{}", std::process::id()));
    let ok = std::fs::write(&tmp, serde_json::to_string_pretty(&doc).unwrap_or_default()).and_then(|_| std::fs::rename(&tmp, path));
    if let Err(e) = ok {
        lg(format!("cannot write {} ({e}): the sweep goes on, but its points are not kept", path.display()));
    }
}

pub fn make_key(coin: &str, bus: &str, board: &str, driver: &str, mem: Option<u32>, kernel: &str) -> String {
    let clean = |s: &str| s.trim().replace(['|', ' '], "_");
    format!("{}|{}|{}|{}|mem{}|{}", coin, bus.to_ascii_lowercase(), clean(board), clean(driver), mem.unwrap_or(0), clean(kernel))
}

fn now_s() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

// ---------------------------------------------------------------------------------------------------------------------
// the child's log: the stats line and the clock lines
// ---------------------------------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub dev: i32,
    pub rate: f64,
    pub w: Option<f64>,
    pub dead: bool,
    pub tagged: bool, // " [tuning ...]", " [check ...]", a temperature-limit tag: not steady mining at the profile
}

fn strip_ansi(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\x1b' {
            if it.peek() == Some(&'[') {
                it.next();
                for d in it.by_ref() {
                    if d.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        o.push(c);
    }
    o
}

/// The periodic stats line of every coin (`[ 60s] GPU0 68.318 (199 W, 0.343 TH/W)  GPU1 ... | total 136.6 TH/s ...`): the unit and
/// one entry per GPU.  None for any other line.
pub fn parse_stats(line: &str) -> Option<(String, Vec<Entry>)> {
    let l = strip_ansi(line);
    if !l.trim_start().starts_with('[') {
        return None;
    }
    let ti = l.find(" | total ")?;
    let mut tot = l[ti + 9..].split_whitespace();
    tot.next()?.parse::<f64>().ok()?;
    let unit = tot.next()?.trim_end_matches(',').to_string();
    if !["TH/s", "MH/s", "GH/s", "FW/s", "MA/s"].contains(&unit.as_str()) {
        return None;
    }
    let head = &l[..ti];
    let b = head.as_bytes();
    let mut starts = Vec::new();
    let mut i = 0;
    while let Some(k) = head[i..].find("GPU") {
        let s = i + k;
        let mut j = s + 3;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if (s == 0 || b[s - 1] == b' ') && j > s + 3 && j + 1 < b.len() && b[j] == b' ' && b[j + 1].is_ascii_digit() {
            starts.push(s);
        }
        i = s + 3;
    }
    let mut out = Vec::new();
    for (n, &s) in starts.iter().enumerate() {
        let seg = head[s..starts.get(n + 1).copied().unwrap_or(head.len())].trim();
        let mut t = seg.split_whitespace();
        let dev: i32 = t.next()?.trim_start_matches("GPU").parse().ok()?;
        let rate: f64 = t.next()?.parse().ok()?;
        let w = seg.find(" (").and_then(|p| seg[p + 2..].split_whitespace().next()).and_then(|x| x.parse::<f64>().ok()).filter(|_| seg.contains(" W"));
        out.push(Entry { dev, rate, w, dead: seg.contains(" DEAD"), tagged: seg.contains('[') });
    }
    (!out.is_empty()).then_some((unit, out))
}

#[derive(Clone, Debug, PartialEq)]
pub enum LockLine {
    Locked(i32, u32),
    Refused(i32, String),
}

/// `clocks: GPU1 (bus ...): core clock locked to 1500 MHz; ...` / `clocks: GPU1 (bus ...): cannot lock core clock (...)`
pub fn parse_lock(line: &str) -> Option<LockLine> {
    let l = strip_ansi(line);
    let p = l.find("clocks: GPU")?;
    let rest = &l[p + 11..];
    let dev: i32 = rest[..rest.find(|c: char| !c.is_ascii_digit())?].parse().ok()?;
    if let Some(q) = rest.find("): core clock locked to ") {
        let m = &rest[q + 24..];
        return Some(LockLine::Locked(dev, m[..m.find(|c: char| !c.is_ascii_digit())?].parse().ok()?));
    }
    let q = rest.find("): cannot lock core clock (")?;
    Some(LockLine::Refused(dev, rest[q + 27..].to_string()))
}

/// 1.8.0 (FFGN 1.0.4): `WARNING GPU3: the core clock lock 1400 MHz does not hold (runs at 1470 MHz): ...` -> (3, 1400, 1470)
pub fn parse_unheld(line: &str) -> Option<(i32, u32, u32)> {
    let l = strip_ansi(line);
    let i = l.find("WARNING GPU")?;
    let r = &l[i + "WARNING GPU".len()..];
    let (d, r) = r.split_once(": the core clock lock ")?;
    let (lk, r) = r.split_once(" MHz does not hold (runs at ")?;
    let (clk, _) = r.split_once(" MHz")?;
    Some((d.trim().parse().ok()?, lk.trim().parse().ok()?, clk.trim().parse().ok()?))
}

static UNHELD_DB: OnceLock<PathBuf> = OnceLock::new();

/// 1.8.0 (FFGN 1.0.4): the mining process of `--auto-lock` found the core lock of `bus` not holding (clocks::mark_not_held): its
/// saved record gets `unheld`, so the next start leaves auto-lock off for this GPU (as auto-uv is off for it now).  Nothing
/// without --auto-lock.
pub fn note_unheld(bus: &str, dev: i32) {
    let Some(path) = UNHELD_DB.get() else { return };
    let b = bus.to_ascii_lowercase();
    for (_, mut r) in load_db(path) {
        if r.key.split('|').nth(1) == Some(b.as_str()) && !r.unheld {
            r.unheld = true;
            save_rec(path, &r);
            lg(format!("auto-lock: GPU{dev}: the core clock lock does not hold on this GPU: auto-lock is off for it from the next start (--auto-lock-reset to try again)"));
        }
    }
}

/// What a refused lock means: 2 = locks not supported on this GPU, 3 = no permission (root / Administrator), 1 = this lock refused.
pub fn refusal_kind(text: &str) -> u8 {
    if text.contains("Not Supported") || text.contains("not in this driver") {
        2
    } else if text.contains("Insufficient Permissions") || text.contains("No permission") || text.contains("permission") {
        3
    } else {
        1
    }
}

/// The command line of a round's child: the miner's own, without the --auto-lock* options, --devices, --lock-cclock and --minutes
/// (given anew), the one-shot --uv-reset / --uv-reset-all only in the first round.
pub fn child_args(argv: &[String], devs: &[i32], locks: &[Option<u32>], first: bool, add_uv: bool) -> Vec<String> {
    const WITH_VALUE: [&str; 12] = [
        "--auto-lock-range",
        "--auto-lock-max-loss",
        "--auto-lock-goal",
        "--auto-lock-reprice-hours",
        "--auto-lock-hysteresis",
        "--power-price",
        "--coin-price",
        "--pool-fee",
        "--lock-cclock",
        "--devices",
        "--device",
        "--minutes",
    ];
    let mut out = Vec::new();
    let mut i = 0;
    while i < argv.len() {
        let a = argv[i].as_str();
        i += 1;
        if WITH_VALUE.contains(&a) {
            i += 1;
            continue;
        }
        if a == "--auto-lock" || a == "--auto-lock-reset" || a == "--no-update-check" || (!first && (a == "--uv-reset" || a == "--uv-reset-all")) {
            continue;
        }
        out.push(a.to_string());
    }
    out.push("--devices".into());
    out.push(devs.iter().map(i32::to_string).collect::<Vec<_>>().join(","));
    out.push("--lock-cclock".into());
    let all_same = locks.iter().all(|l| l.is_some() && *l == locks[0]);
    out.push(if all_same { locks[0].map_or("-".into(), |l| l.to_string()) } else { locks.iter().map(|l| l.map_or("-".into(), |l| l.to_string())).collect::<Vec<_>>().join(",") });
    if add_uv && !out.iter().any(|a| a == "--auto-uv") {
        out.push("--auto-uv".into());
    }
    out.push("--no-update-check".into());
    out
}

// ---------------------------------------------------------------------------------------------------------------------
// one GPU's measurement at one lock (fed with the child's lines and NVML samples; time in seconds: unit-tested)
// ---------------------------------------------------------------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Meas {
    pub state: i32,
    pub rate: f64,
    pub w: f64,
    pub clk: u32,
    pub offset: i32,
    pub why: String,
    pub refused: u8, // refusal_kind of the lock (0 = set)
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    Wait,
    Settle(f64),
    Measure(f64),
}

#[derive(Clone, Debug)]
pub struct Meter {
    pub lock: u32,
    phase: Phase,
    wait_from: f64,
    locked: bool,
    steady_unlocked: Option<f64>,
    attempts: u32,
    rates: Vec<f64>,
    ws: Vec<f64>,
    ews: Vec<f64>,
    clks: Vec<u32>,
    pl: f64,       // the card's enforced power limit in W (0 = unknown: no early PL detection)
    pl_win: std::collections::VecDeque<(f64, u32, bool)>, // the last PL_HOLD_S loaded 1 s samples: watts, MHz, held back (clock under the lock / power cap)
    pub done: Option<Meas>,
}

/// Seconds at the power limit with a clock below the lock after which the step is called PL-limited (no waiting for auto-uv).
const PL_HOLD_S: u32 = 20;
/// Share of the power limit from which the card counts as held by it: NVML's board power of a card in its power cap reads 93-97 %
/// of the limit (RTX 3080 Ti, 350 W limit: 325-341 W with the clock 170 MHz under the lock); 97 % never fired for most of the step.
const PL_FRAC: f64 = 0.88;
/// A clock this far under the lock in a sample counts as held back by the power limit (also the minimum mean shortfall).
const PL_DIP: u32 = 30;
/// NVML clocks event reason: software power cap.
const REASON_SW_POWER_CAP: u64 = 0x4;

impl Meter {
    pub fn new(lock: u32, now: f64) -> Meter {
        Meter { lock, phase: Phase::Wait, wait_from: now, locked: false, steady_unlocked: None, attempts: 0, rates: vec![], ws: vec![], ews: vec![], clks: vec![], pl: 0.0, pl_win: Default::default(), done: None }
    }

    /// The card's enforced power limit (W): with it a step that sits at the limit below its lock ends at once as PL-limited.
    pub fn with_pl(mut self, pl: Option<f64>) -> Meter {
        self.pl = pl.filter(|&p| p > 0.0).unwrap_or(0.0);
        self
    }

    fn finish(&mut self, state: i32, why: impl Into<String>) {
        if self.done.is_none() {
            self.done = Some(Meas { state, why: why.into(), ..Default::default() });
        }
    }

    pub fn measuring(&self) -> bool {
        matches!(self.phase, Phase::Measure(_)) && self.done.is_none()
    }

    pub fn what(&self, now: f64, o: &Opts) -> String {
        match self.phase {
            _ if self.done.is_some() => "measured".into(),
            Phase::Wait => format!("waiting for steady mining (auto-uv at this lock; {} min)", ((now - self.wait_from) / 60.0) as u64),
            Phase::Settle(t) => format!("settling {}/{} s", (now - t) as u64, o.settle),
            Phase::Measure(t) => format!("measuring {}/{} s", (now - t) as u64, o.measure),
        }
    }

    pub fn on_lock(&mut self, l: &LockLine) {
        match l {
            LockLine::Locked(_, m) if *m == self.lock => self.locked = true,
            LockLine::Locked(_, m) => self.finish(FAILED, format!("the child locked {m} MHz instead of {}", self.lock)),
            LockLine::Refused(_, t) => {
                let k = refusal_kind(t);
                self.finish(FAILED, format!("the driver refused the lock ({})", t.trim_end_matches(')').trim()));
                if let Some(d) = self.done.as_mut() {
                    d.refused = k;
                }
            }
        }
    }

    /// 1.8.0 (FFGN 1.0.4): the child says the core lock does not hold on this GPU (`clocks::not_held_message`).
    pub fn unheld(&mut self, lock: u32, clk: u32) {
        if self.done.is_none() {
            self.done = Some(Meas { state: NOT_HELD, clk, why: format!("the miner reported the lock {lock} MHz not holding"), ..Default::default() });
        }
    }

    /// The child restarted itself (after a GPU fault): its clocks come again, the measurement starts over.
    pub fn restart(&mut self, now: f64) {
        if self.done.is_none() {
            self.phase = Phase::Wait;
            self.wait_from = now;
            self.locked = false;
            self.rates.clear();
        }
    }

    pub fn on_entry(&mut self, now: f64, e: &Entry, o: &Opts) {
        if self.done.is_some() {
            return;
        }
        if e.dead {
            self.finish(FAILED, "the GPU stopped while mining at this lock");
            return;
        }
        let steady = e.rate > 0.0 && !e.tagged;
        match self.phase {
            Phase::Wait => {
                if steady && self.locked {
                    self.phase = Phase::Settle(now);
                } else if steady {
                    let t = *self.steady_unlocked.get_or_insert(now);
                    if now - t > 120.0 {
                        self.finish(FAILED, "the core lock was not confirmed in the log");
                    }
                }
            }
            Phase::Settle(_) => {
                if !steady {
                    self.phase = Phase::Wait;
                    self.wait_from = now;
                }
            }
            Phase::Measure(_) => {
                if !steady {
                    self.attempts += 1;
                    self.rates.clear();
                    self.ws.clear();
                    self.ews.clear();
                    self.clks.clear();
                    if self.attempts >= 3 {
                        self.finish(UNSTABLE, "no steady mining in 3 attempts (auto-uv re-check, temperature limit, pause or no job)");
                    } else {
                        self.phase = Phase::Wait;
                        self.wait_from = now;
                    }
                    return;
                }
                self.rates.push(e.rate);
                if let Some(w) = e.w {
                    self.ews.push(w);
                }
            }
        }
        self.tick(now, o);
    }

    /// One 1 s sample of the GPU (NVML): watts, core clock, clocks event reasons.
    pub fn on_sample(&mut self, w: Option<f64>, clk: Option<u32>, reasons: Option<u64>) {
        // PL-limited: the lock is set and, over the last PL_HOLD_S loaded seconds, the card draws about its power limit while the
        // clock is held under the lock (any phase: no waiting for auto-uv / settling at a lock the card cannot hold); the sweep goes
        // on with the next lock.  A card in its power cap does not sit under the lock steadily: its clock jumps between the lock and
        // 150-170 MHz under it every second or two (RTX 3080 Ti at 2000: 1830-2010, 325-335 W of 350 W, SW power cap bit set most of
        // the time), so a run of consecutive samples never forms: the decision is over a window.  An idle second (an auto-uv check
        // in progress, the miner paused: < 50 % of the limit) is skipped.
        if self.pl > 0.0 && self.locked && self.done.is_none() {
            if let (Some(w), Some(c)) = (w, clk) {
                if w >= self.pl * 0.5 && c > 0 {
                    let held = w >= self.pl * PL_FRAC && (c + PL_DIP < self.lock || reasons.is_some_and(|r| r & REASON_SW_POWER_CAP != 0));
                    self.pl_win.push_back((w, c, held));
                    if self.pl_win.len() > PL_HOLD_S as usize {
                        self.pl_win.pop_front();
                    }
                    if self.pl_win.len() == PL_HOLD_S as usize {
                        let n = self.pl_win.len() as f64;
                        let mw = self.pl_win.iter().map(|x| x.0).sum::<f64>() / n;
                        let mc = self.pl_win.iter().map(|x| x.1 as f64).sum::<f64>() / n;
                        let held = self.pl_win.iter().filter(|x| x.2).count() as f64 / n;
                        let deficit = PL_DIP.max(self.lock * 3 / 200) as f64; // 30 MHz or 1.5 %
                        if mw >= self.pl * PL_FRAC && held >= 0.4 && mc + deficit <= self.lock as f64 {
                            let mc = mc.round() as u32;
                            self.done = Some(Meas { state: THROTTLED, w: mw, clk: mc, why: format!("PL-limited: {mw:.0} W at the {:.0} W power limit, {mc} MHz below the lock", self.pl), ..Default::default() });
                            return;
                        }
                    }
                }
            }
        }
        if self.measuring() {
            if let Some(w) = w.filter(|&w| w > 0.0) {
                self.ws.push(w);
            }
            if let Some(c) = clk.filter(|&c| c > 0) {
                self.clks.push(c);
            }
        }
    }

    pub fn tick(&mut self, now: f64, o: &Opts) {
        if self.done.is_some() {
            return;
        }
        match self.phase {
            Phase::Wait if now - self.wait_from > o.max_wait as f64 => {
                self.finish(FAILED, format!("no steady mining at this lock within {} min (auto-uv found no offset? no job?)", o.max_wait / 60))
            }
            Phase::Settle(t) if now - t >= o.settle as f64 => {
                self.phase = Phase::Measure(now);
                self.rates.clear();
                self.ws.clear();
                self.ews.clear();
                self.clks.clear();
            }
            Phase::Measure(t) if now - t >= o.measure as f64 && !self.rates.is_empty() => {
                let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
                let rate = mean(&self.rates);
                let w = if !self.ws.is_empty() { mean(&self.ws) } else if !self.ews.is_empty() { mean(&self.ews) } else { 0.0 };
                let mut c = self.clks.clone();
                c.sort_unstable();
                let clk = c.get(c.len() / 2).copied().unwrap_or(0);
                let tol = 45.max(self.lock * 3 / 100);
                let (state, why) = if clk > 0 && clk + tol < self.lock {
                    (THROTTLED, format!("median {clk} MHz"))
                } else if clk > self.lock + tol {
                    (NOT_HELD, format!("median {clk} MHz"))
                } else {
                    (OK, String::new())
                };
                self.done = Some(Meas { state, rate, w, clk, offset: 0, why, refused: 0 });
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// the sweep over rounds (the child processes behind `run`: unit-tested with a simulated GPU)
// ---------------------------------------------------------------------------------------------------------------------

pub struct Sw {
    pub pos: usize,
    pub label: String,
    pub cands: Vec<u32>,
    pub rec: Rec,
    pub finished: bool,
}

#[derive(Default)]
pub struct RoundRes {
    pub results: Vec<(usize, Meas)>, // by position
    pub aborted: Option<String>,     // the child ended before mining (pool, start-up error): nothing measured
    pub stopped: bool,               // the miner is stopping
    pub unit: String,
}

#[derive(Debug, PartialEq)]
pub enum End {
    Done,
    Stopped,
    GaveUp,
}

fn finish_gpu(g: &mut Sw, o: &Opts, save: &mut dyn FnMut(&Rec), econ: Option<&Econ>) {
    for &l in &g.cands {
        if g.rec.at(l).is_none() {
            g.rec.put(Point { lock: l, state: SKIPPED, ..Default::default() });
        }
    }
    g.rec.choice = choose(&g.rec, &g.cands, o.loss, o.goal, econ);
    g.rec.done = true;
    g.rec.range = g.cands.clone();
    g.rec.goal = o.goal;
    g.rec.loss = o.loss;
    g.rec.time = now_s();
    g.finished = true;
    save(&g.rec);
    if g.rec.unsupported {
        return;
    }
    let econ = econ.filter(|_| o.goal == Goal::Profit);
    if o.goal == Goal::Profit {
        match econ {
            Some(e) => lg(format!("{} RESULT (goal profit: {}):", g.label, money_text(e))),
            None => lg(format!("{} RESULT (goal profit, but no coin price / network data yet: chosen by efficiency within {}% for now, re-chosen as soon as the data is there):", g.label, o.loss)),
        }
    } else {
        lg(format!("{} RESULT (goal {}, max loss {}%, top {:.3} {}):", g.label, o.goal.name(), o.loss, top_rate(&g.rec, &g.cands), g.rec.unit));
    }
    for l in table(&g.rec, &g.cands, g.rec.choice, econ) {
        lg(l);
    }
    if let (Some(e), Some(p)) = (econ, g.rec.at(g.rec.choice)) {
        if e.profit(p.rate, &g.rec.unit, p.w) < 0.0 {
            lg(format!("{} WARNING: every measured lock loses money at this price and tariff: the least loss is chosen (whether to mine at all is your decision)", g.label));
        }
    }
    if g.rec.choice > 0 {
        lg(format!("{} mining at lock {} \u{2605} (saved; --auto-lock-reset to sweep again)", g.label, g.rec.choice));
    } else {
        lg(format!("{} no valid lock (all failed / not valid): mining without a lock of its own (HiveOS OC lock or driver default)", g.label));
    }
}

/// Runs rounds until every GPU has its lock.  `run(round, todo, fixed)`: one child with the GPUs of `todo` (position, lock) under
/// test; `pause(secs)` waits (false: the miner stops).
pub fn sweep(
    gs: &mut [Sw],
    o: &Opts,
    run: &mut dyn FnMut(usize, &[(usize, u32)]) -> RoundRes,
    save: &mut dyn FnMut(&Rec),
    pause: &mut dyn FnMut(u32) -> bool,
    econ: &mut dyn FnMut() -> Option<Econ>,
) -> End {
    let mut aborts = 0;
    for round in 1.. {
        if time_up(o.deadline) {
            lg("--minutes is over during the sweep: stopping (the points measured so far are kept)");
            return End::Stopped;
        }
        let mut todo = Vec::new();
        for g in gs.iter_mut().filter(|g| !g.finished) {
            match next_lock(&g.rec, &g.cands, cut_loss(o)) {
                Some(l) => {
                    g.rec.trying = l; // the run dies at this lock (Xid, freeze, power): the next start skips it
                    g.rec.done = false;
                    save(&g.rec);
                    todo.push((g.pos, l));
                }
                None => {
                    let e = econ();
                    finish_gpu(g, o, save, e.as_ref())
                }
            }
        }
        if todo.is_empty() {
            return End::Done;
        }
        let res = run(round, &todo);
        // the round is over, whatever its outcome: no crash marker stays on disk (only a run that dies mid-round leaves one)
        for g in gs.iter_mut().filter(|g| todo.iter().any(|t| t.0 == g.pos)) {
            g.rec.trying = 0;
            if !res.unit.is_empty() {
                g.rec.unit = res.unit.clone();
            }
            save(&g.rec);
        }
        if res.stopped {
            return End::Stopped;
        }
        if let Some(why) = &res.aborted {
            aborts += 1;
            if aborts >= 3 {
                lg(format!("the round's miner ended before mining 3 times ({why}): the sweep is given up for this run (the points so far are kept; the next start goes on)"));
                return End::GaveUp;
            }
            lg(format!("the round's miner ended before mining ({why}): trying again in 30 s"));
            if !pause(30) || time_up(o.deadline) {
                return End::Stopped;
            }
            continue;
        }
        aborts = 0;
        for (pos, m) in &res.results {
            let Some(g) = gs.iter_mut().find(|g| g.pos == *pos) else { continue };
            let lock = todo.iter().find(|t| t.0 == *pos).map_or(0, |t| t.1);
            if m.refused == 3 {
                lg(format!("{} the driver refused the core lock for lack of permission: --auto-lock needs root / Administrator; the sweep is given up", g.label));
                return End::GaveUp;
            }
            if m.refused == 2 && !g.rec.pts.iter().any(|p| p.state != SKIPPED && p.state != CRASHED) {
                lg(format!("{} core clock locks not supported on this GPU ({}): it mines without one (auto-uv at the current clocks)", g.label, m.why));
                g.rec.unsupported = true;
                finish_gpu(g, o, save, None);
                continue;
            }
            let p = Point { lock, offset: m.offset, rate: m.rate, w: m.w, clk: m.clk, state: m.state };
            if m.state == NOT_HELD && m.why.contains("reported the lock") {
                // 1.8.0 (FFGN 1.0.4): the mining process found the lock not holding at all (clocks.rs keeper): every point of this
                // GPU is meaningless, auto-lock is off for it (as auto-uv)
                lg(format!("{} WARNING: the core clock lock {lock} MHz does not hold (the clock ran at {} MHz; {}): auto-lock is off for this GPU, it mines without a lock of its own (--auto-lock-reset to try again)", g.label, m.clk, m.why));
                g.rec.put(p);
                g.rec.unheld = true;
                finish_gpu(g, o, save, None);
                continue;
            }
            if m.state == OK {
                let money = if o.goal == Goal::Profit && !g.rec.unit.is_empty() { econ().map_or(String::new(), |e| format!(", profit {:.3} $/day", e.profit(p.rate, &g.rec.unit, p.w))) } else { String::new() };
                lg(format!("{} lock {lock} at {}: {:.3} {}, {:.1} W, {:.4} {}, {} MHz{money} (round {round})", g.label, crate::uv::signed(p.offset), p.rate, g.rec.unit, p.w, p.eff_shown(&g.rec.unit), eff_unit(&g.rec.unit), p.clk));
            } else if m.state == FAILED {
                lg(format!("{} lock {lock} FAILED: {}; next lock", g.label, m.why));
            } else {
                lg(format!("{} lock {lock}: {:.3} {}, {:.1} W, {} MHz: {}{}{}", g.label, p.rate, g.rec.unit, p.w, p.clk, state_text(m.state), if m.why.is_empty() { "" } else { " - " }, m.why));
            }
            g.rec.put(p);
            save(&g.rec);
        }
        // a GPU of the round without a result (cannot happen with a complete round): its lock is failed, never measured again
        for &(pos, l) in &todo {
            if !res.results.iter().any(|r| r.0 == pos) {
                if let Some(g) = gs.iter_mut().find(|g| g.pos == pos) {
                    g.rec.put(Point { lock: l, state: FAILED, ..Default::default() });
                    save(&g.rec);
                }
            }
        }
    }
    End::Done
}

// ---------------------------------------------------------------------------------------------------------------------
// goal profit: the network data of the pool's jobs (child -> parent through the log), the price, the saved money data
// ---------------------------------------------------------------------------------------------------------------------

/// The coin's pool client, on every job with the network target (Pearl: nbits, NOID: block_target) and height: a round's /
/// the mining process of goal profit prints it (at most every 30 min; never without goal profit).
pub fn note_net(coin: &str, height: u64, work: f64) {
    if std::env::var_os(NET_ENV).is_none() || height == 0 || !(work > 0.0) || !work.is_finite() {
        return;
    }
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    let mut g = LAST.lock().unwrap();
    if g.is_some_and(|t| t.elapsed() < Duration::from_secs(1800)) {
        return;
    }
    *g = Some(Instant::now());
    println!("auto-lock: network {coin} height {height} work {work:.6e}");
}

/// `auto-lock: network pearl height 143562 work 1.690137e22` -> (coin, height, work)
pub fn parse_net(line: &str) -> Option<(String, u64, f64)> {
    let l = strip_ansi(line);
    let r = &l[l.find("auto-lock: network ")? + 19..];
    let t: Vec<&str> = r.split_whitespace().collect();
    if t.len() < 5 || t[1] != "height" || t[3] != "work" {
        return None;
    }
    Some((t[0].to_string(), t[2].parse().ok()?, t[4].parse::<f64>().ok().filter(|w| *w > 0.0)?))
}

#[derive(Clone, Debug, Default)]
struct Money {
    coin: Option<Coin>,
    price: Option<(f64, String)>,
    net: Option<(Net, bool)>, // bool: from this run's pool jobs
    fee: (f64, String),
    kwh: f64,
    path: PathBuf,
}

static MONEY: Mutex<Option<Money>> = Mutex::new(None);

/// The money data of this run (goal profit), None without price or network data.
pub fn econ_now() -> Option<Econ> {
    let g = MONEY.lock().unwrap();
    let m = g.as_ref()?;
    Some(Econ { coin: m.coin?, price: m.price.as_ref()?.0, per_hs_day: m.net.as_ref()?.0.per_hs_day, pool_fee: m.fee.0, kwh: m.kwh })
}

fn money_text(e: &Econ) -> String {
    format!(
        "{} {:.5} $, {:.4e} {} per H/s per day, fees {}% miner + {}% pool, power {:.4} $/kWh",
        e.coin.sym(),
        e.price,
        e.per_hs_day,
        e.coin.sym(),
        crate::profit::MINER_FEE_PCT,
        e.pool_fee,
        e.kwh
    )
}

fn load_money(path: &Path, coin: Coin) -> Option<Value> {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    v.get("money")?.get(coin.sym()).cloned()
}

fn save_money(m: &Money) {
    let Some(coin) = m.coin else { return };
    let mut doc = std::fs::read_to_string(&m.path).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()).filter(Value::is_object).unwrap_or_else(|| json!({"version": 1, "gpus": {}}));
    if !doc.get("money").is_some_and(Value::is_object) {
        doc["money"] = json!({});
    }
    let mut e = json!({"time": now_s()});
    if let Some((p, src)) = &m.price {
        e["price"] = json!(p);
        e["price_src"] = json!(src);
    }
    if let Some((n, own)) = &m.net {
        e["per_hs_day"] = json!(n.per_hs_day);
        e["net_src"] = json!(n.src);
        e["net_own"] = json!(own);
    }
    doc["money"][coin.sym()] = e;
    if let Some(d) = m.path.parent().filter(|d| !d.as_os_str().is_empty()) {
        let _ = std::fs::create_dir_all(d);
    }
    let tmp = m.path.with_extension("json.mtmp");
    if std::fs::write(&tmp, serde_json::to_string_pretty(&doc).unwrap_or_default()).and_then(|_| std::fs::rename(&tmp, &m.path)).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// The price of the coin: --coin-price, else the median of its public tickers (one log line each).
fn fetch_price(coin: Coin, o: &Opts) -> Result<(f64, String), String> {
    if let Some(p) = o.coin_price {
        return Ok((p, "--coin-price".into()));
    }
    let srcs = crate::profit::sources_of(coin);
    if srcs.is_empty() {
        return Err(format!("no public market of {} is known: goal profit needs --coin-price X", coin.sym()));
    }
    lg(format!("price of {}: asking {} public ticker(s) (GET only, User-Agent {}, nothing of yours is sent; --coin-price X instead asks none)", coin.sym(), srcs.len(), crate::profit::UA));
    let qs = crate::profit::fetch_quotes(&srcs);
    let pr = crate::profit::price_from(coin, qs);
    match pr {
        Ok(p) => {
            for l in &p.lines {
                lg(l);
            }
            if let Some(w) = &p.warn {
                lg(format!("WARNING: {w}"));
            }
            lg(format!("price of {}: {:.5} $ (median of {} ticker(s))", coin.sym(), p.usd, p.used));
            Ok((p.usd, format!("median of {} ticker(s)", p.used)))
        }
        Err(e) => Err(e),
    }
}

fn pool_of(argv: &[String]) -> (String, bool) {
    let v = |k: &str| argv.iter().position(|a| a == k).and_then(|i| argv.get(i + 1)).cloned();
    (v("--pool").unwrap_or_default(), v("--solo").is_some())
}

/// Goal profit, at the start (parent only): the tariff, the pool fee, the price and the network data (the public API of the
/// coin when there is one; the pool's jobs replace it as soon as a round / mining process reports them; else the last saved).
fn money_start(coin_name: &str, o: &Opts, path: &Path) {
    let Some(coin) = Coin::of(coin_name) else { return };
    let kwh = o.tariff.as_ref().map_or(0.0, Tariff::avg);
    lg(format!(
        "goal profit: electricity {:.4} $/kWh averaged over the day (--power-price {}), GPU board watts from NVML (the PSU, CPU and fans are not in them)",
        kwh,
        o.tariff.as_ref().map_or("-", |t| t.text.as_str())
    ));
    let (pool, solo) = pool_of(ARGV.get().map_or(&[][..], |v| v.as_slice()));
    let fee = match (o.pool_fee, crate::profit::known_pool_fee(coin, &pool)) {
        (Some(f), _) => (f, "--pool-fee".to_string()),
        (None, _) if solo => (0.0, "solo".to_string()),
        (None, Some((f, name))) => (f, format!("{name}, known")),
        (None, None) => {
            lg("goal profit: the fee of this pool is not known to the miner: 0% assumed (--pool-fee PCT sets it)");
            (0.0, "unknown, 0".to_string())
        }
    };
    let saved = load_money(path, coin);
    let age = |v: &Value| now_s().saturating_sub(v.get("time").and_then(Value::as_u64).unwrap_or(0));
    let price = match fetch_price(coin, o) {
        Ok(p) => Some(p),
        Err(e) => match saved.as_ref().filter(|v| age(v) < 7 * 86400).and_then(|v| v.get("price").and_then(Value::as_f64)) {
            Some(p) => {
                lg(format!("WARNING: {e}; the last saved price {p:.5} $ ({} h old) is used", age(saved.as_ref().unwrap()) / 3600));
                Some((p, "saved".into()))
            }
            None => {
                lg(format!("WARNING: {e}: no profit choice in this run until a price is there (chosen by efficiency meanwhile)"));
                None
            }
        },
    };
    let net = match crate::profit::net_api(coin) {
        Ok(n) => {
            lg(format!("network of {}: {} (public API; GET only)", coin.sym(), n.src));
            Some((n, false))
        }
        Err(e) => match saved.as_ref().filter(|v| age(v) < 7 * 86400).and_then(|v| v.get("per_hs_day").and_then(Value::as_f64)) {
            Some(x) => {
                lg(format!("network of {}: the last saved data ({} h old; {e}); the pool's jobs replace it once mining", coin.sym(), age(saved.as_ref().unwrap()) / 3600));
                Some((Net { per_hs_day: x, src: "saved".into(), pool_fee: None }, false))
            }
            None => {
                lg(format!("network of {}: {e}", coin.sym()));
                None
            }
        },
    };
    let m = Money { coin: Some(coin), price, net, fee, kwh, path: path.to_path_buf() };
    save_money(&m);
    *MONEY.lock().unwrap() = Some(m);
}

/// A network line of a round's / the mining process (parent side): the pool's own data replaces the API's.
fn money_net_line(l: &str) -> bool {
    let Some((c, h, w)) = parse_net(l) else { return false };
    let mut g = MONEY.lock().unwrap();
    let Some(m) = g.as_mut() else { return false };
    let Some(coin) = m.coin.filter(|k| Coin::of(&c) == Some(*k)) else { return false };
    let Some(n) = crate::profit::net_from_work(coin, h, w) else { return false };
    let first = !m.net.as_ref().is_some_and(|x| x.1);
    m.net = Some((n, true));
    save_money(m);
    first
}

/// Goal profit, every --auto-lock-reprice-hours while mining: the price again (not with --coin-price), the network from the
/// API again unless the pool's jobs give it.  A failed fetch keeps the last value.
fn money_refresh(o: &Opts) {
    let Some((coin, own)) = MONEY.lock().unwrap().as_ref().and_then(|m| m.coin.map(|c| (c, m.net.as_ref().is_some_and(|n| n.1)))) else { return };
    let price = if o.coin_price.is_some() { None } else { Some(fetch_price(coin, o)) };
    let net = if own { None } else { Some(crate::profit::net_api(coin)) };
    let mut g = MONEY.lock().unwrap();
    let Some(m) = g.as_mut() else { return };
    match price {
        Some(Ok(p)) => m.price = Some(p),
        Some(Err(e)) => lg(format!("WARNING: {e}: the last price is kept")),
        None => {}
    }
    match net {
        Some(Ok(n)) => m.net = Some((n, false)),
        Some(Err(e)) => lg(format!("network of {}: {e}: the last data is kept", coin.sym())),
        None => {}
    }
    save_money(m);
}

// ---------------------------------------------------------------------------------------------------------------------
// the child side: stop when the parent says so (or is gone)
// ---------------------------------------------------------------------------------------------------------------------

fn watch(stop: &Arc<AtomicBool>) {
    let Some(file) = std::env::var_os(STOP_ENV).map(PathBuf::from) else { return };
    let parent: Option<u32> = std::env::var(PARENT_ENV).ok().and_then(|p| p.parse().ok());
    let stop = stop.clone();
    let _ = std::thread::Builder::new().name("autolock-watch".into()).spawn(move || {
        loop {
            std::thread::sleep(Duration::from_millis(500));
            let gone = parent_gone(parent);
            if file.exists() || gone {
                if !stop.swap(true, Ordering::SeqCst) {
                    println!("auto-lock: {}", if gone { "the sweep's supervisor is gone: stopping" } else { "this round of the sweep is over: stopping (the sweep goes on)" });
                }
                return;
            }
        }
    });
}

#[cfg(unix)]
fn parent_gone(p: Option<u32>) -> bool {
    p.is_some_and(|p| std::os::unix::process::parent_id() != p)
}
#[cfg(not(unix))]
fn parent_gone(_: Option<u32>) -> bool {
    false
}

// ---------------------------------------------------------------------------------------------------------------------
// the parent side: one round = one child process
// ---------------------------------------------------------------------------------------------------------------------

/// One mining GPU of the coin (in its --devices order).
pub struct Card {
    pub dev: i32,
    pub bus: String,
    pub kernel: String,
}

pub enum Outcome {
    /// mine on (`swept`: a sweep ran in this process: the auto-uv profiles of the chosen locks were just checked / tuned)
    Mine { swept: bool },
    /// the miner was stopped during the sweep
    Stop,
    /// goal profit: the mining process of this one ended by itself (Err: not cleanly; the caller exits with an error)
    Done(Result<(), String>),
}

struct Ctx<'a> {
    nv: &'a Nvml,
    devs: Vec<i32>,
    ds: Vec<Option<Dev>>,
    stopfile: PathBuf,
    o: Opts,
    argv: Vec<String>,
    stop: &'a Arc<AtomicBool>,
}

enum Msg {
    Line(String),
}

fn spawn_reader<R: std::io::Read + Send + 'static>(r: R, err: bool, tx: mpsc::Sender<Msg>) {
    let _ = std::thread::Builder::new().name("autolock-log".into()).spawn(move || {
        let mut br = std::io::BufReader::new(r);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match br.read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => return,
                Ok(_) => {
                    let s = String::from_utf8_lossy(&buf).trim_end_matches(['\r', '\n']).to_string();
                    // the child's line, unchanged (h-stats reads the stats line from the log); colours as for any line of this process
                    if err {
                        eprintln!("{s}");
                    } else {
                        println!("{s}");
                    }
                    let _ = tx.send(Msg::Line(s));
                }
            }
        }
    });
}

fn end_child(ch: &mut Child, stopfile: &Path) -> Option<ExitStatus> {
    let _ = std::fs::write(stopfile, b"stop\n");
    let wait = |ch: &mut Child, secs: u64| -> Option<ExitStatus> {
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(secs) {
            if let Ok(Some(s)) = ch.try_wait() {
                return Some(s);
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        None
    };
    if let Some(s) = wait(ch, 120) {
        return Some(s);
    }
    #[cfg(unix)]
    {
        lg("the round's miner does not stop: SIGINT");
        let _ = Command::new("kill").args(["-INT", &ch.id().to_string()]).status();
        if let Some(s) = wait(ch, 60) {
            return Some(s);
        }
    }
    lg("the round's miner does not stop: killed (its clocks / offsets are put back by the recovery of this process)");
    let _ = ch.kill();
    ch.wait().ok()
}

fn run_round(c: &Ctx, round: usize, todo: &[(usize, u32)], locks: &[Option<u32>], labels: &[String]) -> RoundRes {
    let mut res = RoundRes::default();
    let _ = std::fs::remove_file(&c.stopfile);
    let args = child_args(&c.argv, &c.devs, locks, round == 1, c.o.add_uv);
    let Ok(exe) = std::env::current_exe() else {
        res.aborted = Some("cannot find the miner executable".into());
        return res;
    };
    let what: Vec<String> = todo.iter().map(|(p, l)| format!("GPU{} at {l} MHz", c.devs[*p])).collect();
    lg(format!("round {round}: {} (a miner process with --lock-cclock {}; its log follows)", what.join(", "), args[args.iter().position(|a| a == "--lock-cclock").map_or(0, |i| i + 1)]));
    let mut cmd = Command::new(&exe);
    cmd.args(&args)
        .env(CHILD_ENV, "1")
        .env(STOP_ENV, &c.stopfile)
        .env(PARENT_ENV, std::process::id().to_string())
        .env("FFG_COLOR", "0")
        .envs((c.o.goal == Goal::Profit).then_some((NET_ENV, "1")))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut ch = match cmd.spawn() {
        Ok(ch) => ch,
        Err(e) => {
            res.aborted = Some(format!("cannot start {}: {e}", exe.display()));
            return res;
        }
    };
    let (tx, rx) = mpsc::channel();
    if let Some(o) = ch.stdout.take() {
        spawn_reader(o, false, tx.clone());
    }
    if let Some(e) = ch.stderr.take() {
        spawn_reader(e, true, tx);
    }
    let t0 = Instant::now();
    let now = || t0.elapsed().as_secs_f64();
    let mut meters: Vec<(usize, Meter)> = todo.iter().map(|&(p, l)| (p, Meter::new(l, 0.0).with_pl(c.ds[p].and_then(|d| c.nv.power_limit_w(d))))).collect();
    let (mut last_sample, mut last_status, mut seen_stats) = (0.0f64, 0.0f64, false);
    let exit = loop {
        while let Ok(Msg::Line(l)) = rx.try_recv() {
            let n = now();
            if let Some(ll) = parse_lock(&l) {
                let d = match &ll {
                    LockLine::Locked(d, _) | LockLine::Refused(d, _) => *d,
                };
                for (p, m) in meters.iter_mut().filter(|(p, _)| c.devs[*p] == d) {
                    m.on_lock(&ll);
                    let _ = p;
                }
            } else if let Some((unit, es)) = parse_stats(&l) {
                seen_stats = true;
                res.unit = unit;
                for e in &es {
                    for (_, m) in meters.iter_mut().filter(|(p, _)| c.devs[*p] == e.dev) {
                        m.on_entry(n, e, &c.o);
                    }
                }
            } else if let Some((d, lk, clk)) = parse_unheld(&l) {
                // 1.8.0 (FFGN 1.0.4): the child found the lock not holding (clocks.rs keeper): this GPU's sweep ends
                for (_, m) in meters.iter_mut().filter(|(p, _)| c.devs[*p] == d) {
                    m.unheld(lk, clk);
                }
            } else if l.contains("restarting itself (") {
                for (_, m) in meters.iter_mut() {
                    m.restart(n);
                }
            } else {
                money_net_line(&l);
            }
        }
        let n = now();
        if n - last_sample >= 1.0 {
            last_sample = n;
            for (p, m) in meters.iter_mut() {
                if let Some(d) = c.ds[*p] {
                    m.on_sample(c.nv.power_w(d), c.nv.clock_mhz(d), c.nv.clock_reasons(d));
                }
                m.tick(n, &c.o);
            }
        }
        if n - last_status >= 60.0 {
            last_status = n;
            let s: Vec<String> = meters.iter().filter(|(_, m)| m.done.is_none()).map(|(p, m)| format!("{} lock {}: {}", labels[*p], m.lock, m.what(n, &c.o))).collect();
            if !s.is_empty() {
                lg(format!("round {round}: {}", s.join("; ")));
            }
        }
        if !c.stop.load(Ordering::Relaxed) && time_up(c.o.deadline) {
            lg("--minutes is over during the sweep: stopping the round's miner (its clocks and offsets are put back)");
            c.stop.store(true, Ordering::SeqCst);
        }
        if c.stop.load(Ordering::Relaxed) {
            res.stopped = true;
            break end_child(&mut ch, &c.stopfile);
        }
        if meters.iter().all(|(_, m)| m.done.is_some()) {
            // the offset each GPU measured at (auto-uv's profile of that lock), read before the child puts the owner's back
            for (p, m) in meters.iter_mut() {
                if let (Some(d), Some(r)) = (c.ds[*p], m.done.as_mut()) {
                    if let Ok((_, off, _, _)) = c.nv.probe_offset(d) {
                        r.offset = off;
                    }
                }
            }
            break end_child(&mut ch, &c.stopfile);
        }
        if let Ok(Some(s)) = ch.try_wait() {
            break Some(s);
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let _ = std::fs::remove_file(&c.stopfile);
    std::thread::sleep(Duration::from_millis(300)); // the last lines of the child's log
    if res.stopped {
        return res;
    }
    let code = exit.map_or("unknown".to_string(), |s| s.to_string());
    if !seen_stats && meters.iter().all(|(_, m)| m.done.is_none()) {
        res.aborted = Some(format!("exit {code}"));
        return res;
    }
    for (p, m) in meters {
        let r = m.done.unwrap_or_else(|| Meas { state: FAILED, why: format!("the miner stopped while this lock was measured ({code})"), ..Default::default() });
        res.results.push((p, r));
    }
    res
}

fn short_name(n: &str) -> String {
    n.trim_start_matches("NVIDIA ").trim_start_matches("GeForce ").to_string()
}

fn wait_secs(stop: &AtomicBool, secs: u32) -> bool {
    for _ in 0..secs * 10 {
        if stop.load(Ordering::Relaxed) {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    !stop.load(Ordering::Relaxed)
}

/// The coin's hook, before its clocks are applied.  `core`: the --lock-cclock values per GPU (a GPU with a value keeps it and is
/// not swept); filled with the chosen locks.  `mem`: the memory lock per GPU this run sets (part of the key; `hive_mem`: a GPU
/// without one gets the HiveOS OC profile's).  `dir`: the state directory (uv.json), `clock_dir`: the coin's clock marker
/// directory.  A child of a sweep only starts its stop watch here.
#[allow(clippy::too_many_arguments)]
pub fn resolve(coin: &str, cards: &[Card], core: &mut Vec<Option<u32>>, mem: &[Option<u32>], hive_mem: bool, dir: &Path, clock_dir: &Path, stop: &Arc<AtomicBool>) -> Outcome {
    if is_child() {
        watch(stop);
        return Outcome::Mine { swept: false };
    }
    let o = opts();
    if !o.on {
        return Outcome::Mine { swept: false };
    }
    let n = cards.len();
    core.resize(n, None);
    let nv = match Nvml::load() {
        Ok(nv) => nv,
        Err(e) => {
            lg(format!("NVML not available ({e:#}): mining without --auto-lock"));
            return Outcome::Mine { swept: false };
        }
    };
    let driver = nv.driver_version();
    let hive = if hive_mem { crate::hiveoc::load() } else { None };
    let path = db_path(dir);
    let _ = UNHELD_DB.set(path.clone());
    let db = load_db(&path);
    if o.goal == Goal::Profit {
        money_start(coin, &o, &path);
    }
    let mut save = |r: &Rec| save_rec(&path, r);
    // goal profit: every GPU with a choice of its own (saved or swept), re-chosen while mining (profit_mine)
    let mut managed: Vec<Sw> = Vec::new();
    let mut gs: Vec<Sw> = Vec::new();
    let mut labels: Vec<String> = cards.iter().map(|c| format!("GPU{}", c.dev)).collect();
    let mut ds: Vec<Option<Dev>> = vec![None; n];
    if core.iter().all(Option::is_some) {
        lg("every GPU has its own --lock-cclock value: nothing to sweep (leave a GPU's entry '-' to sweep it, e.g. --lock-cclock 1400,-)");
        return Outcome::Mine { swept: false };
    }
    for (pos, c) in cards.iter().enumerate() {
        let d = match nv.by_bus(&c.bus) {
            Ok(d) => d,
            Err(e) => {
                lg(format!("GPU{} (bus {}): NVML handle failed ({e:#}): not swept", c.dev, c.bus));
                continue;
            }
        };
        ds[pos] = Some(d);
        let name = nv.name(d).unwrap_or_else(|_| "?".into());
        labels[pos] = format!("GPU{} (bus {}, {})", c.dev, crate::uv::short_bus(&c.bus), short_name(&name));
        if let Some(l) = core[pos] {
            lg(format!("{}: --lock-cclock {l} MHz given for this GPU: not swept, it keeps that lock", labels[pos]));
            continue;
        }
        if let Some(left) = nv.uuid(d).ok().and_then(|u| crate::faults::excluded(dir, &u)) {
            lg(format!("{}: left out after crashes ({} min more): not swept in this run", labels[pos], left / 60 + 1));
            continue;
        }
        let m = mem.get(pos).copied().flatten().or_else(|| hive.as_ref().and_then(|h| nv.index(d).and_then(|i| h.mem_lock(i as usize))));
        let key = make_key(coin, &c.bus, &name, &driver, m, &c.kernel);
        let maxclk = nv.max_core_clock_mhz(d).unwrap_or(0);
        let (cands, dropped) = candidates_for(&o.list, maxclk, o.goal, &name);
        if o.list.is_empty() {
            if let Some(t) = safe_top(&name).filter(|&t| t < if o.goal == Goal::Profit { 2000 } else { 1800 }) {
                lg(format!("{}: default range {} (safe top {t} MHz for this card type: a higher lock only runs into its power limit; --auto-lock-range overrides)", labels[pos], join(&cands)));
            }
        }
        if !dropped.is_empty() {
            lg(format!("{}: locks above its maximum {maxclk} MHz left out: {}", labels[pos], join(&dropped)));
        }
        if cands.is_empty() {
            lg(format!("{}: no lock of --auto-lock-range at or below its maximum {maxclk} MHz: not swept", labels[pos]));
            continue;
        }
        let mut rec = db.get(&key).cloned().unwrap_or_default();
        if o.reset && !rec.key.is_empty() {
            lg(format!("{}: --auto-lock-reset: the saved points are forgotten (the auto-uv profiles stay)", labels[pos]));
            rec = Rec::default();
        }
        rec.key = key;
        if let Some(t) = rec.recover_trying() {
            lg(format!("{}: the previous run died at lock {t}: that lock is skipped (crashed)", labels[pos]));
            save(&rec);
        }
        rec.pts.retain(|p| p.state != SKIPPED); // re-evaluated with this run's options
        let label = labels[pos].clone();
        if rec.unsupported {
            lg(format!("{label}: core clock locks are not supported on this GPU (saved): it mines without one (--auto-lock-reset to try again)"));
            continue;
        }
        if rec.unheld {
            lg(format!("{label}: WARNING the core clock lock did not hold on this GPU (saved): auto-lock is off for it, it mines without a lock of its own (--auto-lock-reset to try again)"));
            continue;
        }
        if next_lock(&rec, &cands, cut_loss(&o)).is_none() {
            // nothing to measure: the saved choice, re-chosen when the goal / max loss / range changed (goal profit: always,
            // with this start's price)
            let econ = econ_now();
            let ch = choose(&rec, &cands, o.loss, o.goal, econ.as_ref());
            if o.goal == Goal::Profit || !rec.done || ch != rec.choice || rec.goal != o.goal || rec.loss != o.loss || rec.range != cands {
                if rec.done && rec.choice != 0 && ch != rec.choice {
                    let how = if o.goal == Goal::Profit { "goal profit, this start's price".to_string() } else { format!("goal {}, max loss {}%", o.goal.name(), o.loss) };
                    lg(format!("{label}: re-chosen from the saved points ({how}): lock {ch} instead of {}", rec.choice));
                }
                let mut g = Sw { pos, label: label.clone(), cands: cands.clone(), rec, finished: false };
                finish_gpu(&mut g, &o, &mut save, econ.as_ref());
                rec = g.rec;
            }
            if ch > 0 {
                managed.push(Sw { pos, label: label.clone(), cands: cands.clone(), rec: rec.clone(), finished: true });
            }
            match rec.at(ch) {
                Some(p) if ch > 0 => {
                    lg(format!(
                        "{label}: lock {ch} \u{2605} (saved: {:.3} {}, {:.1} W, {:.4} {}; --auto-lock-reset to sweep again)",
                        p.rate,
                        rec.unit,
                        p.w,
                        p.eff_shown(&rec.unit),
                        eff_unit(&rec.unit)
                    ));
                    core[pos] = Some(ch);
                }
                _ => lg(format!("{label}: no valid lock in the saved points (--auto-lock-reset to sweep again): mining without a lock of its own")),
            }
            continue;
        }
        let todo = cands.iter().filter(|&&l| rec.at(l).is_none()).count();
        lg(format!(
            "{label}: sweep of {} lock(s) {}..{} ({todo} to measure; goal {}{}): per lock auto-uv (a new lock is tuned first, only this GPU pauses), then {} s of mining ({} s measured){}",
            cands.len(),
            cands[0],
            cands[cands.len() - 1],
            o.goal.name(),
            if o.goal == Goal::Profit { String::new() } else { format!(", max loss {}%", o.loss) },
            o.settle + o.measure,
            o.measure,
            if o.goal == Goal::Profit { "; every lock is measured (the best one moves with the price)".to_string() } else { format!("; lower locks are left out once one is {}% below the top", o.loss) }
        ));
        gs.push(Sw { pos, label, cands, rec, finished: false });
    }
    let ctx = Ctx {
        nv: &nv,
        devs: cards.iter().map(|c| c.dev).collect(),
        ds: ds.clone(),
        stopfile: dir.join(format!("autolock-stop-{}", std::process::id())),
        o: o.clone(),
        argv: ARGV.get().cloned().unwrap_or_default(),
        stop,
    };
    let profit_loop = o.goal == Goal::Profit && o.reprice_h > 0.0;
    if gs.is_empty() {
        if profit_loop && !managed.is_empty() {
            let r = profit_mine(&ctx, core, &mut managed, &path, true);
            crate::clocks::recover(clock_dir);
            crate::powerlimit::recover(clock_dir, &cards.iter().map(|c| c.bus.clone()).collect::<Vec<_>>()); // 1.0.12: --power-limit of a killed child
            return r;
        }
        return Outcome::Mine { swept: false };
    }
    lg(format!(
        "the sweep runs each round as a miner process of its own (same options, per-GPU --lock-cclock); the GPUs not under test mine at their chosen / own locks; expect ~5-15 min per new lock (auto-uv) + {} s each",
        o.settle + o.measure
    ));
    let fixed: Vec<Option<u32>> = core.clone();
    let end = {
        // the GPUs not under test: their own lock, their choice (finished in an earlier round), else none of the sweep's
        let choices = std::cell::RefCell::new(vec![None::<u32>; n]);
        let mut run = |round: usize, todo: &[(usize, u32)]| {
            let mut locks = fixed.clone();
            for (p, l) in choices.borrow().iter().enumerate() {
                if l.is_some() {
                    locks[p] = *l;
                }
            }
            for &(p, l) in todo {
                locks[p] = Some(l);
            }
            run_round(&ctx, round, todo, &locks, &labels)
        };
        // finished GPUs: their choice for the next rounds (recorded through the save callback by key)
        let keys: Vec<(String, usize)> = gs.iter().map(|g| (g.rec.key.clone(), g.pos)).collect();
        let mut save3 = |r: &Rec| {
            save_rec(&path, r);
            if r.done {
                if let Some((_, p)) = keys.iter().find(|(k, _)| *k == r.key) {
                    choices.borrow_mut()[*p] = (r.choice > 0).then_some(r.choice);
                }
            }
        };
        let mut pause = |s: u32| wait_secs(stop, s);
        sweep(&mut gs, &o, &mut run, &mut save3, &mut pause, &mut econ_now)
    };
    // a child that had to be killed left its clocks: put back as after a crash (the offsets: by the coin's own recovery next)
    crate::clocks::recover(clock_dir);
    crate::powerlimit::recover(clock_dir, &cards.iter().map(|c| c.bus.clone()).collect::<Vec<_>>()); // 1.0.12: --power-limit of a killed child
    if end == End::Stopped {
        lg("stopped during the sweep (the points measured so far are kept; the next start goes on)");
        return Outcome::Stop;
    }
    for g in &gs {
        let ch = if g.finished { g.rec.choice } else { choose(&g.rec, &g.cands, o.loss, o.goal, econ_now().as_ref()) };
        if ch > 0 {
            core[g.pos] = Some(ch);
            managed.push(Sw { pos: g.pos, label: g.label.clone(), cands: g.cands.clone(), rec: Rec { choice: ch, ..g.rec.clone() }, finished: true });
            if !g.finished {
                lg(format!("{}: mining at the best lock measured so far, {ch} MHz (the sweep goes on at the next start)", g.label));
            }
        }
    }
    let summary: Vec<String> = cards.iter().enumerate().map(|(p, c)| format!("GPU{} {}", c.dev, core[p].map_or("-".to_string(), |l| l.to_string()))).collect();
    lg(format!("sweep over: mining at --lock-cclock {} (per GPU: {})", core.iter().map(|l| l.map_or("-".to_string(), |l| l.to_string())).collect::<Vec<_>>().join(","), summary.join(", ")));
    if profit_loop && !managed.is_empty() {
        let r = profit_mine(&ctx, core, &mut managed, &path, false);
        crate::clocks::recover(clock_dir);
        crate::powerlimit::recover(clock_dir, &cards.iter().map(|c| c.bus.clone()).collect::<Vec<_>>()); // 1.0.12: --power-limit of a killed child
        return r;
    }
    Outcome::Mine { swept: true }
}

/// Goal profit, re-choice while mining: per GPU the most profitable of its measured locks at the new money data, and the
/// lock it mines at now; a switch only when the gain is more than --auto-lock-hysteresis % (one log line either way).
/// Some(new locks) = switch.
pub fn rechoose(gs: &[Sw], cur: &[Option<u32>], e: &Econ, hyst: f64) -> (Option<Vec<Option<u32>>>, Vec<String>) {
    let mut new = cur.to_vec();
    let mut lines = Vec::new();
    let mut switch = false;
    for g in gs {
        let best = choose_profit(&g.rec, &g.cands, e);
        let Some(now) = cur.get(g.pos).copied().flatten() else { continue };
        let pf = |l: u32| g.rec.at(l).filter(|p| p.state == OK).map(|p| e.profit(p.rate, &g.rec.unit, p.w));
        let (Some(pb), Some(pc)) = (pf(best), pf(now)) else {
            continue;
        };
        if best != now && crate::profit::worth_switch(pc, pb, hyst) {
            lines.push(format!("{}: lock {now} -> {best}: profit {pc:.3} -> {pb:.3} $/day ({:+.1}% > {hyst}%)", g.label, (pb - pc) / pc.abs().max(1e-9) * 100.0));
            new[g.pos] = Some(best);
            switch = true;
        } else if best != now {
            lines.push(format!("{}: keeps lock {now} ({pc:.3} $/day; best {best} {pb:.3} $/day, {:+.1}% <= {hyst}%)", g.label, (pb - pc) / pc.abs().max(1e-9) * 100.0));
        } else {
            lines.push(format!("{}: keeps lock {now} ({pc:.3} $/day, still the best)", g.label));
        }
    }
    (switch.then_some(new), lines)
}

/// Goal profit with --auto-lock-reprice-hours: this process supervises one mining process (the same options, the chosen
/// locks; its log follows unchanged) and every N hours fetches the price (and the network data the pool's jobs do not give)
/// again; a better lock (> the hysteresis) restarts the mining process at it.  No new measurement is made.
fn profit_mine(c: &Ctx, core: &[Option<u32>], gs: &mut [Sw], path: &Path, first: bool) -> Outcome {
    let mut first = first; // no sweep ran: the one-shot --uv-reset* belong to the first mining process
    let mut cur: Vec<Option<u32>> = core.to_vec();
    let mins: Option<f64> = minutes_arg(&c.argv);
    let t0 = START.get().copied().unwrap_or_else(Instant::now); // --minutes counts from the start of the process (the sweep included)
    let period = Duration::from_secs_f64(c.o.reprice_h * 3600.0);
    let mut next = Instant::now() + period;
    let mut pending = econ_now().is_none(); // no money data at the start: re-chosen once the pool's jobs give it
    lg(format!(
        "goal profit: mining in a miner process of its own (its log follows); every {} h the price{} is fetched again and the locks re-chosen from the measured points (a switch needs more than {}% more profit; no new measurement)",
        c.o.reprice_h,
        if c.o.coin_price.is_some() { " (--coin-price: fixed)" } else { "" },
        c.o.hyst
    ));
    loop {
        let mut args = child_args(&c.argv, &c.devs, &cur, first, c.o.add_uv);
        first = false;
        if let Some(m) = mins {
            let left = m - t0.elapsed().as_secs_f64() / 60.0;
            if left <= 0.02 {
                return Outcome::Done(Ok(()));
            }
            args.push("--minutes".into());
            args.push(format!("{left:.2}"));
        }
        let _ = std::fs::remove_file(&c.stopfile);
        let Ok(exe) = std::env::current_exe() else { return Outcome::Done(Err("cannot find the miner executable".into())) };
        let mut cmd = Command::new(&exe);
        cmd.args(&args)
            .env(CHILD_ENV, "1")
            .env(STOP_ENV, &c.stopfile)
            .env(PARENT_ENV, std::process::id().to_string())
            .env(NET_ENV, "1")
            .env("FFG_COLOR", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut ch = match cmd.spawn() {
            Ok(ch) => ch,
            Err(e) => return Outcome::Done(Err(format!("cannot start {}: {e}", exe.display()))),
        };
        let (tx, rx) = mpsc::channel();
        if let Some(o) = ch.stdout.take() {
            spawn_reader(o, false, tx.clone());
        }
        if let Some(e) = ch.stderr.take() {
            spawn_reader(e, true, tx);
        }
        loop {
            let mut now_data = false;
            while let Ok(Msg::Line(l)) = rx.try_recv() {
                if money_net_line(&l) && pending {
                    now_data = true;
                }
            }
            if c.stop.load(Ordering::Relaxed) {
                end_child(&mut ch, &c.stopfile);
                let _ = std::fs::remove_file(&c.stopfile);
                return Outcome::Stop;
            }
            if let Ok(Some(st)) = ch.try_wait() {
                std::thread::sleep(Duration::from_millis(300)); // the last lines of its log
                let _ = std::fs::remove_file(&c.stopfile);
                return Outcome::Done(if st.success() { Ok(()) } else { Err(format!("the mining process ended ({st})")) });
            }
            let due = Instant::now() >= next;
            if now_data || due {
                if due {
                    next = Instant::now() + period;
                    money_refresh(&c.o);
                }
                if let Some(e) = econ_now() {
                    pending = false;
                    lg(format!("re-choice ({}): {}", if due { "every --auto-lock-reprice-hours" } else { "the pool's network data is there" }, money_text(&e)));
                    for g in gs.iter() {
                        if let Some(l) = cur.get(g.pos).copied().flatten() {
                            for t in table(&g.rec, &g.cands, l, Some(&e)) {
                                lg(format!("{} {t}", g.label));
                            }
                        }
                    }
                    let (sw, lines) = rechoose(gs, &cur, &e, c.o.hyst);
                    for l in lines {
                        lg(l);
                    }
                    if let Some(new) = sw {
                        for g in gs.iter_mut() {
                            if let Some(l) = new.get(g.pos).copied().flatten() {
                                g.rec.choice = l;
                                save_rec(path, &g.rec);
                            }
                        }
                        lg("restarting the mining process at the new lock(s)");
                        end_child(&mut ch, &c.stopfile);
                        cur = new;
                        break;
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn o() -> Opts {
        Opts { on: true, ..Default::default() }
    }

    #[test]
    fn ranges() {
        assert_eq!(parse_range("1200-1700:100").unwrap(), vec![1700, 1600, 1500, 1400, 1300, 1200]);
        assert_eq!(parse_range("1700-1200").unwrap(), vec![1700, 1600, 1500, 1400, 1300, 1200]);
        assert_eq!(parse_range("1450-1500:15").unwrap(), vec![1500, 1485, 1470, 1455]);
        assert_eq!(parse_range("1300,1600,1450,1600").unwrap(), vec![1600, 1450, 1300]);
        assert_eq!(parse_range("1500").unwrap(), vec![1500]);
        assert!(parse_range("abc").is_err());
        assert!(parse_range("1200-1700:5").unwrap_err().contains("step 5"));
        assert!(parse_range("100,1500").unwrap_err().contains("outside"));
        assert!(parse_range("300-4000:15").unwrap_err().contains("at most"));
        assert!(parse_range("1200-1700:x").is_err());
    }

    #[test]
    fn options() {
        let mut a = Opts::default();
        let mut v = || Ok("speed".to_string());
        assert!(arg(&mut a, "--auto-lock-goal", &mut v).unwrap());
        assert!(a.goal == Goal::Speed && a.sub && !a.on);
        assert!(arg(&mut a, "--auto-lock", &mut || Ok(String::new())).unwrap());
        assert!(a.on);
        assert!(arg(&mut a, "--auto-lock-max-loss", &mut || Ok("25".into())).unwrap());
        assert_eq!(a.loss, 25.0);
        assert!(arg(&mut a, "--auto-lock-max-loss", &mut || Ok("95".into())).is_err());
        assert!(arg(&mut a, "--auto-lock-goal", &mut || Ok("fast".into())).is_err());
        assert!(arg(&mut a, "--auto-lock-range", &mut || Ok("1400-1600".into())).unwrap());
        assert_eq!(a.list, vec![1600, 1500, 1400]);
        assert!(arg(&mut a, "--auto-lock-reset", &mut || Ok(String::new())).unwrap());
        assert!(a.reset);
        assert!(!arg(&mut a, "--auto-uv", &mut || Ok(String::new())).unwrap());
    }

    #[test]
    fn default_locks_by_card() {
        assert_eq!(defaults(0), vec![1800, 1700, 1600, 1500, 1400, 1300, 1200]);
        assert_eq!(defaults(2100), vec![1800, 1700, 1600, 1500, 1400, 1300, 1200]);
        assert_eq!(defaults(1590), vec![1500, 1400, 1300, 1200, 1100]); // a laptop GPU below 1600
        assert_eq!(candidates(&[1900, 1700, 1500], 1800, Goal::Eff), (vec![1700, 1500], vec![1900]));
        assert_eq!(candidates(&[1900, 1700], 0, Goal::Eff).0, vec![1900, 1700]);
    }

    fn pt(lock: u32, rate: f64, w: f64) -> Point {
        Point { lock, rate, w, clk: lock, ..Default::default() }
    }

    #[test]
    fn choice_and_cut() {
        let c = vec![1800, 1700, 1600, 1500, 1400];
        let mut r = Rec::default();
        for p in [pt(1800, 6.48, 118.4), pt(1700, 6.12, 106.2), pt(1600, 5.76, 95.1)] {
            r.put(p);
        }
        // 1600: 5.76 < 6.48 * 0.9 = 5.832: below the cut, so 1500 / 1400 are not measured
        assert!(below_cut(&r, &c, 1500, 10.0));
        assert_eq!(next_lock(&r, &c, 10.0), None);
        assert_eq!(choose(&r, &c, 10.0, Goal::Eff, None), 1700); // 57.6 MH/W within 10 %; 1600 has 60.6 but is below the cut
        assert_eq!(choose(&r, &c, 10.0, Goal::Speed, None), 1800);
        assert_eq!(choose(&r, &c, 15.0, Goal::Eff, None), 1600);
        assert_eq!(next_lock(&r, &c, 15.0), Some(1500)); // a larger loss measures the lower locks
        // invalid points never count, nor do locks outside the candidates
        r.put(Point { state: THROTTLED, ..pt(1800, 9.0, 100.0) });
        assert_eq!(choose(&r, &c, 10.0, Goal::Speed, None), 1700);
        assert_eq!(choose(&r, &[1600], 10.0, Goal::Eff, None), 1600);
        assert_eq!(choose(&Rec::default(), &c, 10.0, Goal::Eff, None), 0);
        // equal efficiency: the faster one
        let mut t = Rec::default();
        t.put(pt(1500, 2.0, 100.0));
        t.put(pt(1400, 1.9, 95.0));
        assert_eq!(choose(&t, &[1500, 1400], 10.0, Goal::Eff, None), 1500);
        // a crash marker turns into a crashed point
        let mut k = Rec { trying: 1700, ..Default::default() };
        assert_eq!(k.recover_trying(), Some(1700));
        assert_eq!(k.at(1700).unwrap().state, CRASHED);
        assert_eq!(k.trying, 0);
        assert_eq!(next_lock(&k, &c, 10.0), Some(1800));
    }

    #[test]
    fn table_marks_the_choice() {
        let c = vec![1800, 1700, 1600, 1500];
        let mut r = Rec { unit: "TH/s".into(), ..Default::default() };
        r.put(Point { offset: 210, ..pt(1800, 64.8, 118.4) });
        r.put(Point { offset: 225, ..pt(1700, 61.2, 106.2) });
        r.put(Point { lock: 1600, state: FAILED, ..Default::default() });
        r.put(Point { lock: 1500, state: SKIPPED, ..Default::default() });
        let t = table(&r, &c, 1700, None);
        assert!(t[0].contains("TH/s") && t[0].contains("TH/W"));
        assert!(t[2].contains("1700") && t[2].contains("+225") && t[2].contains("chosen"));
        assert!(!t[1].contains("chosen"));
        assert!(t[3].contains("FAILED"));
        assert!(t[4].contains("skipped"));
        assert_eq!(eff_unit("GH/s"), "GH/W");
        assert_eq!(eff_unit("MH/s"), "MH/W");
    }

    #[test]
    fn saved_file_round_trip() {
        let dir = std::env::temp_dir().join(format!("ffg-al-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir); // a state directory that does not exist yet is made by the first save
        let p = db_path(&dir);
        let _ = std::fs::remove_file(&p);
        let mut r = Rec { key: make_key("pearl", "00000000:0D:00.0", "NVIDIA GeForce RTX 3070", "595.71", Some(810), "A9h256"), unit: "TH/s".into(), range: vec![1600, 1500], loss: 10.0, choice: 1500, done: true, time: 5, ..Default::default() };
        r.put(Point { offset: 240, ..pt(1500, 61.5, 101.0) });
        r.put(Point { lock: 1600, state: CRASHED, ..Default::default() });
        save_rec(&p, &r);
        let r2 = Rec { key: "noid-eff|x".into(), choice: 1300, ..Default::default() };
        save_rec(&p, &r2);
        let db = load_db(&p);
        assert_eq!(db.len(), 2);
        assert_eq!(db[&r.key], r);
        assert_eq!(db["noid-eff|x"].choice, 1300);
        assert!(r.key.starts_with("pearl|00000000:0d:00.0|NVIDIA_GeForce_RTX_3070|595.71|mem810|"));
        // a foreign / broken file: ignored, replaced at the next save
        std::fs::write(&p, "garbage").unwrap();
        assert!(load_db(&p).is_empty());
        save_rec(&p, &r2);
        assert_eq!(load_db(&p).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stats_lines_of_every_coin() {
        let pearl = "[ 270s] GPU0 68.318 (199 W, 0.343 TH/W) [tuning +150, step 3/10]  GPU1 68.200 (190 W, 0.359 TH/W) | total 136.518 TH/s (5.2 att/s) | 389 W, 0.351 TH/W | hits 4 (row0 dropped 0, bad 0, qdrop 0) | user sub 6 acc 6 rej 0 | fee sub 0 acc 0 rej 0";
        let (u, e) = parse_stats(pearl).unwrap();
        assert_eq!(u, "TH/s");
        assert_eq!(e.len(), 2);
        assert_eq!(e[0], Entry { dev: 0, rate: 68.318, w: Some(199.0), dead: false, tagged: true });
        assert_eq!(e[1], Entry { dev: 1, rate: 68.2, w: Some(190.0), dead: false, tagged: false });
        let noid = "[  60s] GPU1 35.512 (77 W, 0.461 MH/W)  GPU2 0.000 DEAD | total 35.512 MH/s, 77 W, 0.461 MH/W | hits 3 (bad 0, stale 0) | user sub 4 acc 4 rej 0 (stale 0, skipped 0) | fee sub 0 acc 0 rej 0";
        let (u, e) = parse_stats(noid).unwrap();
        assert_eq!(u, "MH/s");
        assert!(e[1].dead && e[1].dev == 2 && e[1].w.is_none());
        let prox = "\x1b[0m[  60s] GPU0 3.210 (199 W, 0.0161 GH/W)  GPU1 2.580 | total 5.790 GH/s, 274 W, 0.0211 GH/W | hits 9 | user sub 9 acc 9 rej 0";
        let (u, e) = parse_stats(prox).unwrap();
        assert_eq!((u.as_str(), e.len(), e[1].w), ("GH/s", 2, None));
        assert_eq!(parse_stats("auto-uv: GPU0 (bus 0d) step +180: ok | total"), None);
        assert_eq!(parse_stats("[stratum:user] share ACCEPTED"), None);
    }

    #[test]
    fn clock_lines() {
        assert_eq!(parse_lock("clocks: GPU1 (bus 00000000:04:00.0): core clock locked to 1500 MHz; on exit: put back 1500 (HiveOS OC profile)"), Some(LockLine::Locked(1, 1500)));
        let r = parse_lock("clocks: GPU3 (bus x): cannot lock core clock (nvmlDeviceSetGpuLockedClocks: Not Supported (code 3)); needs root/Administrator and a driver that supports it").unwrap();
        match &r {
            LockLine::Refused(3, t) => assert_eq!(refusal_kind(t), 2),
            _ => panic!(),
        }
        assert_eq!(refusal_kind("nvmlDeviceSetGpuLockedClocks: Insufficient Permissions (code 4))"), 3);
        assert_eq!(refusal_kind("nvmlDeviceSetGpuLockedClocks: Invalid Argument (code 2))"), 1);
        assert_eq!(parse_lock("clocks: GPU1 (bus x): memory clock locked to 810 MHz"), None);
        assert_eq!(parse_lock("auto-uv: GPU1 lock 1500"), None);
    }

    #[test]
    fn child_command_line() {
        let argv: Vec<String> = "--pool p:1 --wallet w --auto-lock --auto-lock-range 1400-1600 --lock-cclock 1500,- --devices 0,1 --uv-reset --minutes 30 --auto-uv --lock-mclock 810"
            .split(' ')
            .map(String::from)
            .collect();
        let a = child_args(&argv, &[0, 1], &[Some(1500), Some(1600)], true, false);
        assert_eq!(a.join(" "), "--pool p:1 --wallet w --uv-reset --auto-uv --lock-mclock 810 --devices 0,1 --lock-cclock 1500,1600 --no-update-check");
        let a = child_args(&argv, &[0, 1], &[Some(1500), None], false, true);
        assert_eq!(a.join(" "), "--pool p:1 --wallet w --auto-uv --lock-mclock 810 --devices 0,1 --lock-cclock 1500,- --no-update-check");
        let a = child_args(&["--wallet".into(), "w".into(), "--auto-lock".into()], &[2], &[Some(1400)], false, true);
        assert_eq!(a.join(" "), "--wallet w --devices 2 --lock-cclock 1400 --auto-uv --no-update-check");
        let a = child_args(&[], &[0, 1], &[Some(1400), Some(1400)], false, false);
        assert!(a.join(" ").contains("--lock-cclock 1400 "));
    }

    fn e(rate: f64, tagged: bool) -> Entry {
        Entry { dev: 0, rate, w: Some(100.0), dead: false, tagged }
    }

    #[test]
    fn meter_waits_for_auto_uv_then_measures() {
        let o = Opts { settle: 30, measure: 60, ..o() };
        let mut m = Meter::new(1500, 0.0);
        m.on_lock(&LockLine::Locked(0, 1500));
        // auto-uv tunes: tagged, 0 hash rate
        for t in (10..300).step_by(10) {
            m.on_entry(t as f64, &e(0.0, true), &o);
        }
        assert!(m.done.is_none() && !m.measuring());
        let mut t = 300.0;
        while m.done.is_none() && t < 600.0 {
            m.on_entry(t, &e(60.0 + (t / 10.0) % 2.0, false), &o);
            for k in 0..10 {
                m.on_sample(Some(150.0), Some(1500 - k % 2), None);
            }
            t += 10.0;
        }
        let r = m.done.clone().unwrap();
        assert_eq!(r.state, OK);
        assert!(t <= 300.0 + 30.0 + 60.0 + 20.0, "took until {t}");
        assert!((r.rate - 60.5).abs() < 0.6 && r.w == 150.0 && r.clk >= 1499);
    }

    #[test]
    fn meter_flags_throttling_unsteady_and_dead() {
        let o = Opts { settle: 10, measure: 30, ..o() };
        // the clock does not hold: power / thermal limit
        let mut m = Meter::new(1800, 0.0);
        m.on_lock(&LockLine::Locked(0, 1800));
        for t in 1..10 {
            m.on_entry(t as f64 * 10.0, &e(50.0, false), &o);
            m.on_sample(Some(200.0), Some(1650), None);
        }
        assert_eq!(m.done.as_ref().unwrap().state, THROTTLED);
        // the clock runs far above the lock (GPU1 live test: another tool reset the lock, 1755 MHz at "1400")
        let mut m = Meter::new(1400, 0.0);
        m.on_lock(&LockLine::Locked(0, 1400));
        for t in 1..10 {
            m.on_entry(t as f64 * 10.0, &e(79.5, false), &o);
            m.on_sample(Some(199.0), Some(1755), None);
        }
        assert_eq!(m.done.as_ref().unwrap().state, NOT_HELD);
        // within the tolerance: valid
        let mut m = Meter::new(1400, 0.0);
        m.on_lock(&LockLine::Locked(0, 1400));
        for t in 1..10 {
            m.on_entry(t as f64 * 10.0, &e(70.0, false), &o);
            m.on_sample(Some(95.0), Some(1425), None);
        }
        assert_eq!(m.done.as_ref().unwrap().state, OK);
        // an auto-uv re-check during the measurement three times: unstable
        let mut m = Meter::new(1500, 0.0);
        m.on_lock(&LockLine::Locked(0, 1500));
        let mut t = 0.0;
        for _ in 0..3 {
            for _ in 0..3 {
                t += 10.0;
                m.on_entry(t, &e(50.0, false), &o);
            }
            t += 10.0;
            m.on_entry(t, &e(0.0, true), &o);
        }
        assert_eq!(m.done.as_ref().unwrap().state, UNSTABLE);
        // the GPU stops
        let mut m = Meter::new(1500, 0.0);
        m.on_entry(10.0, &Entry { dead: true, ..e(0.0, false) }, &o);
        assert_eq!(m.done.as_ref().unwrap().state, FAILED);
        // a refused lock
        let mut m = Meter::new(1500, 0.0);
        m.on_lock(&LockLine::Refused(0, "x: Not Supported (code 3))".into()));
        assert_eq!(m.done.as_ref().unwrap().refused, 2);
        // the lock never confirmed while it mines
        let mut m = Meter::new(1500, 0.0);
        for t in 1..20 {
            m.on_entry(t as f64 * 10.0, &e(50.0, false), &o);
        }
        assert!(m.done.as_ref().unwrap().why.contains("not confirmed"));
        // auto-uv never finishes
        let mut m = Meter::new(1500, 0.0);
        m.on_lock(&LockLine::Locked(0, 1500));
        m.tick(3601.0, &o);
        assert_eq!(m.done.as_ref().unwrap().state, FAILED);
    }

    /// A simulated RTX 3070-like GPU: hash rate ~ clock, power grows faster above 1500.
    fn sim(lock: u32) -> Meas {
        let rate = lock as f64 / 25.0;
        let w = 40.0 + (lock as f64 - 1000.0).max(0.0) * 0.12 + ((lock as f64 - 1500.0).max(0.0)).powf(1.3) * 0.05;
        Meas { state: OK, rate, w, clk: lock, offset: 200, ..Default::default() }
    }

    #[test]
    fn sweep_on_a_simulated_gpu() {
        let o = Opts { list: vec![1800, 1700, 1600, 1500, 1400, 1300, 1200], ..o() };
        let mut gs = vec![Sw { pos: 0, label: "GPU0".into(), cands: o.list.clone(), rec: Rec { key: "k0".into(), ..Default::default() }, finished: false }];
        let mut rounds = Vec::new();
        let mut saved = Vec::new();
        let end = sweep(
            &mut gs,
            &o,
            &mut |_, todo| {
                rounds.push(todo.to_vec());
                RoundRes { results: todo.iter().map(|&(p, l)| (p, sim(l))).collect(), unit: "TH/s".into(), ..Default::default() }
            },
            &mut |r| saved.push(r.clone()),
            &mut |_| true,
            &mut || None,
        );
        assert_eq!(end, End::Done);
        // 1800 72.0, 1700 68.0, 1600 64.0 (-11 %: below the cut) -> 1500 .. 1200 not measured
        assert_eq!(rounds.iter().map(|r| r[0].1).collect::<Vec<_>>(), vec![1800, 1700, 1600]);
        let r = &gs[0].rec;
        assert!(r.done && r.choice == 1700 && r.unit == "TH/s" && r.trying == 0);
        assert_eq!(r.at(1500).unwrap().state, SKIPPED);
        // every lock was marked as tried before its round (crash marker), and cleared after
        assert!(saved.iter().any(|s| s.trying == 1800) && saved.last().unwrap().trying == 0);
        // the next start with a larger loss measures only the missing locks
        let o2 = Opts { loss: 25.0, ..o.clone() };
        let mut rec = r.clone();
        rec.pts.retain(|p| p.state != SKIPPED);
        let mut gs = vec![Sw { pos: 0, label: "GPU0".into(), cands: o.list.clone(), rec, finished: false }];
        let mut measured = Vec::new();
        sweep(&mut gs, &o2, &mut |_, todo| {
            measured.extend(todo.iter().map(|t| t.1));
            RoundRes { results: todo.iter().map(|&(p, l)| (p, sim(l))).collect(), unit: "TH/s".into(), ..Default::default() }
        }, &mut |_| {}, &mut |_| true, &mut || None);
        assert_eq!(measured, vec![1500, 1400, 1300]); // 1300 (52.0) is below the new cut (54.0): 1200 is not measured
        assert!(gs[0].rec.choice <= 1600 && gs[0].rec.choice >= 1400);
    }

    #[test]
    fn sweep_of_two_gpus_in_parallel_with_failures() {
        let o = Opts { list: vec![1600, 1500, 1400], ..o() };
        let mk = |p: usize| Sw { pos: p, label: format!("GPU{p}"), cands: o.list.clone(), rec: Rec { key: format!("k{p}"), ..Default::default() }, finished: false };
        let mut gs = vec![mk(0), mk(1)];
        let mut rounds = Vec::new();
        let end = sweep(
            &mut gs,
            &o,
            &mut |_, todo| {
                rounds.push(todo.to_vec());
                RoundRes {
                    results: todo
                        .iter()
                        .map(|&(p, l)| (p, if p == 1 && l == 1600 { Meas { state: FAILED, why: "Xid".into(), ..Default::default() } } else if p == 1 && l == 1500 { Meas { state: THROTTLED, ..sim(l) } } else { sim(l) }))
                        .collect(),
                    unit: "GH/s".into(),
                    ..Default::default()
                }
            },
            &mut |_| {},
            &mut |_| true,
            &mut || None,
        );
        assert_eq!(end, End::Done);
        assert_eq!(rounds[0], vec![(0, 1600), (1, 1600)]);
        assert_eq!(rounds[1], vec![(0, 1500), (1, 1500)]);
        assert_eq!(gs[1].rec.choice, 1400); // 1600 failed, 1500 throttled: 1400 is its only valid point
        assert!(gs[0].rec.choice >= 1400);
    }

    #[test]
    fn a_lock_that_does_not_hold_turns_auto_lock_off_for_that_gpu() {
        // FFGN 1.0.4 (RTX 2060 SUPER, driver 620: NVML "locked to 1400", the card ran 1470-2055): no more locks are swept; 1.8.0:
        // the same for every coin (Pearl key here)
        let o = Opts { list: vec![1600, 1500, 1400], ..o() };
        for key in ["pearl|0000:01:00.0|x|620|mem810|k", "neb|0000:01:00.0|x|620|mem0|neb-v2", "noid-eff|0000:01:00.0|x|620|mem810|k"] {
            let mut gs = vec![Sw { pos: 0, label: "GPU0".into(), cands: o.list.clone(), rec: Rec { key: key.into(), ..Default::default() }, finished: false }];
            let mut rounds = 0;
            let end = sweep(&mut gs, &o, &mut |_, t| {
                rounds += 1;
                RoundRes { results: vec![(t[0].0, Meas { state: NOT_HELD, clk: 1470, why: "the miner reported the lock 1600 MHz not holding".into(), rate: 10.0, w: 90.0, ..Default::default() })], ..Default::default() }
            }, &mut |_| {}, &mut |_| true, &mut || None);
            assert_eq!(end, End::Done);
            assert_eq!(rounds, 1, "{key}");
            assert!(gs[0].rec.unheld && gs[0].rec.choice == 0 && gs[0].finished, "{key}");
            let back = rec_from(&gs[0].rec.key, &rec_json(&gs[0].rec));
            assert!(back.unheld);
        }
        // the child's warning line ends the measurement of that GPU at once
        let line = crate::clocks::not_held_message(3, 1400, 1470);
        assert_eq!(parse_unheld(&line), Some((3, 1400, 1470)));
        assert_eq!(parse_unheld("auto-uv: GPU0 step +30: ok"), None);
        let mut m = Meter::new(1400, 0.0);
        m.unheld(1400, 1470);
        assert_eq!(m.done.as_ref().map(|d| (d.state, d.clk)), Some((NOT_HELD, 1470)));
    }

    #[test]
    fn sweep_without_locks_permission_or_mining() {
        let o = Opts { list: vec![1600, 1500], ..o() };
        let mk = || vec![Sw { pos: 0, label: "GPU0".into(), cands: o.list.clone(), rec: Rec { key: "k".into(), ..Default::default() }, finished: false }];
        // locks not supported on this GPU: saved as such, no choice
        let mut gs = mk();
        let end = sweep(&mut gs, &o, &mut |_, t| RoundRes { results: vec![(t[0].0, Meas { state: FAILED, refused: 2, ..Default::default() })], ..Default::default() }, &mut |_| {}, &mut |_| true, &mut || None);
        assert_eq!(end, End::Done);
        assert!(gs[0].rec.unsupported && gs[0].rec.choice == 0 && gs[0].finished);
        // no permission: given up at once
        let mut gs = mk();
        let end = sweep(&mut gs, &o, &mut |_, t| RoundRes { results: vec![(t[0].0, Meas { state: FAILED, refused: 3, ..Default::default() })], ..Default::default() }, &mut |_| {}, &mut |_| true, &mut || None);
        assert_eq!(end, End::GaveUp);
        assert!(gs[0].rec.pts.is_empty());
        // no permission with two GPUs in the round: neither keeps a crash marker on disk
        let mut gs = mk();
        gs.push(Sw { pos: 1, label: "GPU1".into(), cands: o.list.clone(), rec: Rec { key: "k1".into(), ..Default::default() }, finished: false });
        let mut disk: BTreeMap<String, Rec> = BTreeMap::new();
        let end = sweep(
            &mut gs,
            &o,
            &mut |_, t| RoundRes { results: t.iter().map(|&(p, _)| (p, Meas { state: FAILED, refused: 3, ..Default::default() })).collect(), ..Default::default() },
            &mut |r| {
                disk.insert(r.key.clone(), r.clone());
            },
            &mut |_| true,
            &mut || None,
        );
        assert_eq!(end, End::GaveUp);
        assert!(disk.values().all(|r| r.trying == 0) && disk.len() == 2);
        // the child never mines (pool down): 3 tries, nothing recorded as failed
        let mut gs = mk();
        let mut n = 0;
        let end = sweep(&mut gs, &o, &mut |_, _| {
            n += 1;
            RoundRes { aborted: Some("exit 1".into()), ..Default::default() }
        }, &mut |_| {}, &mut |_| true, &mut || None);
        assert_eq!((end, n), (End::GaveUp, 3));
        assert!(gs[0].rec.pts.is_empty() && gs[0].rec.trying == 0);
        // the miner is stopped mid-round: nothing recorded, no crash marker left
        let mut gs = mk();
        let mut last = Rec::default();
        let end = sweep(&mut gs, &o, &mut |_, _| RoundRes { stopped: true, ..Default::default() }, &mut |r| last = r.clone(), &mut |_| true, &mut || None);
        assert_eq!(end, End::Stopped);
        assert_eq!(last.trying, 0);
        assert!(last.pts.is_empty());
    }

    #[test]
    fn profit_options_and_defaults() {
        let mut a = Opts::default();
        assert!(arg(&mut a, "--auto-lock-goal", &mut || Ok("profit".into())).unwrap());
        assert_eq!(a.goal, Goal::Profit);
        assert!(arg(&mut a, "--power-price", &mut || Ok("0.12,0.053@23-07".into())).unwrap());
        assert!((a.tariff.as_ref().unwrap().avg() - 0.0976667).abs() < 1e-6);
        assert!(arg(&mut a, "--coin-price", &mut || Ok("0.034".into())).unwrap());
        assert_eq!(a.coin_price, Some(0.034));
        assert!(arg(&mut a, "--coin-price", &mut || Ok("0".into())).is_err());
        assert!(arg(&mut a, "--pool-fee", &mut || Ok("1%".into())).unwrap());
        assert_eq!(a.pool_fee, Some(1.0));
        assert!(arg(&mut a, "--auto-lock-reprice-hours", &mut || Ok("0".into())).unwrap());
        assert_eq!(a.reprice_h, 0.0);
        assert!(arg(&mut a, "--auto-lock-hysteresis", &mut || Ok("5".into())).unwrap());
        assert_eq!(a.hyst, 5.0);
        assert!(arg(&mut a, "--power-price", &mut || Ok("cheap".into())).is_err());
        assert_eq!(Opts::default().reprice_h, 6.0);
        assert_eq!(Opts::default().hyst, 3.0);
        assert_eq!(defaults_profit(2100), vec![2000, 1900, 1800, 1700, 1600, 1500, 1400, 1300, 1200]);
        assert_eq!(defaults_profit(1590), vec![1500, 1400, 1300, 1200, 1100]);
        assert_eq!(candidates(&[], 2100, Goal::Profit).0[0], 2000);
        assert_eq!(candidates(&[], 2100, Goal::Eff).0[0], 1800);
        assert_eq!(cut_loss(&Opts { goal: Goal::Profit, ..Opts::default() }), 100.0);
        // the money options never reach a round's miner process
        let argv: Vec<String> = "--pool p --auto-lock --auto-lock-goal profit --power-price 0.12,0.053@23-07 --coin-price 0.03 --pool-fee 1 --auto-lock-reprice-hours 6 --auto-lock-hysteresis 3 --auto-uv"
            .split(' ')
            .map(String::from)
            .collect();
        assert_eq!(child_args(&argv, &[0], &[Some(1500)], false, false).join(" "), "--pool p --auto-uv --devices 0 --lock-cclock 1500 --no-update-check");
        // the saved goal
        let r = Rec { key: "k".into(), goal: Goal::Profit, ..Default::default() };
        assert_eq!(rec_from("k", &rec_json(&r)).goal, Goal::Profit);
    }

    #[test]
    fn network_lines() {
        assert_eq!(parse_net("auto-lock: network pearl height 143562 work 1.690137e22"), Some(("pearl".into(), 143562, 1.690137e22)));
        assert_eq!(parse_net("\x1b[0mauto-lock: network noid height 239348 work 3.5e15").unwrap().0, "noid");
        assert_eq!(parse_net("auto-lock: network pearl height x work 1"), None);
        assert_eq!(parse_net("auto-lock: network pearl height 5 work -1"), None);
        assert_eq!(parse_net("[ 60s] GPU0 1.0"), None);
    }

    /// The 3070 at PROX (example of the owner, 15.11.2026: 0.034 $, 0.0977 $/kWh): goal eff takes 1400, goal profit a high lock.
    #[test]
    fn profit_choice_and_rechoice() {
        let c = vec![1800, 1700, 1600, 1500, 1400];
        let mut r = Rec { unit: "GH/s".into(), ..Default::default() };
        for (l, g, w) in [(1800, 3.10, 115.0), (1700, 2.95, 103.0), (1600, 2.80, 93.0), (1500, 2.641, 84.9), (1400, 2.483, 75.3)] {
            r.put(Point { lock: l, rate: g, w, clk: l, ..Default::default() });
        }
        let net = crate::profit::prox_net_api(r#"{"network":{"attempts_per_slot":"471040000000000","slot_seconds":10.24},"reward_motes":"95000000"}"#).unwrap();
        let e = Econ { coin: Coin::Prox, price: 0.034, per_hs_day: net.per_hs_day, pool_fee: 0.0, kwh: crate::profit::Tariff::parse("0.12,0.053@23-07").unwrap().avg() };
        // by hand: 1500: 2.641e9/46e12*8437.5*95 = 46.02 PROX * 0.99 * 0.034 = 1.549 $ - 84.9 W * 24 / 1000 * 0.09767 = 0.199 -> 1.350
        let p1500 = e.profit(2.641, "GH/s", 84.9);
        assert!((p1500 - (2.641e9 / 46e12 * 86400.0 / 10.24 * 95.0 * 0.99 * 0.034 - 84.9 * 0.024 * 0.0976667)).abs() < 1e-4, "{p1500}");
        assert!((p1500 - 1.350).abs() < 0.002, "{p1500}");
        assert_eq!(choose(&r, &c, 10.0, Goal::Eff, None), 1600); // the best GH/W within 10 % of 3.10
        assert_eq!(choose(&r, &[1500, 1400], 10.0, Goal::Eff, None), 1400); // the example: eff takes 1400 of 1500 / 1400
        assert_eq!(choose(&r, &[1500, 1400], 10.0, Goal::Profit, Some(&e)), 1500); // profit 1500
        assert_eq!(choose(&r, &c, 10.0, Goal::Profit, Some(&e)), 1800);
        // without the money data: as eff
        assert_eq!(choose(&r, &c, 10.0, Goal::Profit, None), 1600);
        // invalid points never count
        r.put(Point { lock: 1800, rate: 3.10, w: 115.0, clk: 1650, state: THROTTLED, ..Default::default() });
        assert_eq!(choose(&r, &c, 10.0, Goal::Profit, Some(&e)), 1700);
        // electricity dear: the low lock wins
        let dear = Econ { kwh: 0.60, ..e.clone() };
        assert_eq!(choose(&r, &c, 10.0, Goal::Profit, Some(&dear)), 1400);
        // the table: $ columns, the choice marked
        let t = table(&r, &c, 1700, Some(&e));
        assert!(t[0].contains("income") && t[0].contains("profit") && t[0].contains("GH/W"));
        assert!(t[2].contains("1700") && t[2].contains("chosen") && t[2].contains(&format!("{:.3}", e.profit(2.95, "GH/s", 103.0))));
        // re-choice with hysteresis: the best lock unchanged stays, a big change switches
        let gs = vec![Sw { pos: 0, label: "GPU1".into(), cands: c.clone(), rec: r.clone(), finished: true }];
        let (sw, lines) = rechoose(&gs, &[Some(1700)], &e, 3.0);
        assert!(sw.is_none() && lines[0].contains("still the best"), "{lines:?}");
        let (sw, lines) = rechoose(&gs, &[Some(1700)], &dear, 3.0);
        assert_eq!(sw, Some(vec![Some(1400)]));
        assert!(lines[0].contains("1700 -> 1400"), "{lines:?}");
        // a small gain stays below the hysteresis
        let mut r2 = Rec { unit: "GH/s".into(), ..Default::default() };
        r2.put(Point { lock: 1600, rate: 2.80, w: 93.0, clk: 1600, ..Default::default() });
        r2.put(Point { lock: 1500, rate: 2.78, w: 92.0, clk: 1500, ..Default::default() });
        let gs2 = vec![Sw { pos: 0, label: "GPU0".into(), cands: vec![1600, 1500], rec: r2, finished: true }];
        let (sw, lines) = rechoose(&gs2, &[Some(1500)], &e, 3.0);
        assert!(sw.is_none() && lines[0].contains("keeps lock 1500"), "{lines:?}");
        let (sw, _) = rechoose(&gs2, &[Some(1500)], &e, 0.0);
        assert_eq!(sw, Some(vec![Some(1600)]));
    }

    #[test]
    fn profit_sweep_measures_every_lock() {
        // goal profit: no cut by hash rate, every lock of the range is measured (sim: 1600 is 11 % below 1800)
        let o = Opts { list: vec![1800, 1700, 1600, 1500, 1400], goal: Goal::Profit, ..o() };
        let mut gs = vec![Sw { pos: 0, label: "GPU0".into(), cands: o.list.clone(), rec: Rec { key: "k0".into(), ..Default::default() }, finished: false }];
        let mut measured = Vec::new();
        let e = Econ { coin: Coin::Pearl, price: 1.5, per_hs_day: 7.0e-16, pool_fee: 1.0, kwh: 0.0977 };
        let end = sweep(
            &mut gs,
            &o,
            &mut |_, todo| {
                measured.extend(todo.iter().map(|t| t.1));
                RoundRes { results: todo.iter().map(|&(p, l)| (p, sim(l))).collect(), unit: "TH/s".into(), ..Default::default() }
            },
            &mut |_| {},
            &mut |_| true,
            &mut || Some(e.clone()),
        );
        assert_eq!(end, End::Done);
        assert_eq!(measured, vec![1800, 1700, 1600, 1500, 1400]);
        let r = &gs[0].rec;
        assert_eq!(r.goal, Goal::Profit);
        let best = r.pts.iter().max_by(|a, b| e.profit(a.rate, "TH/s", a.w).partial_cmp(&e.profit(b.rate, "TH/s", b.w)).unwrap()).unwrap().lock;
        assert_eq!(r.choice, best);
    }

    #[test]
    fn cmp_cards_start_below_the_power_limit() {
        // CMP 50HX: NVML max 2250, the default top was 2000 (894 W on 4 cards): now 1500 for both goals
        assert_eq!(defaults_for(2250, "NVIDIA CMP 50HX", Goal::Profit), vec![1500, 1400, 1300, 1200, 1100]);
        assert_eq!(defaults_for(2250, "NVIDIA CMP 50HX", Goal::Eff), vec![1500, 1400, 1300, 1200, 1100]);
        assert_eq!(defaults_for(0, "NVIDIA CMP 40HX", Goal::Profit)[0], 1500);
        // a CMP that cannot reach 1500 keeps its own maximum
        assert_eq!(defaults_for(1400, "NVIDIA CMP 30HX", Goal::Profit)[0], 1400);
        // ordinary cards: the general defaults
        assert_eq!(defaults_for(2100, "NVIDIA GeForce RTX 4070 Ti", Goal::Profit)[0], 2000);
        assert_eq!(defaults_for(2100, "NVIDIA GeForce RTX 4070 Ti", Goal::Eff)[0], 1800);
        assert_eq!(defaults_for(0, "NVIDIA GeForce RTX 5090", Goal::Profit)[0], 2000);
        assert_eq!(candidates_for(&[], 2250, Goal::Profit, "NVIDIA CMP 50HX").0[0], 1500);
        // an explicit --auto-lock-range wins, even above the safe top
        assert_eq!(candidates_for(&[2000, 1900], 2250, Goal::Profit, "NVIDIA CMP 50HX").0, vec![2000, 1900]);
        assert_eq!(safe_top("NVIDIA GeForce RTX 4090"), None);
    }

    #[test]
    fn a_step_at_the_power_limit_is_pl_limited_without_waiting() {
        let mut m = Meter::new(2000, 0.0).with_pl(Some(225.0));
        m.on_lock(&LockLine::Locked(0, 2000));
        // the card draws its power limit and runs 1650 MHz at the 2000 lock: 19 s are not enough, the 20th ends the step
        for _ in 0..19 {
            m.on_sample(Some(219.0), Some(1650), None);
            assert!(m.done.is_none());
        }
        m.on_sample(Some(219.0), Some(1650), None);
        let d = m.done.as_ref().expect("PL-limited");
        assert_eq!(d.state, THROTTLED);
        assert!(d.why.starts_with("PL-limited"), "{}", d.why);
        assert_eq!(d.clk, 1650);
        // a card far under its limit with a low clock (80 %: thermal, no job) is not PL-limited; a clock at the lock at the limit is valid
        let mut m = Meter::new(1500, 0.0).with_pl(Some(225.0));
        m.on_lock(&LockLine::Locked(0, 1500));
        for _ in 0..100 {
            m.on_sample(Some(180.0), Some(1300), None);
        }
        assert!(m.done.is_none());
        for _ in 0..100 {
            m.on_sample(Some(222.0), Some(1500), None);
        }
        assert!(m.done.is_none());
        // even with the power cap bit: a card that holds its lock is valid
        for _ in 0..100 {
            m.on_sample(Some(222.0), Some(1500), Some(4));
        }
        assert!(m.done.is_none());
        // without a known limit / before the lock is confirmed nothing is decided early
        let mut m = Meter::new(2000, 0.0);
        m.on_lock(&LockLine::Locked(0, 2000));
        for _ in 0..60 {
            m.on_sample(Some(219.0), Some(1650), None);
        }
        assert!(m.done.is_none());
        let mut m = Meter::new(2000, 0.0).with_pl(Some(225.0));
        for _ in 0..60 {
            m.on_sample(Some(219.0), Some(1650), None);
        }
        assert!(m.done.is_none());
    }

    /// The 1 s trace of an RTX 3080 Ti (350 W limit) at a 2000 MHz lock, measured on the AORUS box (nvidia-smi, 1.7.8 test): the
    /// power cap throws the clock between 1830 and 2010 MHz every second or two, 320-335 W, SW power cap bit most of the time.
    const TRACE_3080TI_2000: [(f64, u32, u64); 20] = [
        (324.7, 2010, 4), (319.7, 1860, 0), (320.8, 1875, 4), (321.8, 2010, 4), (320.4, 2010, 0), (320.4, 1845, 4), (321.0, 1875, 4),
        (323.8, 2010, 4), (320.5, 1980, 1), (326.1, 1860, 4), (325.3, 1995, 4), (324.5, 2010, 4), (326.1, 1830, 1), (328.5, 1860, 4),
        (324.3, 2010, 4), (323.9, 1905, 1), (329.0, 1860, 4), (328.3, 2010, 4), (329.1, 2010, 4), (327.1, 1785, 1),
    ];

    #[test]
    fn pl_limited_is_found_in_the_first_20_seconds_on_the_real_trace_of_a_3080_ti() {
        // with the power cap bit
        let mut m = Meter::new(2000, 0.0).with_pl(Some(350.0));
        m.on_lock(&LockLine::Locked(0, 2000));
        for (i, &(w, c, r)) in TRACE_3080TI_2000.iter().enumerate() {
            assert!(m.done.is_none(), "decided too early at {i}");
            m.on_sample(Some(w), Some(c), Some(r));
        }
        let d = m.done.as_ref().expect("PL-limited within 20 s");
        assert_eq!(d.state, THROTTLED);
        assert!(d.why.starts_with("PL-limited"), "{}", d.why);
        assert!(d.clk > 1900 && d.clk < 1960, "{}", d.clk);
        // without the reasons (an old driver): the clock alone, 45 % of the samples under the lock
        let mut m = Meter::new(2000, 0.0).with_pl(Some(350.0));
        m.on_lock(&LockLine::Locked(0, 2000));
        for &(w, c, _) in &TRACE_3080TI_2000 {
            m.on_sample(Some(w), Some(c), None);
        }
        assert!(m.done.is_some(), "clock-only detection");
        // an idle gap (the auto-uv check stops the load for a few seconds) does not reset the count
        let mut m = Meter::new(2000, 0.0).with_pl(Some(350.0));
        m.on_lock(&LockLine::Locked(0, 2000));
        for (i, &(w, c, r)) in TRACE_3080TI_2000.iter().enumerate() {
            if i == 10 {
                for _ in 0..5 {
                    m.on_sample(Some(43.0), Some(1200), Some(1));
                }
            }
            m.on_sample(Some(w), Some(c), Some(r));
        }
        assert!(m.done.is_some());
        // the same card at the 1800 lock draws 255 W and holds it: nothing to decide
        let mut m = Meter::new(1800, 0.0).with_pl(Some(350.0));
        m.on_lock(&LockLine::Locked(0, 1800));
        for i in 0..200 {
            m.on_sample(Some(254.0 + (i % 5) as f64), Some(1800), Some(if i % 7 == 0 { 4 } else { 1 }));
        }
        assert!(m.done.is_none());
    }

    #[test]
    fn rtx_30_series_start_below_the_power_limit() {
        // RTX 3080 Ti (PL 350 W): 2000 and 1900 sat at the limit, 150-170 MHz under the lock: the default top is 1800 for both goals
        for name in ["NVIDIA GeForce RTX 3080 Ti", "NVIDIA GeForce RTX 3070 Laptop GPU", "NVIDIA GeForce RTX 3060", "NVIDIA RTX A4000"] {
            assert_eq!(defaults_for(2100, name, Goal::Profit), vec![1800, 1700, 1600, 1500, 1400, 1300, 1200], "{name}");
            assert_eq!(defaults_for(2100, name, Goal::Eff)[0], 1800, "{name}");
        }
        // 40 / 50 series and unknown names are not capped
        assert_eq!(safe_top("NVIDIA GeForce RTX 4080"), None);
        assert_eq!(safe_top("NVIDIA GeForce RTX 5070"), None);
        assert_eq!(safe_top("NVIDIA RTX 6000 Ada Generation"), None);
        assert_eq!(safe_top("?"), None);
        // an explicit --auto-lock-range wins
        assert_eq!(candidates_for(&[2000, 1900], 2100, Goal::Profit, "NVIDIA GeForce RTX 3080 Ti").0, vec![2000, 1900]);
    }

    #[test]
    fn minutes_limit_stops_the_sweep() {
        assert_eq!(minutes_arg(&["--pool".into(), "p:1".into(), "--minutes".into(), "4".into()]), Some(4.0));
        assert_eq!(minutes_arg(&["--minutes".into(), "0.5".into()]), Some(0.5));
        assert_eq!(minutes_arg(&["--minutes".into(), "x".into()]), None);
        assert_eq!(minutes_arg(&["--auto-lock".into()]), None);
        // time over before a round: no round is started
        let mut gs = vec![Sw { pos: 0, label: "GPU0".into(), cands: vec![1800, 1700], rec: Rec::default(), finished: false }];
        let mut o = o();
        o.deadline = Some(Instant::now());
        let mut rounds = 0;
        let end = sweep(&mut gs, &o, &mut |_, _| { rounds += 1; RoundRes::default() }, &mut |_| {}, &mut |_| true, &mut || None);
        assert_eq!(end, End::Stopped);
        assert_eq!(rounds, 0);
        // the time runs out during a round: that round's results are kept, the next one is not started
        let mut gs = vec![Sw { pos: 0, label: "GPU0".into(), cands: vec![1800, 1700, 1600], rec: Rec::default(), finished: false }];
        let mut o = o;
        o.deadline = Some(Instant::now() + Duration::from_millis(150));
        let mut rounds = 0;
        let end = sweep(
            &mut gs,
            &o,
            &mut |_, todo| {
                rounds += 1;
                std::thread::sleep(Duration::from_millis(200));
                RoundRes { results: todo.iter().map(|&(p, _)| (p, Meas { state: OK, rate: 100.0, w: 200.0, clk: 1700, ..Default::default() })).collect(), unit: "TH/s".into(), ..Default::default() }
            },
            &mut |_| {},
            &mut |_| true,
            &mut || None,
        );
        assert_eq!(end, End::Stopped);
        assert_eq!(rounds, 1);
        assert!(gs[0].rec.at(1800).is_some());
        assert!(gs[0].rec.at(1700).is_none());
        // no --minutes: the sweep is not limited; the time left for the mining after a sweep never drops to zero
        assert!(!time_up(None));
        assert!(remaining_minutes(Some(0.0)).unwrap() > 0.0);
        assert_eq!(remaining_minutes(None), None);
    }
}
