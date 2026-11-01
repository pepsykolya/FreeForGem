// FreeForGem — see LICENSE
//! Temperature protection `--temp-limit` (1.7.6, ported from ffgs 0.5.4), meant MAINLY FOR LAPTOPS AND MOBILE GPUs: their cooling
//! is shared with the CPU and the fans cannot be set, so a long mining run sits at the GPU's own throttle point (~87 C).  Desktop
//! rigs keep their temperatures with fans (HiveOS autofan) and need none of this.
//!   --temp-limit C[,C...]   keep the GPU at or below C (one value for all GPUs, or a list in --devices order; '-' / 0 = off)
//!   --temp-stop C2[,...]    hard stop: at C2 the GPU stops mining until it has cooled to C - 6 (default min(C + 10, 90), at least C + 3)
//!
//! How: one control thread reads the core temperature (NVML, no root needed) every second.  Mining starts soft at u = 30 %
//! (ramped up; no log line unless the GPU gets near its limit).  At C - 3 (filtered reading; or a reading at C, or two in a row
//! at C - 2) the GPU enters the limit state; a PI controller (filtered reading, small gains, u changes rate-limited: +5 % / -20 %
//! per second at most) sets the work fraction u (0.10 .. 1) so the filtered temperature stays in C-2 .. C-1.  u is applied as a
//! duty cycle by the worker (one launch, then idle busy * (1 - u) / u; the same mechanism as the light load of idle mode: works
//! without admin rights, on laptops whose clocks cannot be set).  Hard stop: the reading at its stop temperature for 3 readings in a
//! row (or the filtered one there, or one reading 5 C above it): no launches until it is 6 C below the limit (or 3 C below after 60
//! s), then the controller resumes at u = 30 % and ramps up.  The log has one line per state change.
//! Not ported from ffgs: the memory / hotspot sensors (NVML has no hotspot; GeForce rarely reports the memory junction), lowering
//! the core clock lock before adding idle gaps, AMD.  CMFD: the engine is an external binary that cannot be duty-cycled.
//! The controller is pure (`Ctl`), tested on simulated GPUs below.
use crate::nvml::Nvml;
use crate::uv::PerGpu;
use anyhow::{Result, ensure};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const ENTER: f64 = 3.0; // the limit state starts at C - 3 (filtered reading, or any reading at C)
const BAND_HI: f64 = 1.0; // target band C-2 .. C-1 (filtered)
const BAND_LO: f64 = 2.0;
const RESUME_FREE: f64 = 5.0; // full speed again (logged) at or below C - 5 ...
const FULL_HOLD_S: f64 = 30.0; //   ... with u at 1 for this long
const RESUME_STOP: f64 = 6.0; // after a hard stop: mining again at C - 6 ...
const RESUME_LATE: f64 = 3.0; //   ... or at C - 3 once it has been stopped STOP_WAIT_S
const STOP_WAIT_S: f64 = 60.0;
const START_U: f64 = 0.30; // the start and the resume after a stop: from this work fraction, ramped up
const UMIN: f64 = 0.10; // the least work in the limit state
const STOP_N: u32 = 3; // --temp-stop: this many readings in a row at or above it ...
const STOP_NOW: f64 = 5.0; //   ... or one reading this far above it, or the filtered reading at it
const KP: f64 = 0.05; // per C of change of the filtered excess (velocity form)
const KI: f64 = 0.002; // per C and second outside the target band
const TF: f64 = 6.0; // s, filter of the reading
const TF_STOP: f64 = 3.0; // s, filter of the reading against --temp-stop
const UP: f64 = 0.05; // the work fraction rises at most this much per second ...
const DOWN: f64 = 0.20; //   ... and falls at most this much

/// The default hard stop of a limit C.
pub fn default_stop(c: i32) -> i32 {
    (c + 10).min(90).max(c + 3)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limits {
    pub limit: i32,
    pub stop: i32,
}

impl Limits {
    pub fn check(&self) -> Result<()> {
        ensure!((40..=100).contains(&self.limit), "--temp-limit must be 40..100 C");
        ensure!(self.stop >= self.limit + 3 && self.stop <= 110, "--temp-stop must be at least --temp-limit + 3 and at most 110 C");
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum State {
    Free,
    Limit,
    Stop,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event {
    None,
    Limit,
    Free,
    Stop,
    Resume,
}

/// u = the work fraction (1 = full speed, 0 = stopped).  `step` once per `dt` seconds with the current reading.
#[derive(Clone, Debug)]
pub struct Ctl {
    pub st: State,
    pub u: f64,
    quiet: bool, // the soft start: the limit was not reached yet (no log lines)
    have: bool,
    x_prev: f64,
    xf: f64,
    xsf: f64,
    over_n: u32,
    x_last: f64,
    full_s: f64,
    stop_s: f64,
}

impl Default for Ctl {
    fn default() -> Ctl {
        Ctl { st: State::Limit, u: START_U, quiet: true, have: false, x_prev: 0.0, xf: 0.0, xsf: 0.0, over_n: 0, x_last: -1e9, full_s: 0.0, stop_s: 0.0 }
    }
}

fn band_err(x: f64) -> f64 {
    if x > -BAND_HI {
        x + BAND_HI
    } else if x < -BAND_LO {
        x + BAND_LO
    } else {
        0.0
    }
}

impl Ctl {
    pub fn step(&mut self, temp: f64, l: &Limits, dt: f64) -> Event {
        let (x, xs) = (temp - l.limit as f64, temp - l.stop as f64);
        if !self.have {
            self.xf = x;
            self.x_prev = x;
            self.xsf = xs;
            self.have = true;
        } else {
            self.xf += (x - self.xf) * (dt / TF).min(1.0);
            self.xsf += (xs - self.xsf) * (dt / TF_STOP).min(1.0);
        }
        self.over_n = if xs >= 0.0 { self.over_n + 1 } else { 0 };
        // hard stop: not on a single 1 s spike of a fast sensor (the limit controller handles those), but at once far above
        if self.st != State::Stop && (xs >= STOP_NOW || self.over_n >= STOP_N || self.xsf >= 0.0) {
            self.st = State::Stop;
            self.u = 0.0;
            self.stop_s = 0.0;
            self.quiet = false;
            return Event::Stop;
        }
        match self.st {
            State::Stop => {
                self.stop_s += dt;
                if x > -RESUME_STOP && !(self.stop_s >= STOP_WAIT_S && x <= -RESUME_LATE) {
                    return Event::None;
                }
                self.st = State::Limit;
                self.u = START_U;
                self.x_prev = self.xf;
                self.full_s = 0.0;
                self.over_n = 0;
                Event::Resume
            }
            State::Free => {
                // the filtered reading at C-3, any reading at C, or two in a row at C-2
                if self.xf < -ENTER && x < 0.0 && !(x >= -ENTER + 1.0 && self.x_last >= -ENTER + 1.0) {
                    self.x_last = x;
                    return Event::None;
                }
                self.st = State::Limit;
                self.x_prev = self.xf;
                self.full_s = 0.0; // u stays 1: the controller takes it down
                Event::Limit
            }
            State::Limit => {
                let du = -KP * (self.xf - self.x_prev) - KI * dt * band_err(self.xf);
                self.x_prev = self.xf;
                let du = du.clamp(-DOWN * dt, UP * dt);
                self.u = (self.u + du).clamp(UMIN, 1.0);
                if self.quiet && (self.xf >= -ENTER || x >= 0.0) {
                    self.quiet = false; // the soft start reached the limit
                    return Event::Limit;
                }
                // full speed and at or below C-5 (filtered) for FULL_HOLD_S without a break
                self.full_s = if self.u >= 0.98 && self.xf <= -RESUME_FREE { self.full_s + dt } else { 0.0 };
                if self.full_s >= FULL_HOLD_S {
                    self.st = State::Free;
                    self.u = 1.0;
                    self.x_last = -1e9;
                    let q = self.quiet;
                    self.quiet = false;
                    return if q { Event::None } else { Event::Free };
                }
                Event::None
            }
        }
    }

    #[allow(dead_code)]
    pub fn throttled(&self) -> bool {
        self.st == State::Stop || (self.st == State::Limit && self.u < 0.995)
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// the shared work fraction the workers read
// ---------------------------------------------------------------------------------------------------------------------

const MAX_DEV: usize = 64;
// per CUDA device: work fraction in 1/1000 (1000 = not limited, 0 = stopped); u32::MAX = no limit on this device
static WORK: [AtomicU32; MAX_DEV] = [const { AtomicU32::new(u32::MAX) }; MAX_DEV];

/// The share of the time the worker of `dev` may mine now, in %: None = full speed; Some(0) = stopped.  Combines the temperature
/// limit with the light load of idle mode (the smaller one).
pub fn duty(dev: i32) -> Option<u8> {
    let temp = WORK.get(dev as usize).map(|w| w.load(Ordering::Relaxed)).filter(|&w| w != u32::MAX && w < 1000).map(|w| ((w as f64 / 10.0).round() as u8).min(99));
    let idle = match crate::idle::mode() {
        crate::idle::Mode::Light(p) => Some(p),
        _ => None,
    };
    match (temp, idle) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// Is the worker of `dev` throttled on purpose (a stall watchdog must not take it for a stall)?
#[allow(dead_code)]
pub fn throttled(dev: i32) -> bool {
    WORK.get(dev as usize).is_some_and(|w| w.load(Ordering::Relaxed) < 1000)
}

pub struct Opts {
    pub limit: PerGpu,
    pub stop: PerGpu,
}

impl Opts {
    pub fn on(&self) -> bool {
        self.limit.is_set()
    }

    /// The limits of the GPU at position `pos` of --devices (None: not limited).
    pub fn limits(&self, pos: usize) -> Result<Option<Limits>> {
        let Some(c) = self.limit.get(pos).filter(|&c| c > 0) else { return Ok(None) };
        let l = Limits { limit: c, stop: self.stop.get(pos).filter(|&s| s > 0).unwrap_or_else(|| default_stop(c)) };
        l.check()?;
        Ok(Some(l))
    }

    pub fn validate(&self, n: usize) -> Result<()> {
        self.limit.check_count(n, "--temp-limit")?;
        self.stop.check_count(n, "--temp-stop")?;
        for pos in 0..n {
            self.limits(pos)?;
        }
        Ok(())
    }
}

/// Starts the control thread for the mining GPUs `devs` = (CUDA device, PCI bus id, name, position in --devices).  Returns at once.
pub fn start(o: &Opts, devs: &[(i32, String, String, usize)], stop: &Arc<AtomicBool>) {
    if !o.on() {
        return;
    }
    let nv = match Nvml::load() {
        Ok(n) => n,
        Err(e) => {
            println!("temp-limit: NVML not available ({e:#}): --temp-limit is off");
            return;
        }
    };
    let mut cards = Vec::new();
    for (dev, bus, name, pos) in devs {
        let l = match o.limits(*pos) {
            Ok(Some(l)) => l,
            Ok(None) => continue,
            Err(e) => {
                println!("temp-limit: {e:#}");
                continue;
            }
        };
        let Ok(h) = nv.by_bus(bus) else {
            println!("temp-limit: GPU{dev} not found in NVML: not limited");
            continue;
        };
        if nv.temp_c(h).is_none() {
            println!("temp-limit: GPU{dev} ({name}) has no readable core temperature: not limited");
            continue;
        }
        if let Some(w) = WORK.get(*dev as usize) {
            w.store((START_U * 1000.0) as u32, Ordering::Relaxed);
        }
        println!("temp-limit: GPU{dev} ({name}): limit {} C, hard stop {} C (starts soft at {:.0} % and ramps up; the hashrate shown is the real one)", l.limit, l.stop, START_U * 100.0);
        cards.push((*dev, h, name.clone(), l, Ctl::default()));
    }
    if cards.is_empty() {
        return;
    }
    let stop = stop.clone();
    let _ = std::thread::Builder::new().name("temp-limit".into()).spawn(move || {
        let mut last = Instant::now();
        while !stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_secs(1));
            let dt = last.elapsed().as_secs_f64().clamp(0.2, 5.0);
            last = Instant::now();
            for (dev, h, name, l, ctl) in cards.iter_mut() {
                let Some(t) = nv.temp_c(*h) else { continue };
                let ev = ctl.step(t as f64, l, dt);
                if let Some(w) = WORK.get(*dev as usize) {
                    w.store((ctl.u * 1000.0).round() as u32, Ordering::Relaxed);
                }
                match ev {
                    Event::Limit => println!("temp-limit: GPU{dev} ({name}) is near its limit ({t} C of {} C): working {:.0} % of the time", l.limit, ctl.u * 100.0),
                    Event::Free => println!("temp-limit: GPU{dev} ({name}) is cool again ({t} C): full speed"),
                    Event::Stop => println!("temp-limit: GPU{dev} ({name}) reached {t} C (stop {} C): mining stopped until it has cooled to {} C", l.stop, l.limit - RESUME_STOP as i32),
                    Event::Resume => println!("temp-limit: GPU{dev} ({name}) cooled to {t} C: mining again, soft start at {:.0} %", START_U * 100.0),
                    Event::None => {}
                }
            }
        }
    });
}

/// " [temp-limit 72%]" / " [temp-stop 91 C]" for the status line of GPU `dev`.
#[allow(dead_code)]
pub fn tag(dev: i32) -> String {
    match WORK.get(dev as usize).map(|w| w.load(Ordering::Relaxed)) {
        Some(0) => " [temp-stop]".into(),
        Some(w) if w < 1000 => format!(" [temp-limit {}%]", (w as f64 / 10.0).round() as u32),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A first-order thermal model driven by the work fraction the controller sets: ambient + gain * u, time constant tau.
    fn sim(l: &Limits, amb: f64, gain: f64, tau: f64, secs: usize) -> (Vec<f64>, Ctl, Vec<Event>) {
        let mut c = Ctl::default();
        let mut t = amb;
        let mut temps = Vec::new();
        let mut ev = Vec::new();
        for _ in 0..secs {
            let e = c.step(t.round(), l, 1.0);
            if e != Event::None {
                ev.push(e);
            }
            let target = amb + gain * c.u;
            t += (target - t) / tau;
            temps.push(t);
        }
        (temps, c, ev)
    }

    #[test]
    fn a_hot_gpu_is_held_near_its_limit() {
        // 40 C idle, 90 C at full work, 20 s time constant: limit 75 C
        let l = Limits { limit: 75, stop: 85 };
        let (temps, c, ev) = sim(&l, 40.0, 50.0, 20.0, 1800);
        let late = &temps[900..];
        let (lo, hi) = (late.iter().cloned().fold(f64::MAX, f64::min), late.iter().cloned().fold(f64::MIN, f64::max));
        assert!(hi <= 76.5 && lo >= 70.0, "{lo}..{hi}");
        assert!(c.u > 0.5 && c.u < 0.9, "u = {}", c.u);
        assert!(!ev.contains(&Event::Stop), "{ev:?}"); // never needed the hard stop
        assert!(ev.contains(&Event::Limit));
    }

    #[test]
    fn a_cool_gpu_is_not_limited() {
        let l = Limits { limit: 80, stop: 90 };
        let (_, c, ev) = sim(&l, 30.0, 40.0, 20.0, 600); // 70 C at full work: below C - 3
        assert_eq!(c.u, 1.0);
        assert!(!c.throttled());
        assert!(!ev.contains(&Event::Limit) && !ev.contains(&Event::Stop), "{ev:?}"); // the soft start is quiet
    }

    #[test]
    fn the_hard_stop_and_the_resume() {
        let l = Limits { limit: 70, stop: 80 };
        let mut c = Ctl::default();
        for _ in 0..5 {
            assert_eq!(c.step(60.0, &l, 1.0), Event::None);
        }
        // three readings in a row at the stop temperature
        let evs: Vec<Event> = (0..3).map(|_| c.step(80.0, &l, 1.0)).collect();
        assert_eq!(evs[2], Event::Stop);
        assert_ne!(evs[1], Event::Stop);
        assert_eq!((c.st, c.u), (State::Stop, 0.0));
        // stays stopped above C - 6
        for _ in 0..10 {
            assert_eq!(c.step(66.0, &l, 1.0), Event::None);
        }
        // at C - 6 it resumes, softly
        assert_eq!(c.step(64.0, &l, 1.0), Event::Resume);
        assert_eq!((c.st, c.u), (State::Limit, START_U));
        // one reading far above the stop temperature stops at once
        let mut d = Ctl::default();
        assert_eq!(d.step(60.0, &l, 1.0), Event::None);
        assert_eq!(d.step(85.0, &l, 1.0), Event::Stop);
        // after 60 s a warm GPU resumes at C - 3
        let mut e = Ctl::default();
        for _ in 0..5 {
            e.step(60.0, &l, 1.0);
        }
        for _ in 0..3 {
            e.step(80.0, &l, 1.0);
        }
        let mut resumed = false;
        for _ in 0..80 {
            if e.step(67.0, &l, 1.0) == Event::Resume {
                resumed = true;
                break;
            }
        }
        assert!(resumed);
    }

    #[test]
    fn options_and_duty() {
        let o = Opts { limit: PerGpu::parse("75,-,80", "--temp-limit").unwrap(), stop: PerGpu::Unset };
        assert!(o.on());
        assert_eq!(o.limits(0).unwrap(), Some(Limits { limit: 75, stop: 85 }));
        assert_eq!(o.limits(1).unwrap(), None);
        assert_eq!(o.limits(2).unwrap(), Some(Limits { limit: 80, stop: 90 }));
        assert_eq!(default_stop(88), 91);
        assert_eq!(default_stop(70), 80);
        assert_eq!(default_stop(89), 92); // at least C + 3
        assert!(Opts { limit: PerGpu::parse("30", "--temp-limit").unwrap(), stop: PerGpu::Unset }.validate(1).is_err());
        assert!(Opts { limit: PerGpu::parse("75", "--temp-limit").unwrap(), stop: PerGpu::parse("77", "--temp-stop").unwrap() }.validate(1).is_err());
        assert!(o.validate(2).is_err()); // a list must name every GPU
        // no limit running on device 60: full speed
        assert_eq!(duty(60), None);
    }
}
