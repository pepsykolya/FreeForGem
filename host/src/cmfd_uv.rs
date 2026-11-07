// FreeForGem — see LICENSE
//! `--auto-uv` in CMFD mode on NVIDIA: an undervolt (core V/F offset) tuned on the CMFD load itself.
//!
//! A Pearl-stable offset is not stable for CMFD (its int8 tensor-core forward loads the GPU much harder), so CMFD gets its own
//! result per card, found and checked with the CMFD engine's own GPU worker (`cmfd-v4-replay --server MODEL`, the FreeForgeMiner
//! release binary, unchanged) on the real 6.4 GB model:
//!   * check = two fixed batches (fixed coefficients and challenges, the mining batch size) run alternately through
//!     `RUNBATCHDIGEST`; the final digests (BLAKE3 over all 128 x 4096 final activations of every nonce, 384 layers) of every
//!     run must be bit-identical to the first run of that batch and to the reference taken at the base offset (0, stock);
//!     a single wrong bit anywhere in a forward changes the digest. Then a measurement phase (same load) for FW/s and watts;
//!     at the start, at the final point and on a later start's quick check also the official Common Foundry 1.0.16 test
//!     vector (mainnet template, 32 nonces, SHA-256 of the final activations).
//!   * tune: base (0) -> base + step -> ... up to the card's limit (`--uv-max` / the 1.6.2 per-card default, `--uv-min` as the
//!     first step), first failure = edge, one step back (margin; `--uv-keep-last` keeps the last good one), long confirmation
//!     + test vector (one more step down on failure).
//!   * the result goes to uv.json under "cmfd" (the Pearl result under "gpus" is not touched), keyed by the card UUID, with the
//!     driver and the engine identity (engine version, worker hash, mode, batch); a later start applies it (quick check unless
//!     it was verified within --uv-reval-hours).
//!   * the offsets are changed only through cmfd_offset (its marker and restore put the owner's offset back on exit / crash).
//!   * while mining, an engine `INTEGRITY | ERROR` or CUDA fault on a tuned card steps its offset back (and saves it).
//!   * while mining, the offset of a tuned card is read back every 30 s: an OC apply (HiveOS nvidia-oc at a miner start or
//!     after an OC change in the web UI: the profile's Core Clock value, 0 when the field is empty; other tools: the owner's
//!     offset) gets the saved CMFD offset back after a quick check (the engine of that GPU stops for it and restarts after;
//!     a failure steps back), at most 4 times per hour; any other value is the owner's choice and the tuner stands down on
//!     that GPU.  An OC apply during a check makes the check repeat at the right offset.
//!   * no worker, no model, a failed check at the base offset: the card mines at the base offset with a warning.
use crate::climb;
use crate::cmfd;
use crate::hiveoc::ResetLimit;
use crate::monitor::{self, reval_interval};
use crate::nvml::{Dev, Nvml};
use crate::profiles::{self, Profile, Store};
use crate::uv::{self, DISABLE_SECS, HARD_CAP, Saved, fmt_age, fmt_w, now, saved_vs_limit, short_bus, short_name, signed};
use crate::{cmfd_offset, gpu};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

const SECTION: &str = "cmfd"; // before 1.7.2: one result per GPU
const PROFILES: &str = "cmfd_profiles"; // 1.7.2: uuid -> core lock / clock key -> result
/// version of the fixed check inputs (coefficients/challenges); part of the identity, so saved references always match them
const INPUTS: &str = "in1";
const READY_SECS: u64 = 600; // model load (6.4 GB, from disk on a cold cache / slow USB stick)
const BATCH_SECS: u64 = 240; // one batch of 64 on the slowest supported card is ~15 s; longer = hung GPU

fn lg(m: impl AsRef<str>) {
    println!("auto-uv (cmfd): {}", m.as_ref());
}

/// What the tuner needs from cmfd.rs.
#[derive(Clone)]
pub struct Ctx {
    pub engine: PathBuf,
    pub bank: PathBuf,
    pub mode: String,
    pub batch: Option<u32>,
    pub work: PathBuf, // per-GPU scratch root (small files)
    pub core_lock: Vec<Option<u32>>, // --lock-cclock per device position (part of the identity: the edge depends on it)
}

#[derive(Clone, Default, Debug)]
struct Entry {
    name: String,
    bus: String,
    driver: String,
    kernel: String, // engine identity: version, worker hash, mode, batch, inputs version
    orig: i32,      // the owner's offset when tuned (for the log; the base decides)
    base: i32,      // offset the references were taken at (0, or the owner's negative offset)
    offset: i32,
    cap: i32,
    floor: bool, // 1.7.7: the climb stopped at the voltage floor: a higher limit does not re-tune it
    step: i32,
    refs: Vec<String>,
    fws_before: f64,
    fws_after: f64,
    w_before: f64,
    w_after: f64,
    clk_before: u32,
    clk_after: u32,
    pl: f64, // enforced power limit (W) when tuned: the edge moves with the operating point
    lock: u32,      // core lock this miner set (--lock-cclock) when tuned; 0 = not set by the miner (keyed by clk_before)
    migrated: bool, // from before 1.7.2 without a lock: used only after the quick check
    tested_at: u64,
    verified_at: u64,
    disabled_until: u64,
}

impl Entry {
    fn to_json(&self) -> Value {
        let mut v = json!({"name": self.name, "bus": self.bus, "driver": self.driver, "kernel": self.kernel, "orig_offset": self.orig,
               "base_offset": self.base, "offset": self.offset, "cap": self.cap, "step": self.step, "refs": self.refs,
               "fws_before": self.fws_before, "fws_after": self.fws_after, "watts_before": self.w_before, "watts_after": self.w_after,
               "clk_before": self.clk_before, "clk_after": self.clk_after, "power_limit": self.pl, "tested_at": self.tested_at,
               "verified_at": self.verified_at, "disabled_until": self.disabled_until, "core_lock": self.lock, "migrated": self.migrated});
        if self.floor {
            v["floor"] = json!(true); // written only when set: other versions' files stay as they were
        }
        v
    }

    fn from_json(v: &Value) -> Option<Entry> {
        let f = |k: &str| v[k].as_f64().unwrap_or(0.0);
        let u = |k: &str| v[k].as_u64().unwrap_or(0);
        Some(Entry {
            name: v["name"].as_str().unwrap_or("").into(),
            bus: v["bus"].as_str().unwrap_or("").into(),
            driver: v["driver"].as_str()?.into(),
            kernel: v["kernel"].as_str()?.into(),
            orig: v["orig_offset"].as_i64().unwrap_or(0) as i32,
            base: v["base_offset"].as_i64()? as i32,
            offset: v["offset"].as_i64()? as i32,
            cap: v["cap"].as_i64().unwrap_or(0) as i32,
            floor: v["floor"].as_bool().unwrap_or(false),
            step: v["step"].as_i64().unwrap_or(0) as i32,
            refs: v["refs"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default(),
            fws_before: f("fws_before"),
            fws_after: f("fws_after"),
            w_before: f("watts_before"),
            w_after: f("watts_after"),
            clk_before: u("clk_before") as u32,
            clk_after: u("clk_after") as u32,
            pl: f("power_limit"),
            lock: u("core_lock") as u32,
            migrated: v["migrated"].as_bool().unwrap_or(false),
            tested_at: u("tested_at"),
            verified_at: u("verified_at"),
            disabled_until: u("disabled_until"),
        })
    }
}

impl Profile for Entry {
    fn from_json(v: &Value) -> Option<Entry> {
        Entry::from_json(v)
    }
    fn to_json(&self) -> Value {
        Entry::to_json(self)
    }
    fn same_clocks(&self, o: &Entry) -> bool {
        if o.lock != 0 && self.lock != 0 { o.lock == self.lock } else { near(clk_of(o), clk_of(self)) }
    }
}

/// uv.json, shared with the Pearl tuner (other sections are kept as they are): the shared store (profiles.rs) with this tuner's
/// sections ("cmfd_profiles", "cmfd_trying").
struct Db {
    dir: PathBuf,
    s: Store,
}

/// Key of a CMFD profile: the core lock set by the miner ("1805"), else the clock under load at the base ("~1650").
fn pkey(e: &Entry) -> String {
    if e.lock != 0 { e.lock.to_string() } else { format!("~{}", e.clk_before) }
}

fn clk_of(e: &Entry) -> u32 {
    if e.lock != 0 { e.lock + 10 } else { e.clk_before }
}

fn near(a: u32, b: u32) -> bool {
    a != 0 && b != 0 && (a as i64 - b as i64).abs() <= (a as i64 * 3 / 100).max(45) + 15
}

/// Does the profile fit the lock of this run (`lock`), or (lock unknown) the base clock `clk` under load?
fn fits(e: &Entry, lock: Option<u32>, clk: Option<u32>) -> bool {
    match (lock, clk) {
        (Some(l), _) => if e.lock != 0 { e.lock == l } else { near(e.clk_before, l + 10) },
        (None, Some(k)) => near(clk_of(e), k),
        _ => false,
    }
}

/// The engine identity of 1.7.1 carried the lock (" | lock-cclock 1805"); since 1.7.2 the lock is the profile key.
fn split_lock(kernel: &str) -> (String, u32) {
    match kernel.rsplit_once(" | lock-cclock ") {
        Some((k, l)) => (k.to_string(), l.trim().parse().unwrap_or(0)),
        None => (kernel.to_string(), 0),
    }
}

/// Before 1.7.2: one result per GPU ("cmfd"); the lock (if any) was part of the engine identity.  Moves them into the profiles.
fn migrate(v: &mut Value) -> usize {
    let mut n = 0;
    if let Some(old) = v[SECTION].as_object().cloned() {
        for (uuid, x) in old {
            let Some(mut e) = Entry::from_json(&x) else { continue };
            let (k, l) = split_lock(&e.kernel);
            e.kernel = k;
            e.lock = l;
            if l == 0 {
                if e.clk_before == 0 {
                    continue;
                }
                e.migrated = true;
                e.verified_at = 0;
            }
            if !v[PROFILES].is_object() {
                v[PROFILES] = json!({});
            }
            if !v[PROFILES][&uuid].is_object() {
                v[PROFILES][&uuid] = json!({});
            }
            v[PROFILES][&uuid][pkey(&e)] = e.to_json();
            n += 1;
        }
        if let Some(m) = v.as_object_mut() {
            m.remove(SECTION);
        }
        if n > 0 {
            lg(format!("{n} CMFD result(s) from before 1.7.2 kept as profiles of their lock / clock (one without a lock is used only after the quick check)"));
        }
    }
    n
}

fn log_line(m: &str) {
    lg(m);
}

impl Db {
    fn load(dir: &Path) -> Db {
        Db { dir: dir.to_path_buf(), s: Store::load(dir, profiles::CMFD, log_line, migrate) }
    }

    fn profiles(&self, uuid: &str) -> Vec<(String, Entry)> {
        self.s.list(uuid, "")
    }

    fn line(&self, uuid: &str) -> String {
        let p = self.profiles(uuid);
        if p.is_empty() {
            return "none".into();
        }
        p.iter().map(|(k, e)| format!("{k}:{}{}", signed(e.offset), if e.disabled_until > now() { " (paused)" } else { "" })).collect::<Vec<_>>().join(", ")
    }

    fn use_key(&self, uuid: &str, key: &str) {
        self.s.use_key(uuid, key);
    }

    /// The saved profiles of the card for `climb::hint_start`.
    fn hints(&self, uuid: &str, base: i32) -> Vec<climb::HintProfile> {
        self.s.hints::<Entry>(uuid, "", |k, e| {
            Some(climb::HintProfile {
                clk: clk_of(e),
                mlock: 0,
                offset: e.offset,
                usable: !e.migrated && e.verified_at > 0 && e.disabled_until <= now() && e.offset > base,
                label: k.to_string(),
                verified_at: e.verified_at,
            })
        })
    }

    /// The saved results of the card for `climb::prior_start` (1.7.7): any driver / kernel, checked ones and those from before 1.7.2.
    fn priors(&self, uuid: &str, base: i32) -> Vec<climb::PriorProfile> {
        self.s.list::<Entry>(uuid, "").into_iter().map(|(_, e)| climb::PriorProfile {
            clk: clk_of(&e),
            mlock: 0,
            offset: e.offset,
            at: e.verified_at.max(e.tested_at),
            who: format!("driver {}, kernel {}", e.driver, e.kernel),
            usable: (e.verified_at > 0 || e.migrated) && e.disabled_until <= now() && e.offset > base,
        }).collect()
    }

    fn forget(&self, uuid: &str, key: &str) {
        self.s.forget(uuid, &[key.to_string()]);
    }

    /// The profile in use on this card (chosen at start).
    fn entry(&self, uuid: &str) -> Option<Entry> {
        self.s.current(uuid)
    }

    /// Saves `e` as the profile of its clocks (the profile of the same clocks is replaced, the others stay).
    fn save(&self, uuid: &str, e: &Entry) {
        self.s.save(uuid, "", &pkey(e), e);
    }

    // crash marker: the offset under check, written before the check and removed after it (a driver crash or a killed miner
    // during a check leaves it, and the next start tunes below it); "good": the highest step that passed before (set_good)
    fn mark_trying(&self, uuid: &str, at: i32) {
        self.s.mark_trying(uuid, at);
    }

    fn clear_trying(&self, uuid: &str) {
        self.s.clear_trying(uuid);
    }

    fn take_trying(&self, uuid: &str) -> Option<(i32, i32)> {
        self.s.take_trying(uuid).map(|k| (k.at, k.good.unwrap_or(i32::MIN)))
    }
}

#[derive(Clone)]
struct Card {
    dev: i32,
    name: String,
    bus: String,
    uuid: String,
    nv: Dev,
    orig: i32, // the owner's offset (restored on exit by cmfd_offset)
    base: i32, // offset at the start of the CMFD run: 0, or the owner's offset when it is 0 or below
    hi: i32,
    max: i32,
    max_src: &'static str,
    min: Option<i32>,
    batch: u32,
    kernel: String,
    pl: f64, // enforced power limit now (0 = unknown)
    locked: bool, // --lock-cclock of this run is set on it; false: the lock is not known to the miner
    lock: u32,    // that lock (0 = none)
    lock_hint: Option<u32>, // the lock the card will have: --lock-cclock, else the HiveOS profile's (the start by the lock)
    def: uv::Defaults,      // the card type's built-in limits and start floor
}

impl Card {
    fn label(&self) -> String {
        format!("GPU{} (bus {}, {})", self.dev, short_bus(&self.bus), short_name(&self.name))
    }

    /// First offset of the search (before a hint from another profile): --uv-min, the card type's start by the core lock,
    /// the generation floor, else one step above the base.
    fn start_offset(&self, step: i32) -> (i32, &'static str) {
        start_of(&self.name, &self.def, self.lock_hint, self.min, self.max, self.base, step)
    }
}

fn start_of(name: &str, def: &uv::Defaults, lock_hint: Option<u32>, min: Option<i32>, max: i32, base: i32, step: i32) -> (i32, &'static str) {
    let (v, src) = uv::search_start(uv::Coin::Cmfd, name, def, lock_hint, min, max, (base + step, "one step above the base"));
    (v.max(base + step), src)
}

/// A card that mines at a tuned offset: stepped back when the engine reports wrong results; its offset is watched for OC applies.
struct Live {
    dev: i32,
    bus: String,
    uuid: String,
    base: i32,
    step: i32,
    last: Option<Instant>,
    owned: bool, // false: someone else changed the offset (or OC applies kept resetting it): left alone
    busy: bool,  // the quick check after an OC apply / the periodic re-validation runs (the engine of this GPU is stopped meanwhile)
    next_read: Instant,
    next_reval: Instant, // 1.7.6: the next periodic re-validation (--uv-reval-hours)
    what: &'static str,  // why the check runs
    resets: ResetLimit,
    card: Card,
}

struct Shared {
    db: Db,
    live: Mutex<Vec<Live>>,
    faulted: Mutex<Vec<String>>, // UUIDs whose check ended in a driver fault: their crash marker must survive a stop
    ctx: Ctx,
    o: uv::Opts,
    stop: Arc<AtomicBool>,
    driver: String,
}

/// At a stop (second Ctrl-C / SIGHUP of HiveOS miner stop while a check runs, console close): the crash markers of interrupted
/// checks are removed, so the next start does not take the offset under check for a crash point; driver faults keep theirs.
pub fn forget_trying_on_stop() {
    let sh = shared().lock().unwrap().clone();
    if let Some(sh) = sh {
        let faulted = sh.faulted.lock().unwrap().clone();
        let ids: Vec<String> = sh.db.s.trying_ids();
        for id in ids.iter().filter(|id| !faulted.contains(id)) {
            sh.db.clear_trying(id);
        }
    }
}

fn shared() -> &'static Mutex<Option<Arc<Shared>>> {
    static S: OnceLock<Mutex<Option<Arc<Shared>>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(None))
}

/// Batch the engine uses on this card (the same rule as FreeForgeMiner: 64 from 11 GB of GPU memory, else 32), or --cmfd-batch.
fn mining_batch(uuid: &str, given: Option<u32>) -> u32 {
    if let Some(b) = given {
        return b;
    }
    let total = std::process::Command::new("nvidia-smi")
        .args(["--query-gpu=memory.total", "--format=csv,noheader,nounits", "-i", uuid])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|t| t.trim().parse::<u64>().ok())
        .unwrap_or(0);
    if total >= 11_000 { 64 } else { 32 }
}

fn file_tag(p: &Path) -> String {
    cmfd::sha256_hex(p).map(|h| h[..12].to_string()).unwrap_or_else(|_| "?".into())
}

/// Fixed check inputs: batch `s` of `n` nonces, deterministic bytes (splitmix64), never random.
fn fixed_bytes(len: usize, seed: u64) -> Vec<u8> {
    let mut x = seed;
    let mut out = Vec::with_capacity(len + 8);
    while out.len() < len {
        x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = x;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        out.extend_from_slice(&(z ^ (z >> 31)).to_le_bytes());
    }
    out.truncate(len);
    out
}

// ---------------------------------------------------------------------------------------------------------------------
// the GPU worker (`cmfd-v4-replay --server MODEL`): one per card for the whole tuning, the model is loaded once
// ---------------------------------------------------------------------------------------------------------------------

struct Worker {
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<String>,
    dir: PathBuf,
    batch: u32,
    tail: Vec<String>,
}

/// What stops a wait for the worker: Ok(None) = go on, Ok(Some(reason)) = abort (Xid, stop).
type Tick<'a> = &'a mut dyn FnMut() -> Option<(String, bool)>;

#[derive(Debug)]
enum WErr {
    Lost(String),          // the worker exited / cannot start / protocol error
    Hang,                  // no answer within BATCH_SECS
    Abort(String, bool),   // tick abort: reason, driver fault
}

impl std::fmt::Display for WErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WErr::Lost(s) => write!(f, "GPU worker ended: {s}"),
            WErr::Hang => write!(f, "timeout (GPU hang?)"),
            WErr::Abort(s, _) => write!(f, "{s}"),
        }
    }
}

// model loads one card at a time: several 6.4 GB reads at once only thrash the page cache (3.8 GB rigs)
static LOAD: Mutex<()> = Mutex::new(());

impl Worker {
    fn start(ctx: &Ctx, c: &Card, tick: Tick) -> std::result::Result<Worker, WErr> {
        let dir = ctx.work.join(&c.uuid).join("uvcheck");
        let prep = || -> Result<()> {
            std::fs::create_dir_all(&dir)?;
            let per_nonce = 385 * 20;
            for s in 0..2u64 {
                let coef = fixed_bytes(c.batch as usize * per_nonce, 0x434d_4644_0000_0000 ^ (s * 2 + 1));
                std::fs::write(dir.join(format!("coef{s}.bin")), coef)?;
                let chal = fixed_bytes(c.batch as usize * 32, 0x434d_4644_0000_0000 ^ (s * 2 + 2));
                std::fs::write(dir.join(format!("chal{s}.bin")), chal)?;
            }
            Ok(())
        };
        prep().map_err(|e| WErr::Lost(format!("cannot write the check inputs in {} ({e:#})", dir.display())))?;
        let _load = LOAD.lock().unwrap_or_else(|p| p.into_inner());
        let mut child = cmfd::engine_cmd(&ctx.engine, "cmfd-v4-replay", Some(&c.uuid), &ctx.mode)
            .arg("--server")
            .arg(&ctx.bank)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| WErr::Lost(format!("cannot start {} ({e})", cmfd::bin(cmfd::replay_worker()))))?;
        if let Ok(mut v) = cmfd::CHILDREN.lock() {
            v.push(child.id());
        }
        crate::faults::oom_score_adj(Some(child.id()), 900); // the OOM killer takes a check worker before the miner
        let (tx, rx) = channel::<String>();
        for r in [Box::new(child.stdout.take().unwrap()) as Box<dyn Read + Send>, Box::new(child.stderr.take().unwrap())] {
            let tx = tx.clone();
            std::thread::spawn(move || {
                for l in BufReader::new(r).lines().map_while(|l| l.ok()) {
                    if tx.send(l).is_err() {
                        break;
                    }
                }
            });
        }
        let stdin = child.stdin.take().unwrap();
        let mut w = Worker { child, stdin, rx, dir, batch: c.batch, tail: Vec::new() };
        match w.wait_for("CMFD_V4_REPLAY_READY", READY_SECS, tick) {
            Ok(()) => Ok(w),
            Err(e) => {
                w.kill();
                Err(e)
            }
        }
    }

    fn wait_for(&mut self, tag: &str, secs: u64, tick: Tick) -> std::result::Result<(), WErr> {
        let end = Instant::now() + Duration::from_secs(secs);
        loop {
            if let Some((why, fault)) = tick() {
                return Err(WErr::Abort(why, fault));
            }
            match self.rx.recv_timeout(Duration::from_millis(250)) {
                Ok(l) if l.contains(tag) => return Ok(()),
                Ok(l) => {
                    let err = l.starts_with("error");
                    self.tail.push(l);
                    if self.tail.len() > 6 {
                        self.tail.remove(0);
                    }
                    if err {
                        return Err(WErr::Lost(self.tail.last().cloned().unwrap_or_default()));
                    }
                }
                Err(RecvTimeoutError::Disconnected) => {
                    let st = self.child.wait().map(|s| s.to_string()).unwrap_or_default();
                    return Err(WErr::Lost(format!("{st}{}", if self.tail.is_empty() { String::new() } else { format!(": {}", self.tail.join(" | ")) })));
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
            if Instant::now() >= end {
                return Err(WErr::Hang);
            }
        }
    }

    fn send(&mut self, line: &str) -> std::result::Result<(), WErr> {
        writeln!(self.stdin, "{line}").and_then(|_| self.stdin.flush()).map_err(|e| WErr::Lost(format!("stdin: {e}")))
    }

    /// One fixed batch: the 64-bit-folded BLAKE3 of all its final digests (hex).
    fn digest(&mut self, s: usize, tick: Tick) -> std::result::Result<String, WErr> {
        let (coef, chal, out) = (self.dir.join(format!("coef{s}.bin")), self.dir.join(format!("chal{s}.bin")), self.dir.join(format!("run{s}")));
        let file = PathBuf::from(format!("{}-final-digest.bin", out.display()));
        let _ = std::fs::remove_file(&file);
        self.send(&format!("RUNBATCHDIGEST\t{}\t{}\t{}\t{}", self.batch, coef.display(), out.display(), chal.display()))?;
        self.wait_for("CMFD_V4_REPLAY_DONE", BATCH_SECS, tick)?;
        let b = std::fs::read(&file).map_err(|e| WErr::Lost(format!("read {}: {e}", file.display())))?;
        if b.len() != self.batch as usize * 32 {
            return Err(WErr::Lost(format!("{} has {} bytes, expected {}", file.display(), b.len(), self.batch * 32)));
        }
        Ok(blake3::hash(&b).to_hex()[..32].to_string())
    }

    /// Official Common Foundry 1.0.16 test vector: SHA-256 of the final activations of the mainnet template, 32 nonces.
    fn test_vector(&mut self, engine: &Path, tick: Tick) -> std::result::Result<String, WErr> {
        let coef = engine.join(cmfd::REF_COEFFICIENTS);
        let out = self.dir.join("refvec");
        let file = PathBuf::from(format!("{}-final-activation.bin", out.display()));
        let _ = std::fs::remove_file(&file);
        self.send(&format!("RUNBATCH\t32\t{}\t{}", coef.display(), out.display()))?;
        self.wait_for("CMFD_V4_REPLAY_DONE", BATCH_SECS, tick)?;
        let h = cmfd::sha256_hex(&file).map_err(|e| WErr::Lost(format!("{e:#}")));
        let _ = std::fs::remove_file(&file);
        h
    }

    fn kill(&mut self) {
        let _ = writeln!(self.stdin, "QUIT");
        let _ = self.stdin.flush();
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(10) {
            if let Ok(Some(_)) = self.child.try_wait() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Ok(mut v) = cmfd::CHILDREN.lock() {
            v.retain(|&p| p != self.child.id());
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.kill();
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// one check
// ---------------------------------------------------------------------------------------------------------------------

struct Spec {
    secs: u32,
    measure: u32,
    tv: bool,
}

/// Watts scaled to the work of the base check (W x base FW/s / FW/s); unscaled when a rate is missing.
fn same_work_w(w: Option<f64>, fws: Option<f64>, fws0: Option<f64>) -> Option<f64> {
    match (w, fws, fws0) {
        (Some(w), Some(f), Some(f0)) if f > 0.0 && f0 > 0.0 => Some(w * f0 / f),
        _ => w,
    }
}

#[derive(Default, Debug)]
struct Res {
    ok: bool,
    reason: String,
    driver_fault: bool,
    unavail: bool, // the worker could not open the GPU (busy / unavailable): says nothing about the offset (1.7.6)
    runs: u32,
    refs: Vec<String>,
    fws: Option<f64>,
    watts: Option<f64>,
    clk: Option<u32>,
    temp: Option<u32>,
}

impl Res {
    fn brief(&self) -> String {
        format!(
            "{} runs, {}, {}, {} MHz, {} C",
            self.runs,
            self.fws.map_or("? FW/s".into(), |f| format!("{f:.2} FW/s")),
            fmt_w(self.watts),
            self.clk.unwrap_or(0),
            self.temp.unwrap_or(0)
        )
    }
}

struct Tuner<'a> {
    nv: &'a Nvml,
    ctx: &'a Ctx,
    o: &'a uv::Opts,
    sh: Arc<Shared>,
    driver: String,
    stop: &'a Arc<AtomicBool>,
}

/// Base check length: the reference digests and the test vector need only a few runs; the Pearl default warm-up (180 s) is
/// not needed here, so CMFD takes 60 s unless --uv-warmup was given.
fn warmup_secs(o: &uv::Opts) -> u32 {
    if o.warmup_secs == uv::Opts::default().warmup_secs { 60 } else { o.warmup_secs }
}

impl Tuner<'_> {
    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    fn set(&self, c: &Card, v: i32) -> Result<()> {
        ensure!(v <= c.hi && v <= HARD_CAP, "offset {v} above the limit");
        cmfd_offset::set(&c.bus, v)
    }

    fn cur(c: &Card) -> i32 {
        cmfd_offset::current(&c.bus).unwrap_or(c.base)
    }

    /// Runs one check on the card's worker.  An OC apply that resets the offset during the check (HiveOS nvidia-oc at a miner
    /// start / from the web UI) makes the check say nothing about the offset under check: it is set again and the check
    /// repeated (twice at most); any other value there is a failed check.
    fn check(&self, c: &Card, w: &mut Option<Worker>, sp: &Spec, refs: &[String]) -> Res {
        // the stats line / summary show what this GPU does (status.rs); a single check clears its state after
        let tuning = crate::status::is_tuning(c.dev);
        if tuning {
            crate::status::set(c.dev, format!("CMFD tuning {}", signed(Self::cur(c))), true, None);
        } else {
            crate::status::set(c.dev, format!("CMFD check {}", signed(Self::cur(c))), false, Some(Instant::now() + Duration::from_secs(sp.secs as u64 + sp.measure as u64 + 90)));
        }
        let r = self.check_states(c, w, sp, refs);
        if !tuning {
            crate::status::clear(c.dev);
        }
        r
    }

    fn check_states(&self, c: &Card, w: &mut Option<Worker>, sp: &Spec, refs: &[String]) -> Res {
        let cur = Self::cur(c);
        let mut r = Res::default();
        let mut unavail = 0u32;
        let mut attempt = 0;
        while attempt < 3 {
            r = self.check_once(c, w, sp, refs);
            if r.unavail && !self.stopped() {
                // a GPU that cannot be opened: wait and repeat the same check (1.7.6, ffgs)
                monitor::wait_unavailable(unavail, &|| self.stopped(), &c.label(), &signed(cur), &r.reason, c.dev);
                unavail += 1;
                continue;
            }
            if unavail > 0 && r.ok {
                lg(format!("{} can be opened again after {unavail} attempt(s): the check at {} went on", c.label(), signed(cur)));
            }
            attempt += 1;
            let attempt = attempt - 1;
            if r.driver_fault || r.unavail || self.stopped() {
                break;
            }
            let Some(v) = cmfd_offset::read(&c.bus).filter(|&v| v != cur) else { break };
            match cmfd_offset::reset_kind(&c.bus, v) {
                Some(why) if attempt < 2 => {
                    lg(format!("{} the offset was reset to {} during the check ({why}): setting {} again and repeating the check", c.label(), signed(v), signed(cur)));
                    if let Err(e) = self.set(c, cur) {
                        r = Res { reason: format!("could not set {} again after the OC apply: {e:#}", signed(cur)), ..Default::default() };
                        break;
                    }
                    std::thread::sleep(Duration::from_secs(3));
                }
                _ => {
                    r.ok = false;
                    r.reason = format!("the offset was changed to {} by someone else during the check ({} expected)", signed(v), signed(cur));
                    break;
                }
            }
        }
        r
    }

    /// Runs one check on the card's worker (started if needed, dropped on any worker error).
    fn check_once(&self, c: &Card, w: &mut Option<Worker>, sp: &Spec, refs: &[String]) -> Res {
        let cur = Self::cur(c);
        if cur != c.base {
            self.sh.db.mark_trying(&c.uuid, cur);
        }
        let mut samples: Vec<(f64, u32, u32)> = Vec::new();
        let sampling = AtomicBool::new(false);
        let mut last = Instant::now();
        let mut tick = || -> Option<(String, bool)> {
            if self.stopped() {
                return Some(("interrupted".into(), false));
            }
            if let Some(x) = self.nv.take_xids(c.nv).first() {
                return Some((format!("Xid {x} reported by the driver"), true));
            }
            if sampling.load(Ordering::Relaxed) && last.elapsed() >= Duration::from_millis(250) {
                last = Instant::now();
                if let Some(p) = self.nv.power_w(c.nv) {
                    samples.push((p, self.nv.clock_mhz(c.nv).unwrap_or(0), self.nv.temp_c(c.nv).unwrap_or(0)));
                }
            }
            None
        };
        let mut res = Res::default();
        let mut first: [Option<String>; 2] = [None, None];
        let r: std::result::Result<(), WErr> = (|| {
            if w.is_none() {
                *w = Some(Worker::start(self.ctx, c, &mut tick)?);
            }
            let wk = w.as_mut().unwrap();
            let t0 = Instant::now();
            let mut i = 0usize;
            while t0.elapsed() < Duration::from_secs(sp.secs as u64) || i < 4 {
                let s = i & 1;
                let h = wk.digest(s, &mut tick)?;
                res.runs += 1;
                match &first[s] {
                    None => first[s] = Some(h.clone()),
                    Some(f) if *f == h => {}
                    Some(f) => return Err(WErr::Abort(format!("rerun_diff: digest of batch {s} changed between reruns ({f} -> {h}) after {} runs", res.runs), false)),
                }
                if let Some(r) = refs.get(s).filter(|r| !r.is_empty()) {
                    if *r != h {
                        return Err(WErr::Abort(format!("digest: batch {s} differs from the reference taken at the base offset ({}.. -> {}..)", &r[..12], &h[..12]), false));
                    }
                }
                i += 1;
            }
            if sp.measure > 0 {
                sampling.store(true, Ordering::Relaxed);
                let m0 = Instant::now();
                let mut n = 0u32;
                while m0.elapsed() < Duration::from_secs(sp.measure as u64) || n < 2 {
                    let s = (n & 1) as usize;
                    let h = wk.digest(s, &mut tick)?;
                    res.runs += 1;
                    if first[s].as_ref() != Some(&h) {
                        return Err(WErr::Abort(format!("rerun_diff: digest of batch {s} changed during the measurement"), false));
                    }
                    n += 1;
                }
                res.fws = Some((n * wk.batch) as f64 / m0.elapsed().as_secs_f64());
                sampling.store(false, Ordering::Relaxed);
            }
            if sp.tv {
                let h = wk.test_vector(&self.ctx.engine, &mut tick)?;
                if h != cmfd::REF_SHA256 {
                    return Err(WErr::Abort(format!("official test vector: sha256 {}.., expected {}..", &h[..16], &cmfd::REF_SHA256[..16]), false));
                }
            }
            Ok(())
        })();
        match r {
            Ok(()) => res.ok = true,
            Err(e) => {
                res.reason = e.to_string();
                res.driver_fault = matches!(e, WErr::Hang | WErr::Abort(_, true));
                res.unavail = matches!(&e, WErr::Lost(m) if monitor::gpu_unavailable(m));
                if !matches!(e, WErr::Abort(_, false)) || res.reason.starts_with("interrupted") {
                    // the worker is gone or in an unknown state: a new one (model reload) for the next check
                    *w = None;
                }
            }
        }
        res.refs = first.iter().map(|f| f.clone().unwrap_or_default()).collect();
        if samples.len() >= 4 {
            let part = &samples[samples.len() / 2..];
            res.watts = Some(part.iter().map(|s| s.0).sum::<f64>() / part.len() as f64);
            let mut clks: Vec<u32> = part.iter().map(|s| s.1).collect();
            clks.sort_unstable();
            res.clk = Some(clks[clks.len() / 2]);
            res.temp = part.iter().map(|s| s.2).max();
        }
        if !res.driver_fault {
            self.sh.db.clear_trying(&c.uuid);
        } else {
            self.sh.faulted.lock().unwrap().push(c.uuid.clone());
        }
        res
    }

    fn new_entry(&self, c: &Card) -> Entry {
        Entry { name: c.name.clone(), bus: c.bus.clone(), driver: self.driver.clone(), kernel: c.kernel.clone(), orig: c.orig, base: c.base, step: self.o.step, lock: c.lock, ..Default::default() }
    }

    fn go_live(&self, c: &Card) {
        if Self::cur(c) > c.base {
            self.sh.live.lock().unwrap().push(Live {
                dev: c.dev,
                bus: c.bus.clone(),
                uuid: c.uuid.clone(),
                base: c.base,
                step: self.o.step,
                last: None,
                owned: true,
                busy: false,
                next_read: Instant::now() + Duration::from_secs(30),
                next_reval: Instant::now() + reval_interval(self.o.reval_hours),
                what: "quick check after the OC apply",
                resets: ResetLimit::default(),
                card: c.clone(),
            });
        }
    }

    /// The quick check after an OC apply reset the offset of a tuned card while mining (its engine is stopped): the worker
    /// loads the model at the base, then `want` gets the start quick check; a failure steps back one step (saved) and checks
    /// again.  A stop ends it without a verdict.
    fn reapply(&self, c: &Card, want: i32, what: &str) {
        let Some(e) = self.sh.db.entry(&c.uuid) else { return };
        let refs = if e.kernel == c.kernel { e.refs.clone() } else { Vec::new() };
        let mut w = None;
        if self.set(c, c.base).is_err() {
            return;
        }
        let r0 = self.check(c, &mut w, &Spec { secs: 0, measure: 0, tv: false }, &refs);
        if self.stopped() {
            return;
        }
        if !r0.ok {
            lg(format!("{} CMFD check at the base offset {} cannot run ({}); mining at {} (no undervolt)", c.label(), signed(c.base), r0.reason, signed(c.base)));
            return;
        }
        let mut at = want;
        loop {
            if let Err(err) = self.set(c, at) {
                lg(format!("{} cannot set {} again for the {what} ({err:#}); mining at {}", c.label(), signed(at), signed(c.base)));
                let _ = self.set(c, c.base);
                break;
            }
            std::thread::sleep(Duration::from_secs(3));
            let r = self.check(c, &mut w, &Spec { secs: 20, measure: 8, tv: true }, &refs);
            if self.stopped() {
                break; // a stop is not a failure of the offset (the shutdown restore puts the owner's back)
            }
            if r.ok {
                lg(format!("{} {what} at {}: ok ({}); the engine restarts at it", c.label(), signed(at), r.brief()));
                if let Some(mut e2) = self.sh.db.entry(&c.uuid).filter(|x| x.offset == at) {
                    e2.verified_at = now();
                    self.sh.db.save(&c.uuid, &e2);
                }
                break;
            }
            let new = (at - self.o.step).max(c.base);
            let mut e2 = self.sh.db.entry(&c.uuid).unwrap_or_else(|| e.clone());
            e2.offset = new;
            let mut note = String::new();
            if new <= c.base {
                e2.disabled_until = now() + DISABLE_SECS;
                note = format!("; back at the base {}: auto-uv for CMFD paused on this GPU for 24 h", signed(c.base));
            }
            self.sh.db.save(&c.uuid, &e2);
            lg(format!("{} {what} at {} FAILED ({}): {} -> {}, saved{note}", c.label(), signed(at), r.reason, signed(at), signed(new)));
            if r.driver_fault || new <= c.base {
                let _ = self.set(c, new);
                break;
            }
            at = new;
        }
        drop(w);
    }

    fn run_card(&self, c: &Card) -> Result<()> {
        let crashed = self.sh.db.take_trying(&c.uuid);
        if let Some((at, _)) = crashed {
            lg(format!("{} the previous run died while checking CMFD offset {} (miner or driver crash): that offset counts as failed", c.label(), signed(at)));
            return self.tune(c, self.sh.db.entry(&c.uuid), crashed);
        }
        // 1.7.6 (as NOID since 1.7.2): a crash while mining at a tuned CMFD offset (faults.rs, Xid of this miner's own engine): that
        // offset counts as failed; with a profile of these clocks the card steps back from it after a quick check, without one
        // it is tuned below it
        let mut mined_crash: Option<i32> = None;
        if let Some(mut r) = crate::faults::get(&self.sh.db.dir, &c.uuid) {
            if r.n >= 1 && r.offset > c.base {
                lg(format!("{} crashed while mining at CMFD offset {} in the previous run (Xid {}): that offset counts as failed", c.label(), signed(r.offset), r.xid));
                r.n = 0;
                crate::faults::put(&self.sh.db.dir, &c.uuid, &r);
                mined_crash = Some(r.offset);
            }
        }
        // 1.7.2: one profile per core lock (or clock under load at the base when the miner does not set the lock)
        let lock = c.locked.then_some(c.lock);
        let profs = self.sh.db.profiles(&c.uuid);
        let clk = if lock.is_none() && !profs.is_empty() {
            let mut w = None;
            let r0 = self.check(c, &mut w, &Spec { secs: 0, measure: 8, tv: false }, &[]);
            drop(w);
            if self.stopped() {
                return Ok(());
            }
            r0.clk
        } else {
            None
        };
        let sel = profs.iter().filter(|(_, e)| fits(e, lock, clk)).min_by_key(|(_, e)| (e.migrated, u64::MAX - e.verified_at)).map(|(k, _)| k.clone());
        let what = match (lock, clk) {
            (Some(l), _) => format!("core lock {l} MHz"),
            (None, Some(k)) => format!("{k} MHz under load at the base (lock not set by the miner)"),
            _ => "these clocks".into(),
        };
        lg(format!("{} CMFD profiles: {}; {}", c.label(), self.sh.db.line(&c.uuid), sel.as_ref().map_or(format!("none for {what}"), |k| format!("using {k} for {what}"))));
        if self.o.reset {
            if let Some(k) = &sel {
                self.sh.db.forget(&c.uuid, k);
            }
            lg(format!("{} --uv-reset: CMFD profile {} forgotten; the others are kept (--uv-reset-all forgets all)", c.label(), sel.as_deref().unwrap_or("(none)")));
            return self.tune(c, None, None);
        }
        if let Some(k) = &sel {
            self.sh.db.use_key(&c.uuid, k);
        }
        let saved = if sel.is_some() { self.sh.db.entry(&c.uuid) } else { None };
        if let Some(e) = &saved {
            if e.driver != self.driver || e.kernel != c.kernel {
                lg(format!("{} saved CMFD result is for driver {} / {}, now driver {} / {}: tuning again", c.label(), e.driver, e.kernel, self.driver, c.kernel));
            } else if e.pl > 0.0 && c.pl > 0.0 && (c.pl - e.pl).abs() > 0.03 * e.pl {
                lg(format!("{} the power limit changed ({:.0} W when tuned, {:.0} W now; the CMFD edge moves with it): tuning again", c.label(), e.pl, c.pl));
            } else if e.base != c.base {
                lg(format!("{} the base offset changed ({} when tuned, {} now): tuning again", c.label(), signed(e.base), signed(c.base)));
            } else if e.disabled_until > now() {
                lg(format!("{} auto-uv for CMFD is paused on this GPU after a failure ({} h left); mining at {}", c.label(), (e.disabled_until - now()) / 3600 + 1, signed(c.base)));
                return Ok(());
            } else if e.offset <= c.base {
                lg(format!("{} no safe CMFD offset above {} was found when tuned; mining at it (--uv-reset to tune again)", c.label(), signed(c.base)));
                return Ok(());
            } else {
                match saved_vs_limit(e.offset, e.cap, e.floor, c.max, c.max_src) {
                    Saved::Retune => lg(format!("{} the limit was raised ({} when tuned, {} now) and the saved {} is the old limit: tuning on from it", c.label(), signed(uv::eff_cap(e.offset, e.cap, c.max_src)), signed(c.max), signed(e.offset))),
                    Saved::Clamp => return self.apply_saved(c, e, c.max, mined_crash),
                    Saved::Apply => return self.apply_saved(c, e, e.offset, mined_crash),
                }
            }
        }
        self.tune(c, saved, mined_crash.map(|at| (at, i32::MIN)))
    }

    /// A saved result (or the lower limit of this run): applied at once when verified recently, else after a quick check.
    fn apply_saved(&self, c: &Card, e: &Entry, target: i32, crash: Option<i32>) -> Result<()> {
        // after a crash while mining: never above 30 MHz below it, and always the quick check first
        let target = crash.map_or(target, |at| target.min(at - uv::crash_back(self.o.step)));
        if target <= c.base {
            lg(format!("{} saved CMFD offset {} is above the limit {} ({}), which is not above {}: mining at {}", c.label(), signed(e.offset), signed(c.max), c.max_src, signed(c.base), signed(c.base)));
            return Ok(());
        }
        let clamped = target != e.offset && crash.is_none();
        // 1.7.2: without --lock-cclock the lock is not known to the miner (HiveOS / nvidia-smi may have changed it since the
        // result was verified): never applied without the quick check, whose load clock is compared with the saved one
        let fresh = crash.is_none() && c.locked && !e.migrated && !clamped && self.o.reval_hours > 0.0 && now().saturating_sub(e.verified_at) < (self.o.reval_hours * 3600.0) as u64;
        if fresh {
            self.set(c, target)?;
            lg(format!(
                "{} CMFD offset {} -> {}, verified {} ago ({:.2} -> {:.2} FW/s, {:.0} -> {:.0} W at tuning): applied without a start check",
                c.label(),
                signed(c.base),
                signed(target),
                fmt_age(now().saturating_sub(e.verified_at)),
                e.fws_before,
                e.fws_after,
                e.w_before,
                e.w_after
            ));
            self.go_live(c);
            return Ok(());
        }
        let mut w = None;
        // the worker loads the model at the base offset, then the check runs at the saved one
        let r0 = self.check(c, &mut w, &Spec { secs: 0, measure: 0, tv: false }, &e.refs);
        if !r0.ok {
            lg(format!("{} CMFD check at the base offset {} cannot run ({}); mining at {} (no undervolt)", c.label(), signed(c.base), r0.reason, signed(c.base)));
            return Ok(());
        }
        // a failing quick check steps down from the saved offset (at most 3 steps, the model stays loaded) instead of a pause /
        // a tuning from scratch (1.7.2)
        let mut target = target;
        let mut r;
        let mut tries = 0;
        loop {
            self.set(c, target)?;
            std::thread::sleep(Duration::from_secs(3));
            r = self.check(c, &mut w, &Spec { secs: 20, measure: 8, tv: true }, &e.refs);
            if self.stopped() || r.ok || r.driver_fault || tries >= 3 || target - self.o.step <= c.base {
                break;
            }
            lg(format!("{} CMFD offset {} FAILED the quick check ({}): one step down to {} (no tuning from scratch)", c.label(), signed(target), r.reason, signed(target - self.o.step)));
            target -= self.o.step;
            tries += 1;
        }
        drop(w);
        if self.stopped() {
            return Ok(());
        }
        if let (true, true, Some(k), 0) = (r.ok, e.clk_after != 0, r.clk, tries) {
            if let uv::Regime::Differs(d) = uv::regime_probe(e.clk_after, target, k, target) {
                lg(format!("{} CMFD quick check at {} passed, but at {k} MHz under load, {} MHz when verified ({d}: the clock lock changed): tuning again", c.label(), signed(target), e.clk_after));
                self.set(c, c.base)?;
                return self.tune(c, Some(e.clone()), None);
            }
        }
        if r.ok {
            lg(format!(
                "{} CMFD offset {} -> {}{}, saved {} ago, quick check passed ({})",
                c.label(),
                signed(c.base),
                signed(target),
                if clamped { format!(" (the saved {} is above the limit {}, {})", signed(e.offset), signed(c.max), c.max_src) } else if tries > 0 { format!(" (stepped down from {})", signed(e.offset)) } else { String::new() },
                fmt_age(now().saturating_sub(e.tested_at)),
                r.brief()
            ));
            if !clamped {
                let mut e2 = e.clone();
                e2.verified_at = now();
                if tries > 0 || crash.is_some() {
                    e2.offset = target;
                }
                if e2.migrated {
                    e2.migrated = false;
                    e2.lock = c.lock;
                }
                self.sh.db.save(&c.uuid, &e2);
            }
            self.go_live(c);
        } else {
            lg(format!("{} CMFD offset {} FAILED the quick check ({}); back to {}, auto-uv for CMFD paused for 24 h on this GPU at these clocks", c.label(), signed(target), r.reason, signed(c.base)));
            self.set(c, c.base)?;
            let mut e2 = e.clone();
            e2.disabled_until = now() + DISABLE_SECS;
            self.sh.db.save(&c.uuid, &e2);
        }
        Ok(())
    }

    fn tune(&self, c: &Card, prev: Option<Entry>, crashed: Option<(i32, i32)>) -> Result<()> {
        self.sh.db.s.set_good(&c.uuid, None);
        let r = self.tune_inner(c, prev, crashed);
        self.sh.db.s.set_good(&c.uuid, None);
        r
    }

    fn tune_inner(&self, c: &Card, prev: Option<Entry>, crashed: Option<(i32, i32)>) -> Result<()> {
        let _status = crate::status::Guard(c.dev);
        {
            let steps = ((c.max - c.base).max(0) / self.o.step.max(1)) as u64 + 2;
            let eta = Instant::now() + Duration::from_secs(120 + steps * (self.o.step_secs as u64 + 20) + self.o.confirm_secs as u64 + 60);
            crate::status::set(c.dev, "CMFD tuning".into(), true, Some(eta));
        }
        let step = self.o.step;
        let mut cap = c.max;
        if let Some((at, _)) = crashed {
            cap = cap.min(at - uv::crash_back(step));
        }
        // --uv-min, else the card type's default start by the core lock (RTX 3070: +150 ...; RTX 40: at least +200, RTX 50: +300,
        // as for Pearl), else one step above the base; a start that fails is stepped down in 30 MHz steps
        let (start0, start_src) = c.start_offset(step);
        let mut start = start0.max(c.base + step);
        // 1.7.7: the last result of this card at these clocks (any driver / kernel), else of the nearest clock, minus one coarse
        // step (climb::prior_start; never below `start` = --uv-min / default, never above `cap`); not after a crash
        let retune_from = prev.as_ref().filter(|p| crashed.is_none() && uv::saved_vs_limit(p.offset, p.cap, p.floor, c.max, c.max_src) == uv::Saved::Retune).map(|p| p.offset);
        if let Some(from) = retune_from.filter(|&f| f > start && f <= cap) {
            // the limit was raised and the saved result is the old limit: the search goes on from it (not one step below it)
            start = from;
        } else if let Some((h, line)) = crashed.is_none().then(|| climb::prior_start(&self.sh.db.priors(&c.uuid, c.base), c.lock_hint.unwrap_or(0), 0, start, cap, self.o.coarse_step())).flatten() {
            lg(format!("{} {line}", c.label()));
            start = h;
        }
        // 1.7.6: a new clock lock that has a verified profile of a higher clock starts below that profile's offset (not with
        // --uv-min, not after a crash, not on a re-tune of a saved profile)
        else if c.min.is_none() && crashed.is_none() && prev.is_none() {
            let hints = self.sh.db.hints(&c.uuid, c.base);
            if let Some((h, from)) = climb::hint_start(&hints, c.lock_hint.unwrap_or(0), 0, start, cap, self.o.coarse_step()) {
                lg(format!("{} {}", c.label(), climb::hint_line(&from, self.o.coarse_step(), h, start)));
                start = h;
            }
        }
        let _ = start_src;
        if start > cap {
            lg(format!("{} no room above the base {} below the limit {} ({}{}): mining at {}", c.label(), signed(c.base), signed(cap), c.max_src, if crashed.is_some() { ", below the crash" } else { "" }, signed(c.base)));
            if crashed.is_some() {
                let mut e = self.new_entry(c);
                e.offset = c.base;
                e.cap = c.max;
                e.tested_at = now();
                self.sh.db.save(&c.uuid, &e);
            }
            return Ok(());
        }
        lg(format!(
            "{} tuning on the CMFD load (worker {}, batch {}): {} s at {} (references + official test vector), then {} .. {} in +{} MHz steps{} ({} s each)",
            c.label(),
            cmfd::bin(cmfd::replay_worker()),
            c.batch,
            warmup_secs(self.o),
            signed(c.base),
            signed(start),
            signed(cap),
            self.o.coarse_step(),
            if self.o.coarse_step() > step { format!(" (+{step} MHz inside the interval of a failing step)") } else { String::new() },
            self.o.step_secs
        ));
        // the digests depend only on the fixed inputs and the batch (part of the identity): saved references seed the check
        let golden = prev.as_ref().filter(|e| e.kernel == c.kernel).map(|e| e.refs.clone()).unwrap_or_default();
        let mut w: Option<Worker> = None;
        let base = self.check(c, &mut w, &Spec { secs: warmup_secs(self.o), measure: 20, tv: true }, &golden);
        if self.stopped() {
            return Ok(());
        }
        if !base.ok {
            lg(format!(
                "{} WARNING: the CMFD check cannot run or fails at the base offset {} ({}): not tuning, mining at {} (no undervolt)",
                c.label(),
                signed(c.base),
                base.reason,
                signed(c.base)
            ));
            return Ok(());
        }
        let refs = base.refs.clone();
        lg(format!("{} base {}: ok, official test vector ok, {}", c.label(), signed(c.base), base.brief()));
        let coarse = self.o.coarse_step();
        let mut good = c.base;
        // the voltage-floor test needs "power at the same work": a card held at its power limit shows the same watts at every step
        // while the clock (FW/s) still rises, so the watts are scaled to the FW/s of the base (W x base FW/s / FW/s)
        let fw0 = base.fws;
        let same_work = |r: &Res| same_work_w(r.watts, r.fws, fw0);
        let mut good_w = base.watts;
        let mut failed: Option<i32> = None;
        let mut fault = false;
        let mut cand = start;
        if let Some((_, g)) = crashed {
            if g >= start && g <= cap {
                lg(format!("{} steps up to {} passed before the crash: not climbing them again", c.label(), signed(g)));
                good = g;
                good_w = None;
                cand = cap + 1; // the start step is not checked again
            }
        }
        // the start step itself must hold; a start that fails: 30 MHz lower, and the search stays below the failed start
        while cand <= cap {
            if let Err(e) = self.set(c, cand) {
                lg(format!("{} cannot set {} ({e:#}); stopping the search", c.label(), signed(cand)));
                failed = Some(cand);
                break;
            }
            std::thread::sleep(Duration::from_secs(3));
            let r = self.check(c, &mut w, &Spec { secs: self.o.step_secs, measure: 10, tv: false }, &refs);
            if self.stopped() {
                return Ok(());
            }
            if r.ok {
                lg(format!("{} step {}: ok ({})", c.label(), signed(cand), r.brief()));
                good = cand;
                good_w = same_work(&r);
                self.sh.db.s.set_good(&c.uuid, Some(good));
                break;
            }
            lg(format!("{} step {}: FAILED ({})", c.label(), signed(cand), r.reason));
            if !r.driver_fault && cand - 30 > c.base {
                lg(format!("{} the start {} fails: trying {} (30 MHz lower)", c.label(), signed(cand), signed(cand - 30)));
                cap = cap.min(cand - step);
                cand -= 30;
                continue;
            }
            failed = Some(cand);
            fault = r.driver_fault;
            if r.driver_fault {
                // a hang / Xid may have left the GPU unusable: no further steps; the crash marker stays until a result is saved
                lg(format!("{} driver fault: stepping down without more climbing", c.label()));
            }
            break;
        }
        let mut plateau = false;
        if failed.is_none() && good > c.base {
            // coarse steps, the failing interval in fine steps, the voltage floor (climb.rs, shared with Pearl and NOID)
            let mut aborted = false;
            let s = climb::climb(good, good_w, None, cap, step, coarse, |o, g, mhz, refine| {
                if let Err(e) = self.set(c, o) {
                    lg(format!("{} cannot set {} ({e:#}); stopping the search", c.label(), signed(o)));
                    aborted = true;
                    return climb::StepOut { abort: true, ..Default::default() };
                }
                std::thread::sleep(Duration::from_secs(3));
                self.sh.db.s.set_good(&c.uuid, Some(g)); // the crash marker of this check remembers it
                let r = self.check(c, &mut w, &Spec { secs: self.o.step_secs, measure: 10, tv: false }, &refs);
                if self.stopped() {
                    return climb::StepOut { abort: true, ..Default::default() };
                }
                if r.ok {
                    lg(format!("{} step {}{}: ok ({})", c.label(), signed(o), if refine { " (refining)" } else { "" }, r.brief()));
                } else {
                    let note = if refine || mhz <= step {
                        String::new()
                    } else if r.driver_fault {
                        format!("; driver fault on a {mhz} MHz step: not refined, one step below {}", signed(g))
                    } else if g + step <= o - step {
                        format!("; {step} MHz steps between {} and it", signed(g))
                    } else {
                        String::new()
                    };
                    lg(format!("{} step {}{}: FAILED ({}){note}", c.label(), signed(o), if refine { " (refining)" } else { "" }, r.reason));
                }
                climb::StepOut { ok: r.ok, fault: r.driver_fault, w: same_work(&r), t: None, abort: false }
            });
            if self.stopped() {
                return Ok(());
            }
            good = s.good;
            plateau = s.plateau;
            if s.failed {
                failed = Some(s.fail_at);
                fault = s.fault;
            } else if aborted || s.aborted {
                failed = Some(good + step); // the offset could not be set: stop here, one step of margin
            }
            if s.plateau {
                lg(format!("{} power no longer drops above {} (voltage floor: the next two steps up to {} saved < 0.5 W per 15 MHz each): stopping there", c.label(), signed(good), signed(s.flat_top)));
            }
        }
        // the cap below a crash (30 MHz under it) is the margin already
        let crash_capped = failed.is_none() && crashed.is_some() && good == cap;
        if crash_capped {
            failed = crashed.map(|k| k.0);
        }
        let _ = fault;
        // one step of margin below the edge (--uv-keep-last keeps the last good step)
        let mut fin = good;
        if let Some(x) = failed {
            if !self.o.keep_last_good && !crash_capped && fin > c.base {
                fin = (fin - step).max(c.base);
            }
            std::thread::sleep(Duration::from_secs(5));
            self.set(c, fin).context("GPU does not accept the safe offset after the failed step")?;
            lg(format!("{} CMFD edge at {}: stepping back to {}", c.label(), signed(x), signed(fin)));
        }
        let mut conf = None;
        while fin > c.base {
            self.set(c, fin)?;
            std::thread::sleep(Duration::from_secs(3));
            let r = self.check(c, &mut w, &Spec { secs: self.o.confirm_secs, measure: 20, tv: true }, &refs);
            if self.stopped() {
                return Ok(());
            }
            if r.ok {
                lg(format!("{} confirmation at {}: ok, official test vector ok ({})", c.label(), signed(fin), r.brief()));
                conf = Some(r);
                break;
            }
            lg(format!("{} confirmation at {}: FAILED ({}); one step lower", c.label(), signed(fin), r.reason));
            fin = (fin - step).max(c.base);
        }
        drop(w);
        if fin == c.base {
            self.set(c, c.base)?;
        }
        let mut e = self.new_entry(c);
        e.offset = fin;
        e.cap = c.max;
        e.floor = plateau;
        e.pl = c.pl;
        e.refs = refs;
        e.fws_before = base.fws.unwrap_or(0.0);
        e.w_before = base.watts.unwrap_or(0.0);
        e.clk_before = base.clk.unwrap_or(0);
        let (fa, wa, ca) = conf.as_ref().map_or((base.fws, base.watts, base.clk), |r| (r.fws, r.watts, r.clk));
        e.fws_after = fa.unwrap_or(0.0);
        e.w_after = wa.unwrap_or(0.0);
        e.clk_after = ca.unwrap_or(0);
        e.tested_at = now();
        e.verified_at = now();
        self.sh.db.save(&c.uuid, &e);
        self.sh.db.clear_trying(&c.uuid);
        let eff = |f: f64, w: f64| if w > 0.0 { f * 3_600_000.0 / w } else { 0.0 };
        lg(format!(
            "{} RESULT (CMFD): offset {} -> {}: {:.2} -> {:.2} FW/s, {:.1} -> {:.1} W, {:.0} -> {:.0} FW/kWh ({:+.1} %), saved{}; mining at it",
            c.label(),
            signed(c.base),
            signed(fin),
            e.fws_before,
            e.fws_after,
            e.w_before,
            e.w_after,
            eff(e.fws_before, e.w_before),
            eff(e.fws_after, e.w_after),
            if eff(e.fws_before, e.w_before) > 0.0 { (eff(e.fws_after, e.w_after) / eff(e.fws_before, e.w_before) - 1.0) * 100.0 } else { 0.0 },
            if plateau { " (the voltage floor: no edge searched above it)" } else { "" }
        ));
        self.go_live(c);
        Ok(())
    }
}

/// Per-card limits and the identity of the CMFD check; `bases[i]`: base offset of devices[i] from cmfd_offset (None = not
/// manageable).
fn map_card(nv: &Nvml, dev: i32, pos: usize, base: i32, o: &uv::Opts, ctx: &Ctx, worker_tag: &str, engine: &str) -> Result<Card> {
    let bus = gpu::device_pci_bus_id(dev)?;
    let h = nv.by_bus(&bus)?;
    let uuid = nv.uuid(h)?;
    ensure!(uuid.eq_ignore_ascii_case(&gpu::device_uuid(dev)?), "UUID mismatch between NVML and CUDA");
    let name = nv.name(h).unwrap_or_else(|_| gpu::device_name(dev));
    let (_, _, _, hi) = nv.probe_offset(h)?;
    let def = uv::card_defaults(&name, nv.compute_capability(h), nv.pci_device_id(h));
    let (max, max_src) = match o.max_offset.get(pos) {
        Some(v) => (v.min(HARD_CAP), "--uv-max"),
        None => (def.max, def.max_src),
    };
    let batch = mining_batch(&uuid, ctx.batch);
    let orig = crate::cmfd_offset::orig(&bus).unwrap_or(base);
    // 1.7.2: the lock is the profile key, no longer part of the identity
    let kernel = format!("{engine} | worker {worker_tag} | {} | batch {batch} | {INPUTS}", ctx.mode);
    let pl = nv.power_limit_w(h).unwrap_or(0.0);
    let own_lock = ctx.core_lock.get(pos).copied().flatten();
    let lock_hint = own_lock.or_else(|| crate::hiveoc::load().and_then(|p| nv.index(h).and_then(|i| p.core_lock(i as usize))));
    Ok(Card { dev, name, bus, uuid, nv: h, orig, base, hi, max: max.min(hi), max_src, min: o.min_offset.get(pos), batch, kernel, pl, locked: own_lock.is_some(), lock: own_lock.unwrap_or(0), lock_hint, def })
}

/// `--auto-uv-dry-run`: what would be tuned (nothing is changed): the same start the tuning takes (--uv-min / the card type by the core
/// lock / the generation floor / a saved profile of a higher clock), the same limit and steps.
pub fn dry_run(o: &uv::Opts, devices: &[i32], locks: &[Option<u32>]) {
    let Ok(nv) = Nvml::load() else {
        lg("NVML not available: nothing would be tuned");
        return;
    };
    let dir = uv::state_dir(o);
    for (pos, &dev) in devices.iter().enumerate() {
        let r = (|| -> Result<String> {
            let bus = gpu::device_pci_bus_id(dev)?;
            let h = nv.by_bus(&bus)?;
            let name = nv.name(h).unwrap_or_default();
            let uuid = nv.uuid(h).unwrap_or_default();
            let (_, cur, _, hi) = nv.probe_offset(h)?;
            let def = uv::card_defaults(&name, nv.compute_capability(h), nv.pci_device_id(h));
            let max = o.max_offset.get(pos).unwrap_or(def.max).min(HARD_CAP).min(hi);
            let base = cur.min(0);
            let own_lock = locks.get(pos).copied().flatten();
            let lock_hint = own_lock.or_else(|| crate::hiveoc::load().and_then(|p| nv.index(h).and_then(|i| p.core_lock(i as usize))));
            let min = o.min_offset.get(pos);
            let (v, src) = start_of(&name, &def, lock_hint, min, max, base, o.step);
            let mut start = v;
            let mut from = String::new();
            let db = Db { dir: dir.clone(), s: Store::load(&dir, profiles::CMFD, log_line, |_| 0) }; // read only: a dry run migrates nothing
            if let Some((hs, line)) = climb::prior_start(&db.priors(&uuid, base), lock_hint.unwrap_or(0), 0, start, max, o.coarse_step()) {
                from = format!("; {line}");
                start = hs;
            } else if let Some((hs, f)) = climb::hint_start(&db.hints(&uuid, base), lock_hint.unwrap_or(0), 0, start, max, o.coarse_step()).filter(|_| min.is_none()) {
                from = format!("; profile {f} of a higher clock: starts {} MHz below it", o.coarse_step());
                start = hs;
            }
            Ok(format!(
                "DRY RUN: GPU{dev} ({}): offset {} now; CMFD base {}, start {} ({src}{from}), limit {}, +{} MHz steps (+{} inside the interval of a failing step); nothing is changed",
                short_name(&name),
                signed(cur),
                signed(base),
                signed(start.min(max)),
                signed(max),
                o.coarse_step(),
                o.step
            ))
        })();
        lg(r.unwrap_or_else(|e| format!("GPU{dev}: cannot manage ({e:#})")));
    }
}

/// Tunes / applies the CMFD offset of every manageable mining card (in parallel, one worker per card); returns when done.
/// The offsets stay set through cmfd_offset (restored with it).
pub fn start(o: &uv::Opts, devices: &[i32], bases: &[Option<i32>], ctx: &Ctx, stop: &Arc<AtomicBool>) {
    let dir = uv::state_dir(o);
    if !uv::writable_dir(&dir) {
        lg(format!("state directory {} is not writable: no CMFD undervolt (set --uv-dir or $FFGP_STATE_DIR); mining at the base offset", dir.display()));
        return;
    }
    let nv = match Nvml::load() {
        Ok(n) => n,
        Err(e) => {
            lg(format!("NVML not available ({e:#}); mining at the base offset"));
            return;
        }
    };
    let worker = ctx.engine.join(cmfd::bin(cmfd::replay_worker()));
    if !ctx.bank.is_file() || !worker.is_file() || !ctx.engine.join(cmfd::REF_COEFFICIENTS).is_file() {
        lg(format!(
            "WARNING: the CMFD check needs {}, the model {} and {}; something is missing: mining at the base offset (no undervolt)",
            worker.display(),
            ctx.bank.display(),
            ctx.engine.join(cmfd::REF_COEFFICIENTS).display()
        ));
        return;
    }
    let sh = Arc::new(Shared {
        db: Db::load(&dir),
        live: Mutex::new(Vec::new()),
        faulted: Mutex::new(Vec::new()),
        ctx: ctx.clone(),
        o: o.clone(),
        stop: stop.clone(),
        driver: nv.driver_version(),
    });
    *shared().lock().unwrap() = Some(sh.clone());
    let engine = cmfd::engine_label(&ctx.engine);
    let tag = file_tag(&worker);
    let mut cards = Vec::new();
    for (pos, &dev) in devices.iter().enumerate() {
        let Some(base) = bases.get(pos).copied().flatten() else {
            lg(format!("GPU{dev}: core offset not manageable (see above); left alone"));
            continue;
        };
        match map_card(&nv, dev, pos, base, o, ctx, &tag, &engine) {
            Ok(c) => {
                nv.watch_xid(c.nv);
                let range = if c.max_src == "--uv-max" && c.max > uv::card_defaults(&c.name, nv.compute_capability(c.nv), nv.pci_device_id(c.nv)).max {
                    " (WARNING: above the card's default limit)"
                } else {
                    ""
                };
                lg(format!("{}: {}, owner's offset {}, CMFD base {}, limit {} ({}){range}, batch {}, power limit {:.0} W", c.label(), c.uuid, signed(c.orig), signed(c.base), signed(c.max), c.max_src, c.batch, c.pl));
                cards.push(c);
            }
            Err(e) => lg(format!("GPU{dev}: cannot manage ({e:#}); left alone")),
        }
    }
    // 1.7.2: the start checks / tunings run in the background; only the engine of the GPU being checked waits (busy), the
    // engines of the other GPUs start mining at once (before, every GPU waited for the slowest tuning)
    starting().lock().unwrap().extend(cards.iter().map(|c| c.dev));
    let (ctx, o, stop) = (ctx.clone(), o.clone(), stop.clone());
    std::thread::spawn(move || {
        let t = Tuner { nv: &nv, ctx: &ctx, o: &o, sh, driver: nv.driver_version(), stop: &stop };
        std::thread::scope(|s| {
            for c in &cards {
                let t = &t;
                s.spawn(move || {
                    if let Err(e) = t.run_card(c) {
                        lg(format!("{} error: {e:#}; back to the base offset {}", c.label(), signed(c.base)));
                        let _ = t.set(c, c.base);
                    }
                    starting().lock().unwrap().retain(|&d| d != c.dev);
                });
            }
        });
    });
}

/// GPUs whose start check / tuning still runs (their engine waits).
fn starting() -> &'static Mutex<Vec<i32>> {
    static S: Mutex<Vec<i32>> = Mutex::new(Vec::new());
    &S
}

/// The engine of `dev` reported wrong results (`INTEGRITY | ERROR`) or a CUDA fault: when that card runs a tuned CMFD offset,
/// one step back (never below the base), saved; at the base the tuning is paused for 24 h. Returns true when stepped back.
pub fn on_fault(dev: i32, why: &str) -> bool {
    let Some(sh) = shared().lock().unwrap().clone() else { return false };
    let mut live = sh.live.lock().unwrap();
    let Some(l) = live.iter_mut().find(|l| l.dev == dev && l.owned && !l.busy) else { return false };
    if l.last.is_some_and(|t| t.elapsed() < Duration::from_secs(20)) {
        return false; // one fault burst = one step
    }
    let Some(cur) = cmfd_offset::current(&l.bus) else { return false };
    if cur <= l.base {
        return false;
    }
    l.last = Some(Instant::now());
    let new = (cur - l.step).max(l.base);
    match cmfd_offset::set(&l.bus, new) {
        Ok(()) => {
            let mut e = sh.db.entry(&l.uuid).unwrap_or_default();
            e.offset = new;
            let mut note = String::new();
            if new <= l.base {
                e.disabled_until = now() + DISABLE_SECS;
                note = format!("; back at the base {}: auto-uv for CMFD paused on this GPU for 24 h", signed(l.base));
            }
            if !e.driver.is_empty() {
                sh.db.save(&l.uuid, &e);
            }
            lg(format!("GPU{dev}: {why}: CMFD offset {} -> {}, saved{note}; the engine restarts", signed(cur), signed(new)));
            true
        }
        Err(err) => {
            lg(format!("GPU{dev}: {why}: could not step back ({err:#})"));
            false
        }
    }
}

/// While mining (called from the engine loop; every tuned card is read every 30 s): the GPUs whose saved CMFD offset an OC apply
/// reset.  For each of them the caller stops the engine and calls `begin_reapply`; the engine restarts once `busy` is false.
pub fn poll_oc() -> Vec<i32> {
    let Some(sh) = shared().lock().unwrap().clone() else { return Vec::new() };
    if sh.stop.load(Ordering::Relaxed) || cmfd_offset::aside() || crate::idle::mode() != crate::idle::Mode::Full {
        return Vec::new(); // idle mode: the owner's offsets are on the cards, and no re-validation starts while the PC is in use
    }
    let mut out = Vec::new();
    let mut live = sh.live.lock().unwrap();
    for l in live.iter_mut() {
        // 1.7.6: the periodic re-validation of a tuned offset (--uv-reval-hours): the engine of this GPU stops, the offset gets the
        // quick check (the model is loaded for it), a failure steps back; then the engine restarts
        if l.owned && !l.busy && sh.o.reval_hours > 0.0 && Instant::now() >= l.next_reval && cmfd_offset::current(&l.bus).is_some_and(|v| v > l.base) {
            lg(format!("GPU{}: periodic re-validation of the CMFD offset {} (the engine of this GPU stops for it and restarts after)", l.dev, signed(cmfd_offset::current(&l.bus).unwrap_or(0))));
            l.busy = true;
            l.what = "re-validation";
            l.next_reval = Instant::now() + reval_interval(sh.o.reval_hours);
            out.push(l.dev);
            continue;
        }
        if !l.owned || l.busy || Instant::now() < l.next_read {
            continue;
        }
        l.next_read = Instant::now() + Duration::from_secs(30);
        let (Some(want), Some(v)) = (cmfd_offset::current(&l.bus), cmfd_offset::read(&l.bus)) else { continue };
        if v == want {
            continue;
        }
        crate::faults::note_foreign_change(&l.bus);
        match cmfd_offset::reset_kind(&l.bus, v) {
            Some(why) if l.resets.allow() => {
                lg(format!(
                    "GPU{}: the CMFD offset was reset to {} by an OC apply ({why}); re-applying the verified {} after a quick check (the engine of this GPU stops for it and restarts after)",
                    l.dev,
                    signed(v),
                    signed(want)
                ));
                l.busy = true;
                l.what = "quick check after the OC apply";
                out.push(l.dev);
            }
            Some(why) => {
                lg(format!(
                    "GPU{}: the offset was reset to {} by an OC apply ({why}) again: {} times within an hour already, so auto-uv for CMFD stands down on this GPU and leaves it as is (restart the miner once the OC is settled)",
                    l.dev,
                    signed(v),
                    crate::hiveoc::OC_RESETS_MAX
                ));
                l.owned = false;
            }
            None => {
                lg(format!("GPU{}: the offset was changed by someone else ({} now, {} expected): auto-uv for CMFD stands down on this GPU and leaves it as is", l.dev, signed(v), signed(want)));
                l.owned = false;
            }
        }
    }
    out
}

/// Idle mode (1.7.6): may the engines stop and the owner's offsets go back?  False while a start check / tuning / re-validation still
/// runs (it finishes first: cut short it would read as a failed step); call again.
pub fn idle_ready() -> bool {
    if !starting().lock().unwrap().is_empty() {
        return false;
    }
    let Some(sh) = shared().lock().unwrap().clone() else { return true };
    let busy = sh.live.lock().unwrap().iter().any(|l| l.busy);
    !busy
}

/// A quick check after an OC apply runs on `dev` (its engine must stay stopped).
pub fn busy(dev: i32) -> bool {
    if starting().lock().unwrap().contains(&dev) {
        return true;
    }
    let Some(sh) = shared().lock().unwrap().clone() else { return false };
    sh.live.lock().unwrap().iter().any(|l| l.dev == dev && l.busy)
}

/// Starts the quick check after an OC apply on `dev` (after `poll_oc` named it and its engine was stopped), in the background.
pub fn begin_reapply(dev: i32) {
    let Some(sh) = shared().lock().unwrap().clone() else { return };
    let Some((card, want, what)) = sh.live.lock().unwrap().iter().find(|l| l.dev == dev && l.busy).map(|l| (l.card.clone(), cmfd_offset::current(&l.bus), l.what)) else { return };
    std::thread::spawn(move || {
        // the engine and its GPU worker were just killed: let the driver free the GPU memory (7 GB) first
        std::thread::sleep(Duration::from_secs(3));
        if let (Ok(nv), Some(want)) = (Nvml::load(), want) {
            nv.watch_xid(card.nv);
            let t = Tuner { nv: &nv, ctx: &sh.ctx, o: &sh.o, sh: sh.clone(), driver: sh.driver.clone(), stop: &sh.stop };
            t.reapply(&card, want, what);
        }
        if let Some(l) = sh.live.lock().unwrap().iter_mut().find(|l| l.dev == dev) {
            l.busy = false;
            l.next_read = Instant::now() + Duration::from_secs(30);
            l.next_reval = Instant::now() + reval_interval(sh.o.reval_hours);
        }
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn power_limited_card_gains_show_as_less_power_per_work() {
        // held at 219.5 W while FW/s rises 21.45 -> 22.75: 5.9 % more work for the same watts = 12.9 W less per same work
        let w = super::same_work_w(Some(219.5), Some(22.75), Some(21.45)).unwrap();
        assert!(219.5 - w > 12.0 && 219.5 - w < 14.0);
        // no gain in FW/s (voltage floor at a low limit): unchanged, so the floor still fires
        assert!((super::same_work_w(Some(135.0), Some(14.0), Some(14.0)).unwrap() - 135.0).abs() < 1e-9);
        // a missing rate: the plain watts
        assert_eq!(super::same_work_w(Some(100.0), None, Some(14.0)), Some(100.0));
    }

    use super::*;

    #[test]
    fn entry_roundtrip_and_fixed_inputs() {
        let e = Entry { driver: "580".into(), kernel: "k".into(), base: 0, offset: 105, cap: 495, refs: vec!["a".into(), "b".into()], fws_after: 14.2, ..Default::default() };
        let b = Entry::from_json(&e.to_json()).unwrap();
        assert_eq!((b.base, b.offset, b.cap, b.refs.len(), b.fws_after), (0, 105, 495, 2, 14.2));
        // a Pearl entry (no base_offset) is never read as a CMFD one
        let mut v = e.to_json();
        v.as_object_mut().unwrap().remove("base_offset");
        assert!(Entry::from_json(&v).is_none());
        // fixed inputs are fixed
        assert_eq!(fixed_bytes(100, 7), fixed_bytes(100, 7));
        assert_ne!(fixed_bytes(100, 7), fixed_bytes(100, 8));
        assert_eq!(fixed_bytes(77, 1).len(), 77);
    }

    const RIG01: &str = include_str!("../tests/data/uv-rig01-1.7.5.json");

    #[test]
    fn golden_cmfd_profiles_load_and_keep_their_keys() {
        let v: Value = serde_json::from_str(RIG01).unwrap();
        let mut n = 0;
        for (_, m) in v["cmfd_profiles"].as_object().unwrap() {
            for (k, x) in m.as_object().unwrap() {
                let e = Entry::from_json(x).unwrap_or_else(|| panic!("profile {k} is not read"));
                assert_eq!(&e.to_json(), x, "profile {k}: a field is lost or changed by a round trip");
                assert_eq!(&pkey(&e), k, "the key of profile {k} changed");
                n += 1;
            }
        }
        assert!(n >= 8);
    }

    #[test]
    fn golden_cmfd_store_roundtrip_and_markers() {
        let dir = std::env::temp_dir().join(format!("ffg-cmfd-golden-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(uv::results_path(&dir), RIG01).unwrap();
        let before: Value = serde_json::from_str(RIG01).unwrap();
        let db = Db::load(&dir);
        let uuid = before["cmfd_profiles"].as_object().unwrap().keys().next().unwrap().clone();
        let (key, mut e) = db.profiles(&uuid).into_iter().next().unwrap();
        e.verified_at += 1;
        db.save(&uuid, &e);
        let mut want = before.clone();
        want["cmfd_profiles"][&uuid][&key]["verified_at"] = json!(e.verified_at);
        let after: Value = serde_json::from_str(&std::fs::read_to_string(uv::results_path(&dir)).unwrap()).unwrap();
        assert_eq!(after, want);
        // the CMFD marker of 1.7.5 carried {"offset", "good": base}; it is read the same
        std::fs::write(uv::results_path(&dir), r#"{"cmfd_trying":{"U":{"offset":255,"good":0}}}"#).unwrap();
        assert_eq!(Db::load(&dir).take_trying("U"), Some((255, 0)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn golden_cmfd_before_1_7_2_result_migrates() {
        let dir = std::env::temp_dir().join(format!("ffg-cmfd-mig-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let locked = Entry { driver: "580".into(), kernel: "eng | lock-cclock 1805".into(), base: 0, offset: 255, clk_before: 1815, ..Default::default() };
        let free = Entry { driver: "580".into(), kernel: "eng".into(), base: 0, offset: 240, clk_before: 1650, ..Default::default() };
        std::fs::write(uv::results_path(&dir), serde_json::to_string(&json!({"cmfd": {"A": locked.to_json(), "B": free.to_json()}})).unwrap()).unwrap();
        let db = Db::load(&dir);
        let a = db.profiles("A");
        assert_eq!((a[0].0.as_str(), a[0].1.lock, a[0].1.kernel.as_str(), a[0].1.migrated), ("1805", 1805, "eng", false));
        let b = db.profiles("B");
        assert_eq!((b[0].0.as_str(), b[0].1.migrated, b[0].1.verified_at), ("~1650", true, 0)); // used only after a quick check
        assert!(db.s.snapshot().get("cmfd").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
