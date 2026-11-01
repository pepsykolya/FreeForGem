// FreeForGem — see LICENSE
//! The offset climb shared by the three auto-uv tuners (Pearl `uv.rs`, NOID `noid/tune.rs`, CMFD `cmfd_uv.rs`), 1.7.6.
//!
//! No GPU calls here: `climb` is a pure function over a `check` closure (the tuner sets the offset, runs its own check and
//! reports the outcome), so the ladder is tested on simulated GPUs (a port of the ffgs `tools/uvtest.cpp` cases).
//!
//! From `start` up in `coarse` MHz steps (default 2 x the fine step: 30); the first failing coarse step is searched again in
//! fine steps inside its own interval only (`good + fine .. cand - fine`): the same fine-step edge as a fine-step climb with about
//! half the steps.  Past the last coarse step under `cap`: fine steps up to the cap.
//!
//! The voltage floor (FreeForGem 1.7.2): two passed steps in a row that save less than 0.5 W per 15 MHz each (the threshold is
//! scaled by the step: < 1 W for a 30 MHz step) stop the climb at the offset before them: no edge searched above, no refining.
//!
//! A driver fault (Xid, hung check) on a coarse step is NOT refined (owner's rule, 1.7.6): the GPU is not asked again inside the
//! interval, the result is one fine step below the last good offset (up to one fine step lower than a refined edge, but no second
//! crash).  Silent failures (wrong results) are refined as usual.
//!
//! `hint_start`: a new clock lock that has a verified profile of a higher (or equal) core clock starts `back` MHz below that
//! profile's offset instead of the default start (the edge offset grows as the lock goes down, so the offset of a higher clock is a
//! safe lower bound; the warm-up checks the start as always).

/// What the tuner reports for one candidate offset.
#[derive(Clone, Copy, Debug, Default)]
pub struct StepOut {
    pub ok: bool,
    /// the offset could not be set / a stop: the climb ends here, no verdict on the candidate
    pub abort: bool,
    /// the check failed with a driver fault (Xid / hang), not with wrong results: no refining
    pub fault: bool,
    /// power under the check load (W), when measured
    pub w: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Climb {
    pub good: i32,
    pub good_w: Option<f64>,
    pub fail_at: i32,
    pub steps: u32,
    pub fails: u32,
    /// the highest passed offset when the voltage floor stopped the climb
    pub flat_top: i32,
    pub failed: bool,
    pub plateau: bool,
    pub aborted: bool,
    /// the failure was a driver fault (the climb did not refine)
    pub fault: bool,
}

/// The climbing step: `coarse` (--uv-coarse) 0 or below the fine step = one step size (the old behaviour); None = 2 x fine.
pub fn coarse_step(fine: i32, coarse: Option<i32>) -> i32 {
    match coarse {
        None => 2 * fine,
        Some(c) => c.max(fine),
    }
}

/// Smallest power drop (W) a passed step of `mhz` must show not to count as "flat" (voltage floor): 0.5 W per 15 MHz.
pub fn flat_w(mhz: i32) -> f64 {
    0.5 * mhz as f64 / 15.0
}

/// `check(cand, good, mhz, refine)`: set `cand`, check it, report.  `start_w`: power at `start`.
pub fn climb(start: i32, start_w: Option<f64>, cap: i32, fine: i32, coarse: i32, mut check: impl FnMut(i32, i32, i32, bool) -> StepOut) -> Climb {
    let fine = fine.max(1);
    let mut s = Climb { good: start, good_w: start_w, ..Default::default() };
    let mut step = coarse.max(fine);
    let mut top = cap;
    let mut flat = 0u32;
    let mut refine = false;
    let mut hist: Vec<(i32, Option<f64>)> = Vec::new(); // (good, its power) before each passed step
    loop {
        if s.good + step > top {
            if step == fine {
                break;
            }
            step = fine; // the rest below the cap that a coarse step does not fit in
            continue;
        }
        let cand = s.good + step;
        let r = check(cand, s.good, step, refine);
        s.steps += 1;
        if r.abort {
            s.aborted = true;
            break;
        }
        if !r.ok {
            s.fails += 1;
            s.fail_at = cand;
            if step > fine && !refine && !r.fault {
                refine = true;
                step = fine;
                top = cand - fine;
                continue;
            }
            s.fault = r.fault;
            break;
        }
        if refine {
            // inside the failing interval: no floor test
            s.good = cand;
            if r.w.is_some() {
                s.good_w = r.w;
            }
            continue;
        }
        let saved = match (s.good_w, r.w) {
            (Some(a), Some(b)) if a > 0.0 && b > 0.0 => a - b,
            _ => f64::MAX,
        };
        flat = if saved < flat_w(step) { flat + 1 } else { 0 };
        hist.push((s.good, s.good_w));
        s.good = cand;
        if r.w.is_some() {
            s.good_w = r.w;
        }
        if flat >= 2 && hist.len() >= 2 {
            s.flat_top = cand;
            let (g, w) = hist[hist.len() - 2];
            s.good = g;
            s.good_w = w;
            s.plateau = true;
            break;
        }
    }
    s.failed = s.fail_at != 0;
    s
}

/// A saved profile as `hint_start` sees it.
#[derive(Clone, Debug)]
pub struct HintProfile {
    /// core clock of the profile: its lock, else the clock under load it was found at (0 = unknown)
    pub clk: u32,
    /// memory clock lock (0 = none): a profile of another memory lock does not count
    pub mlock: u32,
    pub offset: i32,
    /// verified, not migrated, not paused, not a crash leftover
    pub usable: bool,
    pub label: String,
    pub verified_at: u64,
}

/// Start of a search at the clock `my_clk` (lock, else the clock under load; `my_mlock` the memory lock) from the profiles of
/// the same GPU: `back` MHz below the profile of the nearest higher clock.  Returns (start, label of the profile) only when it
/// is above `start`; never above `cap`.
pub fn hint_start(profiles: &[HintProfile], my_clk: u32, my_mlock: u32, start: i32, cap: i32, back: i32) -> Option<(i32, String)> {
    if my_clk == 0 {
        return None;
    }
    let mut best: Option<&HintProfile> = None;
    for p in profiles {
        if !p.usable || p.clk == 0 || p.clk <= my_clk || p.mlock != my_mlock {
            continue;
        }
        if best.is_none_or(|b| p.clk < b.clk || (p.clk == b.clk && p.verified_at > b.verified_at)) {
            best = Some(p);
        }
    }
    let b = best?;
    let h = (b.offset - back).min(cap);
    if h <= start { None } else { Some((h, format!("{} {:+}", b.label, b.offset))) }
}

/// The log line for a start taken from a higher-lock profile.
pub fn hint_line(from: &str, back: i32, h: i32, start: i32) -> String {
    format!("profile {from} of a higher clock: starting {back} MHz below it, at {h:+} instead of {start:+}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A simulated GPU: every offset >= `edge` fails (edge 0: none fails).
    #[derive(Default)]
    struct Sim {
        edge: i32,
        fault_at: i32, // a failing offset >= this one is a driver fault
        abort_at: i32,
        tested: Vec<i32>,
    }
    impl Sim {
        fn run(&mut self, start: i32, cap: i32, fine: i32, coarse: i32, watts: &dyn Fn(i32) -> f64) -> Climb {
            self.tested.clear();
            let (edge, fault_at, abort_at) = (self.edge, self.fault_at, self.abort_at);
            let tested = &mut self.tested;
            climb(start, Some(watts(start)), cap, fine, coarse, |cand, _, _, _| {
                tested.push(cand);
                if abort_at != 0 && cand >= abort_at {
                    return StepOut { abort: true, ..Default::default() };
                }
                let ok = !(edge != 0 && cand >= edge);
                StepOut { ok, fault: !ok && fault_at != 0 && cand >= fault_at, w: Some(watts(cand)), ..Default::default() }
            })
        }
    }
    fn steep(o: i32) -> f64 {
        120.0 - 0.1 * o as f64 // 1.5 W per 15 MHz: never the voltage floor
    }

    #[test]
    fn steps_and_thresholds() {
        assert_eq!((coarse_step(15, None), coarse_step(15, Some(0)), coarse_step(15, Some(60)), coarse_step(20, None)), (30, 15, 60, 40));
        assert_eq!(coarse_step(15, Some(10)), 15); // never below the fine step
        assert_eq!((flat_w(15), flat_w(30)), (0.5, 1.0));
    }

    #[test]
    fn edge_inside_the_failing_interval() {
        // no floor, edge +405: 9 coarse passes, +420 fails, +405 (refining) fails -> good +390 as a 15 MHz climb
        let mut g = Sim { edge: 405, ..Default::default() };
        let s = g.run(120, 495, 15, 30, &steep);
        assert!(s.failed && !s.plateau && s.good == 390 && s.fail_at == 405 && s.steps == 11 && s.fails == 2);
        assert_eq!(g.tested[g.tested.len() - 1], 405);
        assert_eq!(g.tested[g.tested.len() - 2], 420);
        let f = g.run(120, 495, 15, 15, &steep); // --uv-coarse 0: the old climb
        assert!(f.failed && f.good == 390 && f.fail_at == 405 && f.steps == 19 && f.fails == 1);
        // the refining step passes: edge +420 -> good +405, the edge stays the failed coarse step
        let mut g = Sim { edge: 420, ..Default::default() };
        let s = g.run(120, 495, 15, 30, &steep);
        assert!(s.failed && s.good == 405 && s.fail_at == 420 && s.steps == 11 && s.fails == 1);
    }

    #[test]
    fn same_result_as_fine_climb_for_every_edge() {
        let (mut sum_c, mut sum_f) = (0, 0);
        for edge in (135..=510).step_by(15) {
            let mut g = Sim { edge: if edge > 495 { 0 } else { edge }, ..Default::default() };
            let s = g.run(120, 495, 15, 30, &steep);
            let tc = g.tested.clone();
            let f = g.run(120, 495, 15, 15, &steep);
            assert!(s.good == f.good && s.failed == f.failed, "edge {edge}");
            assert!(s.steps <= f.steps + u32::from(edge == 135), "edge {edge}");
            sum_c += s.steps;
            sum_f += f.steps;
            for o in tc {
                assert!(g.edge == 0 || o < g.edge + 30, "edge {edge}: tested {o}");
                assert!(o <= 495);
            }
        }
        assert!(sum_c * 10 <= sum_f * 6, "{sum_c} vs {sum_f}"); // about half the steps
    }

    #[test]
    fn no_failure_up_to_the_cap() {
        let mut g = Sim::default();
        let s = g.run(120, 495, 15, 30, &steep);
        assert!(!s.failed && !s.plateau && s.good == 495 && s.steps == 13 && *g.tested.last().unwrap() == 495);
        let s = g.run(120, 465, 15, 30, &steep); // a cap off the coarse grid: +450, then +465
        assert!(!s.failed && s.good == 465 && s.steps == 12);
        g.edge = 465; // the fine step to the cap fails
        let s = g.run(120, 465, 15, 30, &steep);
        assert!(s.failed && s.good == 450 && s.fail_at == 465 && s.steps == 12);
    }

    #[test]
    fn voltage_floor_scales_with_the_step() {
        // RTX 3080 Ti at 1400: 155-157 W at every step -> the floor after two coarse steps, at the start
        let w = |o: i32| 156.0 + if (o / 30) % 2 == 1 { 0.3 } else { -0.2 };
        let mut g = Sim::default();
        let s = g.run(120, 495, 15, 30, &w);
        assert!(s.plateau && !s.failed && s.good == 120 && s.steps == 2 && s.flat_top == 180);
        // RTX 3070 at 1400: -1.5 W per 15 MHz up to +250, flat above
        let w = |o: i32| if o < 250 { 120.0 - 0.1 * o as f64 } else { 95.0 + 0.1 * ((o / 30) % 2) as f64 };
        let mut g = Sim { edge: 405, ..Default::default() };
        let s = g.run(120, 495, 15, 30, &w);
        assert!(s.plateau && !s.failed && (240..=270).contains(&s.good) && s.steps <= 7);
        let f = g.run(120, 495, 15, 15, &w); // the old climb: same floor, about twice the steps
        assert!(f.plateau && (f.good - s.good).abs() <= 30 && f.steps >= s.steps + 4);
        // CMP 50HX: 0.2 W per 15 MHz = 0.4 W per 30 MHz < 1 W: the floor at the start
        let mut g = Sim::default();
        let s = g.run(325, 495, 15, 30, &|o| 140.0 - 0.2 * o as f64 / 15.0);
        assert!(s.plateau && s.good == 325 && s.steps == 2);
        // 0.8 W per 15 MHz = 1.6 W per 30 MHz: a real slope, not the floor
        let mut g = Sim { edge: 405, ..Default::default() };
        let s = g.run(120, 495, 15, 30, &|o| 140.0 - 0.8 * o as f64 / 15.0);
        assert!(!s.plateau && s.failed && s.good == 390);
        // one flat step between real ones does not stop it
        let mut g = Sim { edge: 405, ..Default::default() };
        let s = g.run(120, 495, 15, 30, &|o| if o == 210 || o == 270 { 120.0 - 0.1 * (o - 30) as f64 } else { 120.0 - 0.1 * o as f64 });
        assert!(!s.plateau && s.good == 390);
        // no power reading: never the floor
        let s = climb(120, None, 495, 15, 30, |c, _, _, _| StepOut { ok: c < 300, ..Default::default() });
        assert!(!s.plateau && s.good == 285 && s.failed);
    }

    #[test]
    fn abort_and_resume() {
        let mut g = Sim { abort_at: 240, ..Default::default() };
        let s = g.run(120, 495, 15, 30, &steep);
        assert!(s.aborted && !s.failed && s.good == 210);
        // resumed after a crash at a coarse step: cap just above the last good one -> one fine step
        let mut g = Sim::default();
        let s = g.run(300, 315, 15, 30, &steep);
        assert!(!s.failed && s.good == 315 && s.steps == 1);
    }

    #[test]
    fn wider_coarse_step() {
        let mut g = Sim { edge: 375, ..Default::default() };
        let s = g.run(120, 495, 15, 60, &steep); // 180 240 300 360 pass, 420 fails, 375 fails
        assert!(s.failed && s.good == 360 && s.fail_at == 375 && s.steps == 6);
        let mut g = Sim { edge: 405, ..Default::default() };
        let s = g.run(120, 495, 15, 60, &steep); // 375 390 pass, 405 fails
        assert!(s.good == 390 && s.fail_at == 405 && s.steps == 8);
    }

    #[test]
    fn driver_fault_is_not_refined() {
        // Xid at the coarse step +420 (edge +405): no 15 MHz steps inside the interval, good stays +390
        let mut g = Sim { edge: 405, fault_at: 405, ..Default::default() };
        let s = g.run(120, 495, 15, 30, &steep);
        assert!(s.failed && s.fault && s.good == 390 && s.fail_at == 420 && s.fails == 1);
        assert_eq!(*g.tested.last().unwrap(), 420);
        // a fault on a fine step (cap off the grid, or --uv-coarse 0) ends it the same way
        let mut g = Sim { edge: 405, fault_at: 405, ..Default::default() };
        let s = g.run(120, 495, 15, 15, &steep);
        assert!(s.fault && s.good == 390 && s.fail_at == 405);
        // wrong results (no fault) are refined as usual
        let mut g = Sim { edge: 405, ..Default::default() };
        let s = g.run(120, 495, 15, 30, &steep);
        assert!(!s.fault && s.fail_at == 405);
    }

    fn prof(clk: u32, mlock: u32, offset: i32, usable: bool, v: u64) -> HintProfile {
        HintProfile { clk, mlock, offset, usable, label: format!("{clk}"), verified_at: v }
    }

    #[test]
    fn start_from_a_higher_lock() {
        let none: Vec<HintProfile> = vec![];
        assert_eq!(hint_start(&none, 1400, 810, 120, 495, 30), None); // nothing saved
        let mut db = vec![prof(1300, 810, 450, true, 1)]; // a lower clock: not a lower bound
        assert_eq!(hint_start(&db, 1400, 810, 120, 495, 30), None);
        db.push(prof(1600, 810, 300, true, 1));
        db.push(prof(1500, 810, 330, true, 1));
        assert_eq!(hint_start(&db, 1400, 810, 120, 495, 30), Some((300, "1500 +330".into()))); // the nearest higher clock
        assert_eq!(hint_start(&db, 1400, 810, 120, 280, 30).map(|x| x.0), Some(280)); // never above the cap
        assert_eq!(hint_start(&db, 1400, 810, 320, 495, 30), None); // never below the default start
        assert_eq!(hint_start(&db, 1400, 5001, 120, 495, 30), None); // another memory lock
        assert_eq!(hint_start(&db, 0, 810, 120, 495, 30), None); // clock unknown
        let db = vec![prof(1500, 810, 330, false, 1), prof(1450, 810, 330, false, 0)]; // migrated / never verified / paused
        assert_eq!(hint_start(&db, 1400, 810, 120, 495, 30), None);
        let db = vec![prof(1505, 0, 360, true, 1)]; // no lock: the clock under load counts
        assert_eq!(hint_start(&db, 1400, 0, 120, 495, 30).map(|x| x.0), Some(330));
        assert_eq!(hint_start(&db, 1405, 0, 120, 495, 30).map(|x| x.0), Some(330));
    }
}
