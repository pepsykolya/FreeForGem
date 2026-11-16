// FreeForGem — see LICENSE
//! Pool lists, failover and failback (1.7.8), the same behavior for Pearl, NOID and PROX (CMFD: cmfd.rs drives its engine with
//! the same decision code); the reference is ffgs 0.5.4 (`failback.h`), flag names included.
//!
//! `--pool A,B,C` (or `--pool A --pool B`): the first entry is the PRIMARY, the others are backups of the same coin / protocol.
//!   * failover: a connection that could not be established (connect, login, no job) or died quickly moves to the next entry; a
//!     connection that lived a while is retried on the same entry first.
//!   * failback: while the session is on a backup, the primary is checked every `--failback-minutes` (default 10, 0 = off) on a
//!     SEPARATE connection that submits nothing (connect + login + a usable job, the same session code as mining uses). After 2
//!     good checks in a row (the second one 60 s after the first) the session switches back: the GPUs keep their job, the shares
//!     found meanwhile are sent on the old connection (drain), then the connection goes to the primary.
//!   * a primary that was left again soon after a failback is checked less often (x2 up to x16), so a flapping pool is not
//!     switched to every 10 minutes.
//!   * the dev fee login follows the user's entry (it connects to the pool the user is on whenever its slot starts), so the fee
//!     always goes to the pool the user mines on.
use std::io;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

pub const BACKOFF_MIN: Duration = Duration::from_secs(2);
pub const BACKOFF_MAX: Duration = Duration::from_secs(60);
/// A connection that lived this long counts as established: it is retried on the same entry before the next one is tried.
pub const ESTABLISHED: Duration = Duration::from_secs(60);

#[derive(Clone, Debug)]
pub struct Opts {
    /// `--failback-minutes`: check the primary every N minutes while on a backup; 0 = off
    pub failback_min: f64,
    /// good checks in a row before switching back
    pub ok_probes: u32,
    /// the next check after a good one
    pub confirm_s: f64,
    /// the longest check-interval multiplier for a primary that was left again soon after a failback
    pub hold_max: u32,
    /// a primary left within this many seconds of a failback counts as "left bad"
    pub bad_within_s: f64,
    /// 2 h on the primary: trusted again (multiplier back to 1)
    pub trust_s: f64,
    /// how long a check waits for a usable job
    pub probe_timeout: Duration,
    /// prober loop period
    pub tick: Duration,
    /// `--noshare-minutes`: no accepted share of the user for this long while hashing (and at least `noshare_mult` shares expected): the
    /// pool is not crediting; 0 = the watchdog is off
    pub noshare_min: f64,
    /// shares that must have been expected in the window (rare big-difficulty shares must not trigger it)
    pub noshare_mult: f64,
    /// `--reject-minutes`: only rejected shares (at least `reject_count`, none accepted) for this long; 0 = off
    pub reject_min: f64,
    pub reject_count: u64,
}

impl Default for Opts {
    fn default() -> Self {
        Opts {
            failback_min: 10.0,
            ok_probes: 2,
            confirm_s: 60.0,
            hold_max: 16,
            bad_within_s: 300.0,
            trust_s: 7200.0,
            probe_timeout: Duration::from_secs(30),
            tick: Duration::from_secs(2),
            noshare_min: 15.0,
            noshare_mult: 10.0,
            reject_min: 5.0,
            reject_count: 5,
        }
    }
}

static OPTS: OnceLock<RwLock<Opts>> = OnceLock::new();

fn cell() -> &'static RwLock<Opts> {
    OPTS.get_or_init(|| {
        let mut o = Opts::default();
        // test hooks (integration tests with local mock pools): seconds between the two good checks, check timeout, loop period
        if let Some(v) = std::env::var("FFGP_FAILBACK_CONFIRM_S").ok().and_then(|v| v.parse::<f64>().ok()) {
            o.confirm_s = v.max(0.0);
        }
        if let Some(v) = std::env::var("FFGP_FAILBACK_TICK_MS").ok().and_then(|v| v.parse::<u64>().ok()) {
            o.tick = Duration::from_millis(v.max(20));
        }
        if let Some(v) = std::env::var("FFGP_FAILBACK_PROBE_S").ok().and_then(|v| v.parse::<f64>().ok()) {
            o.probe_timeout = Duration::from_secs_f64(v.max(0.5));
        }
        RwLock::new(o)
    })
}

pub fn opts() -> Opts {
    cell().read().unwrap().clone()
}

/// Test hook: timings of the check loop (unit / integration tests with local mock pools).
#[cfg(test)]
pub fn set_test_timing(failback_min: f64, confirm_s: f64, tick_ms: u64) {
    let mut o = cell().write().unwrap();
    o.failback_min = failback_min;
    o.confirm_s = confirm_s;
    o.tick = Duration::from_millis(tick_ms);
    o.probe_timeout = Duration::from_secs(5);
}

/// `--noshare-minutes M` / `--reject-minutes K` (0 = off).
pub fn set_watchdog(noshare_min: Option<f64>, reject_min: Option<f64>) {
    let mut o = cell().write().unwrap();
    if let Some(m) = noshare_min {
        o.noshare_min = m.max(0.0);
    }
    if let Some(m) = reject_min {
        o.reject_min = m.max(0.0);
    }
}

/// Test hook: watchdog windows in seconds.
pub fn set_watchdog_secs(noshare_s: f64, reject_s: f64, mult: f64) {
    let mut o = cell().write().unwrap();
    o.noshare_min = noshare_s / 60.0;
    o.reject_min = reject_s / 60.0;
    o.noshare_mult = mult;
}

/// `--failback-minutes N` (0 = off; fractions allowed).
pub fn set_failback_minutes(m: f64) {
    cell().write().unwrap().failback_min = m.max(0.0);
}

/// The value of `--failback-minutes`.
pub fn parse_minutes(s: &str) -> Result<f64, String> {
    let m: f64 = s.trim().parse().map_err(|_| format!("--failback-minutes {s:?}: a number of minutes (0 = off)"))?;
    if !m.is_finite() || m < 0.0 {
        return Err(format!("--failback-minutes {s:?}: must be 0 (off) or more"));
    }
    Ok(m)
}

/// One `--pool` value: the entries of a comma / space separated list (a URL never contains either).
pub fn split_list(s: &str) -> Vec<String> {
    s.split(|c: char| c == ',' || c.is_whitespace()).map(str::trim).filter(|x| !x.is_empty()).map(String::from).collect()
}

/// Compact `host:port` of an entry for log lines (no scheme, no query: pins and secrets stay out of the log).
pub fn label(entry: &str) -> String {
    let e = entry.split_once("://").map_or(entry, |x| x.1);
    e.split(['?', '/']).next().unwrap_or(e).to_string()
}

pub fn now_s() -> f64 {
    static T0: OnceLock<Instant> = OnceLock::new();
    T0.get_or_init(Instant::now).elapsed().as_secs_f64()
}

// ---------------------------------------------------------------------------------------------------------------------------------
// failback decision (pure, unit-tested)

#[derive(Debug, Clone)]
pub struct Failback {
    next: Option<f64>,
    streak: u32,
    pub hold: u32,
    primary_since: Option<f64>,
}

impl Default for Failback {
    fn default() -> Self {
        Failback { next: None, streak: 0, hold: 1, primary_since: None }
    }
}

impl Failback {
    pub fn next(&self) -> Option<f64> {
        self.next
    }
    pub fn streak(&self) -> u32 {
        self.streak
    }
    /// The session's current entry (call every tick).
    pub fn on_entry(&mut self, e: usize, n: usize, now: f64, o: &Opts) {
        if n < 2 {
            return;
        }
        if e == 0 {
            self.next = None;
            self.streak = 0;
            if self.primary_since.is_none() {
                self.primary_since = Some(now);
            }
            if self.hold > 1 && self.primary_since.is_some_and(|t| now - t >= o.trust_s) {
                self.hold = 1;
            }
            return;
        }
        self.primary_since = None;
        if self.next.is_none() {
            self.next = Some(now + o.failback_min * 60.0 * self.hold as f64);
        }
    }
    pub fn due(&self, now: f64, o: &Opts) -> bool {
        o.failback_min > 0.0 && self.next.is_some_and(|t| now >= t)
    }
    /// A failback that did not last (the primary was left again within `bad_within_s`): the next checks come less often.
    pub fn left_bad(&mut self, o: &Opts) {
        self.hold = (self.hold * 2).min(o.hold_max);
    }
    /// A check's result; true = switch back now.
    pub fn result(&mut self, ok: bool, now: f64, o: &Opts) -> bool {
        if !ok {
            self.streak = 0;
            self.next = Some(now + o.failback_min * 60.0 * self.hold as f64);
            return false;
        }
        self.streak += 1;
        if self.streak >= o.ok_probes {
            self.streak = 0;
            self.next = None;
            return true;
        }
        self.next = Some(now + o.confirm_s);
        false
    }
}

// ---------------------------------------------------------------------------------------------------------------------------------
// entry choice after a connection ended (pure, unit-tested)

#[derive(Debug, Clone)]
pub struct Rotor {
    pub n: usize,
    pub cur: usize,
    backoff: Duration,
    fails: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Next {
    pub entry: usize,
    pub wait: Duration,
    /// the entry changed because of a failure (not a requested switch)
    pub failed_over: bool,
}

impl Rotor {
    pub fn new(n: usize) -> Rotor {
        Rotor { n: n.max(1), cur: 0, backoff: BACKOFF_MIN, fails: 0 }
    }
    /// A connection to `self.cur` ended after `lived`. `switch` = a requested switch (failback / follow): no wait, no failure.
    pub fn after(&mut self, lived: Duration, switch: Option<usize>, fee: bool) -> Next {
        if let Some(t) = switch {
            self.cur = t.min(self.n - 1);
            self.backoff = BACKOFF_MIN;
            self.fails = 0;
            return Next { entry: self.cur, wait: Duration::ZERO, failed_over: false };
        }
        if lived > ESTABLISHED {
            self.backoff = BACKOFF_MIN;
            self.fails = 0;
        }
        // the fee login always retries quickly (its slot is short)
        let step = |b: Duration| if fee { BACKOFF_MIN } else { b };
        if self.n == 1 {
            let w = step(self.backoff);
            self.backoff = (self.backoff * 2).min(BACKOFF_MAX);
            return Next { entry: 0, wait: w, failed_over: false };
        }
        if lived > ESTABLISHED {
            // an established connection dropped: the same entry again first
            return Next { entry: self.cur, wait: step(BACKOFF_MIN), failed_over: false };
        }
        self.fails += 1;
        self.cur = (self.cur + 1) % self.n;
        // the next entry at once (2 s); a whole round without a working pool backs off like a single pool does
        let w = if self.fails % self.n == 0 {
            let w = step(self.backoff);
            self.backoff = (self.backoff * 2).min(BACKOFF_MAX);
            w
        } else {
            BACKOFF_MIN
        };
        Next { entry: self.cur, wait: w, failed_over: true }
    }
}

// ---------------------------------------------------------------------------------------------------------------------------------
// the accepted-shares watchdog of the user's session (ffgs 0.5.4 `Watch`): connected and hashing, but the pool credits nothing

#[derive(Debug, PartialEq)]
pub enum Act {
    None,
    /// something is wrong but the pool is not the cause (the GPUs found nothing): log only
    Warn(String),
    /// the same entry again
    Reconnect(String),
    /// the next entry
    Switch(usize, String),
}

#[derive(Debug, Default)]
pub struct Watch {
    entry: Option<usize>,
    epoch: u64,
    acc0: u64,
    rej0: u64,
    sub0: u64,
    mine_s: f64,
    expect: f64,
    strikes: u32,
    grace_s: f64,
    was_allowed: bool,
    /// the share difficulty seen when the pool changed (entry / reconnect): it belongs to the OLD pool until it differs (a new
    /// job of the new pool) or STALE_HPS_S have passed; nothing is expected meanwhile
    stale_hps: Option<f64>,
    stale_s: f64,
}

/// How long a share difficulty that has not changed since a pool switch is not trusted as the new pool's.
const STALE_HPS_S: f64 = 60.0;

impl Watch {
    fn restart(&mut self, acc: u64, rej: u64, sub: u64) {
        self.acc0 = acc;
        self.rej0 = rej;
        self.sub0 = sub;
        self.mine_s = 0.0;
        self.expect = 0.0;
    }
    /// Every status tick. `dt`: seconds since the last one. `allowed`: the GPUs work for the user and the session is online (not in a fee
    /// minute, not paused / disconnected). `hs`: the user's hashes per second in this tick; `hps`: hashes per share of the current job
    /// (None = unknown: only the rejects rule can fire). `acc` / `rej` / `sub`: the user's accepted / rejected / sent shares.
    #[allow(clippy::too_many_arguments)]
    pub fn tick(&mut self, o: &Opts, dt: f64, allowed: bool, hs: f64, hps: Option<f64>, acc: u64, rej: u64, sub: u64, epoch: u64, entry: usize, entries: usize) -> Act {
        if self.entry != Some(entry) {
            let switched = self.entry.is_some(); // not the first tick of the run: the difficulty on hand is the old pool's
            self.entry = Some(entry);
            self.epoch = epoch;
            self.strikes = 0;
            self.restart(acc, rej, sub);
            self.stale_hps = hps.filter(|_| switched);
            self.stale_s = 0.0;
            return Act::None;
        }
        if epoch != self.epoch {
            self.epoch = epoch; // reconnected: a new window, strikes kept
            self.restart(acc, rej, sub);
            return Act::None;
        }
        if acc != self.acc0 {
            self.strikes = 0;
            self.restart(acc, rej, sub);
            return Act::None;
        }
        // 10 s after a fee minute (its shares are on the pool too) nothing counts
        if !allowed {
            self.was_allowed = false;
            self.grace_s = 10.0;
            return Act::None;
        }
        if !self.was_allowed || self.grace_s > 0.0 {
            self.was_allowed = true;
            self.grace_s -= dt;
            if self.grace_s > 0.0 {
                return Act::None;
            }
        }
        if dt <= 0.0 || hs <= 0.0 {
            return Act::None;
        }
        self.mine_s += dt;
        // the difficulty of the previous pool's job (the GPUs keep it until the new pool's first job): no expectation from it
        let stale = self.stale_hps.is_some() && hps == self.stale_hps && self.stale_s < STALE_HPS_S;
        if stale {
            self.stale_s += dt;
        } else {
            self.stale_hps = None;
        }
        if let Some(p) = hps.filter(|p| *p > 0.0).filter(|_| !stale) {
            self.expect += hs * dt / p;
        }
        let (nrej, nsub) = (rej.saturating_sub(self.rej0), sub.saturating_sub(self.sub0));
        let reject = o.reject_min > 0.0 && nrej >= o.reject_count && self.mine_s >= o.reject_min * 60.0;
        let silent = o.noshare_min > 0.0 && self.mine_s >= o.noshare_min * 60.0 && self.expect >= o.noshare_mult;
        if !reject && !silent {
            return Act::None;
        }
        if silent && !reject && nsub == 0 {
            // the GPUs sent nothing although shares were expected: not a pool problem, switching would not help
            let w = format!("no share found in {:.1} min although ~{:.0} were expected: check the GPUs (hardware errors, clocks); the pool is kept", self.mine_s / 60.0, self.expect);
            self.restart(acc, rej, sub);
            return Act::Warn(w);
        }
        let why = if reject {
            format!("only rejected shares for {:.1} min ({nrej} rejected, none accepted)", self.mine_s / 60.0)
        } else {
            format!("no accepted share for {:.1} min while ~{:.0} were expected ({nsub} sent, {nrej} rejected)", self.mine_s / 60.0, self.expect)
        };
        self.restart(acc, rej, sub);
        self.strikes += 1;
        if self.strikes == 1 {
            return Act::Reconnect(format!("{why}: reconnecting"));
        }
        if entries > 1 {
            self.strikes = 0;
            let n = (entry + 1) % entries;
            return Act::Switch(n, format!("{why} again after a reconnect: switching to pool {} of {entries}", n + 1));
        }
        Act::Reconnect(format!("{why} again after a reconnect (no other pool in --pool): reconnecting"))
    }
}

/// Seconds per share of the user's session, for an engine that does not tell the share difficulty (CMFD): the rate seen on the
/// pool the session is on NOW.  Every pool entry has its own history (a pool with easy shares must not set the expectation for one
/// with hard shares: Rig05, Midas 22 shares in 4 min -> Aria 0 in 11 min, "no share in 5.8 min although ~10 were expected"); a pool
/// without 3 shares of its own is expected to give one per 6 minutes.
#[derive(Debug, Default)]
pub struct ShareRate {
    last_acc: u64,
    started: bool,
    per_entry: Vec<(f64, u64)>, // (seconds mined, shares) per pool entry
}

pub const SHARE_RATE_DEFAULT_S: f64 = 360.0;

impl ShareRate {
    /// Every status tick: `entry` the pool entry of the session, `dt` the tick, `mining` the engines mine for the user on it, `acc` the
    /// running total of accepted shares (it only grows).  Returns the expected seconds per share on this entry.
    pub fn tick(&mut self, entry: usize, dt: f64, mining: bool, acc: u64) -> f64 {
        if self.per_entry.len() <= entry {
            self.per_entry.resize(entry + 1, (0.0, 0));
        }
        let d = if self.started { acc.saturating_sub(self.last_acc) } else { 0 };
        self.started = true;
        self.last_acc = self.last_acc.max(acc);
        let e = &mut self.per_entry[entry];
        if mining {
            e.0 += dt;
        }
        e.1 += d;
        if e.1 >= 3 { (e.0 / e.1 as f64).max(1.0) } else { SHARE_RATE_DEFAULT_S }
    }
}

/// The coin loops call this every status tick with the user's numbers; applies the verdict to the link (log, reconnect / switch, check
/// interval of a primary that was left).
#[allow(clippy::too_many_arguments)]
pub fn watch_tick(w: &mut Watch, link: &Link, name: &str, dt: f64, allowed: bool, hs: f64, hps: Option<f64>, acc: u64, rej: u64, sub: u64) {
    let Some(e) = link.entry() else { return };
    let o = opts();
    match w.tick(&o, dt, allowed, hs, hps, acc, rej, sub, link.epoch.load(Ordering::Relaxed), e, link.n) {
        Act::None => {}
        Act::Warn(why) => eprintln!("[pool:{name}] watchdog: {why}"),
        Act::Reconnect(why) => {
            eprintln!("[pool:{name}] watchdog: {why}");
            link.request(e, &format!("watchdog: pool {} of {} credits nothing", e + 1, link.n));
        }
        Act::Switch(n, why) => {
            eprintln!("[pool:{name}] watchdog: {why}");
            if e == 0 {
                link.fb.lock().unwrap().left_bad(&o);
            }
            link.request(n, &format!("watchdog: pool {} of {} credits nothing", e + 1, link.n));
        }
    }
}

/// Hashes per share for a 256-bit share target (as f64): 2^256 / target.
pub fn hps_from_target(target_f64: f64) -> Option<f64> {
    (target_f64 > 0.0).then(|| 2f64.powi(256) / target_f64)
}

// ---------------------------------------------------------------------------------------------------------------------------------
// the link between a session thread, the prober and the fee login

pub struct Link {
    pub n: usize,
    entry: AtomicI64,
    start: AtomicUsize,
    req: Mutex<Option<(usize, String)>>,
    pub epoch: AtomicU64,
    pub fb: Mutex<Failback>,
    /// pool health for the user's session: counters shown in the 60 s line / status
    pub failovers: AtomicU64,
    pub failbacks: AtomicU64,
}

impl Link {
    pub fn new(n: usize) -> Arc<Link> {
        Arc::new(Link {
            n: n.max(1),
            entry: AtomicI64::new(-1),
            start: AtomicUsize::new(0),
            req: Mutex::new(None),
            epoch: AtomicU64::new(0),
            fb: Mutex::new(Failback::default()),
            failovers: AtomicU64::new(0),
            failbacks: AtomicU64::new(0),
        })
    }
    /// The entry the session is connected to / connecting to (None before the first attempt).
    pub fn entry(&self) -> Option<usize> {
        let e = self.entry.load(Ordering::Relaxed);
        (e >= 0).then_some(e as usize)
    }
    pub fn start_entry(&self) -> usize {
        self.start.load(Ordering::Relaxed)
    }
    pub fn begin(&self, e: usize) {
        self.entry.store(e as i64, Ordering::Relaxed);
        self.start.store(e, Ordering::Relaxed);
        self.epoch.fetch_add(1, Ordering::Relaxed);
    }
    pub fn request(&self, target: usize, why: &str) {
        *self.req.lock().unwrap() = Some((target, why.to_string()));
    }
    pub fn pending(&self) -> bool {
        self.req.lock().unwrap().is_some()
    }
    pub fn take(&self) -> Option<(usize, String)> {
        self.req.lock().unwrap().take()
    }
}

/// A session that should hand over to another entry: send what is queued, wait for the pool's answers, then end.
pub struct Drain {
    since: Option<Instant>,
}

impl Drain {
    pub const MIN: Duration = Duration::from_millis(1000);
    pub const MAX: Duration = Duration::from_millis(3000);
    pub fn new() -> Drain {
        Drain { since: None }
    }
    /// Call every loop turn after the queued shares were sent; `idle` = nothing unanswered, nothing held. True = end the session now.
    /// For the first `MIN` after the switch was requested shares keep going out on the old connection (the GPUs still mine its job);
    /// then new shares are no longer sent (`frozen()`: they belong to a job of the old pool) and the session ends once the pool has
    /// answered everything in flight, at the latest after `MAX`.
    pub fn check(&mut self, link: &Link, idle: bool) -> bool {
        if self.since.is_none() {
            if !link.pending() {
                return false;
            }
            self.since = Some(Instant::now());
        }
        let t = self.since.unwrap().elapsed();
        (t >= Self::MIN && idle) || t >= Self::MAX
    }
    /// True once the shares found from now on are not sent any more (the answers of those in flight are being collected).
    pub fn frozen(&self) -> bool {
        self.since.is_some_and(|t| t.elapsed() >= Self::MIN)
    }
}

impl Default for Drain {
    fn default() -> Self {
        Drain::new()
    }
}

// ---------------------------------------------------------------------------------------------------------------------------------
// the connection loop shared by every coin's session thread

pub struct Loop {
    pub name: &'static str,
    pub link: Arc<Link>,
    /// the user's link of a fee login: it connects to the pool the user is on
    pub follow: Option<Arc<Link>>,
    pub stop: Arc<AtomicBool>,
    pub enabled: Arc<AtomicBool>,
    pub fee: bool,
    /// text of an entry for the log (host:port)
    pub label: Box<dyn Fn(usize) -> String + Send>,
}

/// `set_entry(st, e)`: point the login at entry e (url, wallet text); `session(st)`: one connection (returns when it ends);
/// `after(st)`: tell the miner the connection is gone and drop the queued shares of the dead connection; `idle(st)`: drop
/// shares while the login is disabled. `st` = the coin's login (mutated between connections only).
pub fn run<S>(
    l: Loop,
    st: &mut S,
    mut set_entry: impl FnMut(&mut S, usize),
    mut session: impl FnMut(&mut S) -> io::Result<()>,
    mut after: impl FnMut(&mut S),
    mut idle: impl FnMut(&mut S),
) {
    let mut rot = Rotor::new(l.link.n);
    let mut failback_target = false; // the current connection is one made by a failback
    let mut since_switch: Option<Instant> = None;
    let mut followed: Option<usize> = None; // fee login: the entry of the last connection
    while !l.stop.load(Ordering::Relaxed) {
        if !l.enabled.load(Ordering::Relaxed) {
            idle(st);
            std::thread::sleep(Duration::from_millis(200));
            continue;
        }
        let e = match &l.follow {
            Some(u) => u.entry().unwrap_or_else(|| u.start_entry()).min(l.link.n - 1),
            None => rot.cur,
        };
        if l.follow.is_some() {
            if followed.is_some_and(|p| p != e) {
                eprintln!("[pool:{}] following the user's pool: entry {} of {} ({})", l.name, e + 1, l.link.n, (l.label)(e));
            }
            followed = Some(e);
            rot.cur = e;
        }
        set_entry(st, e);
        l.link.begin(e);
        let t = Instant::now();
        match session(st) {
            Ok(()) => eprintln!("[pool:{}] connection closed", l.name),
            Err(er) => eprintln!("[pool:{}] {er}", l.name),
        }
        let lived = t.elapsed();
        after(st);
        if !l.enabled.load(Ordering::Relaxed) || l.stop.load(Ordering::Relaxed) {
            continue;
        }
        let req = l.link.take();
        // a failback that does not last: the primary is left again within a few minutes (the check interval grows)
        if failback_target && e == 0 && req.is_none() && since_switch.is_some_and(|s| s.elapsed().as_secs_f64() < opts().bad_within_s) {
            let mut fb = l.link.fb.lock().unwrap();
            fb.left_bad(&opts());
            failback_target = false;
            eprintln!("[pool:{}] the primary pool dropped again {} s after the failback: the next checks come less often (x{})", l.name, since_switch.map_or(0, |s| s.elapsed().as_secs()), fb.hold);
        }
        let switch = req.as_ref().map(|r| r.0);
        let nx = rot.after(lived, switch, l.fee);
        if let Some((t, why)) = &req {
            eprintln!("[pool:{}] {why}: now pool {} of {} ({})", l.name, t + 1, l.link.n, (l.label)(*t));
            if *t == 0 && !l.fee {
                l.link.failbacks.fetch_add(1, Ordering::Relaxed);
                failback_target = true;
                since_switch = Some(Instant::now());
            } else {
                failback_target = false;
            }
        } else if nx.failed_over && l.follow.is_none() {
            l.link.failovers.fetch_add(1, Ordering::Relaxed);
            failback_target = false;
            eprintln!(
                "[pool:{}] pool {} of {} ({}) is not usable: failing over to pool {} of {} ({}) in {} s",
                l.name,
                e + 1,
                l.link.n,
                (l.label)(e),
                nx.entry + 1,
                l.link.n,
                (l.label)(nx.entry),
                nx.wait.as_secs()
            );
        } else if !nx.wait.is_zero() || nx.failed_over {
            eprintln!("[pool:{}] reconnecting in {}s", l.name, nx.wait.as_secs());
        }
        let until = Instant::now() + nx.wait;
        while Instant::now() < until && l.enabled.load(Ordering::Relaxed) && !l.stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------------------------
// the check of the primary (separate connection, nothing submitted)

/// Runs `run` (a coin's ordinary session code against the primary) on a scratch channel: the check passes when the session
/// delivers a usable job (`is_job`) within the timeout, i.e. connect + login + job all worked. It fails when the session
/// ends first (refused login, connection error) or nothing usable arrives.
pub fn probe_session<E: Send + 'static, S: Send + 'static>(
    timeout: Duration,
    run: impl FnOnce(Arc<AtomicBool>, Sender<(crate::stratum::Src, E)>, Receiver<S>) -> io::Result<()> + Send + 'static,
    is_job: impl Fn(&E) -> bool,
) -> Result<(), String> {
    let (etx, erx) = mpsc::channel::<(crate::stratum::Src, E)>();
    let (stx, srx) = mpsc::channel::<S>();
    let enabled = Arc::new(AtomicBool::new(true));
    let en2 = enabled.clone();
    let h = std::thread::Builder::new().name("pool-probe".into()).spawn(move || run(en2, etx, srx)).map_err(|e| e.to_string())?;
    let t0 = Instant::now();
    let mut res: Result<(), String> = Err(format!("no usable job within {} s", timeout.as_secs()));
    loop {
        match erx.recv_timeout(Duration::from_millis(50)) {
            Ok((_, ev)) if is_job(&ev) => {
                res = Ok(());
                break;
            }
            Ok(_) => {}
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if h.is_finished() {
            // the session ended: a job it sent just before is still queued
            while let Ok((_, ev)) = erx.try_recv() {
                if is_job(&ev) {
                    return Ok(());
                }
            }
            break;
        }
        if t0.elapsed() >= timeout {
            break;
        }
    }
    enabled.store(false, Ordering::Relaxed);
    drop(stx);
    let t1 = Instant::now();
    while !h.is_finished() && t1.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(20));
    }
    if h.is_finished() {
        match h.join() {
            Ok(Err(e)) if res.is_err() => res = Err(e.to_string()),
            _ => {}
        }
    }
    res
}

/// The prober thread of a user's session: while on a backup entry, check the primary and ask for the switch back.
/// `probe(0)` runs one check (a few seconds at most); `name` = log prefix.
pub fn spawn_prober(name: &'static str, link: Arc<Link>, stop: Arc<AtomicBool>, label: impl Fn(usize) -> String + Send + 'static, probe: impl Fn(usize) -> Result<(), String> + Send + 'static) {
    if link.n < 2 {
        return;
    }
    let _ = std::thread::Builder::new().name(format!("failback-{name}")).spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            let o = opts();
            std::thread::sleep(o.tick);
            if o.failback_min <= 0.0 {
                continue;
            }
            let Some(e) = link.entry() else { continue };
            let now = now_s();
            let due = {
                let mut fb = link.fb.lock().unwrap();
                fb.on_entry(e, link.n, now, &o);
                e > 0 && !link.pending() && fb.due(now, &o)
            };
            if !due {
                continue;
            }
            let r = probe(0);
            let (go, hold) = {
                let mut fb = link.fb.lock().unwrap();
                (fb.result(r.is_ok(), now_s(), &o), fb.hold)
            };
            // the session may have left the backup meanwhile (failover to another entry / a request): nothing to do then
            if link.entry().is_none_or(|x| x == 0) {
                continue;
            }
            match (&r, go) {
                (_, true) => link.request(0, &format!("the primary pool ({}) answered {} checks in a row: switching back to it", label(0), o.ok_probes)),
                (Ok(()), false) => eprintln!("[pool:{name}] failback: the primary pool ({}) answers again (check 1 of {}), checking again in {:.0} s", label(0), o.ok_probes, o.confirm_s),
                (Err(why), false) => eprintln!("[pool:{name}] failback: the primary pool ({}) is still not usable ({why}), next check in {:.0} min", label(0), o.failback_min * hold as f64),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn o() -> Opts {
        Opts { failback_min: 10.0, ..Opts::default() }
    }

    #[test]
    fn list_parsing() {
        assert_eq!(split_list("a:1,b:2"), vec!["a:1", "b:2"]);
        assert_eq!(split_list(" a:1 , b:2  c:3,,"), vec!["a:1", "b:2", "c:3"]);
        assert_eq!(split_list("stratum+ssl://x:1?insecure,cmfd+tls://1.2.3.4:5?pin=ab"), vec!["stratum+ssl://x:1?insecure", "cmfd+tls://1.2.3.4:5?pin=ab"]);
        assert!(split_list("  ").is_empty());
        assert_eq!(label("cmfd+tls://1.2.3.4:5?pin=abc"), "1.2.3.4:5");
        assert_eq!(label("stratum+ssl://p.example:3334/"), "p.example:3334");
        assert_eq!(label("host:3360"), "host:3360");
        assert!(parse_minutes("0").is_ok() && parse_minutes("0.5").is_ok());
        assert!(parse_minutes("-1").is_err() && parse_minutes("x").is_err() && parse_minutes("nan").is_err());
    }

    #[test]
    fn failback_two_good_checks() {
        let o = o();
        let mut f = Failback::default();
        f.on_entry(1, 2, 100.0, &o);
        assert_eq!(f.next(), Some(100.0 + 600.0));
        assert!(!f.due(699.0, &o) && f.due(700.0, &o));
        // first good check: not yet, the second one in confirm_s
        assert!(!f.result(true, 700.0, &o));
        assert_eq!(f.next(), Some(760.0));
        // a failed check between resets the streak: the full interval again
        assert!(!f.result(false, 760.0, &o));
        assert_eq!(f.streak(), 0);
        assert_eq!(f.next(), Some(760.0 + 600.0));
        assert!(!f.result(true, 1360.0, &o));
        assert!(f.result(true, 1420.0, &o), "two good checks in a row switch back");
        assert_eq!(f.next(), None);
    }

    #[test]
    fn failback_off_or_single_pool() {
        let mut o = o();
        let mut f = Failback::default();
        f.on_entry(1, 1, 0.0, &o); // a single pool: nothing to fail back to
        assert_eq!(f.next(), None);
        f.on_entry(1, 2, 0.0, &o);
        o.failback_min = 0.0;
        assert!(!f.due(1e9, &o), "--failback-minutes 0: never due");
    }

    #[test]
    fn failback_flapping_primary_backs_off() {
        let o = o();
        let mut f = Failback::default();
        f.on_entry(1, 2, 0.0, &o);
        assert!(!f.result(true, 600.0, &o));
        assert!(f.result(true, 660.0, &o));
        f.on_entry(0, 2, 661.0, &o); // switched
        // dropped again 100 s later (the connection loop reports it), back on the backup: the multiplier doubles
        f.left_bad(&o);
        f.on_entry(1, 2, 761.0, &o);
        assert_eq!(f.hold, 2);
        assert_eq!(f.next(), Some(761.0 + 1200.0));
        // a stable primary (3 h) is trusted again
        assert!(!f.result(true, 2000.0, &o));
        assert!(f.result(true, 2060.0, &o));
        f.on_entry(0, 2, 2061.0, &o);
        f.on_entry(0, 2, 2061.0 + 7300.0, &o);
        assert_eq!(f.hold, 1);
    }

    #[test]
    fn failback_hold_is_capped() {
        let o = o();
        let mut f = Failback::default();
        let mut t = 0.0;
        for _ in 0..8 {
            f.on_entry(1, 2, t, &o);
            let _ = f.result(true, t + 1.0, &o);
            assert!(f.result(true, t + 2.0, &o));
            f.on_entry(0, 2, t + 3.0, &o);
            f.left_bad(&o);
            t += 10.0;
        }
        f.on_entry(1, 2, t, &o);
        assert_eq!(f.hold, 16);
    }

    #[test]
    fn rotor_single_pool_keeps_old_behavior() {
        let mut r = Rotor::new(1);
        assert_eq!(r.after(Duration::from_secs(1), None, false).wait, Duration::from_secs(2));
        assert_eq!(r.after(Duration::from_secs(1), None, false).wait, Duration::from_secs(4));
        assert_eq!(r.after(Duration::from_secs(1), None, false).wait, Duration::from_secs(8));
        assert_eq!(r.after(Duration::from_secs(90), None, false).wait, Duration::from_secs(2), "a lived connection resets the backoff");
        let n = r.after(Duration::from_secs(1), None, true);
        assert_eq!((n.entry, n.wait), (0, Duration::from_secs(2)), "the fee login retries at once");
    }

    #[test]
    fn rotor_failover_and_wrap() {
        let mut r = Rotor::new(2);
        let n = r.after(Duration::from_secs(1), None, false);
        assert_eq!((n.entry, n.failed_over, n.wait), (1, true, Duration::from_secs(2)));
        // the backup fails too: back to the primary; a full round without success backs off
        let n = r.after(Duration::from_secs(1), None, false);
        assert_eq!((n.entry, n.failed_over, n.wait), (0, true, Duration::from_secs(2)));
        let n = r.after(Duration::from_secs(1), None, false);
        assert_eq!((n.entry, n.wait), (1, Duration::from_secs(2)));
        let n = r.after(Duration::from_secs(1), None, false);
        assert_eq!((n.entry, n.wait), (0, Duration::from_secs(4)));
    }

    #[test]
    fn rotor_established_connection_retries_same_entry() {
        let mut r = Rotor::new(3);
        let _ = r.after(Duration::from_secs(1), None, false); // 0 -> 1
        let n = r.after(Duration::from_secs(600), None, false);
        assert_eq!((n.entry, n.failed_over), (1, false));
        let n = r.after(Duration::from_secs(1), None, false); // the retry failed at once: next
        assert_eq!((n.entry, n.failed_over), (2, true));
    }

    #[test]
    fn rotor_requested_switch() {
        let mut r = Rotor::new(2);
        let _ = r.after(Duration::from_secs(1), None, false);
        assert_eq!(r.cur, 1);
        let n = r.after(Duration::from_secs(300), Some(0), false);
        assert_eq!((n.entry, n.wait, n.failed_over), (0, Duration::ZERO, false));
        assert_eq!(r.cur, 0);
    }

    #[test]
    fn drain_waits_min_then_idle() {
        let l = Link::new(2);
        let mut d = Drain::new();
        assert!(!d.check(&l, true), "no request: no drain");
        l.request(0, "x");
        assert!(!d.check(&l, true), "idle at once, but the minimum wait is kept for the GPUs' last shares");
        assert!(!d.frozen());
        std::thread::sleep(Duration::from_millis(1050));
        assert!(d.frozen(), "after the minimum wait new shares are not sent any more");
        assert!(!d.check(&l, false), "answers still outstanding");
        assert!(d.check(&l, true));
    }

    #[test]
    fn link_request_take() {
        let l = Link::new(3);
        assert!(!l.pending() && l.entry().is_none());
        l.begin(2);
        assert_eq!(l.entry(), Some(2));
        l.request(0, "back");
        assert!(l.pending());
        assert_eq!(l.take(), Some((0, "back".to_string())));
        assert!(!l.pending() && l.take().is_none());
    }

    #[test]
    fn probe_session_job_and_failures() {
        use crate::stratum::Src;
        // a session that sends a job: ok, and the session is told to stop
        let r = probe_session::<u8, ()>(
            Duration::from_secs(5),
            |en, tx, _rx| {
                tx.send((Src::User, 1)).unwrap();
                while en.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Ok(())
            },
            |e| *e == 1,
        );
        assert!(r.is_ok());
        // a refused login: the session ends with an error before any job
        let r = probe_session::<u8, ()>(Duration::from_secs(5), |_en, _tx, _rx| Err(io::Error::other("authorize failed")), |e| *e == 1);
        assert!(r.unwrap_err().contains("authorize failed"));
        // connected but silent: the timeout
        let r = probe_session::<u8, ()>(
            Duration::from_millis(300),
            |en, _tx, _rx| {
                while en.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Ok(())
            },
            |e| *e == 1,
        );
        assert!(r.unwrap_err().contains("no usable job"));
        // events that are not jobs do not count
        let r = probe_session::<u8, ()>(
            Duration::from_millis(300),
            |en, tx, _rx| {
                tx.send((Src::User, 0)).unwrap();
                while en.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Ok(())
            },
            |e| *e == 1,
        );
        assert!(r.is_err());
    }

    // ---- accepted-shares watchdog
    fn wopts() -> Opts {
        Opts { noshare_min: 15.0, noshare_mult: 10.0, reject_min: 5.0, reject_count: 5, ..Opts::default() }
    }

    /// feeds `secs` seconds of mining in 10 s ticks; returns the first non-None action
    fn run_w(w: &mut Watch, o: &Opts, secs: u32, hs: f64, hps: Option<f64>, acc: u64, rej: u64, sub: u64, ep: u64, e: usize, n: usize) -> Act {
        for _ in 0..secs / 10 {
            let a = w.tick(o, 10.0, true, hs, hps, acc, rej, sub, ep, e, n);
            if a != Act::None {
                return a;
            }
        }
        Act::None
    }

    #[test]
    fn watchdog_rare_shares_do_not_fire() {
        let (o, mut w) = (wopts(), Watch::default());
        w.tick(&o, 10.0, true, 1e9, Some(1e13), 0, 0, 0, 1, 0, 2); // first tick: window starts
        // one share per 1000 s expected (1e13 / 1e9 /s... = 10000 s): 2 h without a share is only 0.7 expected shares
        assert_eq!(run_w(&mut w, &o, 7200, 1e9, Some(1e13), 0, 0, 0, 1, 0, 2), Act::None);
        // unknown difficulty and no rejects: never
        assert_eq!(run_w(&mut w, &o, 7200, 1e9, None, 0, 0, 3, 1, 0, 2), Act::None);
    }

    #[test]
    fn watchdog_only_rejects_then_next_pool() {
        let (o, mut w) = (wopts(), Watch::default());
        w.tick(&o, 10.0, true, 1e9, Some(1e6), 0, 0, 0, 1, 0, 2);
        let a = run_w(&mut w, &o, 400, 1e9, Some(1e6), 0, 9, 9, 1, 0, 2);
        assert!(matches!(&a, Act::Reconnect(m) if m.contains("only rejected")), "{a:?}");
        // reconnected (a new epoch): the strike stays, the same again -> the next entry
        w.tick(&o, 10.0, true, 1e9, Some(1e6), 0, 9, 9, 2, 0, 2);
        let a = run_w(&mut w, &o, 400, 1e9, Some(1e6), 0, 20, 20, 2, 0, 2);
        assert!(matches!(&a, Act::Switch(1, m) if m.contains("switching to pool 2 of 2")), "{a:?}");
        // a single pool: reconnect again, with the warning text
        let (mut w1, mut _x) = (Watch::default(), 0);
        w1.tick(&o, 10.0, true, 1e9, Some(1e6), 0, 0, 0, 1, 0, 1);
        let _ = run_w(&mut w1, &o, 400, 1e9, Some(1e6), 0, 9, 9, 1, 0, 1);
        w1.tick(&o, 10.0, true, 1e9, Some(1e6), 0, 9, 9, 2, 0, 1);
        let a = run_w(&mut w1, &o, 400, 1e9, Some(1e6), 0, 20, 20, 2, 0, 1);
        assert!(matches!(&a, Act::Reconnect(m) if m.contains("no other pool")), "{a:?}");
        _x += 1;
    }

    #[test]
    fn watchdog_sent_but_no_reply() {
        let (o, mut w) = (wopts(), Watch::default());
        w.tick(&o, 10.0, true, 1e9, Some(1e8), 0, 0, 0, 1, 1, 2); // 10 shares / s expected
        // shares sent, nothing answered: after 15 min
        assert_eq!(run_w(&mut w, &o, 840, 1e9, Some(1e8), 0, 0, 30, 1, 1, 2), Act::None);
        let a = run_w(&mut w, &o, 120, 1e9, Some(1e8), 0, 0, 30, 1, 1, 2);
        assert!(matches!(&a, Act::Reconnect(m) if m.contains("no accepted share")), "{a:?}");
    }

    #[test]
    fn watchdog_nothing_sent_is_a_gpu_problem() {
        let (o, mut w) = (wopts(), Watch::default());
        w.tick(&o, 10.0, true, 1e9, Some(1e8), 0, 0, 0, 1, 0, 2);
        let a = run_w(&mut w, &o, 1000, 1e9, Some(1e8), 0, 0, 0, 1, 0, 2);
        assert!(matches!(&a, Act::Warn(m) if m.contains("check the GPUs")), "{a:?}");
    }

    #[test]
    fn watchdog_accepted_share_resets_and_fee_is_frozen() {
        let (o, mut w) = (wopts(), Watch::default());
        w.tick(&o, 10.0, true, 1e9, Some(1e8), 0, 0, 0, 1, 0, 2);
        assert_eq!(run_w(&mut w, &o, 800, 1e9, Some(1e8), 0, 0, 30, 1, 0, 2), Act::None);
        // an accepted share: the window starts over
        w.tick(&o, 10.0, true, 1e9, Some(1e8), 1, 0, 31, 1, 0, 2);
        assert_eq!(run_w(&mut w, &o, 800, 1e9, Some(1e8), 1, 0, 60, 1, 0, 2), Act::None);
        // a fee minute (not allowed) does not count as mining time, nor do the 10 s after it
        for _ in 0..200 {
            assert_eq!(w.tick(&o, 10.0, false, 1e9, Some(1e8), 1, 0, 60, 1, 0, 2), Act::None);
        }
        assert_eq!(w.tick(&o, 10.0, true, 1e9, Some(1e8), 1, 0, 60, 1, 0, 2), Act::None);
        // a pool change restarts everything
        assert_eq!(w.tick(&o, 10.0, true, 1e9, Some(1e8), 1, 0, 60, 1, 1, 2), Act::None);
        // off
        let off = Opts { noshare_min: 0.0, reject_min: 0.0, ..wopts() };
        assert_eq!(run_w(&mut w, &off, 7200, 1e9, Some(1e8), 1, 50, 60, 1, 1, 2), Act::None);
    }

    /// Two pools of very different difficulty, the session goes there and back (failover / failback): the expectation of the old pool
    /// never leaks into the new one, so a sparse pool does not make the watchdog warn about shares that were never due.
    #[test]
    fn watchdog_no_false_warning_after_switching_between_pools_of_different_difficulty() {
        let o = wopts();
        let mut w = Watch::default();
        let (hs, easy, hard) = (1e9, 1e9 * 10.0, 1e9 * 3000.0); // easy pool: a share per 10 s; hard pool: one per 50 min
        let mut ep = 1u64;
        let (mut acc, mut sub) = (0u64, 0u64);
        w.tick(&o, 10.0, true, hs, Some(easy), acc, 0, sub, ep, 0, 2);
        for round in 0..4 {
            // 30 min on the easy pool: a share every 10 s
            for _ in 0..180 {
                acc += 1;
                sub += 1;
                assert_eq!(w.tick(&o, 10.0, true, hs, Some(easy), acc, 0, sub, ep, 0, 2), Act::None, "easy pool, round {round}");
            }
            // failover: the GPUs still hold the easy pool's job (its difficulty) for the first ticks, then the hard pool's job comes;
            // 45 min on the hard pool with one share
            ep += 1;
            assert_eq!(w.tick(&o, 10.0, true, hs, Some(easy), acc, 0, sub, ep, 1, 2), Act::None);
            for i in 0..270 {
                let hps = if i < 3 { Some(easy) } else { Some(hard) };
                if i == 100 {
                    acc += 1;
                    sub += 1;
                }
                assert_eq!(w.tick(&o, 10.0, true, hs, hps, acc, 0, sub, ep, 1, 2), Act::None, "hard pool, round {round}, tick {i}");
            }
            // failback to the easy pool
            ep += 1;
            assert_eq!(w.tick(&o, 10.0, true, hs, Some(hard), acc, 0, sub, ep, 0, 2), Act::None);
        }
    }

    /// The old difficulty is kept (the new pool's first job has not come) after a switch: it is not trusted for 60 s.
    #[test]
    fn watchdog_stale_difficulty_after_a_switch_counts_nothing_for_a_minute() {
        let o = Opts { noshare_min: 0.5, noshare_mult: 10.0, ..wopts() };
        let mut w = Watch::default();
        w.tick(&o, 10.0, true, 1e9, Some(1e8), 0, 0, 0, 1, 0, 2);
        w.tick(&o, 10.0, true, 1e9, Some(1e8), 0, 0, 0, 2, 1, 2); // switch: stale 1e8
        // 5 ticks of 10 s on the stale value: expect stays 0 (it would be 5 x 10 s x 10 = 500 shares otherwise)
        for _ in 0..5 {
            assert_eq!(w.tick(&o, 10.0, true, 1e9, Some(1e8), 0, 0, 0, 2, 1, 2), Act::None);
        }
        assert!(w.expect < 1.0, "expect {}", w.expect);
    }

    #[test]
    fn share_rate_is_per_pool_entry() {
        let mut r = ShareRate::default();
        // pool 0: no share yet = the default, 6 minutes
        assert_eq!(r.tick(0, 10.0, true, 0), SHARE_RATE_DEFAULT_S);
        // the session fails over to pool 1 (easy): 30 shares in 5 min -> ~10 s per share
        let mut acc = 0;
        let mut est = 0.0;
        for _ in 0..30 {
            acc += 1;
            est = r.tick(1, 10.0, true, acc);
        }
        assert!((est - 10.0).abs() < 0.5, "{est}");
        // back to pool 0 (hard): 3 shares in 40 min = ~800 s, NOT pool 1's 10 s
        for _ in 0..239 {
            r.tick(0, 10.0, true, acc);
        }
        let est0 = r.tick(0, 10.0, true, acc + 3);
        assert!(est0 > 500.0, "{est0}");
        // pool 1 again: its own history again
        let est1 = r.tick(1, 10.0, true, acc + 3);
        assert!(est1 < 20.0, "{est1}");
        // fewer than 3 shares on a new entry: the default
        assert_eq!(r.tick(2, 10.0, true, acc + 4), SHARE_RATE_DEFAULT_S);
    }
}
