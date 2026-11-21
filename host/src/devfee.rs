// FreeForGemPear — see LICENSE
//! Developer fee: 1% of the mining time, taken as one 60 s slot in every 100-minute cycle.
//!
//! ALL FEE CONSTANTS LIVE IN THIS FILE. The LICENSE (clause 3, "Dev Fee must stay intact") forbids removing, disabling, reducing or
//! otherwise altering the developer fee, its wallet, its schedule or its accounting, in source or binary form.
//!
//! How it works: in every cycle the miner switches, at a random moment of the cycle, to a second stratum login on
//! the SAME pool the user configured (wallet = fee_wallet(pool), worker = FEE_WORKER), mines SLOT_SECS for it, and returns
//! to the user's login. The fee connection is opened shortly before the slot (so its job is already prepared when
//! the slot starts) and closed shortly after it. If the fee login cannot be served the slot is skipped; the user's
//! mining is never blocked. The first slot is placed at a random offset inside the first cycle (not at startup), so
//! runs shorter than a cycle pay the same 1% on average, and restarting the miner does not avoid the fee.
use rand::Rng;
use std::time::{Duration, Instant};

pub const FEE_WALLET: &str = "prl1pjcsjdpjzhccamllqmy6fncpjy526nrrza626y27zsa6hwqzt03ssfw8fyr";
pub const FEE_WORKER: &str = "ffgp-devfee";
/// NOID (Parano1d) fee address: the same 1% schedule, a second login on the user's NOID pool (noid/mod.rs).
pub const FEE_WALLET_NOID: &str = "o1qxh67m5rugg8t43gcn47lp775thsq4padlacn2jv9l3d7lmv7cmq4gmhdc";
/// Proxima (PROX) fee login (prox/mod.rs): the same 1% schedule, ALWAYS on the proximamine pool (also when the user mines on
/// another pool or solo through a node), login FEE_WALLET_PROX.FEE_WORKER_PROX with the dev account's pool secret.
pub const FEE_POOL_PROX: &str = "stratum+ssl://pool.proximamine.com:3334";
pub const FEE_WALLET_PROX: &str = "fb8f33895d20ed0d244d1476272b0ccddd244f84acd2e423e55c7a95f6a6f6a6";
pub const FEE_WORKER_PROX: &str = "ffg";
/// The dev account's pool secret is NOT in the source: it is injected at build time from the environment variable
/// FFG_FEE_PASS_PROX (build.sh / build-windows.ps1 read it from ~/.secrets/prox-devfee/pool-secret); build.rs refuses a release
/// build without it unless FFG_ALLOW_NO_PROX_FEE=1 (dev build: empty, the PROX fee login then fails and prox/mod.rs warns).
pub const FEE_PASS_PROX: &str = match option_env!("FFG_FEE_PASS_PROX") { Some(s) => s, None => "" };
/// Nockchain address merge-mined with the fee slot on LuckyPool (`PRL+NOCK` wallet syntax); other pools get FEE_WALLET only.
pub const FEE_NOCK: &str = "DZgTtG2fuAYQSBbbV1GZ1ab4UfTpKtkmDyheEacKBd3Y6JsfrUat4ih";

/// Fee login wallet for `pool` (HOST:PORT): `FEE_WALLET+FEE_NOCK` on LuckyPool (PRL+NOCK merge mining), FEE_WALLET elsewhere.
/// If the pool refuses the merged login, the stratum client falls back to FEE_WALLET (Login::fallback_wallet).
pub fn fee_wallet(pool: &str) -> String {
    if crate::stratum::is_luckypool(pool) { format!("{FEE_WALLET}+{FEE_NOCK}") } else { FEE_WALLET.to_string() }
}
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
    /// FFGP_DEVFEE_TEST=1 (testing only): 4 minute cycle, 20 s slot.  F-10: honoured only by a test build (a debug build, or
    /// one built with FFG_DEVFEE_TEST_BUILD / FFGN_DEVFEE_TEST_BUILD set); a release build ignores it and always runs 60 s per 100 minutes.
    pub fn from_env() -> Params {
        const TEST_BUILD: bool = cfg!(debug_assertions) || option_env!("FFG_DEVFEE_TEST_BUILD").is_some() || option_env!("FFGN_DEVFEE_TEST_BUILD").is_some();
        let t = if TEST_BUILD { std::env::var("FFGP_DEVFEE_TEST").unwrap_or_default() } else { String::new() };
        if t == "2" {
            // testing only (1.7.8, PROX fee routing tests): 70 s cycle, 15 s slot - one slot within the first minute
            Params { cycle: Duration::from_secs(70), slot: Duration::from_secs(15) }
        } else if t == "1" {
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
    /// F-11: a slot whose fee connection was not ready is tried again later in the same cycle (at most RETRIES times), so a
    /// fee pool that needs a failover (Aria -> Suprnova) does not lose the slot
    retries: u32,
}

const RETRIES: u32 = 3;
const RETRY_AFTER: Duration = Duration::from_secs(300);

impl DevFee {
    pub fn new(p: Params, now: Instant) -> DevFee {
        let mut d = DevFee { p, cycle_start: now, slot_at: now, phase: Phase::Idle, bad_since: None, retries: 0 };
        d.slot_at = d.pick();
        d
    }

    /// Random slot start inside the current cycle, leaving room for the retry window and the tail.
    fn pick(&self) -> Instant {
        let latest = self.p.cycle.saturating_sub(self.p.slot + TAIL + READY_WINDOW + Duration::from_secs(5));
        let (lo, hi) = (MIN_OFFSET.as_millis() as u64, (latest.as_millis() as u64).max(MIN_OFFSET.as_millis() as u64 + 1));
        self.cycle_start + Duration::from_millis(rand::rng().random_range(lo..hi))
    }

    /// Should the fee stratum connection be up?  (Between the retries of a slot it stays up: F-11.)
    pub fn conn_wanted(&self, now: Instant) -> bool {
        match self.phase {
            Phase::Idle => now + CONNECT_LEAD >= self.slot_at || self.retries > 0,
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
                    let retry = now + RETRY_AFTER;
                    if self.retries < RETRIES && retry + self.p.slot + TAIL < self.cycle_start + self.p.cycle {
                        // the fee connection keeps trying meanwhile (conn_wanted), the slot comes again
                        self.retries += 1;
                        self.phase = Phase::Idle;
                        self.slot_at = retry;
                        return Change::Skipped;
                    }
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
        self.retries = 0;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slot_without_a_ready_fee_connection_is_tried_again_in_the_cycle() {
        let t0 = Instant::now();
        let p = Params { cycle: Duration::from_secs(6000), slot: Duration::from_secs(60) };
        let mut d = DevFee::new(p, t0);
        d.slot_at = t0 + Duration::from_secs(100);
        let mut t = t0 + Duration::from_secs(101);
        assert_eq!(d.tick(t, false), Change::None); // waiting
        t += READY_WINDOW + Duration::from_secs(1);
        assert_eq!(d.tick(t, false), Change::Skipped);
        assert!(d.conn_wanted(t), "the fee connection stays up between retries");
        // the slot comes again 5 min later and starts once the connection is ready
        t += RETRY_AFTER + Duration::from_secs(1);
        assert_eq!(d.tick(t, true), Change::Started);
        assert!(d.active());
    }

    #[test]
    fn the_test_cycle_is_ignored_by_a_release_build() {
        // SAFETY: test-only, single-threaded use of the variable
        unsafe { std::env::set_var("FFGP_DEVFEE_TEST", "2") };
        let p = Params::from_env();
        unsafe { std::env::remove_var("FFGP_DEVFEE_TEST") };
        if cfg!(debug_assertions) || option_env!("FFG_DEVFEE_TEST_BUILD").is_some() || option_env!("FFGN_DEVFEE_TEST_BUILD").is_some() {
            assert_eq!(p.slot, Duration::from_secs(15));
        } else {
            assert_eq!((p.cycle, p.slot), (Duration::from_secs(CYCLE_SECS), Duration::from_secs(SLOT_SECS)));
        }
    }

    #[test]
    fn fee_wallet_merges_nock_on_luckypool_only() {
        let merged = format!("{FEE_WALLET}+{FEE_NOCK}");
        assert_eq!(fee_wallet("pearl-eu2.luckypool.io:3360"), merged);
        assert_eq!(fee_wallet("PEARL-RU.LuckyPool.io:3361"), merged);
        assert_eq!(fee_wallet("prl-eu.kryptex.network:7048"), FEE_WALLET);
        assert_eq!(fee_wallet("notluckypool.io:3360"), FEE_WALLET);
        assert_eq!(fee_wallet("luckypool.io.evil.com:3360"), FEE_WALLET);
    }
}
