// FreeForGem — see LICENSE
//! What the auto-uv tuners watch while mining, shared since 1.7.6 (before, Pearl had it and NOID / CMFD did not):
//!   * the burst of rejected shares: wrong GPU results show up as rejects, so a burst steps the undervolted cards back one step;
//!   * the interval of the periodic re-validation (--uv-reval-hours).
//! Pure logic (time and counters are arguments), tested here; the tuners decide what a burst or a due re-validation does.
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// How long a burst stays quiet after it fired (the step back needs time to show).
const COOLDOWN: Duration = Duration::from_secs(600);
/// The window the counters are compared over, and the shortest span before the first verdict.
const WINDOW: Duration = Duration::from_secs(300);
const MIN_SPAN: Duration = Duration::from_secs(120);
/// Rejects in the window to call it a burst (and more than accepted).
const MIN_REJECTS: u64 = 5;

pub struct RejectBurst {
    hist: VecDeque<(Instant, u64, u64, u64)>,
    cooldown: Instant,
}

impl RejectBurst {
    pub fn new(now: Instant) -> RejectBurst {
        RejectBurst { hist: VecDeque::new(), cooldown: now }
    }

    /// Feed the user pool's counters (rejected, accepted, low-difficulty rejects: a target-mode matter, not an undervolt one).
    /// Some((rejected, accepted)) over the last 5 minutes when they form a burst (at least 5 rejects, more than accepted).
    pub fn tick(&mut self, now: Instant, rej: u64, acc: u64, low: u64) -> Option<(u64, u64)> {
        if self.hist.back().is_none_or(|b| now.saturating_duration_since(b.0) >= Duration::from_secs(30)) {
            self.hist.push_back((now, rej, acc, low));
        }
        while self.hist.front().is_some_and(|f| now.saturating_duration_since(f.0) > WINDOW) {
            self.hist.pop_front();
        }
        let f = self.hist.front()?;
        if now.saturating_duration_since(f.0) < MIN_SPAN {
            return None;
        }
        let (drej, dacc) = ((rej.saturating_sub(f.1)).saturating_sub(low.saturating_sub(f.3)), acc.saturating_sub(f.2));
        if drej >= MIN_REJECTS && drej > dacc && now >= self.cooldown {
            self.cooldown = now + COOLDOWN;
            self.hist.clear();
            return Some((drej, dacc));
        }
        None
    }
}

/// The re-validation interval (--uv-reval-hours); 0 or less = never (only after a step back).
pub fn reval_interval(hours: f64) -> Duration {
    if hours <= 0.0 { Duration::from_secs(10 * 365 * 24 * 3600) } else { Duration::from_secs_f64((hours * 3600.0).max(30.0)) }
}

// ---------------------------------------------------------------------------------------------------------------------
// a GPU that cannot be opened (ported from ffgs 0.5.4, Rig02: "busy or unavailable" after a fault of the previous miner)
// ---------------------------------------------------------------------------------------------------------------------

/// Does the text of a failed check say that the GPU could not be opened (busy, unavailable, not found)?  That says nothing about
/// the offset under check: it used to walk the offset down 30 MHz per attempt to 0 (ffgs 0.5.3) / count as a failed step.
pub fn gpu_unavailable(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    ["busy or unavailable", "no cuda-capable device", "cuda_error_no_device", "cudaerrordevicesunavailable", "cuda_error_device_unavailable", "devices unavailable", "gpu is lost", "gpu requires reset", "unable to determine the device handle"]
        .iter()
        .any(|k| m.contains(k))
}

/// How long to wait before the next try of a check on a GPU that cannot be opened: 1, 5, 15 min, then every 15 min.
pub fn unavail_wait(fails: u32) -> Duration {
    Duration::from_secs(match fails {
        0 => 60,
        1 => 300,
        _ => 900,
    })
}

/// Waits `unavail_wait(fails)` (a stop ends it at once); the first time it says what is going on.  `at` = the offset under check.
pub fn wait_unavailable(fails: u32, stopped: &dyn Fn() -> bool, label: &str, at: &str, why: &str, dev: i32) {
    let w = unavail_wait(fails);
    if fails == 0 {
        println!(
            "auto-uv: {label} cannot be opened for the check at {at} ({why}): not an undervolt failure, the offset and the profile stay; this GPU likely needs a reset or a reboot. Trying again in {} min (then 5, 15 min, then every 15 min); the other GPUs mine",
            w.as_secs() / 60
        );
    }
    crate::status::set(dev, format!("unavailable, next try in {} min", w.as_secs() / 60), false, Some(Instant::now() + w));
    let t = Instant::now();
    while t.elapsed() < w && !stopped() {
        std::thread::sleep(Duration::from_secs(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn burst_needs_rejects_beyond_accepted_over_two_minutes() {
        let t0 = Instant::now();
        let mut b = RejectBurst::new(t0);
        let at = |s: u64| t0 + Duration::from_secs(s);
        assert_eq!(b.tick(at(0), 0, 100, 0), None);
        // 6 rejects, 2 accepted within 2 minutes: a burst
        assert_eq!(b.tick(at(60), 3, 101, 0), None); // too early for a verdict
        assert_eq!(b.tick(at(130), 6, 102, 0), Some((6, 2)));
        // quiet for 10 minutes after it fired, even if the rejects go on
        assert_eq!(b.tick(at(200), 30, 102, 0), None);
        assert_eq!(b.tick(at(400), 60, 102, 0), None);
    }

    #[test]
    fn low_difficulty_rejects_and_normal_rejects_do_not_count() {
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let mut b = RejectBurst::new(t0);
        b.tick(at(0), 0, 100, 0);
        // 8 rejects, all low-difficulty (a target-mode matter)
        assert_eq!(b.tick(at(130), 8, 110, 8), None);
        // a few rejects among many accepted shares are normal
        let mut b = RejectBurst::new(t0);
        b.tick(at(0), 0, 0, 0);
        assert_eq!(b.tick(at(150), 6, 40, 0), None);
    }

    #[test]
    fn interval() {
        assert_eq!(reval_interval(6.0), Duration::from_secs(21600));
        assert!(reval_interval(0.0) > Duration::from_secs(86400 * 365));
        assert_eq!(reval_interval(0.001), Duration::from_secs(30));
    }

    #[test]
    fn unavailable_gpu_is_not_a_failed_offset() {
        assert!(gpu_unavailable("CUDA error: all CUDA-capable devices are busy or unavailable"));
        assert!(gpu_unavailable("cudaSetDevice: no CUDA-capable device is detected"));
        assert!(!gpu_unavailable("WRONG RESULTS: 3 of 5984 launches gave a wrong candidate set"));
        assert!(!gpu_unavailable("rerun_diff: digest changed"));
        assert_eq!([0, 1, 2, 3, 9].map(|f| unavail_wait(f).as_secs()), [60, 300, 900, 900, 900]);
    }
}
