// FreeForGem — see LICENSE
//! Dead-link detection of a pool connection (half-open TCP: the pool or the route is gone, the socket stays "open").
//! Every pool session keeps a `Silence`: it is told about every byte received and about every liveness probe sent, and
//! says when the connection is dead.
//!
//! Two limits.  The slow one (the old per-protocol 180..300 s) applies as long as nobody proved that the pool answers a
//! probe: a pool that only talks when it has a job may be quiet for minutes without being dead.  The fast one
//! (`FAST`, 60 s) applies once this pool answered a probe on this connection AND a probe is outstanding: a silent
//! link then means a missing answer, not a missing job, so 60 s with no byte at all (three keepalive intervals) is a
//! verdict.  A live pool never trips it: it answers a probe within a second, every answer restarts the clock.
//!   InnovLab: its own `keepalive` every 20 s, answered `ok`.  NOID: `mining.hashrate` every 30 s, answered.
//!   Pearl / PROX stratum: no keepalive of their own; after 20 s of silence a `mining.ping` is sent (an unknown method: the
//!   pools answer with an error, which is an answer too).  A pool that drops the connection at once on such a probe
//!   switches the probes off for the rest of the run (`PROBE_REFUSED`), so it never causes a reconnect loop.
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub const FAST: Duration = Duration::from_secs(60);
pub const PROBE_AFTER: Duration = Duration::from_secs(20);
/// id of the synthetic probe request (replies carrying it are swallowed by the sessions)
pub const PROBE_ID: u64 = 4_000_000_001;
static PROBE_REFUSED: AtomicBool = AtomicBool::new(false);

pub struct Silence {
    slow: Duration,
    fast: Duration,
    probe_after: Duration,
    born: Instant,
    last_rx: Instant,
    last_probe: Option<Instant>,
    /// when the oldest probe sent since the last received byte went out
    pending: Option<Instant>,
    answered: bool,
    synthetic: bool,
}

impl Silence {
    /// `slow`: the limit before this pool is known to answer probes (the former per-protocol value)
    pub fn new(slow: Duration) -> Self {
        Self::with_limits(slow, FAST, PROBE_AFTER)
    }

    pub fn with_limits(slow: Duration, fast: Duration, probe_after: Duration) -> Self {
        Silence { slow, fast, probe_after, born: Instant::now(), last_rx: Instant::now(), last_probe: None, pending: None, answered: false, synthetic: false }
    }

    /// bytes arrived from the pool
    pub fn rx(&mut self) {
        self.last_rx = Instant::now();
        self.pending = None;
    }

    /// a protocol-native probe (keepalive, hashrate report) went out
    pub fn probe_sent(&mut self) {
        let now = Instant::now();
        self.last_probe = Some(now);
        self.pending.get_or_insert(now);
    }

    /// the pool answered a probe: from now on this connection is judged by the fast limit
    pub fn answered(&mut self) {
        self.answered = true;
    }

    /// time for a synthetic probe (protocols without a keepalive), not once refused earlier: (1) idle long enough and none sent
    /// lately, or (2) the connection is `probe_after` old and nothing was probed yet - a pool that sends a job every few seconds
    /// would never be idle, so without this one early probe it would never be known to answer, and its death would only be
    /// found by the slow limit
    pub fn probe_due(&self) -> bool {
        if PROBE_REFUSED.load(Ordering::Relaxed) {
            return false;
        }
        let idle = self.last_rx.elapsed() >= self.probe_after && self.last_probe.map_or(true, |p| p.elapsed() >= self.probe_after);
        let learn = !self.answered && self.last_probe.is_none() && self.born.elapsed() >= self.probe_after;
        idle || learn
    }

    /// a synthetic probe went out (the caller sent it after `probe_due`)
    pub fn synthetic_sent(&mut self) {
        self.synthetic = true;
        self.probe_sent();
    }

    /// `Some(silent for)` once the connection must be considered dead
    pub fn expired(&self) -> Option<Duration> {
        let idle = self.last_rx.elapsed();
        let limit = if self.answered && self.pending.is_some() { self.fast } else { self.slow };
        (idle > limit).then_some(idle)
    }

    /// the synthetic probe request of the stratum sessions (one JSON line)
    pub fn probe_line() -> String {
        format!("{}{}", serde_json::json!({"id": PROBE_ID, "method": "mining.ping", "params": []}), char::from(10))
    }

    pub fn error(idle: Duration) -> std::io::Error {
        std::io::Error::new(std::io::ErrorKind::TimedOut, format!("no data from the pool for {}s", idle.as_secs()))
    }
}

impl Drop for Silence {
    /// a connection that ended within 5 s of our synthetic probe, before any answer: the pool does not like it
    fn drop(&mut self) {
        if self.synthetic && !self.answered {
            if let Some(p) = self.pending {
                if p.elapsed() < Duration::from_secs(5) {
                    PROBE_REFUSED.store(true, Ordering::Relaxed);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn slow_limit_until_a_probe_is_answered() {
        let s = Silence::with_limits(ms(400), ms(60), ms(20));
        std::thread::sleep(ms(120));
        assert!(s.expired().is_none(), "unproven pool: the slow limit");
        std::thread::sleep(ms(400));
        assert!(s.expired().is_some());
    }

    #[test]
    fn fast_limit_needs_an_answer_and_an_outstanding_probe() {
        let mut s = Silence::with_limits(ms(2000), ms(60), ms(20));
        s.answered();
        std::thread::sleep(ms(100));
        assert!(s.expired().is_none(), "answered before but no probe outstanding (e.g. nothing to report): slow limit");
        s.probe_sent();
        assert!(s.expired().is_some(), "probe outstanding and 100 ms silence > 60 ms");
        s.rx();
        assert!(s.expired().is_none(), "any byte restarts the clock and clears the outstanding probe");
    }

    #[test]
    fn probe_due_after_idle_and_not_repeated() {
        let mut s = Silence::with_limits(ms(2000), ms(500), ms(30));
        assert!(!s.probe_due());
        std::thread::sleep(ms(40));
        assert!(s.probe_due());
        s.synthetic_sent();
        assert!(!s.probe_due());
        std::thread::sleep(ms(40));
        assert!(s.probe_due());
        s.answered();
        s.rx();
        assert!(!s.probe_due(), "traffic resets the idle time");
    }

    #[test]
    fn a_chatty_pool_still_gets_one_early_probe() {
        let mut s = Silence::with_limits(ms(5000), ms(2000), ms(200));
        for _ in 0..8 {
            std::thread::sleep(ms(10));
            s.rx(); // a job every 10 ms: never idle
        }
        assert!(!s.probe_due(), "not yet 200 ms old");
        std::thread::sleep(ms(150));
        s.rx();
        assert!(s.probe_due(), "200 ms old, never probed: one learning probe");
        s.synthetic_sent();
        s.rx();
        assert!(!s.probe_due());
        s.answered();
        std::thread::sleep(ms(30));
        s.rx();
        assert!(!s.probe_due(), "answered and chatty: no more probes");
    }
}
