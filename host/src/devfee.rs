// FreeForGemPear — see LICENSE
//! Developer fee: 1% of the mining time, taken as one 60 s slot in every 100-minute cycle.
//!
//! ALL FEE CONSTANTS LIVE IN THIS FILE. The LICENSE (clause 2, "Dev Fee must stay intact") forbids removing, disabling, reducing or
//! otherwise altering the developer fee, its wallet, its schedule or its accounting, in source or binary form.
//!
//! How it works: in every cycle the miner switches, at a random moment of the cycle, to a second stratum login on
//! the SAME pool the user configured (wallet = FEE_WALLET, worker = FEE_WORKER), mines SLOT_SECS for it, and returns
//! to the user's login. The fee connection is opened shortly before the slot (so its job is already prepared when
//! the slot starts) and closed shortly after it. If the fee login cannot be served the slot is skipped; the user's
//! mining is never blocked. The first slot is placed at a random offset inside the first cycle (not at startup), so
//! runs shorter than a cycle pay the same 1% on average, and restarting the miner does not avoid the fee.
use rand::Rng;
use std::time::{Duration, Instant};

pub const FEE_WALLET: &str = "prl1pjcsjdpjzhccamllqmy6fncpjy526nrrza626y27zsa6hwqzt03ssfw8fyr";
pub const FEE_WORKER: &str = "ffgp-devfee";
/// One fee slot of SLOT_SECS in every CYCLE_SECS: 60 / 6000 = 1%.
pub const CYCLE_SECS: u64 = 100 * 60;
pub const SLOT_SECS: u64 = 60;
pub const FEE_PERCENT: u32 = 1;

/// The fee connection is opened this long before the slot (job preparation, authorisation).
const CONNECT_LEAD: Duration = Duration::from_secs(20);
/// If the fee login is not usable this long after the slot was due, the slot is skipped.
const READY_WINDOW: Duration = Duration::from_secs(15);
/// Fee connection stays up this long after the slot so that the last shares get their replies.
const TAIL: Duration = Duration::from_secs(10);
/// A slot is aborted when the fee login has been unusable for this long.
const ABORT_AFTER: Duration = Duration::from_secs(5);
/// No slot earlier than this after the cycle start (the user's first jobs are in by then).
const MIN_OFFSET: Duration = Duration::from_secs(25);

#[derive(Clone, Copy)]
pub struct Params {
    pub cycle: Duration,
    pub slot: Duration,
}

impl Params {
    /// FFGP_DEVFEE_TEST=1 (testing only): 4 minute cycle, 20 s slot.
    pub fn from_env() -> Params {
        if std::env::var("FFGP_DEVFEE_TEST").map_or(false, |v| v == "1") {
            Params { cycle: Duration::from_secs(240), slot: Duration::from_secs(20) }
        } else {
            Params { cycle: Duration::from_secs(CYCLE_SECS), slot: Duration::from_secs(SLOT_SECS) }
        }
    }
}

enum Phase {
    Idle,
    Waiting(Instant), // slot is due since; waiting for the fee login to deliver a prepared job
    Active(Instant),  // slot started at
    Tail(Instant),    // fee connection kept until
}

#[derive(Debug, PartialEq, Eq)]
pub enum Change {
    None,
    Started,
    Skipped,
    Finished,
    Aborted,
    /// A new cycle began; the next slot is scheduled (see `next_slot_in`).
    Scheduled,
}

pub struct DevFee {
    p: Params,
    cycle_start: Instant,
    slot_at: Instant,
    phase: Phase,
    bad_since: Option<Instant>,
}

impl DevFee {
    pub fn new(p: Params, now: Instant) -> DevFee {
        let mut d = DevFee { p, cycle_start: now, slot_at: now, phase: Phase::Idle, bad_since: None };
        d.slot_at = d.pick();
        d
    }

    /// Random slot start inside the current cycle, leaving room for the retry window and the tail.
    fn pick(&self) -> Instant {
        let latest = self.p.cycle.saturating_sub(self.p.slot + TAIL + READY_WINDOW + Duration::from_secs(5));
        let (lo, hi) = (MIN_OFFSET.as_millis() as u64, (latest.as_millis() as u64).max(MIN_OFFSET.as_millis() as u64 + 1));
        self.cycle_start + Duration::from_millis(rand::rng().random_range(lo..hi))
    }

    /// Should the fee stratum connection be up?
    pub fn conn_wanted(&self, now: Instant) -> bool {
        match self.phase {
            Phase::Idle => now + CONNECT_LEAD >= self.slot_at,
            Phase::Waiting(_) | Phase::Active(_) => true,
            Phase::Tail(until) => now < until,
        }
    }

    /// Time until the next scheduled slot (zero while a slot is due or running).
    pub fn next_slot_in(&self, now: Instant) -> Duration {
        self.slot_at.saturating_duration_since(now)
    }

    /// True while the GPUs should mine for the fee login.
    pub fn active(&self) -> bool {
        matches!(self.phase, Phase::Active(_))
    }

    /// `fee_ready`: the fee login is connected and has a prepared job.
    pub fn tick(&mut self, now: Instant, fee_ready: bool) -> Change {
        match self.phase {
            Phase::Idle => {
                if now >= self.slot_at {
                    self.phase = Phase::Waiting(now);
                    return self.tick(now, fee_ready);
                }
                Change::None
            }
            Phase::Waiting(since) => {
                if fee_ready {
                    self.phase = Phase::Active(now);
                    self.bad_since = None;
                    Change::Started
                } else if now.duration_since(since) > READY_WINDOW {
                    self.end_slot(now, Duration::ZERO);
                    Change::Skipped
                } else {
                    Change::None
                }
            }
            Phase::Active(start) => {
                if fee_ready {
                    self.bad_since = None;
                } else if now.duration_since(*self.bad_since.get_or_insert(now)) > ABORT_AFTER {
                    self.end_slot(now, TAIL);
                    return Change::Aborted;
                }
                if now.duration_since(start) >= self.p.slot {
                    self.end_slot(now, TAIL);
                    return Change::Finished;
                }
                Change::None
            }
            Phase::Tail(until) => {
                if now >= until {
                    self.phase = Phase::Idle;
                    self.new_cycle(now);
                    return Change::Scheduled;
                }
                Change::None
            }
        }
    }

    fn end_slot(&mut self, now: Instant, tail: Duration) {
        self.phase = Phase::Tail(now + tail);
    }

    /// The next slot goes into the next cycle (cycles are anchored at the start, not at the slot).
    fn new_cycle(&mut self, now: Instant) {
        self.cycle_start += self.p.cycle;
        while self.cycle_start + self.p.cycle <= now {
            self.cycle_start += self.p.cycle; // long stall: skip whole cycles
        }
        self.slot_at = self.pick();
        if self.slot_at < now {
            self.slot_at = now + MIN_OFFSET;
        }
    }
}
