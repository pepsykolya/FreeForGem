// FreeForGemPear — see LICENSE
//! `--auto-uv` in NOID mode: a core V/F offset (undervolt) tuned and checked on the NOID kernel itself, per card.
//!
//! Why its own tuner: NOID fails at offsets Pearl runs fine with, and near the edge it fails SILENTLY — no Xid, no crash, the
//! quick self-test passes, yet some launches return wrong results (RTX 3070 @1560 MHz, +250: 168 of 5984 launches wrong).
//! Every step is therefore checked by agent M's stress check in a child process (`--noid-uv-check`): full-grid launches of the
//! NOID kernel of this run's mode on a fixed job for `step` seconds, every launch's candidate set compared bit-exactly with the
//! CPU set of the official crate (thousands of checks under full load).
//!   * tune: the base offset (0, or the owner's negative one) is checked first; then from the start offset (--uv-min, default
//!     +120; one step lower while it fails) up in --uv-step steps to the card's limit (--uv-max / the 1.6.2 per-card default);
//!     the first failing step is the edge; the result is one step lower (--uv-keep-last: the last good one), confirmed with a
//!     long check (--uv-confirm-secs, one more step down on failure).
//!   * the result goes to uv.json under "noid" (keyed by card UUID and mode; the Pearl "gpus" and CMFD sections are kept),
//!     with the driver, the kernel, the clock lock and the power limit it was found at (a change of any of them = tune again);
//!     a later start applies it after a short check (none when verified within --uv-reval-hours).
//!   * while mining, a hit that fails the CPU check steps that card's offset back by one step (saved) and mining goes on.
//!   * the owner's offset is put back on exit by offset.rs (marker file for crashes).
use super::{gpu, offset};
use crate::nvml::{Dev, Nvml};
use crate::uv::{self, DISABLE_SECS, HARD_CAP, Saved, fmt_age, fmt_w, now, saved_vs_limit, short_bus, short_name, signed};
use anyhow::Result;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const SECTION: &str = "noid";
const TRYING: &str = "noid_trying";
const START: i32 = 120; // default first step (RTX 3070: NOID edge +235..+250 at 1400..1650 MHz)
const STEP_SECS: u32 = 40; // agent M: 40 s of stress per step (first silent errors after ~12 s at the edge)
const BASE_SECS: u32 = 30;
const QUICK_SECS: u32 = 30;

fn lg(m: impl AsRef<str>) {
    println!("auto-uv (noid): {}", m.as_ref());
}

pub struct CardRef {
    pub dev: i32,
    pub pos: usize,
    pub bus: String,
    pub name: String,
    pub core_lock: Option<u32>,
}

#[derive(Clone, Default, Debug)]
struct Entry {
    name: String,
    bus: String,
    driver: String,
    kernel: String,
    core_lock: u32,
    pl: f64,
    orig: i32,
    base: i32,
    offset: i32,
    cap: i32,
    step: i32,
    edge: i32, // first failing offset (0 = none found below the cap)
    mhs_base: f64,
    w_base: f64,
    mhs: f64,
    w: f64,
    tested_at: u64,
    verified_at: u64,
    disabled_until: u64,
    stepped_back: u32,
}

impl Entry {
    fn to_json(&self) -> Value {
        json!({"name": self.name, "bus": self.bus, "driver": self.driver, "kernel": self.kernel, "core_lock": self.core_lock,
               "power_limit": self.pl, "orig_offset": self.orig, "base_offset": self.base, "offset": self.offset, "cap": self.cap,
               "step": self.step, "edge": self.edge, "mhs_base": self.mhs_base, "watts_base": self.w_base, "mhs": self.mhs,
               "watts": self.w, "tested_at": self.tested_at, "verified_at": self.verified_at, "disabled_until": self.disabled_until,
               "stepped_back_in_mining": self.stepped_back})
    }
    fn from_json(v: &Value) -> Option<Entry> {
        let f = |k: &str| v[k].as_f64().unwrap_or(0.0);
        let u = |k: &str| v[k].as_u64().unwrap_or(0);
        Some(Entry {
            name: v["name"].as_str().unwrap_or("").into(),
            bus: v["bus"].as_str().unwrap_or("").into(),
            driver: v["driver"].as_str()?.into(),
            kernel: v["kernel"].as_str()?.into(),
            core_lock: u("core_lock") as u32,
            pl: f("power_limit"),
            orig: v["orig_offset"].as_i64().unwrap_or(0) as i32,
            base: v["base_offset"].as_i64()? as i32,
            offset: v["offset"].as_i64()? as i32,
            cap: v["cap"].as_i64().unwrap_or(0) as i32,
            step: v["step"].as_i64().unwrap_or(0) as i32,
            edge: v["edge"].as_i64().unwrap_or(0) as i32,
            mhs_base: f("mhs_base"),
            w_base: f("watts_base"),
            mhs: f("mhs"),
            w: f("watts"),
            tested_at: u("tested_at"),
            verified_at: u("verified_at"),
            disabled_until: u("disabled_until"),
            stepped_back: u("stepped_back_in_mining") as u32,
        })
    }
}

/// uv.json, shared with the Pearl tuner and the CMFD one (other sections are kept as they are).
struct Db {
    dir: PathBuf,
    v: Mutex<Value>,
}

impl Db {
    fn load(dir: &Path) -> Db {
        let v = std::fs::read_to_string(uv::results_path(dir))
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({"version": 1, "gpus": {}}));
        Db { dir: dir.to_path_buf(), v: Mutex::new(v) }
    }
    fn write(&self, v: &Value) {
        if let Err(e) = uv::write_atomic(&uv::results_path(&self.dir), &serde_json::to_string_pretty(v).unwrap()) {
            lg(format!("cannot save {}: {e:#}", uv::results_path(&self.dir).display()));
        }
    }
    fn entry(&self, uuid: &str, mode: &str) -> Option<Entry> {
        self.v.lock().unwrap()[SECTION].get(uuid).and_then(|c| c.get(mode)).and_then(Entry::from_json)
    }
    fn save(&self, uuid: &str, mode: &str, e: &Entry) {
        let mut v = self.v.lock().unwrap();
        if !v[SECTION].is_object() {
            v[SECTION] = json!({});
        }
        if !v[SECTION][uuid].is_object() {
            v[SECTION][uuid] = json!({});
        }
        v[SECTION][uuid][mode] = e.to_json();
        self.write(&v);
    }
    fn mark_trying(&self, uuid: &str, at: i32) {
        let mut v = self.v.lock().unwrap();
        if !v[TRYING].is_object() {
            v[TRYING] = json!({});
        }
        v[TRYING][uuid] = json!({"offset": at});
        self.write(&v);
    }
    fn clear_trying(&self, uuid: &str) {
        let mut v = self.v.lock().unwrap();
        if v[TRYING].as_object_mut().is_some_and(|t| t.remove(uuid).is_some()) {
            self.write(&v);
        }
    }
    fn take_trying(&self, uuid: &str) -> Option<i32> {
        let t = self.v.lock().unwrap()[TRYING].get(uuid).cloned()?;
        self.clear_trying(uuid);
        Some(t["offset"].as_i64()? as i32)
    }
}

/// Cards that mine at an offset above their base: stepped back when a hit fails the CPU check.
struct LiveCard {
    pos: usize,
    #[allow(dead_code)]
    bus: String,
    uuid: String,
    base: i32,
    step: i32,
    tuned: bool,
    last: Option<Instant>,
}

pub struct Live {
    db: Option<Db>,
    mode: &'static str,
    cards: Mutex<Vec<LiveCard>>,
}

struct Card<'a> {
    r: &'a CardRef,
    nv: Dev,
    uuid: String,
    orig: i32,
    base: i32,
    hi: i32,
    max: i32,
    max_src: &'static str,
    start: i32,
    kernel: String,
    pl: f64,
}

impl Card<'_> {
    fn label(&self) -> String {
        format!("GPU{} (bus {}, {})", self.r.dev, short_bus(&self.r.bus), short_name(&self.r.name))
    }
}

#[derive(Default, Debug)]
struct Res {
    ok: bool,
    reason: String,
    fault: bool,
    infra: bool, // the check could not run at all (no executable / process start): says nothing about the GPU
    launches: u64,
    mhs: f64,
    watts: Option<f64>,
    clk: Option<u32>,
    temp: Option<u32>,
}

impl Res {
    fn brief(&self) -> String {
        let eff = self.watts.filter(|&w| w > 0.0 && self.mhs > 0.0).map_or(String::new(), |w| format!(", {:.3} MH/W", self.mhs / w));
        format!("{} launches checked, {:.2} MH/s, {}{eff}, {} MHz, {} C", self.launches, self.mhs, fmt_w(self.watts), self.clk.unwrap_or(0), self.temp.unwrap_or(0))
    }
}

struct Tuner<'a> {
    nv: &'a Nvml,
    o: &'a uv::Opts,
    mode: i32,
    db: &'a Db,
    driver: String,
    stop: &'a Arc<AtomicBool>,
}

impl Tuner<'_> {
    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    fn step_secs(&self) -> u32 {
        if self.o.step_secs == uv::Opts::default().step_secs { STEP_SECS } else { self.o.step_secs }
    }

    /// One stress check at offset `at` (set first) for `secs` seconds.
    fn check(&self, c: &Card, at: i32, secs: u32) -> Res {
        let mut res = Res::default();
        if let Err(e) = offset::set(&c.r.bus, at) {
            res.reason = format!("cannot set offset {}: {e:#}", signed(at));
            return res;
        }
        if at != c.base {
            self.db.mark_trying(&c.uuid, at);
        }
        let _ = self.nv.take_xids(c.nv);
        // Linux: /proc/self/exe still starts this very binary when the file was replaced or deleted meanwhile (an update)
        let proc_exe = std::path::Path::new("/proc/self/exe");
        let exe = if cfg!(target_os = "linux") && proc_exe.exists() {
            proc_exe.to_path_buf()
        } else {
            match std::env::current_exe() {
                Ok(e) => e,
                Err(e) => {
                    res.reason = format!("no executable path ({e})");
                    res.infra = true;
                    self.db.clear_trying(&c.uuid);
                    return res;
                }
            }
        };
        let child = Command::new(exe)
            .args(["--noid-uv-check", "--devices", &c.r.dev.to_string(), "--noid-mode", super::mode_name(self.mode), "--uv-secs", &secs.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        let mut child = match child {
            Ok(ch) => ch,
            Err(e) => {
                res.reason = format!("cannot start the check process ({e})");
                res.infra = true;
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
        let mut samples: Vec<(f64, u32, u32)> = Vec::new();
        let mut last_sample = Instant::now();
        let mut result: Option<String> = None;
        let mut tail: Vec<String> = Vec::new();
        let limit = Duration::from_secs(secs as u64 + 180); // + CPU reference, CUDA start
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) => {}
                Err(_) => break,
            }
            while let Ok(l) = rx.try_recv() {
                if l.starts_with("NOID_CHECK") {
                    result = Some(l);
                } else {
                    tail.push(l);
                    if tail.len() > 4 {
                        tail.remove(0);
                    }
                }
            }
            if self.stopped() {
                let _ = child.kill();
                res.reason = "interrupted".into();
                res.infra = true;
                let _ = child.wait();
                self.db.clear_trying(&c.uuid);
                return res;
            }
            if let Some(x) = self.nv.take_xids(c.nv).first() {
                let _ = child.kill();
                let _ = child.wait();
                res.reason = format!("Xid {x} reported by the driver");
                res.fault = true;
                return res;
            }
            if t0.elapsed() > limit {
                let _ = child.kill();
                let _ = child.wait();
                res.reason = "timeout (GPU hang?)".into();
                res.fault = true;
                return res;
            }
            // power after the CPU reference and warm-up: the last two thirds of the run
            if t0.elapsed() > Duration::from_secs(8 + secs as u64 / 3) && last_sample.elapsed() >= Duration::from_millis(500) {
                last_sample = Instant::now();
                if let Some(p) = self.nv.power_w(c.nv) {
                    samples.push((p, self.nv.clock_mhz(c.nv).unwrap_or(0), self.nv.temp_c(c.nv).unwrap_or(0)));
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        while let Ok(l) = rx.recv_timeout(Duration::from_millis(500)) {
            if l.starts_with("NOID_CHECK") {
                result = Some(l);
            } else {
                tail.push(l);
            }
        }
        if let Some(x) = self.nv.take_xids(c.nv).first() {
            res.reason = format!("Xid {x} reported by the driver");
            res.fault = true;
            return res;
        }
        let Some(line) = result else {
            res.reason = format!("the check process ended without a result{}", if tail.is_empty() { String::new() } else { format!(": {}", tail.join(" | ")) });
            res.fault = true;
            return res;
        };
        let kv = |k: &str| line.split_whitespace().find_map(|w| w.strip_prefix(&format!("{k}="))).map(str::to_string);
        res.launches = kv("launches").and_then(|v| v.parse().ok()).unwrap_or(0);
        res.mhs = kv("mhs").and_then(|v| v.parse().ok()).unwrap_or(0.0);
        if samples.len() >= 3 {
            res.watts = Some(samples.iter().map(|s| s.0).sum::<f64>() / samples.len() as f64);
            let mut clks: Vec<u32> = samples.iter().map(|s| s.1).collect();
            clks.sort_unstable();
            res.clk = Some(clks[clks.len() / 2]);
            res.temp = samples.iter().map(|s| s.2).max();
        }
        if line.starts_with("NOID_CHECK ok") {
            res.ok = true;
        } else if line.starts_with("NOID_CHECK FAIL") {
            res.reason = format!(
                "WRONG RESULTS: {} of {} launches gave a wrong candidate set{} (no Xid, no crash: the silent edge)",
                kv("bad").unwrap_or_default(),
                res.launches,
                kv("first_bad_at").map_or(String::new(), |t| format!(", first after {t}"))
            );
        } else {
            res.reason = line.trim_start_matches("NOID_CHECK ").to_string();
            res.fault = true;
        }
        self.db.clear_trying(&c.uuid);
        res
    }

    fn new_entry(&self, c: &Card) -> Entry {
        Entry {
            name: c.r.name.clone(),
            bus: c.r.bus.clone(),
            driver: self.driver.clone(),
            kernel: c.kernel.clone(),
            core_lock: c.r.core_lock.unwrap_or(0),
            pl: c.pl,
            orig: c.orig,
            base: c.base,
            step: self.o.step,
            ..Default::default()
        }
    }

    /// Offset to mine at for this card.
    fn run_card(&self, c: &Card) -> i32 {
        let mode = super::mode_name(self.mode);
        if let Some(at) = self.db.take_trying(&c.uuid) {
            lg(format!("{} the previous run died while checking NOID offset {} (crash or kill): tuning again below it", c.label(), signed(at)));
            return self.tune(c, Some(at));
        }
        if self.o.reset {
            return self.tune(c, None);
        }
        let Some(e) = self.db.entry(&c.uuid, mode) else { return self.tune(c, None) };
        let why = if e.driver != self.driver {
            Some(format!("driver {} when tuned, {} now", e.driver, self.driver))
        } else if e.kernel != c.kernel {
            Some(format!("kernel {} when tuned, {} now", e.kernel, c.kernel))
        } else if e.core_lock != c.r.core_lock.unwrap_or(0) {
            Some(format!("core clock lock {} MHz when tuned, {} now", e.core_lock, c.r.core_lock.unwrap_or(0)))
        } else if e.pl > 0.0 && c.pl > 0.0 && (c.pl - e.pl).abs() > 0.03 * e.pl {
            Some(format!("power limit {:.0} W when tuned, {:.0} W now", e.pl, c.pl))
        } else if e.base != c.base {
            Some(format!("base offset {} when tuned, {} now", signed(e.base), signed(c.base)))
        } else {
            None
        };
        if let Some(w) = why {
            lg(format!("{} saved NOID result does not fit this run ({w}; the edge moves with it): tuning again", c.label()));
            return self.tune(c, None);
        }
        if e.disabled_until > now() {
            lg(format!("{} NOID auto-uv is paused on this GPU after a failure ({} h left); mining at {}", c.label(), (e.disabled_until - now()) / 3600 + 1, signed(c.base)));
            return c.base;
        }
        if e.offset <= c.base {
            lg(format!("{} no safe NOID offset above {} was found when tuned; mining at it (--uv-reset to tune again)", c.label(), signed(c.base)));
            return c.base;
        }
        let at = match saved_vs_limit(e.offset, e.cap, c.max) {
            Saved::Apply => e.offset,
            Saved::Clamp => {
                lg(format!("{} saved NOID offset {} is above the limit {} ({}): mining at the limit", c.label(), signed(e.offset), signed(c.max), c.max_src));
                c.max
            }
            Saved::Retune => {
                lg(format!("{} the limit was raised to {} ({}): tuning again above {}", c.label(), signed(c.max), c.max_src, signed(e.offset)));
                return self.tune(c, None);
            }
        };
        let age = now().saturating_sub(e.verified_at);
        if (age as f64) < self.o.reval_hours * 3600.0 && self.o.reval_hours > 0.0 {
            lg(format!("{} NOID offset {} (tuned, verified {} ago): applied", c.label(), signed(at), fmt_age(age)));
            let _ = offset::set(&c.r.bus, at);
            return at;
        }
        lg(format!("{} saved NOID offset {}: quick check ({QUICK_SECS} s)", c.label(), signed(at)));
        let r = self.check(c, at, QUICK_SECS);
        if r.infra {
            return self.abort(c, &r);
        }
        if r.ok {
            lg(format!("{} {}: ok ({}); mining at it", c.label(), signed(at), r.brief()));
            let mut e2 = e.clone();
            e2.verified_at = now();
            self.db.save(&c.uuid, mode, &e2);
            return at;
        }
        lg(format!("{} {}: FAILED ({}): tuning again", c.label(), signed(at), r.reason));
        self.tune(c, Some(at))
    }

    /// The check itself could not run (not a GPU failure): nothing is saved, the card mines at its base offset.
    fn abort(&self, c: &Card, r: &Res) -> i32 {
        lg(format!("{} the NOID check could not run ({}): auto-uv skipped for this run, mining at {}", c.label(), r.reason, signed(c.base)));
        let _ = offset::set(&c.r.bus, c.base);
        c.base
    }

    /// Full tune; `below`: an offset known to fail (search stays under it).
    fn tune(&self, c: &Card, below: Option<i32>) -> i32 {
        let mode = super::mode_name(self.mode);
        let step = self.o.step.max(1);
        let mut max = c.max;
        if let Some(b) = below {
            max = max.min(b - step);
        }
        lg(format!(
            "{} tuning the NOID offset ({mode} kernel {}): base {}, start {}, limit {} ({}), step {step}, {} s of stress per step",
            c.label(),
            c.kernel,
            signed(c.base),
            signed(c.start.min(max)),
            signed(max),
            c.max_src,
            self.step_secs()
        ));
        let mut e = self.new_entry(c);
        let r = self.check(c, c.base, BASE_SECS);
        if r.infra {
            return self.abort(c, &r);
        }
        if !r.ok {
            lg(format!("{} base {}: FAILED ({}): NOID auto-uv off on this GPU for 24 h; mining at the base", c.label(), signed(c.base), r.reason));
            e.offset = c.base;
            e.disabled_until = now() + DISABLE_SECS;
            e.tested_at = now();
            self.db.save(&c.uuid, mode, &e);
            let _ = offset::set(&c.r.bus, c.base);
            return c.base;
        }
        lg(format!("{} base {}: ok ({})", c.label(), signed(c.base), r.brief()));
        (e.mhs_base, e.w_base) = (r.mhs, r.watts.unwrap_or(0.0));
        let mut last_ok = c.base;
        let mut good = (r.mhs, r.watts.unwrap_or(0.0));
        let mut edge = 0;
        if max < c.base + step {
            lg(format!("{} the limit {} leaves no step above the base {}: mining at the base", c.label(), signed(max), signed(c.base)));
            e.offset = c.base;
            e.cap = max;
            (e.mhs, e.w) = good;
            e.tested_at = now();
            e.verified_at = now();
            self.db.save(&c.uuid, super::mode_name(self.mode), &e);
            let _ = offset::set(&c.r.bus, c.base);
            return c.base;
        }
        let mut o = c.start.min(max).max(c.base + step);
        // the start step itself must hold; else one step lower until one does (or the base)
        while o > c.base && !self.stopped() {
            let r = self.check(c, o, self.step_secs());
            if r.infra {
                return self.abort(c, &r);
            }
            if r.ok {
                lg(format!("{} {}: ok ({})", c.label(), signed(o), r.brief()));
                last_ok = o;
                good = (r.mhs, r.watts.unwrap_or(0.0));
                break;
            }
            lg(format!("{} {}: FAILED ({})", c.label(), signed(o), r.reason));
            edge = o;
            o -= step;
        }
        let mut plateau = false;
        if last_ok > c.base && edge == 0 {
            // upwards to the first failure, or to the voltage floor: two steps in a row that save < 0.5 W (agent M's rule:
            // below ~1400 MHz an RTX 3070 already sits at its minimum voltage, a higher offset only adds risk)
            let mut o = last_ok + step;
            let mut flat = 0;
            let mut plateau_at = None;
            let mut hist: Vec<(i32, f64, f64)> = vec![(last_ok, good.0, good.1)];
            while o <= max && !self.stopped() {
                let r = self.check(c, o, self.step_secs());
                if r.infra {
                    return self.abort(c, &r);
                }
                if !r.ok {
                    lg(format!("{} {}: FAILED ({}): edge found", c.label(), signed(o), r.reason));
                    edge = o;
                    break;
                }
                lg(format!("{} {}: ok ({})", c.label(), signed(o), r.brief()));
                let w = r.watts.unwrap_or(0.0);
                let prev_w = hist.last().map_or(0.0, |h| h.2);
                flat = if w > 0.0 && prev_w > 0.0 && prev_w - w < 0.5 { flat + 1 } else { 0 };
                last_ok = o;
                good = (r.mhs, w);
                hist.push((o, r.mhs, w));
                if flat >= 2 {
                    plateau_at = Some(hist[hist.len() - 3]);
                    break;
                }
                o += step;
            }
            if let Some((p, m, w)) = plateau_at {
                lg(format!(
                    "{} power no longer drops above {} (voltage floor: {} and {} saved < 0.5 W each): stopping here, no edge searched beyond",
                    c.label(),
                    signed(p),
                    signed(p + step),
                    signed(p + 2 * step)
                ));
                last_ok = p;
                good = (m, w);
                plateau = true;
            }
        }
        if self.stopped() {
            let _ = offset::set(&c.r.bus, c.base);
            return c.base;
        }
        let mut res = if edge != 0 && !self.o.keep_last_good { (last_ok - step).max(c.base) } else { last_ok };
        if edge == 0 && last_ok > c.base && !plateau {
            lg(format!("{} no failure up to the limit {}: the result is the limit (raise --uv-max to search further)", c.label(), signed(max)));
        }
        // confirmation: a long check at the result; one step lower on failure
        while res > c.base && !self.stopped() {
            let r = self.check(c, res, self.o.confirm_secs.max(self.step_secs()));
            if r.infra {
                return self.abort(c, &r);
            }
            if r.ok {
                lg(format!("{} confirmation {}: ok ({})", c.label(), signed(res), r.brief()));
                good = (r.mhs, r.watts.unwrap_or(0.0));
                break;
            }
            lg(format!("{} confirmation {}: FAILED ({}): one step lower", c.label(), signed(res), r.reason));
            res -= step;
        }
        if res <= c.base {
            res = c.base;
            good = (e.mhs_base, e.w_base);
        }
        e.offset = res;
        e.cap = max;
        e.edge = edge;
        (e.mhs, e.w) = good;
        e.tested_at = now();
        e.verified_at = now();
        self.db.save(&c.uuid, mode, &e);
        let _ = offset::set(&c.r.bus, res);
        let gain = if e.w_base > 0.0 && e.w > 0.0 && e.mhs_base > 0.0 { format!("; MH/W {:.3} -> {:.3} ({:+.1}%)", e.mhs_base / e.w_base, e.mhs / e.w, 100.0 * ((e.mhs / e.w) / (e.mhs_base / e.w_base) - 1.0)) } else { String::new() };
        lg(format!(
            "{} RESULT {mode}: offset {} (edge {}, {}){gain}; saved to {}",
            c.label(),
            signed(res),
            if edge != 0 { signed(edge) } else { "not reached".into() },
            if plateau { "start of the voltage floor" } else if self.o.keep_last_good || edge == 0 { "last good step" } else { "one step below the last good one" },
            uv::results_path(&self.db.dir).display()
        ));
        res
    }
}

/// Before mining: sets every card's core offset for NOID and returns the cards to watch while mining.
///  * --uv-offset N: N (at the user's risk);  * --auto-uv: the NOID-tuned value (tuning now if needed, all cards in parallel);
///  * otherwise: a positive owner's offset is set to 0 (NOID computes wrong results with Pearl undervolts), others are kept.
pub fn prepare(nv: Option<&'static Nvml>, cards: &[CardRef], o: &uv::Opts, mode: i32, dir: &Path, stop: &Arc<AtomicBool>) -> Result<Arc<Live>> {
    let live = Arc::new(Live { db: None, mode: super::mode_name(mode), cards: Mutex::new(Vec::new()) });
    let Some(nv) = nv else {
        if o.auto || o.fixed.is_set() {
            lg("NVML not available: --auto-uv / --uv-offset skipped, offsets left as they are");
        }
        return Ok(live);
    };
    if o.reset {
        let db = Db::load(dir);
        let mut v = db.v.lock().unwrap();
        if v.as_object_mut().is_some_and(|m| m.remove(SECTION).is_some()) {
            db.write(&v);
            lg("saved NOID results forgotten (--uv-reset)");
        }
    }
    let mut list = Vec::new();
    for r in cards {
        let Some((orig, lo, hi)) = offset::info(&r.bus) else { continue };
        let Ok(d) = nv.by_bus(&r.bus) else { continue };
        let base = if orig > 0 { 0 } else { orig };
        let def = uv::card_defaults(&r.name, nv.compute_capability(d), nv.pci_device_id(d));
        let (max, max_src) = match o.max_offset.get(r.pos) { Some(m) => (m, "--uv-max"), None => (def.max, def.max_src) };
        let max = max.min(HARD_CAP).min(hi);
        let start = o.min_offset.get(r.pos).unwrap_or(START).max(lo);
        list.push(Card {
            r,
            nv: d,
            uuid: nv.uuid(d).unwrap_or_else(|_| r.bus.clone()),
            orig,
            base,
            hi,
            max,
            max_src,
            start,
            kernel: gpu::kernel_label_for(r.dev, mode),
            pl: nv.power_limit_w(d).unwrap_or(0.0),
        });
    }
    let mut live_cards = Vec::new();
    if o.fixed.is_set() {
        for c in &list {
            let Some(v) = o.fixed.get(c.r.pos) else { continue };
            let v = v.min(c.hi).min(HARD_CAP);
            match offset::set(&c.r.bus, v) {
                Ok(()) => lg(format!("{} core offset {} -> {} (--uv-offset; not checked for NOID: at your own risk)", c.label(), signed(c.orig), signed(v))),
                Err(e) => lg(format!("{} cannot set offset {}: {e:#}", c.label(), signed(v))),
            }
            live_cards.push(LiveCard { pos: c.r.pos, bus: c.r.bus.clone(), uuid: c.uuid.clone(), base: c.base, step: o.step, tuned: false, last: None });
        }
    } else if o.auto && o.dry_run {
        for c in &list {
            lg(format!("{} dry run: would tune the NOID offset from {} (base {}, start {}, limit {} {}); offset now {}", c.label(), signed(c.base), signed(c.base), signed(c.start), signed(c.max), c.max_src, signed(c.orig)));
        }
        set_base(&list);
    } else if o.auto {
        let db = Db::load(dir);
        for c in &list {
            nv.watch_xid(c.nv);
        }
        let t = Tuner { nv, o, mode, db: &db, driver: nv.driver_version(), stop };
        let res: Vec<i32> = std::thread::scope(|s| {
            let hs: Vec<_> = list.iter().map(|c| s.spawn(|| t.run_card(c))).collect();
            hs.into_iter().zip(&list).map(|(h, c)| h.join().unwrap_or(c.base)).collect()
        });
        for (c, &v) in list.iter().zip(&res) {
            if v > c.base {
                live_cards.push(LiveCard { pos: c.r.pos, bus: c.r.bus.clone(), uuid: c.uuid.clone(), base: c.base, step: o.step, tuned: true, last: None });
            }
        }
        return Ok(Arc::new(Live { db: Some(db), mode: super::mode_name(mode), cards: Mutex::new(live_cards) }));
    } else {
        set_base(&list);
    }
    *live.cards.lock().unwrap() = live_cards;
    Ok(live)
}

fn set_base(list: &[Card]) {
    for c in list {
        if c.orig > 0 {
            match offset::set(&c.r.bus, 0) {
                Ok(()) => println!("noid offset: {}", format!(
                    "{} core offset {} -> 0 for NOID (Pearl undervolt offsets give silently wrong NOID results; --auto-uv finds a NOID-safe one); put back on exit",
                    c.label(),
                    signed(c.orig)
                )),
                Err(e) => println!("noid offset: {}", format!("{} cannot set the offset to 0 ({e:#}); run as root/Administrator, else NOID may compute wrong results", c.label())),
            }
        }
    }
}

/// A hit of card `pos` failed the CPU check: one step back (saved for a tuned card), at most once per 20 s.
pub fn on_bad_hit(live: &Live, pos: usize, bus: &str, dev: i32) {
    let mut g = live.cards.lock().unwrap();
    let Some(c) = g.iter_mut().find(|c| c.pos == pos) else {
        eprintln!("GPU{dev}: wrong GPU results (CPU check of a hit / watchdog) at the card's own offset (no undervolt by this miner): check clocks/temperature");
        return;
    };
    if c.last.is_some_and(|t| t.elapsed() < Duration::from_secs(20)) {
        return;
    }
    c.last = Some(Instant::now());
    let cur = offset::current(bus).unwrap_or(c.base);
    if cur <= c.base {
        eprintln!("GPU{dev}: wrong GPU results (CPU check of a hit / watchdog) at the base offset {}: check clocks/temperature", signed(cur));
        return;
    }
    let new = (cur - c.step).max(c.base);
    match offset::set(bus, new) {
        Ok(()) => lg(format!("GPU{dev}: wrong GPU results at offset {} (CPU check of a hit / watchdog): stepped back to {} and mining on", signed(cur), signed(new))),
        Err(e) => lg(format!("GPU{dev}: cannot step the offset back ({e:#})")),
    }
    if c.tuned {
        if let Some(db) = &live.db {
            if let Some(mut e) = db.entry(&c.uuid, live.mode) {
                e.offset = new;
                e.stepped_back += 1;
                db.save(&c.uuid, live.mode, &e);
            }
        }
    }
}
