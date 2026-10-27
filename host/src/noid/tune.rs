// FreeForGem — see LICENSE
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
//!   * while mining, the offset is read back every 30 s: an OC apply (HiveOS nvidia-oc at a miner start or after an OC change
//!     in the web UI writes the profile's Core Clock value, 0 when the field is empty; other tools put the owner's offset back)
//!     gets the verified NOID offset back after a quick stress check with mining on that GPU paused (a failure steps back),
//!     a base / --uv-offset value is simply set again; at most 4 times per hour, any other value is the owner's own choice
//!     and the miner stands down on that GPU.  An OC apply during a check makes the check repeat at the right offset.
//!   * a stop (Ctrl-C, SIGTERM, HiveOS miner stop / restart: Ctrl-C, then SIGHUP) during a check is not a failure: no
//!     verdict, no 24 h pause, and the crash marker of the interrupted check is removed (a driver fault keeps its marker).
//!   * the owner's offset is put back on exit by offset.rs (marker file for crashes).
use super::{gpu, offset};
use crate::hiveoc::{self, ResetLimit};
use crate::nvml::{Dev, Nvml};
use crate::uv::{self, DISABLE_SECS, HARD_CAP, Saved, fmt_age, fmt_w, now, saved_vs_limit, short_bus, short_name, signed};
use anyhow::Result;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

const SECTION: &str = "noid";
const TRYING: &str = "noid_trying";
const START: i32 = 120; // default first step (RTX 3070: NOID edge +235..+250 at 1400..1650 MHz)
const STEP_SECS: u32 = 40; // agent M: 40 s of stress per step (first silent errors after ~12 s at the edge)
const BASE_SECS: u32 = 20; // the base is measured once, for the efficiency comparison of the result
const QUICK_SECS: u32 = 30;

fn lg(m: impl AsRef<str>) {
    println!("auto-uv (noid): {}", m.as_ref());
}

#[derive(Clone)]
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
    clk: u32,    // load clock at the base check (1.7.2)
    clk_ok: u32, // load clock at the result (confirmation / last quick check); 0 = unknown
}

impl Entry {
    fn to_json(&self) -> Value {
        json!({"name": self.name, "bus": self.bus, "driver": self.driver, "kernel": self.kernel, "core_lock": self.core_lock,
               "power_limit": self.pl, "orig_offset": self.orig, "base_offset": self.base, "offset": self.offset, "cap": self.cap,
               "step": self.step, "edge": self.edge, "mhs_base": self.mhs_base, "watts_base": self.w_base, "mhs": self.mhs,
               "watts": self.w, "tested_at": self.tested_at, "verified_at": self.verified_at, "disabled_until": self.disabled_until,
               "stepped_back_in_mining": self.stepped_back, "clk_mhz": self.clk, "clk_ok_mhz": self.clk_ok})
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
            clk: u("clk_mhz") as u32,
            clk_ok: u("clk_ok_mhz") as u32,
        })
    }
}

/// uv.json, shared with the Pearl tuner and the CMFD one (other sections are kept as they are).
struct Db {
    dir: PathBuf,
    v: Mutex<Value>,
    keys: Mutex<std::collections::HashMap<String, String>>, // uuid -> profile key in use ("eff@1400", "eff@~1815")
}

/// Key of a NOID profile (1.7.2): the mode and the core lock set by the miner ("eff@1400"), else the clock under load at the base
/// check ("eff@~1815").
fn pkey(mode: &str, e: &Entry) -> String {
    if e.core_lock != 0 {
        format!("{mode}@{}", e.core_lock)
    } else if e.clk != 0 {
        format!("{mode}@~{}", e.clk)
    } else {
        format!("{mode}@?")
    }
}

/// Two NOID profiles of the same clocks.
fn same_clocks(a: &Entry, b: &Entry) -> bool {
    if a.core_lock != 0 && b.core_lock != 0 {
        return a.core_lock == b.core_lock;
    }
    let (ca, cb) = (if a.core_lock != 0 { a.core_lock + 10 } else { a.clk }, if b.core_lock != 0 { b.core_lock + 10 } else { b.clk });
    ca != 0 && cb != 0 && (ca as i64 - cb as i64).abs() <= (ca as i64 * 3 / 100).max(45)
}

/// Does the profile fit the lock set by this run, or (lock unknown) the clock `clk` under load at the base check?
fn fits(e: &Entry, lock: Option<u32>, clk: Option<u32>) -> bool {
    let tol = |c: u32| (c as i64 * 3 / 100).max(45) + 15;
    match (lock, clk) {
        (Some(l), _) => if e.core_lock != 0 { e.core_lock == l } else { e.clk != 0 && (e.clk as i64 - l as i64).abs() <= tol(e.clk) },
        (None, Some(k)) => {
            let c = if e.clk != 0 { e.clk } else { e.core_lock + 10 };
            c > 10 && (c as i64 - k as i64).abs() <= tol(c)
        }
        _ => false,
    }
}

impl Db {
    fn load(dir: &Path) -> Db {
        let v = std::fs::read_to_string(uv::results_path(dir))
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({"version": 1, "gpus": {}}));
        let mut v = v;
        // before 1.7.2: one result per card and mode ("eff"); kept as the profile of its --lock-cclock, dropped without one (the
        // clock it was found at is unknown)
        let mut moved = 0;
        if let Some(sec) = v[SECTION].as_object_mut() {
            for (_, card) in sec.iter_mut() {
                let Some(m) = card.as_object_mut() else { continue };
                let old: Vec<String> = m.keys().filter(|k| !k.contains('@')).cloned().collect();
                for k in old {
                    let e = m.remove(&k).and_then(|x| Entry::from_json(&x));
                    if let Some(e) = e.filter(|e| e.core_lock != 0) {
                        m.insert(pkey(&k, &e), e.to_json());
                        moved += 1;
                    }
                }
            }
        }
        if moved > 0 {
            lg(format!("{moved} NOID result(s) from before 1.7.2 kept as the profiles of their --lock-cclock"));
            let _ = uv::write_atomic(&uv::results_path(dir), &serde_json::to_string_pretty(&v).unwrap());
        }
        Db { dir: dir.to_path_buf(), v: Mutex::new(v), keys: Mutex::new(Default::default()) }
    }
    /// All profiles of this card and mode.
    fn profiles(&self, uuid: &str, mode: &str) -> Vec<(String, Entry)> {
        let pre = format!("{mode}@");
        let v = self.v.lock().unwrap();
        let mut out: Vec<(String, Entry)> = v[SECTION][uuid].as_object().map(|m| m.iter().filter(|(k, _)| k.starts_with(&pre)).filter_map(|(k, x)| Entry::from_json(x).map(|e| (k.clone(), e))).collect()).unwrap_or_default();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }
    fn line(&self, uuid: &str, mode: &str) -> String {
        let p = self.profiles(uuid, mode);
        if p.is_empty() {
            return "none".into();
        }
        p.iter().map(|(k, e)| format!("{}:{}{}", k.split_once('@').map_or(k.as_str(), |x| x.1), signed(e.offset), if e.disabled_until > now() { " (paused)" } else { "" })).collect::<Vec<_>>().join(", ")
    }
    fn use_key(&self, uuid: &str, key: &str) {
        self.keys.lock().unwrap().insert(uuid.into(), key.into());
    }
    fn forget(&self, uuid: &str, key: &str) {
        let mut v = self.v.lock().unwrap();
        if v[SECTION][uuid].as_object_mut().is_some_and(|m| m.remove(key).is_some()) {
            self.write(&v);
        }
        self.keys.lock().unwrap().remove(uuid);
    }
    fn write(&self, v: &Value) {
        if let Err(e) = uv::write_atomic(&uv::results_path(&self.dir), &serde_json::to_string_pretty(v).unwrap()) {
            lg(format!("cannot save {}: {e:#}", uv::results_path(&self.dir).display()));
        }
    }
    /// The profile in use on this card (chosen at start), if any.
    fn entry(&self, uuid: &str, _mode: &str) -> Option<Entry> {
        let k = self.keys.lock().unwrap().get(uuid).cloned()?;
        self.v.lock().unwrap()[SECTION].get(uuid).and_then(|c| c.get(&k)).and_then(Entry::from_json)
    }
    /// Saves `e` as the profile of its mode and clocks; the profile of the same clocks is replaced, the others stay.
    fn save(&self, uuid: &str, mode: &str, e: &Entry) {
        let key = pkey(mode, e);
        let mut v = self.v.lock().unwrap();
        if !v[SECTION].is_object() {
            v[SECTION] = json!({});
        }
        if !v[SECTION][uuid].is_object() {
            v[SECTION][uuid] = json!({});
        }
        let pre = format!("{mode}@");
        let same: Vec<String> = v[SECTION][uuid].as_object().unwrap().iter().filter(|(k, x)| k.starts_with(&pre) && **k != key && Entry::from_json(x).is_some_and(|o| same_clocks(&o, e))).map(|(k, _)| k.clone()).collect();
        let m = v[SECTION][uuid].as_object_mut().unwrap();
        for k in same {
            m.remove(&k);
        }
        m.insert(key.clone(), e.to_json());
        self.write(&v);
        drop(v);
        self.use_key(uuid, &key);
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

/// The uv.json of this run (for the stop handler) and the cards whose check ended in a driver fault (their crash marker stays).
fn stop_db() -> &'static Mutex<(Option<Arc<Db>>, Vec<String>)> {
    static S: OnceLock<Mutex<(Option<Arc<Db>>, Vec<String>)>> = OnceLock::new();
    S.get_or_init(|| Mutex::new((None, Vec::new())))
}

/// At a stop (second Ctrl-C / SIGHUP of HiveOS miner stop while a check runs, console close): the crash markers of interrupted
/// NOID checks are removed, so the next start does not take the offset under check for a crash point; driver faults keep theirs.
pub fn forget_trying_on_stop() {
    let (db, faulted) = {
        let g = stop_db().lock().unwrap();
        (g.0.clone(), g.1.clone())
    };
    let Some(db) = db else { return };
    let ids: Vec<String> = db.v.lock().unwrap()[TRYING].as_object().map(|t| t.keys().cloned().collect()).unwrap_or_default();
    for id in ids.iter().filter(|id| !faulted.contains(id)) {
        db.clear_trying(id);
    }
}

/// Cards whose core offset this run changed (base 0 for a positive owner's offset, a tuned NOID offset, --uv-offset): stepped
/// back when a hit fails the CPU check (tuned ones), and watched for OC applies that reset the offset while mining.
struct LiveCard {
    job: bool, // its check / tuning runs once mining has started (start_jobs)
    pos: usize,
    dev: i32,
    bus: String,
    nv: Option<Dev>,
    uuid: String,
    orig: i32,
    base: i32,
    step: i32,
    tuned: bool,
    want: i32, // the offset this run set (the read-back must equal it)
    owned: bool,
    busy: Arc<AtomicBool>, // a quick check after an OC apply runs
    next_read: Instant,
    resets: Arc<ResetLimit>,
    card: Option<Card>, // tuned cards: what the quick check after an OC apply needs
    pause: Option<(Arc<AtomicBool>, Arc<AtomicBool>)>, // (pause request, paused) of the mining thread of this GPU
    last: Option<Instant>,
}

pub struct Live {
    db: Option<Arc<Db>>,
    mode: &'static str,
    mode_id: i32,
    nv: Option<&'static Nvml>,
    o: uv::Opts,
    driver: String,
    stop: Arc<AtomicBool>,
    cards: Mutex<Vec<LiveCard>>,
}

#[derive(Clone)]
struct Card {
    r: CardRef,
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

impl Card {
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

/// After the check: the offset of the card now, when it is not `at` (an OC apply or someone else changed it meanwhile).
fn changed_during(c: &Card, at: i32) -> Option<i32> {
    offset::current(&c.r.bus).filter(|&v| v != at)
}

impl Tuner<'_> {
    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    fn step_secs(&self) -> u32 {
        if self.o.step_secs == uv::Opts::default().step_secs { STEP_SECS } else { self.o.step_secs }
    }

    /// One stress check at offset `at` (set first) for `secs` seconds.  An OC apply that resets the offset during the check
    /// (HiveOS nvidia-oc at a miner start / from the web UI) makes the check say nothing about `at`: `at` is set again and the
    /// check repeated (twice at most); any other value there is a failed check.
    fn check(&self, c: &Card, at: i32, secs: u32) -> Res {
        // the stats line / summary show what this GPU does (status.rs); a single check clears its state after
        let tuning = crate::status::is_tuning(c.r.dev);
        if tuning {
            crate::status::set(c.r.dev, format!("NOID tuning {}", signed(at)), true, None);
        } else {
            crate::status::set(c.r.dev, format!("NOID check {}", signed(at)), false, Some(Instant::now() + Duration::from_secs(secs as u64 + 25)));
        }
        let r = self.check_states(c, at, secs);
        if !tuning {
            crate::status::clear(c.r.dev);
        }
        r
    }

    fn check_states(&self, c: &Card, at: i32, secs: u32) -> Res {
        let mut r = Res::default();
        for attempt in 0..3 {
            r = self.check_once(c, at, secs);
            if r.fault || r.infra || self.stopped() {
                break;
            }
            let Some(v) = changed_during(c, at) else { break };
            match hiveoc::reset_kind(self.nv, c.nv, v, c.orig) {
                Some(why) if attempt < 2 => {
                    lg(format!("{} the offset was reset to {} during the check ({why}): setting {} again and repeating the check", c.label(), signed(v), signed(at)));
                }
                _ => {
                    r.ok = false;
                    r.reason = format!("the offset was changed to {} by someone else during the check ({} expected)", signed(v), signed(at));
                    break;
                }
            }
        }
        if r.fault {
            stop_db().lock().unwrap().1.push(c.uuid.clone());
        }
        r
    }

    fn check_once(&self, c: &Card, at: i32, secs: u32) -> Res {
        let mut res = Res::default();
        if self.stopped() {
            res.reason = "interrupted".into();
            res.infra = true;
            return res;
        }
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
        let mut load_at: Option<Instant> = None; // the child started its GPU loop (NOID_LOAD)
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
                if l.starts_with("NOID_LOAD") {
                    load_at = Some(Instant::now());
                } else if l.starts_with("NOID_CHECK") {
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
            // power under the GPU load only (after the child's CPU reference), past a warm-up of a third of the run
            let warm = Duration::from_secs((secs as u64 / 3).max(3));
            if result.is_none() && load_at.is_some_and(|t| t.elapsed() > warm) && last_sample.elapsed() >= Duration::from_millis(500) {
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
        // a stop kills the check process too (Ctrl-C reaches the whole process group; HiveOS miner stop): not a GPU failure.
        // The stop flag may be set a moment after the check process died, so look again shortly
        if result.is_none() && (self.stopped() || {
            std::thread::sleep(Duration::from_millis(700));
            self.stopped()
        }) {
            res.reason = "interrupted".into();
            res.infra = true;
            self.db.clear_trying(&c.uuid);
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

    /// At start, before mining: the offset when no check is needed (known lock and a recently verified profile of it, a
    /// pause, nothing above the base, a GPU left out); None = a check or a tuning, run once mining has started (start_jobs).
    fn quick(&self, c: &Card) -> Option<i32> {
        let mode = super::mode_name(self.mode);
        if self.db.v.lock().unwrap()[TRYING].get(&c.uuid).is_some() || self.o.reset {
            return None;
        }
        if let Some(r) = crate::faults::get(&self.db.dir, &c.uuid) {
            if crate::faults::excluded(&self.db.dir, &c.uuid).is_some() {
                let _ = offset::set(&c.r.bus, c.base);
                return Some(c.base);
            }
            if r.n >= 1 && r.offset > c.base {
                return None;
            }
        }
        let l = c.r.core_lock?;
        let (key, e) = self.db.profiles(&c.uuid, mode).into_iter().filter(|(_, e)| fits(e, Some(l), None)).min_by_key(|(_, e)| u64::MAX - e.verified_at)?;
        if e.driver != self.driver || e.kernel != c.kernel || (e.pl > 0.0 && c.pl > 0.0 && (c.pl - e.pl).abs() > 0.03 * e.pl) || e.base != c.base {
            return None;
        }
        self.db.use_key(&c.uuid, &key);
        let line = self.db.line(&c.uuid, mode);
        if e.disabled_until > now() || e.offset <= c.base {
            lg(format!("{} NOID profiles ({mode}): {line}; using {key}: {}; mining at {}", c.label(), if e.disabled_until > now() { "paused after a failure" } else { "nothing above the base" }, signed(c.base)));
            let _ = offset::set(&c.r.bus, c.base);
            return Some(c.base);
        }
        let age = now().saturating_sub(e.verified_at);
        if saved_vs_limit(e.offset, e.cap, c.max) != Saved::Apply || self.o.reval_hours <= 0.0 || (age as f64) >= self.o.reval_hours * 3600.0 {
            return None;
        }
        lg(format!("{} NOID profiles ({mode}): {line}; using {key}: offset {} (verified {} ago at this lock): applied", c.label(), signed(e.offset), fmt_age(age)));
        offset::set(&c.r.bus, e.offset).ok()?;
        Some(e.offset)
    }

    /// Offset to mine at for this card.
    fn run_card(&self, c: &Card) -> i32 {
        let mode = super::mode_name(self.mode);
        // a crash at an offset (during a check, or while mining: faults.rs): with a profile of these clocks the card steps back from
        // it (to the verified offset, or one step below the crash) after a quick check; without one it is tuned below it (1.7.2)
        let mut crash_at: Option<i32> = None;
        if let Some(at) = self.db.take_trying(&c.uuid) {
            lg(format!("{} the previous run died while checking NOID offset {} (crash or kill): that offset counts as failed", c.label(), signed(at)));
            crash_at = Some(at);
        }
        // a crash while mining at a tuned NOID offset (faults.rs, 1.7.2): that offset counts as failed
        if let Some(mut r) = crate::faults::get(&self.db.dir, &c.uuid) {
            if crate::faults::excluded(&self.db.dir, &c.uuid).is_some() {
                let _ = offset::set(&c.r.bus, c.base);
                return c.base; // left out of mining (logged by the miner)
            }
            if r.n >= 1 && r.offset > c.base {
                lg(format!("{} crashed while mining at NOID offset {} in the previous run (Xid {}): that offset counts as failed", c.label(), signed(r.offset), r.xid));
                r.n = 0;
                crate::faults::put(&self.db.dir, &c.uuid, &r);
                crash_at = Some(crash_at.map_or(r.offset, |a| a.min(r.offset)));
            }
        }
        // 1.7.2: one profile per mode and core clock lock.  The lock is known when this run sets it (--lock-cclock); else a short
        // check at the base offset measures the clock under load, which picks the profile
        let lock = c.r.core_lock;
        let profs = self.db.profiles(&c.uuid, mode);
        let clk = if lock.is_none() && !profs.is_empty() {
            let r = self.check(c, c.base, 12);
            if r.infra {
                return self.abort(c, &r);
            }
            r.clk
        } else {
            None
        };
        let sel = profs.iter().filter(|(_, e)| fits(e, lock, clk)).min_by_key(|(_, e)| u64::MAX - e.verified_at).map(|(k, _)| k.clone());
        let what = match (lock, clk) {
            (Some(l), _) => format!("core lock {l} MHz"),
            (None, Some(k)) => format!("{k} MHz under load at the base (lock not set by the miner)"),
            _ => "these clocks".into(),
        };
        lg(format!("{} NOID profiles ({mode}): {}; {}", c.label(), self.db.line(&c.uuid, mode), sel.as_ref().map_or(format!("none for {what}"), |k| format!("using {k} for {what}"))));
        if self.o.reset {
            if let Some(k) = &sel {
                self.db.forget(&c.uuid, k);
            }
            lg(format!("{} --uv-reset: {} forgotten; the other NOID profiles are kept (--uv-reset-all forgets all)", c.label(), sel.as_deref().unwrap_or("no profile")));
            return self.tune(c, None);
        }
        let Some(key) = sel else { return self.tune(c, crash_at) };
        self.db.use_key(&c.uuid, &key);
        let Some(e) = self.db.entry(&c.uuid, mode) else { return self.tune(c, crash_at) };
        if let Some(at) = crash_at {
            let want = if e.offset < at { e.offset } else { at - uv::crash_back(self.o.step.max(1)) };
            lg(format!("{} NOID profile {key}: {} {} after a quick check (no tuning from scratch)", c.label(), if e.offset < at { "back to the verified" } else { "30 MHz below the crash:" }, signed(want)));
            return self.step_down(c, &e, want);
        }
        let why = if e.driver != self.driver {
            Some(format!("driver {} when tuned, {} now", e.driver, self.driver))
        } else if e.kernel != c.kernel {
            Some(format!("kernel {} when tuned, {} now", e.kernel, c.kernel))
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
                // the verified offset stays the floor: the search goes on above it, not from --uv-min
                lg(format!("{} the limit was raised to {} ({}): tuning on above the verified {}", c.label(), signed(c.max), c.max_src, signed(e.offset)));
                let mut from = c.clone();
                from.start = e.offset;
                return self.tune(&from, None);
            }
        };
        let age = now().saturating_sub(e.verified_at);
        // 1.7.2: without --lock-cclock the lock is not known to the miner (HiveOS / nvidia-smi may have changed it since): never
        // applied without the quick check, whose load clock is compared with the one the offset was verified at
        if c.r.core_lock.is_some() && (age as f64) < self.o.reval_hours * 3600.0 && self.o.reval_hours > 0.0 {
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
            if let (true, Some(k)) = (e.clk_ok != 0, r.clk) {
                if let uv::Regime::Differs(d) = uv::regime_probe(e.clk_ok, at, k, at) {
                    lg(format!("{} {}: the check passed, but at {k} MHz under load, {} MHz when verified ({d}: the clock lock changed): tuning again", c.label(), signed(at), e.clk_ok));
                    return self.tune(c, None);
                }
            }
            lg(format!("{} {}: ok ({}); mining at it", c.label(), signed(at), r.brief()));
            let mut e2 = e.clone();
            e2.verified_at = now();
            if e2.clk_ok == 0 {
                e2.clk_ok = r.clk.unwrap_or(0);
            }
            self.db.save(&c.uuid, mode, &e2);
            return at;
        }
        lg(format!("{} {}: FAILED ({}): one step down from it (no tuning from scratch)", c.label(), signed(at), r.reason));
        self.step_down(c, &e, at - self.o.step.max(1))
    }

    /// From a verified NOID offset that failed (or crashed) downwards: a quick check per step (at most 5); the first one that passes
    /// becomes the profile's offset; nothing above the base passes: the base, NOID auto-uv paused at these clocks for 24 h.
    fn step_down(&self, c: &Card, e: &Entry, want: i32) -> i32 {
        let mode = super::mode_name(self.mode);
        let step = self.o.step.max(1);
        let mut at = want;
        for _ in 0..5 {
            if at <= c.base || self.stopped() {
                break;
            }
            let r = self.check(c, at, QUICK_SECS);
            if r.infra {
                return self.abort(c, &r);
            }
            if self.stopped() {
                return c.base;
            }
            if r.ok {
                lg(format!("{} {}: ok ({}); mining at it (stepped down from {})", c.label(), signed(at), r.brief(), signed(e.offset)));
                let mut e2 = e.clone();
                e2.offset = at;
                e2.edge = at + step;
                e2.verified_at = now();
                e2.clk_ok = r.clk.unwrap_or(e2.clk_ok);
                e2.stepped_back += 1;
                self.db.save(&c.uuid, mode, &e2);
                let _ = offset::set(&c.r.bus, at);
                return at;
            }
            lg(format!("{} {}: FAILED ({}): one step down", c.label(), signed(at), r.reason));
            at -= step;
        }
        if self.stopped() {
            return c.base;
        }
        lg(format!("{} no NOID offset above the base {} passes at these clocks: mining at the base, NOID auto-uv paused on this GPU at these clocks for 24 h", c.label(), signed(c.base)));
        let mut e2 = e.clone();
        e2.offset = c.base;
        e2.disabled_until = now() + DISABLE_SECS;
        self.db.save(&c.uuid, mode, &e2);
        let _ = offset::set(&c.r.bus, c.base);
        c.base
    }

    /// The check itself could not run (not a GPU failure): nothing is saved, the card mines at its base offset.
    fn abort(&self, c: &Card, r: &Res) -> i32 {
        if self.stopped() {
            // a stop: no verdict; the shutdown puts the owner's offset back
            return c.base;
        }
        lg(format!("{} the NOID check could not run ({}): auto-uv skipped for this run, mining at {}", c.label(), r.reason, signed(c.base)));
        let _ = offset::set(&c.r.bus, c.base);
        c.base
    }

    /// Full tune; `below`: an offset known to fail (search stays under it).
    fn tune(&self, c: &Card, below: Option<i32>) -> i32 {
        let mode = super::mode_name(self.mode);
        let _status = crate::status::Guard(c.r.dev);
        {
            let steps = ((c.max - c.start).max(0) / self.o.step.max(1)) as u64 + 2;
            let eta = Instant::now() + Duration::from_secs(BASE_SECS as u64 + 20 + steps * (self.step_secs() as u64 + 15) + self.o.confirm_secs.max(self.step_secs()) as u64 + 20);
            crate::status::set(c.r.dev, "NOID tuning".into(), true, Some(eta));
        }
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
        e.clk = r.clk.unwrap_or(0);
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
        e.clk = r.clk.unwrap_or(0);
        let mut conf_clk = r.clk;
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
            o -= step.max(30); // a failing start: 30 MHz lower (1.7.2 beta3)
        }
        let mut plateau = false;
        if last_ok > c.base && edge == 0 {
            // upwards to the first failure, or to the voltage floor: two steps in a row that save < 0.5 W (agent M's rule:
            // below ~1400 MHz an RTX 3070 already sits at its minimum voltage, a higher offset only adds risk)
            // 30 MHz steps below the edge, then the step (15) between the last good one and the first failure (beta5: Rig01
            // speed edges +315..+435 took ~20 min per lock in 15 MHz steps)
            let mut st = if step < 30 { 30 } else { step };
            let mut coarse_fail = 0;
            let mut o = last_ok + st;
            let mut flat = 0;
            let mut plateau_at = None;
            let mut hist: Vec<(i32, f64, f64)> = vec![(last_ok, good.0, good.1)];
            while o <= max && !self.stopped() {
                let r = self.check(c, o, self.step_secs());
                if r.infra {
                    return self.abort(c, &r);
                }
                if !r.ok && st > step && o - st + step < o && last_ok + step < o {
                    lg(format!("{} {}: FAILED ({}): {step} MHz steps from {} below it", c.label(), signed(o), r.reason, signed(last_ok + step)));
                    coarse_fail = o;
                    max = max.min(o - step);
                    st = step;
                    o = last_ok + step;
                    continue;
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
                o += st;
            }
            if edge == 0 && coarse_fail != 0 && plateau_at.is_none() && !self.stopped() {
                // every fine step below the coarse failure passed: that failure is the edge
                lg(format!("{} edge at {} (the 30 MHz step that failed)", c.label(), signed(coarse_fail)));
                edge = coarse_fail;
            }
            if let Some((p, m, w)) = plateau_at {
                lg(format!(
                    "{} power no longer drops above {} (voltage floor: {} and {} saved < 0.5 W each): stopping here, no edge searched beyond",
                    c.label(),
                    signed(p),
                    signed(hist[hist.len() - 2].0),
                    signed(hist[hist.len() - 1].0)
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
                conf_clk = r.clk;
                break;
            }
            lg(format!("{} confirmation {}: FAILED ({}): one step lower", c.label(), signed(res), r.reason));
            res -= step;
        }
        if res <= c.base {
            res = c.base;
            good = (e.mhs_base, e.w_base);
        }
        // never a result that is less efficient than the base (power barely changes with the offset at a low lock: the voltage
        // is already at its floor); keep the base and say so
        let eff = |m: f64, w: f64| if m > 0.0 && w > 0.0 { m / w } else { 0.0 };
        let (eb, er) = (eff(e.mhs_base, e.w_base), eff(good.0, good.1));
        if res > c.base && eb > 0.0 && er > 0.0 && er < eb * 1.01 {
            lg(format!(
                "{} {}: {:.3} MH/W, the base {}: {:.3} MH/W: no gain at this clock (the voltage is already at its floor): keeping the base {}",
                c.label(),
                signed(res),
                er,
                signed(c.base),
                eb,
                signed(c.base)
            ));
            res = c.base;
            good = (e.mhs_base, e.w_base);
        }
        e.offset = res;
        e.cap = max;
        e.edge = edge;
        e.clk_ok = conf_clk.unwrap_or(0);
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
    let mut live = Live {
        db: None,
        mode: super::mode_name(mode),
        mode_id: mode,
        nv,
        o: o.clone(),
        driver: String::new(),
        stop: stop.clone(),
        cards: Mutex::new(Vec::new()),
    };
    let Some(nv) = nv else {
        if o.auto || o.fixed.is_set() {
            lg("NVML not available: --auto-uv / --uv-offset skipped, offsets left as they are");
        }
        return Ok(Arc::new(live));
    };
    live.driver = nv.driver_version();
    if o.reset_all {
        let db = Db::load(dir);
        let mut v = db.v.lock().unwrap();
        if v.as_object_mut().is_some_and(|m| m.remove(SECTION).is_some()) {
            db.write(&v);
            lg("saved NOID results forgotten (--uv-reset-all)");
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
        // --uv-min, else the card type's default start by the core lock (RTX 3070 at 1400: +105, the voltage floor starts at
        // +120..+195), else +120
        let lock_hint = r.core_lock.or_else(|| crate::hiveoc::load().and_then(|p| nv.index(d).and_then(|i| p.core_lock(i as usize))));
        let coin = if mode == super::gpu::MODE_SPEED { uv::Coin::NoidSpeed } else { uv::Coin::Noid };
        let start = o.min_offset.get(r.pos).or_else(|| uv::arch_start(&r.name, coin, lock_hint).map(|x| x.0)).unwrap_or(START).max(lo);
        list.push(Card {
            r: r.clone(),
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
    // the offset each card mines at (None: left as the owner had it), and whether it is a tuned (verified) NOID offset
    let mut set_to: Vec<(Option<i32>, bool)> = vec![(None, false); list.len()];
    let mut pending: Vec<usize> = Vec::new(); // positions whose check / tuning runs once mining has started
    if o.fixed.is_set() {
        for (i, c) in list.iter().enumerate() {
            let Some(v) = o.fixed.get(c.r.pos) else { continue };
            let v = v.min(c.hi).min(HARD_CAP);
            match offset::set(&c.r.bus, v) {
                Ok(()) => {
                    lg(format!("{} core offset {} -> {} (--uv-offset; not checked for NOID: at your own risk)", c.label(), signed(c.orig), signed(v)));
                    set_to[i] = (Some(v), false);
                }
                Err(e) => lg(format!("{} cannot set offset {}: {e:#}", c.label(), signed(v))),
            }
        }
    } else if o.auto && !o.dry_run {
        let db = Arc::new(Db::load(dir));
        stop_db().lock().unwrap().0 = Some(db.clone());
        for c in &list {
            nv.watch_xid(c.nv);
        }
        // 1.7.2: only what needs no check is decided here; tunings and checks run once mining has started (start_jobs), each
        // pausing only its own GPU: the other GPUs mine meanwhile (before, every GPU waited for the slowest tuning)
        let t = Tuner { nv, o, mode, db: &db, driver: live.driver.clone(), stop };
        for (i, c) in list.iter().enumerate() {
            match t.quick(c) {
                Some(v) => set_to[i] = (Some(v), v > c.base),
                None => {
                    if c.orig != c.base {
                        let _ = offset::set(&c.r.bus, c.base);
                    }
                    set_to[i] = (Some(c.base), false);
                    pending.push(c.r.pos);
                    lg(format!("{} NOID check / tuning starts once mining runs (this GPU mines at {} until then and pauses for it; the other GPUs mine)", c.label(), signed(c.base)));
                }
            }
        }
        live.db = Some(db);
    } else {
        if o.auto {
            for c in &list {
                lg(format!("{} dry run: would tune the NOID offset from {} (base {}, start {}, limit {} {}); offset now {}", c.label(), signed(c.base), signed(c.base), signed(c.start), signed(c.max), c.max_src, signed(c.orig)));
            }
        }
        set_base(&list);
        for (i, c) in list.iter().enumerate() {
            if c.orig > 0 {
                set_to[i] = (Some(0), false);
            }
        }
    }
    let mut live_cards = Vec::new();
    for (c, (v, tuned)) in list.into_iter().zip(set_to) {
        let Some(v) = v else { continue };
        let job = pending.contains(&c.r.pos);
        if v == c.orig && !tuned && !job {
            continue; // nothing changed on this card: nothing to keep
        }
        live_cards.push(LiveCard {
            job,
            pos: c.r.pos,
            dev: c.r.dev,
            bus: c.r.bus.clone(),
            nv: Some(c.nv),
            uuid: c.uuid.clone(),
            orig: c.orig,
            base: c.base,
            step: o.step,
            tuned,
            want: v,
            owned: true,
            busy: Arc::new(AtomicBool::new(job)),
            next_read: Instant::now() + Duration::from_secs(30),
            resets: Arc::new(ResetLimit::default()),
            card: (tuned || job).then_some(c),
            pause: None,
            last: None,
        });
    }
    *live.cards.lock().unwrap() = live_cards;
    Ok(Arc::new(live))
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

impl Live {
    /// The pause handshake of the mining thread of the card at `pos` (a quick check after an OC apply pauses it).
    pub fn attach(&self, pos: usize, pause: Arc<AtomicBool>, paused: Arc<AtomicBool>) {
        if let Some(c) = self.cards.lock().unwrap().iter_mut().find(|c| c.pos == pos) {
            c.pause = Some((pause, paused));
        }
    }
}

/// Mining has started (the pause handshakes are attached): the NOID checks / tunings that need the GPU run now, one thread per
/// GPU, each pausing only the mining of its own GPU.
pub fn start_jobs(live: &Arc<Live>) {
    let jobs: Vec<(usize, Card, Option<(Arc<AtomicBool>, Arc<AtomicBool>)>)> =
        live.cards.lock().unwrap().iter_mut().filter(|c| c.job).filter_map(|c| { c.job = false; c.card.clone().map(|k| (c.pos, k, c.pause.clone())) }).collect();
    for (pos, card, pause) in jobs {
        let l2 = live.clone();
        std::thread::spawn(move || {
            let (Some(nv), Some(db)) = (l2.nv, l2.db.as_ref()) else { return };
            let t = Tuner { nv, o: &l2.o, mode: l2.mode_id, db, driver: l2.driver.clone(), stop: &l2.stop };
            if let Some((p, _)) = &pause {
                p.store(true, Ordering::SeqCst);
            }
            let t0 = Instant::now();
            while pause.as_ref().is_some_and(|(_, d)| !d.load(Ordering::SeqCst)) && t0.elapsed() < Duration::from_secs(60) && !t.stopped() {
                std::thread::sleep(Duration::from_millis(50));
            }
            let v = if t.stopped() { card.base } else {
                let _status = crate::status::Guard(card.r.dev);
                t.run_card(&card)
            };
            if let Some(c) = l2.cards.lock().unwrap().iter_mut().find(|c| c.pos == pos) {
                c.want = v;
                c.tuned = v > card.base;
                c.next_read = Instant::now() + Duration::from_secs(30);
                c.busy.store(false, Ordering::SeqCst);
            }
            if let Some((p, _)) = &pause {
                p.store(false, Ordering::SeqCst);
            }
        });
    }
}

/// A hit of card `pos` failed the CPU check: one step back (saved for a tuned card), at most once per 20 s.
pub fn on_bad_hit(live: &Live, pos: usize, bus: &str, dev: i32) {
    let mut g = live.cards.lock().unwrap();
    let Some(c) = g.iter_mut().find(|c| c.pos == pos && c.owned) else {
        eprintln!("GPU{dev}: wrong GPU results (CPU check of a hit / watchdog) at the card's own offset (no undervolt by this miner): check clocks/temperature");
        return;
    };
    if c.busy.load(Ordering::SeqCst) {
        return; // the quick check after an OC apply decides
    }
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
        Ok(()) => {
            c.want = new;
            lg(format!("GPU{dev}: wrong GPU results at offset {} (CPU check of a hit / watchdog): stepped back to {} and mining on", signed(cur), signed(new)))
        }
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

/// While mining (called from the main loop; reads every card's offset every 30 s): an offset that is no longer the one this
/// run set.  An OC apply (HiveOS nvidia-oc at a miner start / after an OC change: the profile's Core Clock value, 0 when the
/// field is empty; or the owner's offset from before) gets the verified NOID offset back after a quick check with mining on
/// that GPU paused; a base / --uv-offset value is set again at once.  At most 4 times per hour; any other value is the
/// owner's own choice: the miner stands down on that GPU and leaves it as it is.
pub fn watch(live: &Arc<Live>) {
    let Some(nv) = live.nv else { return };
    if live.stop.load(Ordering::Relaxed) {
        return;
    }
    let mut g = live.cards.lock().unwrap();
    for c in g.iter_mut() {
        if !c.owned || c.busy.load(Ordering::SeqCst) || Instant::now() < c.next_read {
            continue;
        }
        c.next_read = Instant::now() + Duration::from_secs(30);
        let Some(v) = offset::current(&c.bus) else { continue };
        if v == c.want {
            continue;
        }
        let label = format!("GPU{} (bus {})", c.dev, short_bus(&c.bus));
        let kind = c.nv.and_then(|d| hiveoc::reset_kind(nv, d, v, c.orig));
        match kind {
            Some(why) if c.resets.allow() => {
                let reapply_it = c.tuned && c.want > c.base;
                if let (true, Some(card), Some((pause, paused))) = (reapply_it, c.card.clone(), c.pause.clone()) {
                    lg(format!(
                        "{label} the NOID offset was reset to {} by an OC apply ({why}); re-applying the verified {} after a quick check (mining on this GPU pauses for ~{QUICK_SECS} s)",
                        signed(v),
                        signed(c.want)
                    ));
                    c.busy.store(true, Ordering::SeqCst);
                    let (l2, want, pos) = (live.clone(), c.want, c.pos);
                    std::thread::spawn(move || {
                        let fin = reapply(&l2, &card, want, &pause, &paused);
                        let mut g = l2.cards.lock().unwrap();
                        if let Some(c) = g.iter_mut().find(|c| c.pos == pos) {
                            if let Some(f) = fin {
                                c.want = f;
                            }
                            c.next_read = Instant::now() + Duration::from_secs(30);
                            c.busy.store(false, Ordering::SeqCst);
                        }
                    });
                } else {
                    match offset::set(&c.bus, c.want) {
                        Ok(()) => lg(format!(
                            "{label} the offset was reset to {} by an OC apply ({why}); set back to {} ({})",
                            signed(v),
                            signed(c.want),
                            if c.tuned {
                                "the verified NOID offset"
                            } else if c.want == c.base {
                                "the NOID base: Pearl undervolt offsets give silently wrong NOID results"
                            } else {
                                "--uv-offset"
                            }
                        )),
                        Err(e) => {
                            lg(format!("{label} the offset was reset to {} by an OC apply ({why}); cannot set {} again ({e:#}): left as it is", signed(v), signed(c.want)));
                            c.owned = false;
                        }
                    }
                }
            }
            Some(why) => {
                lg(format!(
                    "{label} the offset was reset to {} by an OC apply ({why}) again: {} times within an hour already, so the miner stands down on this GPU and leaves it as is (restart the miner once the OC is settled)",
                    signed(v),
                    hiveoc::OC_RESETS_MAX
                ));
                c.owned = false;
            }
            None => {
                lg(format!(
                    "{label} the offset was changed by someone else ({} now, {} expected): the miner stands down on this GPU and leaves it as is{}",
                    signed(v),
                    signed(c.want),
                    if v > 0 { " (NOID may compute wrong results at a positive offset: the watchdog reports them)" } else { "" }
                ));
                c.owned = false;
            }
        }
    }
}

/// The quick check after an OC apply reset the offset of a tuned card while mining: mining on the GPU pauses, `want` is
/// checked (stress, QUICK_SECS); a failure steps back one step (saved) and checks again.  Returns the offset the card mines at
/// (None: a stop, nothing decided).
fn reapply(live: &Live, c: &Card, want: i32, pause: &AtomicBool, paused: &AtomicBool) -> Option<i32> {
    let (Some(nv), Some(db)) = (live.nv, live.db.as_ref()) else { return None };
    let t = Tuner { nv, o: &live.o, mode: live.mode_id, db, driver: live.driver.clone(), stop: &live.stop };
    pause.store(true, Ordering::SeqCst);
    let t0 = Instant::now();
    while !paused.load(Ordering::SeqCst) && t0.elapsed() < Duration::from_secs(60) && !t.stopped() {
        std::thread::sleep(Duration::from_millis(50));
    }
    let fin = if t.stopped() {
        None
    } else if !paused.load(Ordering::SeqCst) {
        lg(format!("{} quick check after the OC apply skipped (the mining thread did not pause): {} set again without it", c.label(), signed(want)));
        let _ = offset::set(&c.r.bus, want);
        Some(want)
    } else {
        let step = t.o.step.max(1);
        let mut at = want;
        loop {
            let r = t.check(c, at, QUICK_SECS);
            if t.stopped() {
                break None; // a stop is not a failure of the offset (the shutdown puts the owner's back)
            }
            if r.infra {
                lg(format!("{} the quick check after the OC apply could not run ({}): mining at the base {}", c.label(), r.reason, signed(c.base)));
                let _ = offset::set(&c.r.bus, c.base);
                break Some(c.base);
            }
            if r.ok {
                lg(format!("{} quick check after the OC apply at {}: ok ({})", c.label(), signed(at), r.brief()));
                if let Some(mut e) = db.entry(&c.uuid, live.mode) {
                    if e.offset == at {
                        e.verified_at = now();
                        db.save(&c.uuid, live.mode, &e);
                    }
                }
                break Some(at);
            }
            let new = (at - step).max(c.base);
            lg(format!("{} quick check after the OC apply at {} FAILED ({}): stepped back to {}", c.label(), signed(at), r.reason, signed(new)));
            if let Some(mut e) = db.entry(&c.uuid, live.mode) {
                e.offset = new;
                e.stepped_back += 1;
                db.save(&c.uuid, live.mode, &e);
            }
            if r.fault || new <= c.base {
                let _ = offset::set(&c.r.bus, new);
                break Some(new);
            }
            at = new;
        }
    };
    pause.store(false, Ordering::SeqCst);
    fin
}
