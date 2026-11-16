// FreeForGem — see LICENSE
//! PROX undervolt auto-tuner (`--auto-uv`, 1.7.8; off unless asked, as for Pearl).  Ported from
//! our ffgprox host/uv.h onto FreeForGem's shared ladder (climb.rs), profile store (profiles.rs, uv.json section "prox") and offset
//! keeper (noid/offset.rs: the owner's offset is recorded in a marker before the first change and put back on exit, on a signal
//! and by the next start after a crash).
//!
//! A higher core V/F offset at the same clock = a lower voltage = less power at the same hash rate.  The BLAKE2b kernel is pure
//! integer ALU and fails SILENTLY at the edge, so every step is checked in a child process (`freeforgem --prox-uv-check`) that runs
//! the mining kernel at full load on 8 fixed jobs with a LOW k (k=12: ~1000 candidates per 4M-nonce launch):
//!   * the kernel's answer (candidate count + nonce sum + xor) of each fixed (job, range) must repeat exactly in every launch and
//!     equal the reference taken at the base offset (the reference is saved with the profile): one flipped hash changes it;
//!   * every candidate of the reference launches and a random sample of the others is re-hashed on the CPU (blake2b.rs);
//!   * a CUDA fault, a hang or an Xid only kills the child (and counts as a fault: no refining inside that interval).
//! Ladder: climb.rs (coarse 2 x step, the failing coarse interval refined in fine steps, voltage floor), result = last good step
//! minus one step (--uv-keep-last: the last good step), confirmed by a longer check.  Profile per GPU (UUID) + driver + kernel/
//! miner version + core lock.  At the next start a saved profile is re-applied after a quick check; while mining a candidate that
//! fails the CPU check steps the GPU back one step (saved) and the new offset is re-checked a minute later; every --uv-reval-hours
//! (6) a short re-check pauses that GPU for ~20 s.  Someone else changing the offset makes the tuner leave that GPU alone.
//!
//! Without a core lock (no --lock-cclock, no HiveOS OC lock) the Pearl convention applies: the profile is keyed by the clock under
//! load measured at the base check ("~1905"); it holds only while the card boosts the same way, so a lock is recommended.
use super::{blake2b, gpu};
use crate::climb;
use crate::noid::offset;
use crate::nvml::{Dev, Nvml};
use crate::profiles::{self, Profile, Store};
use crate::uv::{self, signed};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const JOBS: usize = 8;
const CHECK_K: u32 = 12;
const CHECK_COUNT: u64 = 1 << 22;
const DEF_MAX: i32 = 495;
const DISABLE_SECS: u64 = 24 * 3600;
/// PROX check lengths (the defaults of uv::Opts are Pearl's 180 / 30 / 120 s; ffgprox: 30 / 15 / 60, quick 15)
const WARMUP: u32 = 30;
const STEP_SECS: u32 = 15;
const CONFIRM: u32 = 60;
const QUICK: u32 = 15;

fn lg(m: impl AsRef<str>) {
    println!("auto-uv: {}", m.as_ref());
}

fn kernel_id() -> String {
    format!("ffg{}/a", env!("CARGO_PKG_VERSION"))
}

// ---------------------------------------------------------------------------------------------------------------------
// the check (child process)
// ---------------------------------------------------------------------------------------------------------------------

fn check_jobs() -> Vec<(Vec<u8>, u64)> {
    const LENS: [usize; JOBS] = [369, 600, 372, 450, 369, 521, 600, 410];
    (0..JOBS)
        .map(|j| {
            let mut x = 0x7f4a_7c15u32.wrapping_mul(j as u32 + 1);
            let pre: Vec<u8> = (0..LENS[j])
                .map(|_| {
                    x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    (x >> 24) as u8
                })
                .collect();
            // bits 40-47 zero (the mining layout: the kernel's m[15] path), top 16 bits like an extranonce1
            (pre, 0xa5a5_0000_0000_0000u64 | ((j as u64) << 28))
        })
        .collect()
}

/// `freeforgem --prox-uv-check --devices D --uv-secs S [--uv-ref SIG,..]`: prints `PROX_LOAD`, then
/// `PROX_CHECK ok launches N ghs G counts SIG,.. cpu K` or `PROX_CHECK FAIL reason`; exit code 0 = ok.
pub fn check_child(dev: i32, secs: u32, refs: &str) -> i32 {
    let parent = parent_id();
    std::thread::spawn(move || {
        let t = Instant::now();
        loop {
            std::thread::sleep(Duration::from_millis(500));
            if parent.is_some() && parent_id() != parent {
                std::process::exit(6); // the tuner died: do not keep loading its GPU
            }
            if t.elapsed() > Duration::from_secs(secs as u64 + 60) {
                println!("PROX_CHECK FAIL the check hung (no result after {} s)", secs + 60);
                std::process::exit(5);
            }
        }
    });
    let fail = |why: String| {
        println!("PROX_CHECK FAIL {why}");
        1
    };
    if let Err(e) = gpu::load() {
        return fail(format!("{e:#}"));
    }
    let jobs = check_jobs();
    let pj: Vec<gpu::ProxJob> = match jobs.iter().map(|(m, _)| gpu::job_build(m, CHECK_K, None)).collect() {
        Ok(v) => v,
        Err(e) => return fail(format!("{e:#}")),
    };
    let given = !refs.is_empty();
    let mut refv: Vec<String> = refs.split(',').map(String::from).collect();
    refv.resize(JOBS, String::new());
    let mut buf = vec![0u64; 1 << 14];
    let mut out = Vec::new();
    let mut rng = 0x9e37_79b9_7f4a_7c15u64 ^ uv::now();
    let expv = CHECK_COUNT as f64 / (1u64 << CHECK_K) as f64;
    let (mut launches, mut cpu) = (0u64, 0u64);
    println!("PROX_LOAD");
    let t0 = Instant::now();
    let mut n = 0usize;
    loop {
        let el = t0.elapsed().as_secs_f64();
        if el >= secs as f64 && launches >= 2 * JOBS as u64 {
            break;
        }
        if el >= secs as f64 + 30.0 {
            return fail(format!("too few launches ({launches})"));
        }
        let j = n % JOBS;
        n += 1;
        out.clear();
        if let Err(e) = gpu::search(dev, &pj[j], jobs[j].1, CHECK_COUNT, &mut buf, &mut out) {
            return fail(format!("CUDA: {e:#}"));
        }
        launches += 1;
        let (sum, xr) = out.iter().fold((0u64, 0u64), |(s, x), &v| (s.wrapping_add(v), x ^ v));
        let sig = format!("{:x}:{sum:x}:{xr:x}", out.len());
        if refv[j].is_empty() {
            refv[j] = sig;
        } else if refv[j] != sig {
            return fail(format!("the kernel's answer changed (job {j}, launch {launches}): {sig}, expected {}: the GPU computed wrong", refv[j]));
        }
        if (out.len() as f64 - expv).abs() > 6.0 * expv.sqrt() + 2.0 {
            return fail(format!("implausible candidate count {} (expected ~{expv:.0})", out.len()));
        }
        let full = !given && launches <= JOBS as u64;
        let m = if full { out.len() } else { out.len().min(24) };
        for i in 0..m {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            let v = if full { out[i] } else { out[(rng % out.len().max(1) as u64) as usize] };
            if blake2b::work_bits(&blake2b::hash_nonce(&jobs[j].0, v)) < CHECK_K {
                return fail(format!("a candidate failed the CPU re-hash (job {j})"));
            }
            cpu += 1;
        }
    }
    let dt = t0.elapsed().as_secs_f64();
    println!(
        "PROX_CHECK ok launches {launches} ghs {:.3} counts {} cpu {cpu}{}",
        launches as f64 * CHECK_COUNT as f64 / dt / 1e9,
        refv.join(","),
        if given { "" } else { " (self-reference)" }
    );
    0
}

#[cfg(unix)]
fn parent_id() -> Option<u32> {
    Some(std::os::unix::process::parent_id())
}
#[cfg(not(unix))]
fn parent_id() -> Option<u32> {
    None
}

// ---------------------------------------------------------------------------------------------------------------------
// profiles (uv.json section "prox")
// ---------------------------------------------------------------------------------------------------------------------

#[derive(Clone, Default, Debug)]
struct Entry {
    name: String,
    bus: String,
    driver: String,
    kernel: String,
    core_lock: u32,
    clk: u32,
    orig: i32,
    base: i32,
    offset: i32,
    step: i32,
    edge: i32,
    w_base: f64,
    w: f64,
    ghs: f64,
    refs: String,
    tested_at: u64,
    verified_at: u64,
    disabled_until: u64,
    stepped_back: u32,
}

impl Profile for Entry {
    fn from_json(v: &Value) -> Option<Entry> {
        let u = |k: &str| v[k].as_u64().unwrap_or(0);
        let i = |k: &str| v[k].as_i64().unwrap_or(0) as i32;
        let f = |k: &str| v[k].as_f64().unwrap_or(0.0);
        let s = |k: &str| v[k].as_str().unwrap_or("").to_string();
        Some(Entry {
            name: s("name"),
            bus: s("bus"),
            driver: s("driver"),
            kernel: v["kernel"].as_str()?.into(),
            core_lock: u("core_lock") as u32,
            clk: u("clk_mhz") as u32,
            orig: i("orig_offset"),
            base: v["base_offset"].as_i64()? as i32,
            offset: v["offset"].as_i64()? as i32,
            step: i("step"),
            edge: i("edge"),
            w_base: f("watts_base"),
            w: f("watts"),
            ghs: f("ghs"),
            refs: s("refs"),
            tested_at: u("tested_at"),
            verified_at: u("verified_at"),
            disabled_until: u("disabled_until"),
            stepped_back: u("stepped_back_in_mining") as u32,
        })
    }
    fn to_json(&self) -> Value {
        json!({"name": self.name, "bus": self.bus, "driver": self.driver, "kernel": self.kernel, "core_lock": self.core_lock, "clk_mhz": self.clk,
               "orig_offset": self.orig, "base_offset": self.base, "offset": self.offset, "step": self.step, "edge": self.edge,
               "watts_base": self.w_base, "watts": self.w, "ghs": self.ghs, "refs": self.refs, "tested_at": self.tested_at,
               "verified_at": self.verified_at, "disabled_until": self.disabled_until, "stepped_back_in_mining": self.stepped_back})
    }
    fn same_clocks(&self, o: &Entry) -> bool {
        self.driver == o.driver
            && match (self.core_lock, o.core_lock) {
                (0, 0) => (self.clk as i64 - o.clk as i64).abs() <= 45,
                (a, b) => a == b,
            }
    }
}

/// "1500" (the core lock) or "~1905" (no lock: the clock under load at the base check)
fn pkey(e: &Entry) -> String {
    if e.core_lock != 0 { format!("{}", e.core_lock) } else { format!("~{}", (e.clk + 7) / 15 * 15) }
}

// ---------------------------------------------------------------------------------------------------------------------
// one GPU
// ---------------------------------------------------------------------------------------------------------------------

pub struct CardRef {
    pub dev: i32,
    pub bus: String,
    pub name: String,
    pub core_lock: Option<u32>,
    pub pause: Arc<AtomicBool>,
    pub paused: Arc<AtomicBool>,
    pub bad: Arc<AtomicU64>,
}

struct Card {
    r: CardRef,
    uuid: String,
    nv: Dev,
    orig: i32,
    lo: i32,
    cap: i32,
    base: AtomicI32,  // the checked start offset: step-backs stop here
    cur: AtomicI32,
    key: Mutex<String>,
    refs: Mutex<String>,
    owned: AtomicBool, // the tuner watches this GPU (offset above its base)
    busy: AtomicBool,
    bad_seen: AtomicU64,
    next_reval: AtomicU64,
    next_readback: AtomicU64,
    resets: crate::hiveoc::ResetLimit, // OC applies (HiveOS nvidia-oc) answered with a re-apply, per hour
}

impl Card {
    fn label(&self) -> String {
        format!("GPU{} (bus {}, {}{})", self.r.dev, uv::short_bus(&self.r.bus), self.r.name.replace("NVIDIA GeForce ", ""), self.r.core_lock.map_or(String::new(), |l| format!(", lock {l}")))
    }
}

#[derive(Default)]
struct Res {
    ok: bool,
    fault: bool, // driver fault / hang: no refining
    infra: bool, // not a GPU verdict (stop, no executable)
    reason: String,
    refs: String,
    launches: u64,
    ghs: f64,
    w: Option<f64>,
    clk: Option<u32>,
    t: Option<i32>,
}

pub struct Params {
    pub step: i32,
    pub coarse: i32,
    pub keep_last: bool,
    pub reval_h: f64,
    pub warmup: u32,
    pub step_secs: u32,
    pub confirm: u32,
    pub max: uv::PerGpu,
    pub min: uv::PerGpu,
    pub reset: bool,
}

impl Params {
    /// From the shared --uv-* options; check lengths left at the Pearl defaults get the PROX ones.
    pub fn from(o: &uv::Opts) -> Params {
        let d = uv::Opts::default();
        let pick = |v: u32, pearl: u32, prox: u32| if v == pearl { prox } else { v };
        Params {
            step: o.step,
            coarse: o.coarse_step(),
            keep_last: o.keep_last_good,
            reval_h: o.reval_hours,
            warmup: pick(o.warmup_secs, d.warmup_secs, WARMUP),
            step_secs: pick(o.step_secs, d.step_secs, STEP_SECS),
            confirm: pick(o.confirm_secs, d.confirm_secs, CONFIRM),
            max: o.max_offset.clone(),
            min: o.min_offset.clone(),
            reset: o.reset,
        }
    }
}

pub struct Live {
    nv: &'static Nvml,
    db: Store,
    p: Params,
    driver: String,
    cards: Vec<Arc<Card>>,
    stop: Arc<AtomicBool>,
}

impl Live {
    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    fn check(&self, c: &Card, at: i32, secs: u32, refs: &str) -> Res {
        let mut res = Res::default();
        if self.stopped() {
            res.infra = true;
            res.reason = "interrupted".into();
            return res;
        }
        if let Err(e) = offset::set(&c.r.bus, at) {
            res.infra = true;
            res.reason = format!("cannot set offset {}: {e:#}", signed(at));
            return res;
        }
        c.cur.store(at, Ordering::Relaxed);
        if at != c.base.load(Ordering::Relaxed) {
            self.db.mark_trying(&c.uuid, at);
        }
        let _ = self.nv.take_xids(c.nv);
        let proc_exe = std::path::Path::new("/proc/self/exe");
        let exe = if cfg!(target_os = "linux") && proc_exe.exists() { proc_exe.to_path_buf() } else { std::env::current_exe().unwrap_or_else(|_| "freeforgem".into()) };
        let mut args = vec!["--prox-uv-check".to_string(), "--devices".into(), c.r.dev.to_string(), "--uv-secs".into(), secs.to_string()];
        if !refs.is_empty() {
            args.push("--uv-ref".into());
            args.push(refs.into());
        }
        let mut child = match Command::new(exe).args(&args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn() {
            Ok(ch) => ch,
            Err(e) => {
                res.infra = true;
                res.reason = format!("cannot start the check process ({e})");
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
        let (mut load_at, mut result, mut tail): (Option<Instant>, Option<String>, Vec<String>) = (None, None, Vec::new());
        let mut samples: Vec<(f64, u32, u32)> = Vec::new();
        let mut last = Instant::now();
        let limit = Duration::from_secs(secs as u64 + 90);
        let take = |l: String, load_at: &mut Option<Instant>, result: &mut Option<String>, tail: &mut Vec<String>| {
            if l.starts_with("PROX_LOAD") {
                *load_at = Some(Instant::now());
            } else if l.starts_with("PROX_CHECK") {
                *result = Some(l);
            } else if !l.trim().is_empty() {
                tail.push(l);
                if tail.len() > 3 {
                    tail.remove(0);
                }
            }
        };
        loop {
            if !matches!(child.try_wait(), Ok(None)) {
                break;
            }
            while let Ok(l) = rx.try_recv() {
                take(l, &mut load_at, &mut result, &mut tail);
            }
            if self.stopped() {
                let _ = child.kill();
                let _ = child.wait();
                self.db.clear_trying(&c.uuid);
                res.infra = true;
                res.reason = "interrupted".into();
                return res;
            }
            if let Some(x) = self.nv.take_xids(c.nv).first() {
                let _ = child.kill();
                let _ = child.wait();
                res.fault = true;
                res.reason = format!("Xid {x} reported by the driver");
                return res;
            }
            if t0.elapsed() > limit {
                let _ = child.kill();
                let _ = child.wait();
                res.fault = true;
                res.reason = "timeout (GPU hang?)".into();
                return res;
            }
            if result.is_none() && load_at.is_some_and(|t| t.elapsed() > Duration::from_secs(4)) && last.elapsed() >= Duration::from_millis(500) {
                last = Instant::now();
                if let Some(p) = self.nv.power_w(c.nv) {
                    samples.push((p, self.nv.clock_mhz(c.nv).unwrap_or(0), self.nv.temp_c(c.nv).unwrap_or(0)));
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        while let Ok(l) = rx.recv_timeout(Duration::from_millis(500)) {
            take(l, &mut load_at, &mut result, &mut tail);
        }
        if let Some(x) = self.nv.take_xids(c.nv).first() {
            res.fault = true;
            res.reason = format!("Xid {x} reported by the driver");
            return res;
        }
        if result.is_none() && self.stopped() {
            self.db.clear_trying(&c.uuid);
            res.infra = true;
            res.reason = "interrupted".into();
            return res;
        }
        self.db.clear_trying(&c.uuid);
        if !samples.is_empty() {
            let n = samples.len() as f64;
            res.w = Some(samples.iter().map(|s| s.0).sum::<f64>() / n);
            res.clk = samples.last().map(|s| s.1).filter(|&x| x > 0);
            res.t = samples.last().map(|s| s.2 as i32).filter(|&x| x > 0);
        }
        match result {
            Some(l) if l.starts_with("PROX_CHECK ok ") => {
                res.ok = true;
                let mut it = l["PROX_CHECK ok ".len()..].split_whitespace();
                while let (Some(k), Some(v)) = (it.next(), it.next()) {
                    match k {
                        "launches" => res.launches = v.parse().unwrap_or(0),
                        "ghs" => res.ghs = v.parse().unwrap_or(0.0),
                        "counts" => res.refs = v.to_string(),
                        _ => {}
                    }
                }
            }
            Some(l) => res.reason = l.trim_start_matches("PROX_CHECK FAIL ").to_string(),
            None => {
                res.fault = true;
                res.reason = format!("the check process ended without a result{}", tail.last().map_or(String::new(), |t| format!(" (last line: {t})")));
            }
        }
        res
    }

    fn line(&self, c: &Card, what: &str, at: i32, r: &Res) -> String {
        let w = r.w.map_or("? W".into(), |w| format!("{w:.1} W"));
        if r.ok {
            format!("{} {what} {}: ok ({} launches, {:.2} GH/s in the check), {w}, {} MHz, {} C", c.label(), signed(at), r.launches, r.ghs, r.clk.unwrap_or(0), r.t.unwrap_or(0))
        } else {
            format!("{} {what} {}: FAILED ({})", c.label(), signed(at), r.reason)
        }
    }

    fn entry(&self, c: &Card, clk: u32) -> Entry {
        Entry {
            name: c.r.name.clone(),
            bus: c.r.bus.clone(),
            driver: self.driver.clone(),
            kernel: kernel_id(),
            core_lock: c.r.core_lock.unwrap_or(0),
            clk,
            orig: c.orig,
            step: self.p.step,
            ..Default::default()
        }
    }

    /// Base check at `base` (falling 30 MHz at a time to 0 like Pearl when it fails); Some((base, result)) or None (nothing passes).
    fn base_check(&self, c: &Card, base: i32) -> Option<(i32, Res)> {
        let r = self.check(c, base, self.p.warmup, "");
        lg(self.line(c, "base", base, &r));
        if r.ok {
            return Some((base, r));
        }
        if r.infra {
            return None;
        }
        // 1.7.8 (1.7.7 plan, point 4): a failing start is checked once more before the search goes lower (one failure can be a
        // transient one); not after a driver fault
        if !r.fault && !self.stopped() {
            lg(format!("{} the start {} failed once: checking it again before going lower", c.label(), signed(base)));
            let r = self.check(c, base, self.p.warmup, "");
            lg(self.line(c, "base (again)", base, &r));
            if r.ok {
                return Some((base, r));
            }
            if r.infra {
                return None;
            }
        }
        for b in climb::fall_steps(base, c.lo.max(0)) {
            let r = self.check(c, b, self.p.warmup, "");
            lg(self.line(c, "base", b, &r));
            if r.ok {
                return Some((b, r));
            }
            if r.infra {
                return None;
            }
        }
        None
    }

    fn tune(&self, c: &Card, base: i32, br: Res, mut cap: i32, crash: Option<profiles::Crash>) {
        let step = self.p.step;
        let mut from = base;
        if let Some(cr) = crash {
            if cr.failed() {
                cap = cap.min(cr.at - step);
            }
            if let Some(g) = cr.good.filter(|&g| g > base && g <= cap) {
                from = g;
                lg(format!("{} steps up to {} passed before the crash{}: not climbing them again", c.label(), signed(g), if cr.failed() { format!(" at {}", signed(cr.at)) } else { " of another GPU".into() }));
            } else if cr.failed() {
                lg(format!("{} the previous run crashed at {}: tuning up to {}", c.label(), signed(cr.at), signed(cap)));
            }
        }
        let refs = br.refs.clone();
        let mut e = self.entry(c, br.clk.unwrap_or(0));
        e.base = base;
        e.w_base = br.w.unwrap_or(0.0);
        if from + step > cap {
            lg(format!("{} no room above the base {} (cap {}): mining at the base", c.label(), signed(base), signed(cap)));
            e.offset = base;
            e.refs = refs.clone();
            e.tested_at = uv::now();
            e.verified_at = e.tested_at;
            self.save(c, &e);
            return;
        }
        lg(format!("{} tuning {}..{} (+{step} MHz steps, coarse +{}, {} s each, {} s confirmation); this GPU mines after it", c.label(), signed(from), signed(cap), self.p.coarse, self.p.step_secs, self.p.confirm));
        let s = climb::climb(from, br.w, br.t, cap, step, self.p.coarse, |cand, _good, _mhz, _refine| {
            self.db.set_good(&c.uuid, Some(_good));
            let r = self.check(c, cand, self.p.step_secs, &refs);
            if r.infra {
                return climb::StepOut { abort: true, ..Default::default() };
            }
            lg(self.line(c, "step", cand, &r));
            climb::StepOut { ok: r.ok, fault: r.fault, w: r.w, t: r.t, abort: false }
        });
        self.db.set_good(&c.uuid, None);
        if s.aborted {
            let _ = offset::set(&c.r.bus, base);
            c.cur.store(base, Ordering::Relaxed);
            return;
        }
        let mut fin = if s.failed && !self.p.keep_last { (s.good - step).max(base) } else { s.good };
        if s.plateau {
            lg(format!("{} power stops falling above {} (voltage floor): not climbing further", c.label(), signed(s.good)));
        }
        let mut conf_w = s.good_w;
        while fin > base {
            let r = self.check(c, fin, self.p.confirm, &refs);
            lg(self.line(c, "confirmation", fin, &r));
            if r.infra {
                return;
            }
            if r.ok {
                conf_w = r.w;
                e.ghs = r.ghs;
                break;
            }
            fin = (fin - step).max(base);
        }
        let _ = offset::set(&c.r.bus, fin);
        c.cur.store(fin, Ordering::Relaxed);
        e.offset = fin;
        e.edge = if s.failed { s.fail_at } else { 0 };
        e.w = conf_w.unwrap_or(0.0);
        e.refs = refs;
        e.tested_at = uv::now();
        e.verified_at = e.tested_at;
        self.save(c, &e);
        lg(format!(
            "{} RESULT: offset {} -> {}, check power {} -> {}, saved",
            c.label(),
            signed(base),
            signed(fin),
            br.w.map_or("?".into(), |w| format!("{w:.1} W")),
            conf_w.map_or("?".into(), |w| format!("{w:.1} W"))
        ));
    }

    fn save(&self, c: &Card, e: &Entry) {
        let k = pkey(e);
        *c.key.lock().unwrap() = k.clone();
        *c.refs.lock().unwrap() = e.refs.clone();
        c.base.store(e.base, Ordering::Relaxed);
        self.db.save(&c.uuid, "", &k, e);
    }

    /// One card before mining: a crash marker, a saved profile (quick check) or a new tuning.
    fn prepare_card(&self, c: &Card) {
        let start = self.p.min.get(c_pos(self, c)).unwrap_or(c.orig).clamp(c.lo, c.cap);
        let mut crash = self.db.take_trying(&c.uuid);
        if let Some(k) = crash {
            // 1.7.8: a CUDA fault on one card ends the whole process; only the card with the Xid has a failed offset
            if let Some(by) = crate::faults::bystander(&self.db.dir, &c.uuid) {
                lg(format!("{} the previous run died while this GPU checked offset {}, but the fault was on {by}: this GPU did not fail, its offset is not counted as failed{}", c.label(), signed(k.at), k.good.map_or(String::new(), |g| format!("; the steps up to {} that passed are kept", signed(g)))));
                crash = k.bystander();
            }
        }
        // a crash while mining at a tuned offset (faults.rs): that offset counts as failed
        if let Some(mut r) = crate::faults::get(&self.db.dir, &c.uuid) {
            if r.n >= 1 && r.offset > start {
                lg(format!("{} crashed while mining at offset {} in the previous run (Xid {}): that offset counts as failed", c.label(), signed(r.offset), r.xid));
                r.n = 0;
                crate::faults::put(&self.db.dir, &c.uuid, &r);
                crash = Some(crash.map_or(profiles::Crash { at: r.offset, good: None }, |x| profiles::Crash { at: x.at.min(r.offset), good: x.good }));
            }
        }
        let lock = c.r.core_lock;
        let profs: Vec<(String, Entry)> = self.db.list::<Entry>(&c.uuid, "").into_iter().filter(|(_, e)| e.driver == self.driver).collect();
        if self.p.reset {
            let keys: Vec<String> = profs.iter().filter(|(_, e)| lock.is_some_and(|l| e.core_lock == l) || lock.is_none()).map(|(k, _)| k.clone()).collect();
            if !keys.is_empty() && self.db.forget(&c.uuid, &keys) {
                lg(format!("{} --uv-reset: profile(s) {keys:?} forgotten", c.label()));
            }
        }
        let profs: Vec<(String, Entry)> = self.db.list::<Entry>(&c.uuid, "").into_iter().filter(|(_, e)| e.driver == self.driver).collect();
        // the profile of this lock; without a lock the base check measures the clock under load first
        let mut base_res: Option<(i32, Res)> = None;
        let sel = match lock {
            Some(l) => profs.iter().find(|(_, e)| e.core_lock == l).cloned(),
            None => {
                let Some(b) = self.base_check(c, start) else { return self.give_up(c, start) };
                let clk = b.1.clk.unwrap_or(0);
                base_res = Some(b);
                profs.iter().filter(|(_, e)| e.core_lock == 0 && (e.clk as i64 - clk as i64).abs() <= 45).max_by_key(|(_, e)| e.verified_at).cloned()
            }
        };
        if crash.is_none() {
            if let Some((k, e)) = &sel {
                if e.disabled_until > uv::now() {
                    lg(format!("{} profile {k}: auto-uv paused until the next 24 h (the base failed earlier); mining at {}", c.label(), signed(e.base)));
                    let _ = offset::set(&c.r.bus, e.base);
                    c.cur.store(e.base, Ordering::Relaxed);
                    return;
                }
                if e.kernel == kernel_id() && !e.refs.is_empty() && e.offset >= e.base {
                    self.db.use_key(&c.uuid, k);
                    let r = self.check(c, e.offset, QUICK, &e.refs);
                    lg(self.line(c, &format!("saved profile {k}: quick check at"), e.offset, &r));
                    if r.ok {
                        let mut x = e.clone();
                        x.verified_at = uv::now();
                        self.save(c, &x);
                        let _ = offset::set(&c.r.bus, e.offset);
                        c.cur.store(e.offset, Ordering::Relaxed);
                        return;
                    }
                    if r.infra {
                        return;
                    }
                    lg(format!("{} the saved offset {} failed its quick check: tuning again below it", c.label(), signed(e.offset)));
                    let b = match base_res.take() {
                        Some(b) => b,
                        None => match self.base_check(c, e.base.min(start)) {
                            Some(b) => b,
                            None => return self.give_up(c, start),
                        },
                    };
                    return self.tune(c, b.0, b.1, c.cap.min(e.offset - self.p.step), None);
                }
                lg(format!("{} profile {k} is from another miner version / kernel ({}): a new reference at its base, then the climb", c.label(), e.kernel));
            }
        }
        let b = match base_res.take() {
            Some(b) => b,
            None => match self.base_check(c, start) {
                Some(b) => b,
                None => return self.give_up(c, start),
            },
        };
        self.tune(c, b.0, b.1, c.cap, crash);
    }

    fn give_up(&self, c: &Card, start: i32) {
        if self.stopped() {
            return;
        }
        let at = c.orig.min(0);
        lg(format!("{} no offset from {} down to 0 passes the check: auto-uv paused for 24 h, mining at {}", c.label(), signed(start), signed(at)));
        let _ = offset::set(&c.r.bus, at);
        c.cur.store(at, Ordering::Relaxed);
        let mut e = self.entry(c, 0);
        e.base = at;
        e.offset = at;
        e.disabled_until = uv::now() + DISABLE_SECS;
        if let Some(l) = c.r.core_lock {
            e.core_lock = l;
            self.db.save(&c.uuid, "", &pkey(&e), &e);
        }
    }
}

fn c_pos(l: &Live, c: &Card) -> usize {
    l.cards.iter().position(|x| std::ptr::eq(x.as_ref(), c)).unwrap_or(0)
}

/// Before mining: every GPU in parallel (one check child each); returns the state the miner keeps for `tick`.  `nv` from
/// offset::begin (the owner's offsets recorded).
pub fn prepare(nv: &'static Nvml, cards: Vec<CardRef>, o: &uv::Opts, dir: &std::path::Path, stop: &Arc<AtomicBool>) -> Arc<Live> {
    let db = Store::load(dir, profiles::PROX, |m| lg(m), |_| 0);
    if o.reset_all && db.forget_all() {
        lg("--uv-reset-all: every PROX profile forgotten");
    }
    let mut v = Vec::new();
    for (i, r) in cards.into_iter().enumerate() {
        let Ok(d) = nv.by_bus(&r.bus) else {
            lg(format!("GPU{}: no NVML handle; not tuned", r.dev));
            continue;
        };
        let Some((orig, lo, hi)) = offset::info(&r.bus) else {
            lg(format!("GPU{} (bus {}): the core offset cannot be read or set (root / Administrator needed?); not tuned", r.dev, uv::short_bus(&r.bus)));
            continue;
        };
        if let Err(e) = offset::set(&r.bus, orig) {
            lg(format!("GPU{} (bus {}): the core offset cannot be set ({e:#}); not tuned (Linux: root, Windows: Run as administrator)", r.dev, uv::short_bus(&r.bus)));
            continue;
        }
        let max = o.max_offset.get(i).unwrap_or(DEF_MAX).min(uv::HARD_CAP).min(hi);
        nv.watch_xid(d);
        let uuid = nv.uuid(d).unwrap_or_else(|_| r.bus.clone());
        v.push(Arc::new(Card {
            uuid,
            nv: d,
            orig,
            lo,
            cap: max,
            base: AtomicI32::new(orig),
            cur: AtomicI32::new(orig),
            key: Mutex::new(String::new()),
            refs: Mutex::new(String::new()),
            owned: AtomicBool::new(false),
            busy: AtomicBool::new(false),
            bad_seen: AtomicU64::new(r.bad.load(Ordering::Relaxed)),
            next_reval: AtomicU64::new(0),
            next_readback: AtomicU64::new(0),
            resets: Default::default(),
            r,
        }));
    }
    let live = Arc::new(Live { nv, db, p: Params::from(o), driver: nv.driver_version(), cards: v, stop: stop.clone() });
    std::thread::scope(|s| {
        for c in &live.cards {
            let l = live.clone();
            s.spawn(move || {
                lg(format!("{}: current offset {}, range {}..{} (max: {})", c.label(), signed(c.orig), signed(c.lo), signed(c.cap), if l.p.max.get(c_pos(&l, c)).is_some() { "--uv-max" } else { "default" }));
                l.prepare_card(c);
            });
        }
    });
    let t = uv::now();
    for c in &live.cards {
        let cur = c.cur.load(Ordering::Relaxed);
        c.owned.store(cur > c.base.load(Ordering::Relaxed) && !live.stopped(), Ordering::Relaxed);
        c.bad_seen.store(c.r.bad.load(Ordering::Relaxed), Ordering::Relaxed);
        c.next_reval.store(t + (live.p.reval_h * 3600.0) as u64, Ordering::Relaxed);
        c.next_readback.store(t + 30, Ordering::Relaxed);
        if !live.stopped() {
            lg(format!("{} mining at offset {}", c.label(), signed(cur)));
        }
    }
    live
}

impl Live {
    /// One step down (never below the checked base), saved.
    fn step_back(&self, c: &Card, why: &str) {
        let cur = c.cur.load(Ordering::Relaxed);
        let base = c.base.load(Ordering::Relaxed);
        let nw = (cur - self.p.step).max(base);
        if nw == cur {
            return;
        }
        if let Err(e) = offset::set(&c.r.bus, nw) {
            lg(format!("{} {why}: cannot step back ({e:#}); auto-uv stops watching this GPU", c.label()));
            c.owned.store(false, Ordering::Relaxed);
            return;
        }
        c.cur.store(nw, Ordering::Relaxed);
        lg(format!("{} {why}: offset {} -> {}, saved", c.label(), signed(cur), signed(nw)));
        let k = c.key.lock().unwrap().clone();
        if let Some(mut e) = self.db.get::<Entry>(&c.uuid, &k) {
            e.offset = nw;
            e.stepped_back += 1;
            self.db.save(&c.uuid, "", &k, &e);
        }
        if nw <= base {
            c.owned.store(false, Ordering::Relaxed);
        }
    }

    fn revalidate(&self, c: &Card) {
        let refs = c.refs.lock().unwrap().clone();
        lg(format!("{} re-check at {} (mining on this GPU pauses for ~{} s)", c.label(), signed(c.cur.load(Ordering::Relaxed)), QUICK + 5));
        c.r.pause.store(true, Ordering::SeqCst);
        let t = Instant::now();
        while !c.r.paused.load(Ordering::SeqCst) && t.elapsed() < Duration::from_secs(30) && !self.stopped() {
            std::thread::sleep(Duration::from_millis(50));
        }
        if c.r.paused.load(Ordering::SeqCst) {
            while c.owned.load(Ordering::Relaxed) && !self.stopped() {
                let cur = c.cur.load(Ordering::Relaxed);
                let r = self.check(c, cur, QUICK, &refs);
                lg(self.line(c, "re-check", cur, &r));
                if r.ok || r.infra {
                    if r.ok {
                        let k = c.key.lock().unwrap().clone();
                        if let Some(mut e) = self.db.get::<Entry>(&c.uuid, &k) {
                            e.verified_at = uv::now();
                            self.db.save(&c.uuid, "", &k, &e);
                        }
                    }
                    break;
                }
                self.step_back(c, "re-check failed");
            }
            let _ = offset::set(&c.r.bus, c.cur.load(Ordering::Relaxed));
        } else {
            lg(format!("{} re-check skipped (the mining thread did not pause)", c.label()));
        }
        c.r.pause.store(false, Ordering::SeqCst);
    }
}

/// The offset the tuner keeps on `dev` (faults.rs: a crash at a tuned offset counts against that offset).
pub fn expected_offset(live: &Live, dev: i32) -> Option<i32> {
    live.cards.iter().find(|c| c.r.dev == dev).map(|c| c.cur.load(Ordering::Relaxed))
}

/// From the miner's main loop: wrong GPU results -> one step back (+ a re-check in a minute); someone else changed the offset ->
/// leave the GPU alone; the periodic re-check.
pub fn tick(live: &Arc<Live>) {
    let t = uv::now();
    for c in &live.cards {
        if !c.owned.load(Ordering::Relaxed) || c.busy.load(Ordering::Relaxed) {
            continue;
        }
        if t >= c.next_readback.load(Ordering::Relaxed) {
            c.next_readback.store(t + 30, Ordering::Relaxed);
            if let Some(v) = offset::current(&c.r.bus) {
                let want = c.cur.load(Ordering::Relaxed);
                if v != want {
                    crate::faults::note_foreign_change(&c.r.bus);
                    match crate::hiveoc::reset_kind(live.nv, c.nv, v, c.orig) {
                        Some(why) if c.resets.allow() => {
                            // 1.7.8: HiveOS nvidia-oc (a miner start, an OC change in the web UI) wrote its own value: the verified
                            // offset goes back after a quick check (mining on this GPU pauses for ~QUICK s), as for Pearl / NOID
                            lg(format!(
                                "{} the offset was reset to {} by an OC apply ({why}); re-applying the verified {} after a quick check (mining on this GPU pauses for ~{} s)",
                                c.label(),
                                signed(v),
                                signed(want),
                                QUICK + 5
                            ));
                            c.busy.store(true, Ordering::Relaxed);
                            crate::clocks::reassert(&c.r.bus); // the lock first: a check at the verified offset on an unlocked clock crashes
                            let (l, c) = (live.clone(), c.clone());
                            std::thread::spawn(move || {
                                if !c.refs.lock().unwrap().is_empty() {
                                    l.revalidate(&c);
                                }
                                // revalidate sets the offset itself when it could pause the GPU; otherwise (or after a step back) once more here
                                if c.owned.load(Ordering::Relaxed) {
                                    let cur = c.cur.load(Ordering::Relaxed);
                                    if offset::current(&c.r.bus) != Some(cur) {
                                        let _ = offset::set(&c.r.bus, cur);
                                    }
                                }
                                c.bad_seen.store(c.r.bad.load(Ordering::Relaxed), Ordering::Relaxed);
                                c.next_readback.store(uv::now() + 30, Ordering::Relaxed);
                                c.busy.store(false, Ordering::Relaxed);
                            });
                            continue;
                        }
                        Some(why) => {
                            lg(format!(
                                "{} the offset was reset to {} by an OC apply ({why}) again: {} times within an hour already, so auto-uv stands down on this GPU and leaves it as is (restart the miner once the OC is settled)",
                                c.label(),
                                signed(v),
                                crate::hiveoc::OC_RESETS_MAX
                            ));
                            c.owned.store(false, Ordering::Relaxed);
                            continue;
                        }
                        None => {
                            lg(format!("{} the offset was changed by someone else ({} now, {} expected): auto-uv leaves this GPU as it is", c.label(), signed(v), signed(want)));
                            c.owned.store(false, Ordering::Relaxed);
                            continue;
                        }
                    }
                }
            }
        }
        let bad = c.r.bad.load(Ordering::Relaxed);
        let seen = c.bad_seen.swap(bad, Ordering::Relaxed);
        if bad > seen {
            live.step_back(c, &format!("{} candidate(s) failed the CPU check while mining", bad - seen));
            c.next_reval.store(t + 60, Ordering::Relaxed);
            continue;
        }
        if live.p.reval_h > 0.0 && t >= c.next_reval.load(Ordering::Relaxed) && !c.refs.lock().unwrap().is_empty() {
            c.busy.store(true, Ordering::Relaxed);
            let (l, c) = (live.clone(), c.clone());
            std::thread::spawn(move || {
                l.revalidate(&c);
                c.bad_seen.store(c.r.bad.load(Ordering::Relaxed), Ordering::Relaxed);
                c.next_reval.store(uv::now() + (l.p.reval_h * 3600.0) as u64, Ordering::Relaxed);
                c.busy.store(false, Ordering::Relaxed);
            });
        }
    }
}

/// `--uv-offset N` without `--auto-uv` (1.7.8): the fixed offsets are kept like the tuned ones: when an OC apply (HiveOS nvidia-oc)
/// writes its own value over them, the offset of this run is set again (one log line, at most OC_RESETS_MAX an hour per GPU).
pub struct Fixed {
    cards: Mutex<Vec<FixedCard>>,
}

struct FixedCard {
    dev: i32,
    bus: String,
    nv: Dev,
    orig: i32,
    want: i32,
    owned: bool,
    resets: crate::hiveoc::ResetLimit,
    next: Instant,
}

/// `items` = (device, bus id, offset that was set); only offsets that were set count.
pub fn fixed_prepare(nv: &'static Nvml, items: &[(i32, String, i32)]) -> Option<(&'static Nvml, Fixed)> {
    let cards: Vec<FixedCard> = items
        .iter()
        .filter_map(|(dev, bus, want)| {
            let d = nv.by_bus(bus).ok()?;
            let (orig, _, _) = offset::info(bus)?;
            Some(FixedCard { dev: *dev, bus: bus.clone(), nv: d, orig, want: *want, owned: true, resets: Default::default(), next: Instant::now() + Duration::from_secs(30) })
        })
        .collect();
    (!cards.is_empty()).then(|| (nv, Fixed { cards: Mutex::new(cards) }))
}

pub fn fixed_tick(nv: &Nvml, f: &Fixed) {
    let mut g = f.cards.lock().unwrap();
    for c in g.iter_mut() {
        if !c.owned || Instant::now() < c.next {
            continue;
        }
        c.next = Instant::now() + Duration::from_secs(30);
        let Some(v) = offset::current(&c.bus) else { continue };
        if v == c.want {
            continue;
        }
        let label = format!("GPU{} (bus {})", c.dev, uv::short_bus(&c.bus));
        crate::faults::note_foreign_change(&c.bus);
        match crate::hiveoc::reset_kind(nv, c.nv, v, c.orig) {
            Some(why) if c.resets.allow() => match {
                crate::clocks::reassert(&c.bus); // the lock first: the offset on an unlocked clock is too much
                offset::set(&c.bus, c.want)
            } {
                Ok(()) => lg(format!("{label} the offset was reset to {} by an OC apply ({why}); set back to {} (--uv-offset)", signed(v), signed(c.want))),
                Err(e) => {
                    lg(format!("{label} the offset was reset to {} by an OC apply ({why}); cannot set {} again ({e:#}): left as it is", signed(v), signed(c.want)));
                    c.owned = false;
                }
            },
            Some(why) => {
                lg(format!(
                    "{label} the offset was reset to {} by an OC apply ({why}) again: {} times within an hour already, so the miner stands down on this GPU and leaves it as is (restart the miner once the OC is settled)",
                    signed(v),
                    crate::hiveoc::OC_RESETS_MAX
                ));
                c.owned = false;
            }
            None => {
                lg(format!("{label} the offset was changed by someone else ({} now, {} expected): the miner leaves this GPU as it is", signed(v), signed(c.want)));
                c.owned = false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn keys_and_json() {
        let mut e = Entry { kernel: kernel_id(), driver: "580".into(), core_lock: 1500, offset: 210, base: 0, refs: "a:b:c".into(), ..Default::default() };
        assert_eq!(pkey(&e), "1500");
        let back = Entry::from_json(&e.to_json()).unwrap();
        assert_eq!((back.offset, back.refs.as_str(), back.core_lock), (210, "a:b:c", 1500));
        e.core_lock = 0;
        e.clk = 1903;
        assert_eq!(pkey(&e), "~1905");
        let f = Entry { clk: 1897, ..e.clone() };
        assert!(e.same_clocks(&f));
    }
}
