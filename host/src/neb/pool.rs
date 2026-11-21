// FFGN — see LICENSE
//! Pool client of `neb-pool/1` (spec/pool.md of NEB 1.0.0): JSON lines over TCP (or TLS: `stratum+ssl://`), stratum style.
//!   -> mining.subscribe [agent, "neb-pool/1"]   <- {session, extranonce1, extranonce2_size, chain_id, dataset_hash}
//!   -> mining.authorize [login, password]       (the user's login; the dev-fee login in the SAME session, spec section 4)
//!   <- mining.set_target [target] / mining.set_model [slot, weights_hash, weights] / mining.notify [job]
//!   -> mining.submit [login, job_id, extranonce2, time, nonce]
//! The same client talks to `ffgn --gate` (gate.rs), which adds `ffgn_nonce: [base, log2 span]` to the subscribe result: the
//! nonce range of this session on the node's reference header (extranonce2_size 0).
//! `--pool A,B` failover / failback through failover.rs, as every FreeForGem coin.
use super::proto::{self, Job, hex, unhex, unhex32};
use super::refc::Model;
use super::{Ev, Find, NJob, Origin};
use crate::net::{Conn, PoolUrl, connect};
use crate::stratum::{Src, Stats};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const PROTOCOL: &str = "neb-pool/1";
/// submits per second at most (F-14)
const SUBMIT_RATE: u32 = 25;
const SILENCE_LIMIT: Duration = Duration::from_secs(300);

/// The protocol of a pool entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// not known yet: JSON first (neb-pool/1 or Suprnova stratum); no answer to mining.subscribe = the work protocol
    Auto,
    /// JSON lines: neb-pool/1 or the Suprnova dialect (told apart by the subscribe result)
    Json,
    /// the NEB work protocol (binary frames of spec/node.md 7.1) with the login as the Hello name: Aria
    Work,
    /// an Auto entry that gave no JSON answer: the next connection tries the work protocol; it becomes Work only when a job
    /// arrives that way, else Auto again (F-04: nothing is decided for good without a positive sign)
    TryWork,
}

/// `work://HOST:PORT` (or `neb+work://`, `aria://`) = the NEB work protocol; `stratum+tcp://` / `stratum+ssl://` = JSON
/// stratum; a bare HOST:PORT = Auto, except known hosts (ariabrain.com = work protocol).
pub fn kind_of(s: &str) -> Kind {
    let t = s.trim().to_ascii_lowercase();
    if t.starts_with("work://") || t.starts_with("neb+work://") || t.starts_with("aria://") || t.contains("ariabrain.com") {
        Kind::Work
    } else if t.contains("://") {
        Kind::Json
    } else {
        Kind::Auto
    }
}

/// `stratum+tcp://` / `tcp://` / bare HOST:PORT = plain TCP (the NEB reference pool is plain); `stratum+ssl://` = TLS.
pub fn parse_url(s: &str) -> Result<PoolUrl, String> {
    let s = &{
        let t = s.trim();
        let lo = t.to_ascii_lowercase();
        ["work://", "neb+work://", "aria://"].iter().find(|p| lo.starts_with(*p)).map_or(t.to_string(), |p| t[p.len()..].to_string())
    };
    let t = s.trim();
    let plain = !t.contains("://") || t.starts_with("stratum+tcp://") || t.starts_with("tcp://") || t.starts_with("stratum://") || t.starts_with("neb://");
    let fixed = if t.starts_with("neb://") { t.replacen("neb://", "stratum+tcp://", 1) } else if !t.contains("://") { format!("stratum+tcp://{t}") } else { t.to_string() };
    let mut u = crate::net::parse_url(&fixed)?;
    if plain {
        u.tls = false;
    }
    Ok(u)
}

pub struct Login {
    pub name: &'static str,
    /// index of `url` in `urls`
    pub entry: usize,
    pub url: PoolUrl,
    pub urls: Vec<PoolUrl>,
    pub link: Arc<crate::failover::Link>,
    pub follow: Option<Arc<crate::failover::Link>>,
    pub src: Src,
    /// the user's login (address.worker), or the dev-fee login of a fee-only session
    pub login: String,
    pub password: String,
    /// dev-fee login authorized in the same session (pool mode); None for a gate / fee-only session
    pub fee_login: Option<String>,
    pub enabled: Arc<AtomicBool>,
    pub stop: Arc<AtomicBool>,
    pub dataset_hash: [u8; 32],
    /// the fee login is authorized on the current connection
    pub fee_ok: Arc<AtomicBool>,
    pub conn_seq: Arc<AtomicU64>,
    /// a token for `ffgn --gate` (sent as the password when set)
    pub fee_stats: Arc<Stats>,
    /// protocol of each entry (Auto entries learn theirs on the first connection)
    pub kinds: Arc<Mutex<Vec<Kind>>>,
    /// work-protocol session counters
    pub ns: Arc<super::node::NodeStats>,
    /// the session has a usable job (fee-only sessions: the fee slot may start)
    pub ready: Arc<AtomicBool>,
    /// the pool is an ffgn gate (solo through someone's node): the dev fee goes to the public fee pools
    pub gate: Arc<AtomicBool>,
}

pub fn spawn(l: Login, tx: Sender<(Src, Ev)>, rx: Receiver<Find>, stats: Arc<Stats>) {
    std::thread::Builder::new()
        .name(format!("neb-pool-{}", l.name))
        .spawn(move || {
            let mut l = l;
            let src = l.src;
            let lp = crate::failover::Loop {
                name: l.name,
                link: l.link.clone(),
                follow: l.follow.clone(),
                stop: l.stop.clone(),
                enabled: l.enabled.clone(),
                fee: src == Src::Fee,
                label: Box::new({
                    let u = l.urls.clone();
                    move |e| {
                        let x = &u[e.min(u.len() - 1)];
                        format!("{}:{}", x.host, x.port)
                    }
                }),
            };
            crate::failover::run(
                lp,
                &mut l,
                |l, e| {
                    l.url = l.urls[e].clone();
                    l.entry = e;
                },
                |l| session(l, &tx, &rx, &stats),
                |l| {
                    stats.connected.store(0, Ordering::Relaxed);
                    l.fee_ok.store(false, Ordering::Relaxed);
                    let _ = tx.send((l.src, Ev::Down));
                    while rx.try_recv().is_ok() {}
                },
                |_| while rx.try_recv().is_ok() {},
            );
        })
        .expect("spawn NEB pool thread");
}

/// failback / --pool-check: connect + subscribe + authorize + a usable job, nothing submitted
/// 1.0.3: the password of `mining.authorize`.  `--password` belongs to the first pool of the list (on HiveOS the Pass of the flight
/// sheet, for an ffgn gate its token) or to any ffgn gate (it says so in its subscribe answer, before the login); a backup pool
/// gets "x" (before 1.0.3 the gate token went to Suprnova / Aria while the gate was down).
pub fn auth_password(password: &str, entry: usize, is_gate: bool, supr: bool) -> String {
    if (entry == 0 || is_gate) && !password.is_empty() {
        return password.to_string();
    }
    if entry == 0 && !supr {
        return String::new();
    }
    "x".into()
}

#[cfg(test)]
mod pw_tests {
    use super::auth_password;
    #[test]
    fn the_password_goes_to_the_first_pool_or_a_gate_only() {
        // HiveOS: GATE first with the token, then Suprnova and Aria
        assert_eq!(auth_password("TOKEN", 0, true, false), "TOKEN");
        assert_eq!(auth_password("TOKEN", 1, false, true), "x");
        assert_eq!(auth_password("TOKEN", 2, false, false), "x");
        // a gate further down the list still gets it (it identifies itself before the login)
        assert_eq!(auth_password("TOKEN", 2, true, false), "TOKEN");
        // a pool password for the first pool; "x" / empty as before
        assert_eq!(auth_password("d=1024", 0, false, true), "d=1024");
        assert_eq!(auth_password("", 0, false, true), "x");
        assert_eq!(auth_password("", 0, false, false), "");
    }
}

pub fn probe(l: &Login, entry: usize, timeout: Duration) -> Result<(), String> {
    probe_job(l, entry, timeout, |_| Ok(()))
}

/// `probe` whose first job must also pass `accept` (1.0.3 --pool-check: a job of this dataset file's network); its Err is the verdict.
pub fn probe_job(l: &Login, entry: usize, timeout: Duration, accept: impl Fn(&NJob) -> Result<(), String>) -> Result<(), String> {
    let p = Login {
        name: "probe",
        url: l.urls[entry].clone(),
        urls: l.urls.clone(),
        link: crate::failover::Link::new(1),
        follow: None,
        src: Src::User,
        login: l.login.clone(),
        password: l.password.clone(),
        fee_login: None,
        enabled: Arc::new(AtomicBool::new(true)),
        stop: Arc::new(AtomicBool::new(false)),
        dataset_hash: l.dataset_hash,
        fee_ok: Arc::new(AtomicBool::new(false)),
        conn_seq: Arc::new(AtomicU64::new(1 << 40)),
        fee_stats: Arc::new(Stats::default()),
        kinds: l.kinds.clone(),
        ns: Arc::new(super::node::NodeStats::default()),
        ready: Arc::new(AtomicBool::new(false)),
        gate: Arc::new(AtomicBool::new(false)),
        entry,
    };
    let wrong: std::cell::RefCell<Option<String>> = std::cell::RefCell::new(None);
    let r = crate::failover::probe_session::<Ev, Find>(
        timeout,
        move |en, tx, rx| {
            let p = Login { enabled: en, ..p };
            session(&p, &tx, &rx, &Stats::default())
        },
        |e| match e {
            Ev::Job(j) => {
                if let Err(m) = accept(j) {
                    wrong.borrow_mut().get_or_insert(m);
                }
                true
            }
            _ => false,
        },
    );
    match wrong.into_inner() {
        Some(m) => Err(m),
        None => r,
    }
}

fn err(s: impl Into<String>) -> std::io::Error {
    std::io::Error::other(s.into())
}

fn short(line: &str) -> String {
    if line.len() > 300 { format!("{}...", &line[..300]) } else { line.to_string() }
}

/// a job of `mining.notify` (fields of spec/pool.md section 4)
fn parse_job(j: &Value, en_size: usize) -> Result<(String, Job, u8, [u8; 32]), String> {
    let s = |k: &str| j.get(k).and_then(Value::as_str).ok_or_else(|| format!("{k} missing"));
    let u = |k: &str| j.get(k).and_then(Value::as_u64).ok_or_else(|| format!("{k} missing")).and_then(|v| u32::try_from(v).map_err(|_| format!("{k} too large")));
    let b32 = |k: &str| s(k).and_then(|v| unhex32(v).ok_or_else(|| format!("{k} is not 32 bytes of hex")));
    let by = |k: &str| s(k).and_then(|v| unhex(v).ok_or_else(|| format!("{k} is not hex")));
    let branch: Vec<[u8; 32]> = j
        .get("branch")
        .and_then(Value::as_array)
        .ok_or("branch missing")?
        .iter()
        .map(|x| x.as_str().and_then(unhex32).ok_or_else(|| "branch entry is not 32 bytes".to_string()))
        .collect::<Result<_, _>>()?;
    if branch.len() > 4096 {
        return Err("branch too long".into());
    }
    let job_id = s("job_id")?.to_string();
    let slot = u("slot")?;
    let job = Job {
        id: 0,
        clean: j.get("clean").and_then(Value::as_bool).unwrap_or(false),
        version: u("version")?,
        parent: b32("parent")?,
        time_start: u("time_start")?,
        time_window: u("time_window")?,
        bits: u("bits")?,
        target: b32("target")?,
        share_target: b32("share_target")?,
        coinbase_prefix: by("coinbase_prefix")?,
        coinbase_suffix: by("coinbase_suffix")?,
        extranonce_size: en_size.min(255) as u8,
        branch,
        tx_count: u("tx_count")?,
        shares_root: b32("shares_root")?,
        height: u("height")?,
        expiry: u("expiry")?,
    };
    Ok((job_id, job, u8::try_from(slot).map_err(|_| "slot too large")?, b32("weights_hash")?))
}

fn session(l: &Login, tx: &Sender<(Src, Ev)>, rx: &Receiver<Find>, stats: &Stats) -> std::io::Result<()> {
    let kind = l.kinds.lock().unwrap().get(l.entry).copied().unwrap_or(Kind::Auto);
    if kind == Kind::Work || kind == Kind::TryWork {
        let nl = super::node::NodeLogin {
            addrs: vec![],
            addr: format!("{}:{}", l.url.host, l.url.port),
            link: l.link.clone(),
            stop: l.stop.clone(),
            enabled: l.enabled.clone(),
            name: l.login.clone(),
            src: l.src,
            pool: true,
            label: l.name.to_string(),
            ready: l.ready.clone(),
            conn_seq: l.conn_seq.clone(),
            held: Arc::new(Mutex::new(Vec::new())),
        };
        // the fee login of a work-protocol pool is a connection of its own (fee session), not a second login here
        l.fee_ok.store(l.fee_login.is_none() && l.src == Src::Fee, Ordering::Relaxed);
        let r = super::node::session(&nl, tx, rx, stats, &l.ns);
        if kind == Kind::TryWork {
            let got = l.ready.load(Ordering::Relaxed);
            if let Some(k) = l.kinds.lock().unwrap().get_mut(l.entry) {
                *k = if got { Kind::Work } else { Kind::Auto };
            }
        }
        l.ready.store(false, Ordering::Relaxed);
        return r;
    }
    eprintln!("[pool:{}] connecting to {} as {}", l.name, l.url, l.login);
    let mut c: Conn = connect(&l.url)?;
    let conn = l.conn_seq.fetch_add(1, Ordering::Relaxed) + 1;
    stats.connected.store(1, Ordering::Relaxed);
    let agent = format!("{}/{}", crate::PRODUCT.to_ascii_lowercase(), env!("CARGO_PKG_VERSION"));
    c.send(format!("{}\n", json!({"id":1,"method":"mining.subscribe","params":[agent, PROTOCOL]})).as_bytes())?;
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = vec![0u8; 1 << 16];
    let mut next_id = 10u64;
    let mut en1: Option<Vec<u8>> = None;
    let mut en2_size = 0usize;
    let mut range: Option<(u32, u64)> = None;
    let mut target: Option<[u8; 32]> = None;
    let mut models: HashMap<[u8; 32], (u8, Arc<Model>)> = HashMap::new();
    let mut model_order: Vec<[u8; 32]> = Vec::new();
    // id -> (fee login?, job id, sent)
    let mut pending: HashMap<u64, (bool, String, Instant)> = HashMap::new();
    let mut auth_ids: HashMap<u64, bool> = HashMap::new(); // id -> fee login
    let mut user_ok = false;
    let mut valid: Vec<String> = Vec::new();
    let mut sil = crate::silence::Silence::new(SILENCE_LIMIT);
    let mut drain = crate::failover::Drain::new();
    let fee_stats = &l.fee_stats;
    let mut raw_shown = 0;
    let mut supr = false;
    let mut is_gate = false;
    let mut rate_win = (Instant::now(), 0u32);
    let started = Instant::now();
    let mut answered = false;
    loop {
        if kind == Kind::Auto && !answered && started.elapsed() > Duration::from_secs(6) {
            l.kinds.lock().unwrap()[l.entry] = Kind::TryWork;
            return Err(err("no answer to mining.subscribe in 6 s: the next connection tries the NEB work protocol (as Aria)"));
        }
        if l.stop.load(Ordering::Relaxed) || !l.enabled.load(Ordering::Relaxed) {
            return Ok(());
        }
        if let Some(idle) = sil.expired() {
            return Err(crate::silence::Silence::error(idle));
        }
        while let Ok(f) = rx.try_recv() {
            let Origin::Pool { job_id } = &f.job.origin else { continue };
            if f.job.conn != conn || drain.frozen() || !valid.contains(job_id) {
                stats.stale_drop.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            // F-14: at most SUBMIT_RATE submits per second (a pool target far too easy), the rest dropped and counted
            if rate_win.0.elapsed() >= Duration::from_secs(1) {
                rate_win = (Instant::now(), 0);
            }
            rate_win.1 += 1;
            if rate_win.1 > SUBMIT_RATE && !f.block {
                stats.skipped.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            let fee = f.fee;
            let login = if fee { l.fee_login.clone().unwrap_or_else(|| l.login.clone()) } else { l.login.clone() };
            next_id += 1;
            let en2 = hex(&f.extranonce[f.job.en1.len()..]);
            let line = if supr {
                json!({"id": next_id, "method": "mining.submit", "params": [login, job_id, en2, format!("{:08x}", f.time), format!("{:08x}", f.nonce)]})
            } else {
                json!({"id": next_id, "method": "mining.submit", "params": [login, job_id, en2, f.time, f.nonce]})
            };
            pending.insert(next_id, (fee, job_id.clone(), Instant::now()));
            let st = if fee { &**fee_stats } else { stats };
            st.submitted.fetch_add(1, Ordering::Relaxed);
            if f.block {
                println!("[pool:{}] BLOCK candidate (ticket under the network target) submitted: height {} nonce {}", l.name, f.job.job.height, f.nonce);
            }
            c.send(format!("{line}\n").as_bytes())?;
        }
        if pending.len() > 256 {
            pending.retain(|_, (_, _, t)| t.elapsed() < Duration::from_secs(120));
        }
        if drain.check(&l.link, pending.is_empty()) {
            return Ok(());
        }
        match c.recv(&mut chunk)? {
            Some(0) => {
                if kind == Kind::Auto && !answered {
                    l.kinds.lock().unwrap()[l.entry] = Kind::TryWork;
                    return Err(err("the pool closed the connection without a JSON answer: the next connection tries the NEB work protocol"));
                }
                return Err(err("the pool closed the connection"));
            }
            Some(n) => {
                buf.extend_from_slice(&chunk[..n]);
                sil.rx();
            }
            None => {}
        }
        if buf.len() > 262_144 * 2 && !buf.contains(&b'\n') {
            return Err(err("a line from the pool is longer than 256 KiB"));
        }
        while let Some(p) = buf.iter().position(|&b| b == b'\n') {
            let raw: Vec<u8> = buf.drain(..=p).collect();
            let line = String::from_utf8_lossy(&raw);
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(line) else {
                if kind == Kind::Auto && !answered {
                    l.kinds.lock().unwrap()[l.entry] = Kind::TryWork;
                    return Err(err("the pool does not speak JSON: the next connection tries the NEB work protocol"));
                }
                eprintln!("[pool:{}] unparsable: {}", l.name, short(line));
                continue;
            };
            if !answered {
                answered = true;
                if kind == Kind::Auto {
                    l.kinds.lock().unwrap()[l.entry] = Kind::Json;
                }
            }
            let method = v.get("method").and_then(Value::as_str).unwrap_or("");
            let params = v.get("params").cloned().unwrap_or(Value::Null);
            match method {
                "mining.set_nonce_range" => {
                    // ffgn gate: this session's range of the reference header's nonces [base, log2 span]
                    range = params.as_array().and_then(|a| Some((a.first()?.as_u64()? as u32, 1u64 << a.get(1)?.as_u64()?.min(32))));
                    if let Some((b, sp)) = range {
                        eprintln!("[pool:{}] gate nonce range {b:#010x} + {sp}", l.name);
                    }
                }
                "mining.set_difficulty" => match params.get(0).and_then(Value::as_f64) {
                    Some(d) if d > 0.0 => target = Some(super::supr::target_of_difficulty(d)),
                    _ => eprintln!("[pool:{}] set_difficulty not understood: {}", l.name, short(line)),
                },
                "mining.set_target" => {
                    target = params.get(0).and_then(Value::as_str).and_then(unhex32);
                    if target.is_none() {
                        eprintln!("[pool:{}] set_target not understood: {}", l.name, short(line));
                    }
                }
                "mining.set_model" => {
                    match super::supr::parse_model(&params) {
                        Ok((s, h, w)) => match Model::from_packed(&w, &h) {
                            Ok(m) => {
                                models.insert(h, (s as u8, Arc::new(m)));
                                model_order.retain(|x| x != &h);
                                model_order.push(h);
                                while model_order.len() > 8 {
                                    let o = model_order.remove(0);
                                    models.remove(&o);
                                }
                            }
                            Err(e) => eprintln!("[pool:{}] model refused: {e}", l.name),
                        },
                        Err(e) => eprintln!("[pool:{}] set_model not understood: {e}", l.name),
                    }
                }
                "mining.notify" => {
                    if raw_shown < 1 {
                        raw_shown += 1;
                        eprintln!("[pool:{}] first job: {}", l.name, short(line));
                    }
                    let Some(e1) = en1.clone() else {
                        eprintln!("[pool:{}] job before the subscription result: ignored", l.name);
                        continue;
                    };
                    let parsed = if supr || params.get(0).is_some_and(Value::is_string) {
                        super::supr::parse_notify(&params, e1.len() + en2_size, 0).map(|sj| {
                            let mut job = sj.job;
                            job.target = super::supr::compact_target(job.bits);
                            // share target = block target x 32 (spec/shares.md 11.2)
                            job.share_target = shl5(&job.target);
                            (sj.job_id, job, sj.slot, sj.weights_hash)
                        })
                    } else {
                        parse_job(params.get(0).unwrap_or(&Value::Null), e1.len() + en2_size)
                    };
                    match parsed {
                        Ok((job_id, job, slot, wh)) => {
                            if job.clean {
                                valid.clear();
                            }
                            valid.retain(|x| x != &job_id);
                            valid.push(job_id.clone());
                            if valid.len() > 16 {
                                valid.remove(0);
                            }
                            let Some((mslot, m)) = models.get(&wh).cloned() else {
                                eprintln!("[pool:{}] job {job_id} names a model the pool did not send: not mined", l.name);
                                continue;
                            };
                            if mslot != slot {
                                eprintln!("[pool:{}] job {job_id}: slot {slot} but its model came for slot {mslot}: not mined", l.name);
                                continue;
                            }
                            let Some(t) = target else {
                                eprintln!("[pool:{}] job {job_id} before any set_target: not mined", l.name);
                                continue;
                            };
                            // the pool target is never below the share target (spec section 7.3): a lower one would lose chain shares
                            let pt = if proto::admits(&t, &job.share_target) { t } else { job.share_target };
                            let nj = NJob {
                                src: l.src,
                                origin: Origin::Pool { job_id },
                                slot,
                                model: m,
                                pool_target: Some(pt),
                                en1: e1.clone(),
                                en2_size,
                                nonce_range: range,
                                conn,
                                received: Instant::now(),
                        ref_next: AtomicU64::new(0),
                        ref_base: super::random_base(),
                                job,
                            };
                            l.ready.store(true, Ordering::Relaxed);
                            let _ = tx.send((l.src, Ev::Job(Arc::new(nj))));
                        }
                        Err(e) => eprintln!("[pool:{}] job not understood ({e}): {}", l.name, short(line)),
                    }
                }
                "" => {
                    let id = v.get("id").and_then(Value::as_u64).unwrap_or(0);
                    let ok = v.get("result").is_some_and(|r| r.as_bool().unwrap_or(!r.is_null())) && v.get("error").map_or(true, Value::is_null);
                    let etext = v.get("error").filter(|e| !e.is_null()).map(|e| e.to_string()).unwrap_or_default();
                    if id == 1 {
                        let mut r = v.get("result").cloned().unwrap_or(Value::Null);
                        if !ok {
                            return Err(err(format!("subscribe refused: {etext}")));
                        }
                        if let Some(a) = r.as_array().cloned() {
                            // classic stratum result (Suprnova): [[subscriptions], extranonce1, extranonce2_size]
                            supr = true;
                            r = json!({"extranonce1": a.get(1).cloned().unwrap_or(Value::Null), "extranonce2_size": a.get(2).cloned().unwrap_or(Value::Null), "session": "stratum"});
                        }
                        let e1 = r.get("extranonce1").and_then(Value::as_str).and_then(unhex).ok_or_else(|| err("subscribe: no extranonce1"))?;
                        en2_size = r.get("extranonce2_size").and_then(Value::as_u64).ok_or_else(|| err("subscribe: no extranonce2_size"))? as usize;
                        if e1.len() + en2_size == 0 || e1.len() + en2_size > 32 {
                            return Err(err(format!("subscribe: extranonce {} + {en2_size} bytes is out of range", e1.len())));
                        }
                        if let Some(dh) = r.get("dataset_hash").and_then(Value::as_str).and_then(unhex32) {
                            if dh != l.dataset_hash {
                                // F-19: a pool of another network is no use: with one pool the miner stops, with a list it fails over
                                let msg = format!(
                                    "the pool works on another dataset file (BLAKE3 {}…, ours {}…): wrong network for this pool / dataset",
                                    &hex(&dh)[..16],
                                    &hex(&l.dataset_hash)[..16]
                                );
                                if l.urls.len() == 1 && l.src == Src::User {
                                    super::fatal(&msg);
                                }
                                return Err(err(msg));
                            }
                        }
                        // an ffgn gate (the nonce range comes after the login: mining.set_nonce_range)
                        is_gate = r.get("ffgn_gate").is_some();
                        en1 = Some(e1);
                        l.gate.store(is_gate, Ordering::Relaxed);
                        eprintln!(
                            "[pool:{}] subscribed: session {} extranonce2 {} bytes{}",
                            l.name,
                            r.get("session").and_then(Value::as_str).unwrap_or("?"),
                            en2_size,
                            range.map_or(String::new(), |(b, s)| format!(", gate nonce range {b:#010x} + {s}"))
                        );
                        c.send(format!("{}\n", json!({"id":2,"method":"mining.authorize","params":[l.login, auth_password(&l.password, l.entry, is_gate, supr)]})).as_bytes())?;
                        auth_ids.insert(2, false);
                        if let Some(f) = l.fee_login.as_ref().filter(|_| !is_gate) {
                            c.send(format!("{}\n", json!({"id":3,"method":"mining.authorize","params":[f, "x"]})).as_bytes())?;
                            auth_ids.insert(3, true);
                        }
                    } else if let Some(fee) = auth_ids.remove(&id) {
                        if fee {
                            l.fee_ok.store(ok, Ordering::Relaxed);
                            if !ok {
                                eprintln!("[pool:{}] the dev-fee login was refused ({etext}): the fee slots are skipped on this pool", l.name);
                            }
                        } else if ok {
                            user_ok = true;
                            if l.fee_login.is_none() && l.src == Src::Fee {
                                l.fee_ok.store(true, Ordering::Relaxed);
                            }
                            eprintln!("[pool:{}] authorized {}", l.name, l.login);
                        } else {
                            return Err(err(format!("login {} refused: {etext}", l.login)));
                        }
                    } else if let Some((fee, job_id, sent)) = pending.remove(&id) {
                        let st = if fee { &**fee_stats } else { stats };
                        let who = if fee { " (dev fee)" } else { "" };
                        if ok {
                            st.accepted.fetch_add(1, Ordering::Relaxed);
                            println!("[pool:{}] share ACCEPTED{who} (job {job_id}, {} ms)", l.name, sent.elapsed().as_millis());
                        } else {
                            st.rejected.fetch_add(1, Ordering::Relaxed);
                            if etext.contains("21") || etext.to_lowercase().contains("expir") || etext.to_lowercase().contains("not found") {
                                st.stale_rej.fetch_add(1, Ordering::Relaxed);
                            }
                            println!("[pool:{}] share REJECTED{who} (job {job_id}): {etext}", l.name);
                        }
                        sil.answered();
                    }
                }
                _ => eprintln!("[pool:{}] ignored: {}", l.name, short(line)),
            }
        }
        let _ = user_ok;
    }
}

/// The fee login's stats live in `Login::fee_stats`; a mutex-free snapshot helper for the stats line.
#[allow(dead_code)]
pub fn snapshot(s: &Stats) -> (u64, u64, u64) {
    (s.submitted.load(Ordering::Relaxed), s.accepted.load(Ordering::Relaxed), s.rejected.load(Ordering::Relaxed))
}

/// v * 32 (saturating at 2^256 - 1), little-endian
pub fn shl5(v: &[u8; 32]) -> [u8; 32] {
    if v[31] >> 3 != 0 {
        return [0xff; 32];
    }
    let mut o = [0u8; 32];
    for i in (0..32).rev() {
        o[i] = (v[i] << 5) | if i > 0 { v[i - 1] >> 3 } else { 0 };
    }
    o
}
