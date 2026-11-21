// FFGN — see LICENSE
//! FFGN 1.0.10 auto-uv fix: a tuning step measured on the MINING load of the card itself.
//!
//! Before (1.0.3-1.0.9) every step ran in a check process on the engine's synthetic plain-path bench while the card's own
//! mining was paused: the card showed 0.000 MA/s for the whole tuning (17+ min on an 8x 3070 rig), drew 100-148 W for nothing,
//! and the offsets were judged on a load (the plain path, ~160+ W) the card never mines on since engine 0.6.0 picks the digest
//! table / cheap path per model (~110-140 W).
//!
//! Now the card keeps mining during every step on whatever path the engine chose; this module only watches it:
//!  * the evaluated attempts of the card's worker (its MA/s is counted all the time, shares are submitted as usual);
//!  * stability = no Xid on this card, the worker alive, and no new `bad` (every hit is re-checked on the CPU, the engine's
//!    per-call canary, the periodic audit and the model probe all count there) over the step window;
//!  * the power of the step is PAIRED: the window alternates the candidate offset (B) with the best offset so far (A) on the
//!    same mining load; only samples taken under the same load key (dataset slot + model + path) are compared, so a new block,
//!    a slot switch (Kepler / TESS draw 7+ W apart) or a path change does not look like a saving or a rise.  The window ends
//!    when the paired difference is decided against the 0.5 W rule (its standard error is small enough) after a minimum time at
//!    the candidate, or at the maximum time.
//!
//! No GPU calls here (the `Probe` trait), so the window is tested on simulated cards.

/// One reading of the card.
#[derive(Clone, Copy, Debug, Default)]
pub struct Sample {
    /// seconds since the probe started
    pub time: f64,
    pub w: Option<f64>,
    pub clk: Option<u32>,
    pub temp: Option<i32>,
    /// evaluated attempts of this card's worker (monotonic)
    pub attempts: u64,
    /// wrong results of this card (CPU check of a hit, canary, audit, model probe, worker errors; monotonic)
    pub bad: u64,
    /// 1.0.12: crashes of this card's own engine process (a sticky CUDA error, the process died / hung: iso.rs; monotonic, also
    /// counted in `bad`): a GPU fault like an Xid, not a wrong result (no refining above the step)
    pub crashes: u64,
    /// results of this card that passed the CPU check (hits and audits; monotonic)
    pub checked: u64,
    /// what the card mines now (dataset slot + model + path); 0 = unknown
    pub key: u64,
    /// the mining worker of this card runs (not given up, not paused)
    pub alive: bool,
    /// a critical Xid of THIS card since the last sample
    pub xid: Option<u64>,
    /// NVML GPU utilisation (%): busy without progress = a hung kernel, idle without progress = no work
    pub util: Option<u32>,
}

pub trait Probe {
    fn set_offset(&mut self, at: i32) -> Result<(), String>;
    fn sample(&mut self) -> Sample;
    fn sleep(&mut self, secs: f64);
    fn stopped(&self) -> bool;
}

#[derive(Clone, Copy, Debug)]
pub struct Cfg {
    /// seconds between samples
    pub tick: f64,
    /// seconds after an offset change before the power counts (NVML averages ~1 s)
    pub settle: f64,
    /// busy seconds of one candidate (B) / reference (A) segment
    pub seg_b: f64,
    pub seg_a: f64,
    /// least busy seconds at the candidate before a paired window may end (stability)
    pub min_b: f64,
    /// the paired difference is decided when it is this many standard errors away from -noise_w ...
    pub z: f64,
    /// ... or when its standard error is below this (W)
    pub se_ok: f64,
    /// the step rule's noise (W): a step is taken when it lowers the power by more than this
    pub noise_w: f64,
    /// no progress of the attempts for this long = the card does not mine right now (s)
    pub busy_gap: f64,
    /// a window without mining load for this long (in total) ends with `no_load` (s)
    pub noload_max: f64,
    /// no progress this long while the GPU reports itself busy (>= 90 %) = the GPU hangs at this offset: a fault (s)
    pub hang_s: f64,
}

impl Default for Cfg {
    fn default() -> Cfg {
        Cfg { tick: 0.5, settle: 1.5, seg_b: 5.0, seg_a: 3.5, min_b: 8.0, z: 2.5, se_ok: 0.12, noise_w: 0.5, busy_gap: 1.6, noload_max: 300.0, hang_s: 30.0 }
    }
}

/// The paired power of a step: the reference (A, the best offset so far) and the candidate (B) under the same loads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pair {
    pub wa: f64,
    pub wb: f64,
    /// standard error of wb - wa (W), from the per-cycle differences
    pub se: f64,
    /// evaluated MA/s at A and at B under the same loads
    pub ga: f64,
    pub gb: f64,
    /// cycles (A-B pairs on the same load) behind it
    pub n: usize,
}

#[derive(Clone, Debug, Default)]
pub struct Out {
    pub ok: bool,
    /// a driver fault (Xid) or the worker gone: no refining
    pub fault: bool,
    /// not a verdict on the offset: interrupted / the offset cannot be set
    pub infra: bool,
    /// not a verdict on the offset: the card did not mine for `noload_max` (no work, idle mode, pool down)
    pub no_load: bool,
    /// the fault is a hang (no results while busy): the worker cannot come back by itself, the miner must restart
    pub hang: bool,
    pub reason: String,
    /// evaluated MA/s of the card over the busy time of the window (both offsets: the card mined all the time)
    pub mas: f64,
    /// mean power at the candidate (W) and its sd / samples
    pub w: Option<f64>,
    pub w_sd: Option<f64>,
    pub w_n: usize,
    pub clk: Option<u32>,
    pub clk_med: Option<u32>,
    pub t: Option<i32>,
    pub pair: Option<Pair>,
    /// CPU-checked results / wrong results of the card in the window
    pub checked: u64,
    pub bad: u64,
    /// busy seconds at the candidate / wall seconds of the window
    pub busy_b: f64,
    pub secs: f64,
}

#[derive(Clone, Copy)]
struct Pt {
    key: u64,
    w: f64,
}

/// One segment's power per load: (key, mean, n) for every key with 2+ readings, in the order of their first reading.
/// 1.0.11: per key, not only the last key of the segment: with --table-mclock the path (pure table at the low memory clock,
/// table + L=3 at the full one, 15-20 W apart) can change inside a segment, and a segment may share with its neighbour only
/// the load it started on.
fn seg_means(v: &[Pt]) -> Vec<(u64, f64, usize)> {
    let mut out: Vec<(u64, f64, usize)> = Vec::new();
    for p in v {
        match out.iter_mut().find(|x| x.0 == p.key) {
            Some(x) => {
                x.1 += p.w;
                x.2 += 1;
            }
            None => out.push((p.key, p.w, 1)),
        }
    }
    out.retain(|x| x.2 >= 2);
    for x in &mut out {
        x.1 /= x.2 as f64;
    }
    out
}

/// Per-cycle paired differences: every B segment against the A segments next to it under the same load key (each load the
/// B segment and an A neighbour both saw is one pair); returns (mean A, mean B, se of B - A, pairs).  A segment list in time
/// order: (is_b, points).
pub fn paired_from_segments(segs: &[(bool, Vec<(u64, f64)>)]) -> Option<(f64, f64, f64, usize)> {
    let means: Vec<(bool, Vec<(u64, f64, usize)>)> =
        segs.iter().map(|(b, v)| (*b, seg_means(&v.iter().map(|&(key, w)| Pt { key, w }).collect::<Vec<_>>()))).collect();
    let mut d: Vec<(f64, f64)> = Vec::new(); // (a, b) per cycle and load
    for (i, (b, m)) in means.iter().enumerate() {
        if !*b {
            continue;
        }
        for &(kb, wb, _) in m {
            // the A neighbours (before and after) under the same load
            let mut a: Vec<f64> = Vec::new();
            for j in [i.wrapping_sub(1), i + 1] {
                if let Some((false, ma)) = means.get(j) {
                    if let Some(&(_, wa, _)) = ma.iter().find(|x| x.0 == kb) {
                        a.push(wa);
                    }
                }
            }
            if !a.is_empty() {
                d.push((a.iter().sum::<f64>() / a.len() as f64, wb));
            }
        }
    }
    if d.is_empty() {
        return None;
    }
    let n = d.len() as f64;
    let wa = d.iter().map(|x| x.0).sum::<f64>() / n;
    let wb = d.iter().map(|x| x.1).sum::<f64>() / n;
    let diff = wb - wa;
    let se = if d.len() >= 2 { (d.iter().map(|x| (x.1 - x.0 - diff).powi(2)).sum::<f64>() / (n - 1.0)).sqrt() / n.sqrt() } else { f64::INFINITY };
    Some((wa, wb, se, d.len()))
}

/// Is the paired difference decided against the step rule (lower by more than `noise_w`, or not)?
pub fn decided(diff: f64, se: f64, n: usize, c: &Cfg) -> bool {
    n >= 2 && (se <= c.se_ok || (diff + c.noise_w).abs() >= c.z * se)
}

/// One step at `at` on the mining load; `reference` = the best offset so far (paired power), None = the whole window at `at`
/// (a base, resume, confirmation or re-check: `secs` busy seconds at `at`).  A paired window ends when it is decided
/// (`decided`, after `min_b` s at the candidate) or after `secs` busy seconds at the candidate.  The offset is left at `at`
/// (at `reference` after a fault or without load, when there is one).
pub fn window(p: &mut impl Probe, at: i32, reference: Option<i32>, secs: f64, c: &Cfg) -> Out {
    let mut o = Out::default();
    let reference = reference.filter(|&r| r != at);
    if p.stopped() {
        o.infra = true;
        o.reason = "interrupted".into();
        return o;
    }
    if let Err(e) = p.set_offset(at) {
        o.infra = true;
        o.reason = format!("cannot set offset {at:+}: {e}");
        return o;
    }
    let s0 = p.sample();
    let mut prev = s0;
    let mut last_progress = s0.time;
    // 1.0.10 (live test, Shahtyor GPU1 +300): the kernel hung (util 100 %, 44 W, no Xid) and the window took it for "no work"
    // for 300 s; every sample busy since the last progress -> a hang
    let mut stall_busy = true;
    let (mut seg_b, mut seg_busy, mut since_switch) = (true, 0.0f64, 0.0f64);
    let (mut busy_b, mut busy_all, mut idle) = (0.0f64, 0.0f64, 0.0f64);
    let mut att_busy = 0u64;
    let mut segs: Vec<(bool, Vec<(u64, f64)>)> = vec![(true, Vec::new())];
    let mut bsmp: Vec<(f64, u32, i32)> = Vec::new();
    // evaluated attempts / busy seconds per (segment kind, key): the rate at A and at B under the same loads
    let mut rate: std::collections::HashMap<(bool, u64), (u64, f64)> = std::collections::HashMap::new();
    #[derive(Default)]
    struct Pend {
        dt: f64,
        att: u64,
        pts: Vec<(u64, f64, u32, i32)>,
    }
    let mut pend = Pend::default();
    let back = |p: &mut dyn FnMut(i32) -> Result<(), String>| {
        if let Some(r) = reference {
            let _ = p(r);
        }
    };
    loop {
        if p.stopped() {
            o.infra = true;
            o.reason = "interrupted".into();
            break;
        }
        p.sleep(c.tick);
        let s = p.sample();
        let dt = (s.time - prev.time).max(0.0);
        o.secs = s.time - s0.time;
        if let Some(x) = s.xid {
            o.fault = true;
            o.reason = format!("Xid {x} reported by the driver");
            back(&mut |v| p.set_offset(v));
            break;
        }
        if !s.alive {
            o.fault = true;
            o.reason = "the mining worker of this GPU stopped (engine / CUDA error)".into();
            back(&mut |v| p.set_offset(v));
            break;
        }
        if s.crashes > s0.crashes {
            // 1.0.12 (PC_AORUS RTX 3080 Ti: "cudaEventSynchronize: unknown error" after the driver's Xid records): the GPU faulted
            o.fault = true;
            o.reason = format!(
                "the engine process of this GPU crashed while mining at {} (a GPU fault: CUDA error / Xid; only this GPU restarts)",
                if seg_b { format!("{at:+}") } else { format!("{at:+} (seen while back at {:+})", reference.unwrap_or(at)) }
            );
            back(&mut |v| p.set_offset(v));
            break;
        }
        if s.bad > s0.bad {
            o.reason = format!(
                "{} wrong result(s) of this GPU while mining at {} (CPU check of a hit / the engine's canary / an audit)",
                s.bad - s0.bad,
                if seg_b { format!("{at:+}") } else { format!("{at:+} (seen right after switching back to {:+})", reference.unwrap_or(at)) }
            );
            back(&mut |v| p.set_offset(v));
            break;
        }
        since_switch += dt;
        // 1.0.11 (--table-mclock): a new load key (block, slot, path: pure table vs table + L=3, each with its own memory
        // clock, 15-20 W apart) settles as after an offset change: the readings of the first second still show the old load
        if s.key != prev.key {
            since_switch = 0.0;
        }
        // a tick counts once the attempts move on again (a scan call returns every 0.1-0.5 s); ticks without progress for
        // longer than busy_gap are idle time and their readings are dropped (no work, a pause, a model upload)
        pend.dt += dt;
        pend.att += s.attempts - prev.attempts;
        if since_switch >= c.settle && s.key != 0 {
            if let Some(w) = s.w {
                pend.pts.push((s.key, w, s.clk.unwrap_or(0), s.temp.unwrap_or(0)));
            }
        }
        if s.attempts > prev.attempts {
            last_progress = s.time;
            stall_busy = true;
            seg_busy += pend.dt;
            busy_all += pend.dt;
            att_busy += pend.att;
            if seg_b {
                busy_b += pend.dt;
            }
            let e = rate.entry((seg_b, s.key)).or_insert((0, 0.0));
            e.0 += pend.att;
            e.1 += pend.dt;
            for &(k, w, clk, t) in &pend.pts {
                segs.last_mut().unwrap().1.push((k, w));
                if seg_b {
                    bsmp.push((w, clk, t));
                }
            }
            pend = Pend::default();
        } else if s.time - last_progress > c.busy_gap {
            stall_busy &= s.util.is_some_and(|u| u >= 90);
            if stall_busy && s.time - last_progress >= c.hang_s {
                o.fault = true;
                o.hang = true;
                o.reason = format!("this GPU hangs: no results for {:.0} s while it reports itself busy (a hung kernel)", s.time - last_progress);
                back(&mut |v| p.set_offset(v));
                break;
            }
            idle += pend.dt;
            pend = Pend::default();
            since_switch = 0.0; // the power ramps up again once the work comes back: settle as after an offset change
            if idle >= c.noload_max {
                o.no_load = true;
                o.infra = true;
                o.reason = format!("this GPU did not mine for {:.0} s (no work / idle / pool down)", idle);
                back(&mut |v| p.set_offset(v));
                break;
            }
        }
        prev = s;
        // the end: enough time at the candidate (and decided, when paired); a paired window ends after an A segment
        if let Some(r) = reference {
            let pr = paired_from_segments(&segs);
            let done_max = busy_b >= secs && !seg_b;
            let done_dec = busy_b >= c.min_b && !seg_b && seg_busy >= c.seg_a && pr.is_some_and(|(a, b, se, n)| decided(b - a, se, n, c));
            if done_max || done_dec {
                o.ok = true;
                let _ = p.set_offset(at);
                break;
            }
            let want = if seg_b { c.seg_b } else { c.seg_a };
            if seg_busy >= want {
                seg_b = !seg_b;
                if let Err(e) = p.set_offset(if seg_b { at } else { r }) {
                    o.infra = true;
                    o.reason = format!("cannot set offset: {e}");
                    break;
                }
                segs.push((seg_b, Vec::new()));
                seg_busy = 0.0;
                since_switch = 0.0;
            }
        } else if busy_b >= secs {
            o.ok = true;
            break;
        }
    }
    let end = p.sample();
    o.checked = end.checked.saturating_sub(s0.checked);
    o.bad = end.bad.saturating_sub(s0.bad);
    o.busy_b = busy_b;
    o.mas = if busy_all > 0.0 { att_busy as f64 / busy_all / 1e6 } else { 0.0 };
    if !bsmp.is_empty() {
        let n = bsmp.len() as f64;
        let mean = bsmp.iter().map(|s| s.0).sum::<f64>() / n;
        o.w = Some(mean);
        o.w_n = bsmp.len();
        o.w_sd = (bsmp.len() >= 2).then(|| (bsmp.iter().map(|s| (s.0 - mean).powi(2)).sum::<f64>() / (n - 1.0)).sqrt());
        o.clk = bsmp.last().map(|s| s.1).filter(|&x| x > 0);
        let mut ck: Vec<u32> = bsmp.iter().map(|s| s.1).filter(|&x| x > 0).collect();
        ck.sort_unstable();
        o.clk_med = ck.get(ck.len() / 2).copied();
        o.t = bsmp.last().map(|s| s.2).filter(|&x| x > 0);
    }
    if reference.is_some() {
        if let Some((wa, wb, se, n)) = paired_from_segments(&segs) {
            // the rate at A and B over the loads both saw (weighted by B's busy time on each)
            let (mut ga, mut gb, mut wt) = (0.0, 0.0, 0.0);
            for (&(b, k), &(att, t)) in &rate {
                if !b || t <= 0.0 {
                    continue;
                }
                if let Some(&(aa, ta)) = rate.get(&(false, k)).filter(|x| x.1 > 0.0) {
                    gb += att as f64 / t / 1e6 * t;
                    ga += aa as f64 / ta / 1e6 * t;
                    wt += t;
                }
            }
            let (ga, gb) = if wt > 0.0 { (ga / wt, gb / wt) } else { (0.0, 0.0) };
            o.pair = Some(Pair { wa, wb, se, ga, gb, n });
        }
    }
    o
}

/// 1.0.10 (live test, Shahtyor GPU1 21.11.2026): after a fault (Xid / the worker re-opening its CUDA context) the next window must
/// not start until the card mines cleanly again: the worker alive, its attempts growing and no new `bad` for `calm` seconds.
/// Without it the errors of the broken context (a worker error counts as `bad`, then the worker stopped) were charged to every
/// lower offset in turn (+300 Xid -> +225, +210, ... +135 "failed" in seconds) and the crash was remembered at +150.
/// Ok(seconds waited); Err = the card does not come back within `max` (the worker gave up, or no clean mining): no verdict.
pub fn wait_healthy(p: &mut impl Probe, calm: f64, max: f64, tick: f64) -> Result<f64, String> {
    let s0 = p.sample();
    let (mut prev, mut calm_from) = (s0, s0.time);
    loop {
        if p.stopped() {
            return Err("interrupted".into());
        }
        p.sleep(tick);
        let s = p.sample();
        let waited = s.time - s0.time;
        if !s.alive {
            return Err("the mining worker of this GPU stopped after the fault (engine / CUDA error)".into());
        }
        let clean = s.bad == prev.bad && s.xid.is_none() && s.attempts > prev.attempts;
        if !clean {
            // still faulting, or no progress (the context is re-opened): the calm starts again
            calm_from = s.time;
        } else if s.time - calm_from >= calm {
            return Ok(waited);
        }
        if waited >= max {
            return Err(format!("this GPU does not mine cleanly {max:.0} s after the fault"));
        }
        prev = s;
    }
}

#[cfg(test)]
pub mod sim {
    //! A simulated mining card for the tests: power = f(offset, load), loads change on a schedule, an Xid / wrong results from
    //! an offset up, pauses without work.
    use super::*;
    pub struct Card {
        pub t: f64,
        pub off: i32,
        /// power at offset `o` under load `k`
        pub power: Box<dyn Fn(i32, u64) -> f64>,
        /// the load key at time `t`
        pub load: Box<dyn Fn(f64) -> u64>,
        /// evaluated attempts per second under load `k`, at offset `o`
        pub rate: Box<dyn Fn(i32, u64) -> f64>,
        /// first offset with an Xid / with wrong results (after this many seconds at it)
        pub xid_at: Option<(i32, f64)>,
        pub bad_at: Option<(i32, f64)>,
        /// no work in these time intervals
        pub idle: Vec<(f64, f64)>,
        pub att: f64,
        pub bad: u64,
        pub checked: u64,
        pub at_off: f64,
        pub xid_pending: Option<u64>,
        pub sets: Vec<i32>,
        /// deterministic noise of the power readings (W, +- this)
        pub noise: f64,
        pub n: u64,
        /// the worker gives up from this time on
        pub dead_from: Option<f64>,
        /// the kernel hangs from this time on (no attempts, util 100 %)
        pub hang_from: Option<f64>,
        /// the power reading follows a load change this many seconds late (NVML averaging, a memory clock switch)
        pub w_lag: f64,
        /// 1.0.12: the card's engine process crashes at this time (counted in `crashes` and in `bad`, as the miner counts it)
        pub crash_at: Option<f64>,
    }
    impl Card {
        pub fn new(power: impl Fn(i32, u64) -> f64 + 'static) -> Card {
            Card {
                t: 0.0,
                off: 0,
                power: Box::new(power),
                load: Box::new(|_| 1),
                rate: Box::new(|_, _| 200e6),
                xid_at: None,
                bad_at: None,
                idle: Vec::new(),
                att: 0.0,
                bad: 0,
                checked: 0,
                at_off: 0.0,
                xid_pending: None,
                sets: Vec::new(),
                noise: 0.0,
                n: 0,
                dead_from: None,
                hang_from: None,
                w_lag: 0.0,
                crash_at: None,
            }
        }
        fn idle_now(&self) -> bool {
            self.idle.iter().any(|&(a, b)| self.t >= a && self.t < b)
        }
    }
    impl Probe for Card {
        fn set_offset(&mut self, at: i32) -> Result<(), String> {
            if at != self.off {
                self.at_off = 0.0;
            }
            self.off = at;
            self.sets.push(at);
            Ok(())
        }
        fn sample(&mut self) -> Sample {
            let k = (self.load)(self.t);
            self.n += 1;
            // a fixed pseudo-random sequence in [-1, 1]
            let r = ((self.n.wrapping_mul(2654435761) >> 7) % 2001) as f64 / 1000.0 - 1.0;
            // the power of the interval that just ended
            let was_idle = self.idle.iter().any(|&(a, b)| self.t - 1e-9 >= a && self.t - 1e-9 < b);
            let kw = if self.w_lag > 0.0 { (self.load)((self.t - self.w_lag).max(0.0)) } else { k };
            let w = if was_idle { 30.0 } else { (self.power)(self.off, kw) + r * self.noise };
            Sample {
                time: self.t,
                w: Some(w),
                clk: Some(1650),
                temp: Some(60),
                attempts: self.att as u64,
                bad: self.bad + self.crash_at.is_some_and(|c| self.t >= c) as u64,
                crashes: self.crash_at.is_some_and(|c| self.t >= c) as u64,
                checked: self.checked,
                key: k,
                alive: self.dead_from.map_or(true, |d| self.t < d),
                xid: self.xid_pending.take(),
                util: Some(if self.hang_from.is_some_and(|h| self.t >= h) { 100 } else if was_idle { 0 } else { 100 }),
            }
        }
        fn sleep(&mut self, secs: f64) {
            let k = (self.load)(self.t);
            if !self.idle_now() && !self.hang_from.is_some_and(|h| self.t >= h) {
                self.att += (self.rate)(self.off, k) * secs;
                self.checked += 1;
                self.at_off += secs;
                if let Some((o, after)) = self.xid_at {
                    if self.off >= o && self.at_off >= after {
                        self.xid_pending = Some(31);
                    }
                }
                if let Some((o, after)) = self.bad_at {
                    if self.off >= o && self.at_off >= after {
                        self.bad += 1;
                    }
                }
            }
            self.t += secs;
        }
        fn stopped(&self) -> bool {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::sim::Card;
    use super::*;

    /// a 3070-like card: 160 W at +0, -1.2 W per 15 MHz up to +300, flat above; load 2 (TESS) draws 7 W more than load 1 (Kepler)
    fn w3070(o: i32, k: u64) -> f64 {
        160.0 - 1.2 * (o.min(300) as f64 / 15.0) + if k == 2 { 7.0 } else { 0.0 }
    }

    #[test]
    fn a_step_on_the_mining_load_counts_the_hashrate_and_pairs_the_power() {
        let mut c = Card::new(w3070);
        let o = window(&mut c, 120, Some(90), 25.0, &Cfg::default());
        assert!(o.ok && !o.fault && !o.infra, "{o:?}");
        // the card mined the whole time: 200 MA/s counted
        assert!((o.mas - 200.0).abs() < 1.0, "{}", o.mas);
        let p = o.pair.expect("paired");
        assert!((p.wb - p.wa - (-2.4)).abs() < 0.05, "{p:?}");
        // decided early (no noise): far less than the 25 s maximum at the candidate
        assert!(o.busy_b < 25.0 && o.busy_b >= Cfg::default().min_b, "{}", o.busy_b);
        // the offset is left at the candidate; the reference was visited in between
        assert_eq!(c.off, 120);
        assert!(c.sets.contains(&90));
    }

    /// 1.0.12 (PC_AORUS RTX 3080 Ti): +300 was taken, the next paired window (+360 against +300) ends in a crash of the card's
    /// engine process: a GPU fault (no refining above), not a wrong result; the offset goes back to the reference (+300).
    #[test]
    fn a_crash_of_the_engine_process_in_the_window_is_a_fault() {
        let mut c = Card::new(w3070);
        c.crash_at = Some(7.0); // in the first A segment (B 5 s busy, then A)
        let o = window(&mut c, 360, Some(300), 25.0, &Cfg::default());
        assert!(!o.ok && o.fault && !o.infra, "{o:?}");
        assert!(o.reason.contains("engine process of this GPU crashed") && o.reason.contains("seen while back at +300"), "{}", o.reason);
        assert_eq!(c.off, 300);
        // in a B segment
        let mut c = Card::new(w3070);
        c.crash_at = Some(2.0);
        let o = window(&mut c, 360, Some(300), 25.0, &Cfg::default());
        assert!(o.fault && o.reason.contains("crashed while mining at +360 (a GPU"), "{}", o.reason);
    }

    #[test]
    fn a_load_change_in_the_window_is_not_a_saving() {
        // the block changes every 7 s between Kepler (1) and TESS (2, +7 W): the absolute mean would be off by several W
        let mut c = Card::new(w3070);
        c.load = Box::new(|t| if (t / 7.0) as u64 % 2 == 0 { 1 } else { 2 });
        c.noise = 0.4;
        let o = window(&mut c, 120, Some(90), 25.0, &Cfg::default());
        assert!(o.ok, "{o:?}");
        let p = o.pair.expect("paired");
        assert!((p.wb - p.wa + 2.4).abs() < 0.5, "{p:?}");
    }

    #[test]
    fn a_path_switch_with_its_memory_clock_is_not_a_saving() {
        // 1.0.11 --table-mclock: the slot alternates between the pure table (load 1, memory 810: 120 W) and table + L=3
        // (load 2, full memory: 140 W); the power reading follows a switch ~1 s late (NVML averaging, the memory P-state).
        // The readings right after a key change belong to the old path: they must not count for the new key.
        for (period, phase) in [(6.0, 0.0), (6.0, 2.3), (7.0, 1.1), (9.0, 3.7), (20.0, 8.0), (45.0, 30.0)] {
            let mut c = Card::new(|o, k| 140.0 - 1.2 * (o.min(300) as f64 / 15.0) - if k == 1 { 20.0 } else { 0.0 });
            c.load = Box::new(move |t| if ((t + phase) / period) as u64 % 2 == 0 { 1 } else { 2 });
            c.w_lag = 1.0;
            c.noise = 0.3;
            let o = window(&mut c, 120, Some(90), 25.0, &Cfg::default());
            assert!(o.ok, "{o:?}");
            let p = o.pair.unwrap_or_else(|| panic!("period {period} phase {phase}: not paired {o:?}"));
            assert!((p.wb - p.wa + 2.4).abs() < 0.6, "period {period} phase {phase}: {p:?}");
        }
    }

    #[test]
    fn a_flat_step_is_measured_to_the_maximum_when_undecided() {
        // above +300 the power does not fall: 0 W difference with 0.4 W noise is near the -0.5 W rule -> more cycles
        let mut c = Card::new(w3070);
        c.noise = 0.6;
        let cfg = Cfg::default();
        let o = window(&mut c, 345, Some(330), 25.0, &cfg);
        assert!(o.ok, "{o:?}");
        let p = o.pair.unwrap();
        assert!((p.wb - p.wa).abs() < 0.6, "{p:?}");
        assert!(p.n >= 2);
    }

    #[test]
    fn an_xid_fails_the_step_as_a_fault_and_goes_back_to_the_reference() {
        let mut c = Card::new(w3070);
        c.xid_at = Some((390, 3.0));
        let o = window(&mut c, 395, Some(365), 25.0, &Cfg::default());
        assert!(!o.ok && o.fault && !o.infra, "{o:?}");
        assert!(o.reason.contains("Xid 31"), "{}", o.reason);
        assert_eq!(c.off, 365, "back to the last good offset");
    }

    #[test]
    fn a_wrong_result_fails_the_step_without_a_fault() {
        let mut c = Card::new(w3070);
        c.bad_at = Some((300, 4.0));
        let o = window(&mut c, 300, Some(270), 25.0, &Cfg::default());
        assert!(!o.ok && !o.fault && !o.infra, "{o:?}");
        assert!(o.reason.contains("wrong result"), "{}", o.reason);
        assert_eq!(c.off, 270);
    }

    #[test]
    fn time_without_work_is_not_counted_and_a_long_pause_is_no_verdict() {
        let mut c = Card::new(w3070);
        c.idle = vec![(3.0, 23.0)];
        let o = window(&mut c, 120, None, 15.0, &Cfg::default());
        assert!(o.ok, "{o:?}");
        assert!(o.secs >= 35.0, "the 20 s without work are not part of the 15 s at the offset: {}", o.secs);
        assert!((o.w.unwrap() - w3070(120, 1)).abs() < 0.01, "no idle sample in the power: {:?}", o.w);
        let mut c = Card::new(w3070);
        c.idle = vec![(1.0, 10_000.0)];
        let cfg = Cfg { noload_max: 60.0, ..Default::default() };
        let o = window(&mut c, 120, Some(90), 25.0, &cfg);
        assert!(!o.ok && o.no_load && o.infra && !o.fault, "{o:?}");
        assert_eq!(c.off, 90);
    }

    #[test]
    fn a_window_without_reference_stays_at_the_offset() {
        let mut c = Card::new(w3070);
        let o = window(&mut c, 240, None, 15.0, &Cfg::default());
        assert!(o.ok && o.pair.is_none());
        assert!(c.sets.iter().all(|&x| x == 240));
        assert!((o.busy_b - 15.0).abs() < 0.6);
        assert!((o.w.unwrap() - w3070(240, 1)).abs() < 0.01);
    }

    #[test]
    fn segments_are_paired_only_under_the_same_load() {
        let s = |b: bool, k: u64, w: f64| (b, vec![(k, w), (k, w), (k, w)]);
        // B (load 2) between A segments of load 1: no pair; then a clean A-B-A on load 1
        let segs = vec![s(false, 1, 150.0), s(true, 2, 155.0), s(false, 1, 150.0), s(true, 1, 148.0), s(false, 1, 150.0)];
        let (wa, wb, _se, n) = paired_from_segments(&segs).unwrap();
        assert_eq!(n, 1);
        assert!((wa - 150.0).abs() < 1e-9 && (wb - 148.0).abs() < 1e-9);
        assert!(paired_from_segments(&[s(true, 1, 150.0), s(false, 2, 140.0)]).is_none());
        let c = Cfg::default();
        assert!(!decided(-2.0, f64::INFINITY, 1, &c));
        assert!(decided(-2.0, 0.3, 2, &c)); // 1.5 W below the rule, 5 se
        assert!(!decided(-0.6, 0.3, 3, &c)); // 0.1 W from the rule: more cycles
        assert!(decided(-0.6, 0.1, 3, &c)); // se small enough
    }

    #[test]
    fn after_a_fault_the_next_window_waits_for_clean_mining() {
        // the context is re-opened 0..10 s (no attempts), a worker error counts as bad at 2 s and 11 s, then it mines cleanly
        let mut c = Card::new(w3070);
        c.idle = vec![(0.0, 10.0)];
        let mut t = 0.0;
        let mut errs = vec![2.0, 11.0];
        let waited = loop {
            // drive the errors by time: a small wrapper probe
            struct P<'a>(&'a mut Card, &'a mut Vec<f64>, &'a mut f64);
            impl Probe for P<'_> {
                fn set_offset(&mut self, at: i32) -> Result<(), String> { self.0.set_offset(at) }
                fn sample(&mut self) -> Sample { self.0.sample() }
                fn sleep(&mut self, secs: f64) {
                    self.0.sleep(secs);
                    *self.2 = self.0.t;
                    while self.1.first().is_some_and(|&e| self.0.t >= e) {
                        self.1.remove(0);
                        self.0.bad += 1;
                    }
                }
                fn stopped(&self) -> bool { false }
            }
            break wait_healthy(&mut P(&mut c, &mut errs, &mut t), 3.0, 60.0, 0.5);
        };
        let w = waited.expect("comes back");
        assert!(w >= 14.0 && w < 15.5, "calm 3 s after the last error at 11 s: {w}");
        // the worker gives up: no verdict (Err), not a failure of the next offset
        let mut c = Card::new(w3070);
        c.idle = vec![(0.0, 1e9)];
        c.dead_from = Some(15.0);
        let e = wait_healthy(&mut c, 3.0, 60.0, 0.5).unwrap_err();
        assert!(e.contains("stopped"), "{e}");
        // never clean within max
        let mut c = Card::new(w3070);
        c.idle = vec![(0.0, 1e9)];
        assert!(wait_healthy(&mut c, 3.0, 20.0, 0.5).unwrap_err().contains("does not mine"));
        // a card that mines: ~3 s
        let mut c = Card::new(w3070);
        assert!(wait_healthy(&mut c, 3.0, 20.0, 0.5).unwrap() <= 3.6);
    }

    #[test]
    fn a_hung_kernel_is_a_fault_not_a_pause_without_work() {
        let mut c = Card::new(w3070);
        c.hang_from = Some(6.0);
        let o = window(&mut c, 300, Some(240), 25.0, &Cfg::default());
        assert!(!o.ok && o.fault && o.hang && !o.no_load, "{o:?}");
        assert!(o.secs < 40.0, "decided after hang_s, not after noload_max: {}", o.secs);
        assert_eq!(c.off, 240);
        // no work (the GPU idle): still no verdict after noload_max, never a hang
        let mut c = Card::new(w3070);
        c.idle = vec![(1.0, 10_000.0)];
        let o = window(&mut c, 120, Some(90), 25.0, &Cfg { noload_max: 60.0, ..Default::default() });
        assert!(o.no_load && !o.fault && !o.hang, "{o:?}");
    }
}
