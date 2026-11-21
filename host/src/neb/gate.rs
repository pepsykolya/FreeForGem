// FFGN — see LICENSE
//! `ffgn --gate`: the NEB work service of a node, served to rigs over the network.
//!
//! The node's work service listens on 127.0.0.1 without authentication (spec/node.md 7.1), so a rig without its own node
//! cannot reach it.  The gate runs next to the node, is one miner of it (Hello "ffgn-gate"), and speaks neb-pool/1
//! (spec/pool.md) to the rigs, so the miner has one network client for gates and pools:
//!   * authentication: the password of `mining.authorize` must be the gate token (`--gate-token-file`); the login is free
//!     text (the node's `--miner-lock` gets every reward);
//!   * every session mines the node's REFERENCE header (extranonce of zero bytes, `time_start`), each on its own range of
//!     the 2^32 nonces (`ffgn_nonce: [base, log2 span]` in the subscribe result), so its chain shares can go to the node as
//!     Share frames (a Share frame can only name a nonce of the reference header, spec/shares.md 11.5);
//!   * the pool target of a session is the share target: every submit is a chain share; the gate evaluates it again with
//!     the C reference, sends it to the node (Share), and a solution also as Submit; the node's Result is logged.
//! A new Job of the node (new block, 30 s refresh) gives a new `time_start` and so a fresh header for every range.
use super::proto::{self, hex, unhex};
use super::refc::{self, DatasetFile};
use super::{Ev, Find, NJob, Origin};
use crate::stratum::{Src, Stats};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub struct Opts {
    pub listen: String,
    pub node: Vec<String>,
    pub token: String,
    pub dataset: Arc<DatasetFile>,
    /// log2 of the number of nonce ranges (sessions at most)
    pub bits: u32,
    pub minutes: Option<f64>,
}

struct Sess {
    id: u64,
    /// extranonce size the session was given (F-01): a job of another size closes the session (the rig subscribes again)
    en: usize,
    kick: Arc<AtomicBool>,
    out: mpsc::Sender<String>,
    models: HashSet<[u8; 32]>,
    authorized: bool,
    worker: String,
}

struct Shared {
    sessions: Mutex<Vec<Sess>>,
    jobs: Mutex<VecDeque<(String, Arc<NJob>)>>,
    current: Mutex<Option<Arc<NJob>>>,
    slots: Mutex<Vec<bool>>,
    find_tx: Mutex<mpsc::Sender<Find>>,
    ds: Arc<DatasetFile>,
    token: String,
    bits: u32,
    shares: AtomicU64,
    blocks: AtomicU64,
    bad: AtomicU64,
    per_worker: Mutex<HashMap<String, u64>>,
    /// open connections (F-05: bounded)
    conns: std::sync::atomic::AtomicUsize,
}

fn job_id(j: &NJob) -> String {
    match j.origin {
        Origin::Node { id } => format!("{id:x}"),
        _ => String::new(),
    }
}

fn notify_lines(s: &mut Sess, j: &NJob) -> Vec<String> {
    let mut v = Vec::new();
    if s.models.insert(j.model.hash) {
        v.push(json!({"id": null, "method": "mining.set_model", "params": [j.slot, hex(&j.model.hash), hex(&j.model.packed)]}).to_string());
        if s.models.len() > 8 {
            s.models.clear();
            s.models.insert(j.model.hash);
        }
    }
    let job = &j.job;
    v.push(json!({"id": null, "method": "mining.set_target", "params": [hex(&job.share_target)]}).to_string());
    v.push(
        json!({"id": null, "method": "mining.notify", "params": [{
            "job_id": job_id(j), "clean": true, "version": job.version, "parent": hex(&job.parent), "time_start": job.time_start,
            "time_window": job.time_window, "bits": job.bits, "target": hex(&job.target), "share_target": hex(&job.share_target),
            "coinbase_prefix": hex(&job.coinbase_prefix), "coinbase_suffix": hex(&job.coinbase_suffix),
            "branch": job.branch.iter().map(|b| hex(b)).collect::<Vec<_>>(), "tx_count": job.tx_count, "shares_root": hex(&job.shares_root),
            "height": job.height, "slot": j.slot, "weights_hash": hex(&j.model.hash), "expiry": job.expiry
        }]})
        .to_string(),
    );
    v
}

pub fn run(o: Opts) -> Result<()> {
    ensure!(o.token.len() >= 16, "the gate token must have at least 16 characters (--gate-token-file)");
    ensure!((1..=16).contains(&o.bits), "--gate-slots-bits must be 1..16");
    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = stop.clone();
        ctrlc::set_handler(move || {
            if stop.swap(true, Ordering::SeqCst) {
                crate::conout::exit(130);
            }
            eprintln!("gate: stopping...");
        })?;
    }
    let listener = TcpListener::bind(&o.listen).with_context(|| format!("gate: cannot listen on {}", o.listen))?;
    listener.set_nonblocking(true)?;
    let (ev_tx, ev_rx) = mpsc::channel::<(Src, Ev)>();
    let (find_tx, find_rx) = mpsc::channel::<Find>();
    let stats = Arc::new(Stats::default());
    let ns = Arc::new(super::node::NodeStats::default());
    let link = crate::failover::Link::new(o.node.len());
    super::node::spawn(
        super::node::NodeLogin { addrs: o.node.clone(), addr: o.node[0].clone(), link: link.clone(), stop: stop.clone(), enabled: Arc::new(AtomicBool::new(true)), name: format!("ffgn-gate/{}", env!("CARGO_PKG_VERSION")), src: Src::User, pool: false, label: String::new(), ready: Arc::new(AtomicBool::new(false)), conn_seq: Arc::new(AtomicU64::new(0)), held: Arc::new(Mutex::new(Vec::new())) },
        ev_tx,
        find_rx,
        stats.clone(),
        ns.clone(),
    );
    let sh = Arc::new(Shared {
        sessions: Mutex::new(Vec::new()),
        jobs: Mutex::new(VecDeque::new()),
        current: Mutex::new(None),
        slots: Mutex::new(vec![false; 1 << o.bits]),
        find_tx: Mutex::new(find_tx),
        ds: o.dataset.clone(),
        token: o.token.clone(),
        bits: o.bits,
        shares: AtomicU64::new(0),
        blocks: AtomicU64::new(0),
        bad: AtomicU64::new(0),
        per_worker: Mutex::new(HashMap::new()),
        conns: std::sync::atomic::AtomicUsize::new(0),
    });
    println!("{} gate {} | listening on {} | node {} | {} nonce ranges of 2^{} | dataset {} ({})", crate::PRODUCT, env!("CARGO_PKG_VERSION"), o.listen, o.node.join(","), 1u32 << o.bits, 32 - o.bits, o.dataset.network, &hex(&o.dataset.hash)[..16]);
    let t0 = Instant::now();
    let mut last = Instant::now();
    let mut next_sess = 1u64;
    while !stop.load(Ordering::Relaxed) && o.minutes.map_or(true, |m| t0.elapsed().as_secs_f64() < m * 60.0) {
        match listener.accept() {
            Ok((s, peer)) => {
                // F-05: at most one connection per nonce range plus a few being set up
                let limit = (1usize << o.bits) + 8;
                if sh.conns.load(Ordering::SeqCst) >= limit {
                    eprintln!("gate: {peer} refused: {limit} connections open");
                    drop(s);
                    continue;
                }
                sh.conns.fetch_add(1, Ordering::SeqCst);
                let (sh2, id) = (sh.clone(), next_sess);
                next_sess += 1;
                let stop2 = stop.clone();
                std::thread::Builder::new().name(format!("gate-{id}")).spawn(move || {
                    if let Err(e) = serve(&sh2, s, id, &stop2) {
                        eprintln!("gate: session {id} ({peer}): {e:#}");
                    }
                    sh2.sessions.lock().unwrap().retain(|x| x.id != id);
                    sh2.conns.fetch_sub(1, Ordering::SeqCst);
                })?;
                println!("gate: session {id} from {peer}");
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => eprintln!("gate: accept: {e}"),
        }
        for (_, ev) in ev_rx.try_iter() {
            match ev {
                Ev::Job(j) => {
                    {
                        let mut q = sh.jobs.lock().unwrap();
                        q.push_back((job_id(&j), j.clone()));
                        while q.len() > 64 {
                            q.pop_front();
                        }
                    }
                    let new_block = sh.current.lock().unwrap().as_ref().map_or(true, |c| c.job.parent != j.job.parent);
                    if new_block {
                        println!("gate: job height {} slot {} ({}) parent {}…", j.job.height, j.slot, j.model.arch.name(), &hex(&j.job.parent)[..16]);
                    }
                    *sh.current.lock().unwrap() = Some(j.clone());
                    for s in sh.sessions.lock().unwrap().iter_mut().filter(|s| s.authorized) {
                        if s.en != j.job.extranonce_size as usize {
                            s.kick.store(true, Ordering::SeqCst); // F-01: the rig's coinbase would be wrong
                            continue;
                        }
                        for l in notify_lines(s, &j) {
                            let _ = s.out.send(l);
                        }
                    }
                }
                Ev::Down => {
                    *sh.current.lock().unwrap() = None;
                    println!("gate: the node is down: rigs keep their job until it is back");
                }
            }
        }
        if last.elapsed() >= Duration::from_secs(60) {
            last = Instant::now();
            let w = sh.per_worker.lock().unwrap();
            let mut ws: Vec<String> = w.iter().map(|(k, v)| format!("{k} {v}")).collect();
            ws.sort();
            println!(
                "gate [{}s]: {} session(s) | shares {} (bad {}) | blocks sub {} acc {} rej {} | node {} | {}",
                t0.elapsed().as_secs(),
                sh.sessions.lock().unwrap().len(),
                sh.shares.load(Ordering::Relaxed),
                sh.bad.load(Ordering::Relaxed),
                stats.submitted.load(Ordering::Relaxed),
                stats.accepted.load(Ordering::Relaxed),
                stats.rejected.load(Ordering::Relaxed),
                if stats.connected.load(Ordering::Relaxed) == 1 { "up" } else { "DOWN" },
                ws.join(", ")
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    stop.store(true, Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(300));
    Ok(())
}

fn serve(sh: &Arc<Shared>, s: TcpStream, id: u64, stop: &AtomicBool) -> Result<()> {
    s.set_nodelay(true)?;
    s.set_read_timeout(Some(Duration::from_millis(200)))?;
    let mut w = s.try_clone()?;
    let (otx, orx) = mpsc::channel::<String>();
    let wstop = Arc::new(AtomicBool::new(false));
    {
        let wstop = wstop.clone();
        std::thread::spawn(move || {
            while !wstop.load(Ordering::Relaxed) {
                match orx.recv_timeout(Duration::from_millis(200)) {
                    Ok(l) => {
                        if w.write_all(format!("{l}\n").as_bytes()).is_err() {
                            break;
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(_) => break,
                }
            }
            let _ = w.shutdown(std::net::Shutdown::Both);
        });
    }
    let _g = scopeguard(wstop.clone());
    s.set_read_timeout(Some(Duration::from_millis(200)))?;
    let mut slot: Option<usize> = None;
    let res = session_loop(sh, s, id, stop, &otx, &mut slot);
    if let Some(i) = slot {
        sh.slots.lock().unwrap()[i] = false;
    }
    res
}

struct Flag(Arc<AtomicBool>);
impl Drop for Flag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}
fn scopeguard(f: Arc<AtomicBool>) -> Flag {
    Flag(f)
}

fn reply(out: &mpsc::Sender<String>, id: &Value, ok: bool, code: i64, msg: &str) {
    let l = if ok { json!({"id": id, "result": true, "error": null}) } else { json!({"id": id, "result": null, "error": [code, msg]}) };
    let _ = out.send(l.to_string());
}

/// Reads one line (at most MAX_LINE bytes; F-05) without losing a partial line across read timeouts (F-15).
const MAX_LINE: usize = 8192;

fn session_loop(sh: &Arc<Shared>, mut s: TcpStream, id: u64, stop: &AtomicBool, out: &mpsc::Sender<String>, slot: &mut Option<usize>) -> Result<()> {
    let mut pending: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    let mut subscribed: Option<usize> = None; // the extranonce size given
    let mut authorized = false;
    let mut worker = String::new();
    let mut seen: HashSet<(String, Vec<u8>, u32)> = HashSet::new();
    let mut sc = refc::Scratch::default();
    let mut window = (Instant::now(), 0u32);
    let mut bad = 0u32;
    let started = Instant::now();
    let kick = Arc::new(AtomicBool::new(false));
    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        if kick.load(Ordering::SeqCst) {
            bail!("the node's extranonce size changed: the rig subscribes again");
        }
        if !authorized && started.elapsed() > Duration::from_secs(10) {
            bail!("not authorized within 10 s");
        }
        let line = match pending.iter().position(|&b| b == b'\n') {
            Some(p) => {
                let l: Vec<u8> = pending.drain(..=p).collect();
                l
            }
            None => {
                if pending.len() > MAX_LINE {
                    bail!("line too long");
                }
                match std::io::Read::read(&mut s, &mut chunk) {
                    Ok(0) => return Ok(()),
                    Ok(n) => pending.extend_from_slice(&chunk[..n]),
                    Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut | std::io::ErrorKind::Interrupted) => {}
                    Err(e) => return Err(e.into()),
                }
                continue;
            }
        };
        if line.len() > MAX_LINE {
            bail!("line too long");
        }
        let text = String::from_utf8_lossy(&line);
        if text.trim().is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(text.trim()) else { bail!("not JSON") };
        let rid = v.get("id").cloned().unwrap_or(Value::Null);
        let p = v.get("params").cloned().unwrap_or(Value::Null);
        match v.get("method").and_then(Value::as_str).unwrap_or("") {
            "mining.subscribe" => {
                // F-01: the extranonce size comes from a job of the node; without one the rig is told to come back
                let mut cur = sh.current.lock().unwrap().clone();
                let t = Instant::now();
                while cur.is_none() && t.elapsed() < Duration::from_secs(4) && !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(200));
                    cur = sh.current.lock().unwrap().clone();
                }
                let Some(cur) = cur else {
                    reply(out, &rid, false, 20, "the gate has no job from its node yet: try again");
                    bail!("no job from the node yet");
                };
                let en = cur.job.extranonce_size as usize;
                subscribed = Some(en);
                // no nonce range before the login (F-05): it comes with mining.set_nonce_range after authorize
                let _ = out.send(
                    json!({"id": rid, "error": null, "result": {
                        "session": format!("{id:x}"), "extranonce1": "", "extranonce2_size": en,
                        "chain_id": "", "dataset_hash": hex(&sh.ds.hash), "ffgn_gate": 1
                    }})
                    .to_string(),
                );
            }
            "mining.authorize" => {
                let login = p.get(0).and_then(Value::as_str).unwrap_or("").to_string();
                let pass = p.get(1).and_then(Value::as_str).unwrap_or("");
                let Some(en) = subscribed else {
                    reply(out, &rid, false, 25, "session not subscribed");
                    continue;
                };
                if !ct_eq(pass.as_bytes(), sh.token.as_bytes()) {
                    reply(out, &rid, false, 24, "wrong gate token");
                    if authorized {
                        continue; // a second login of an authorized session (e.g. a dev-fee login): refused, the session stays
                    }
                    bail!("wrong gate token (login {login})");
                }
                if authorized {
                    reply(out, &rid, true, 0, "");
                    continue;
                }
                // one nonce range per session, given at its login (F-05)
                let i = {
                    let mut sl = sh.slots.lock().unwrap();
                    let Some(i) = sl.iter().position(|x| !x) else {
                        reply(out, &rid, false, 20, "all nonce ranges of this gate are in use");
                        bail!("all {} nonce ranges are in use", sl.len());
                    };
                    sl[i] = true;
                    i
                };
                *slot = Some(i);
                authorized = true;
                worker = login.chars().filter(|c| c.is_ascii_graphic()).take(128).collect();
                reply(out, &rid, true, 0, "");
                let base = (i as u64) << (32 - sh.bits);
                let _ = out.send(json!({"id": null, "method": "mining.set_nonce_range", "params": [base, 32 - sh.bits]}).to_string());
                println!("gate: session {id} authorized as {worker} (nonce range {i})");
                let mut ss = Sess { id, en, kick: kick.clone(), out: out.clone(), models: HashSet::new(), authorized: true, worker: worker.clone() };
                if let Some(j) = sh.current.lock().unwrap().clone() {
                    if j.job.extranonce_size as usize == en {
                        for l in notify_lines(&mut ss, &j) {
                            let _ = out.send(l);
                        }
                    }
                }
                sh.sessions.lock().unwrap().push(ss);
            }
            "mining.submit" => {
                if !authorized {
                    reply(out, &rid, false, 24, "not authorized");
                    continue;
                }
                if window.0.elapsed() >= Duration::from_secs(1) {
                    window = (Instant::now(), 0);
                }
                window.1 += 1;
                if window.1 > 20 {
                    reply(out, &rid, false, 20, "too many submits");
                    continue;
                }
                let jid = p.get(1).and_then(Value::as_str).unwrap_or("").to_string();
                let en2 = p.get(2).and_then(Value::as_str).and_then(unhex);
                let time = p.get(3).and_then(Value::as_u64).unwrap_or(0) as u32;
                let nonce = p.get(4).and_then(Value::as_u64).unwrap_or(u64::MAX);
                let job = sh.jobs.lock().unwrap().iter().rev().find(|(k, _)| *k == jid).map(|x| x.1.clone());
                // the job's lifetime from its arrival at the gate (F-03), not the clocks
                let Some(j) = job.filter(|j| !j.expired()) else {
                    reply(out, &rid, false, 21, "job not found or past its expiry");
                    continue;
                };
                let Some(en2) = en2.filter(|e| e.len() == j.job.extranonce_size as usize) else {
                    reply(out, &rid, false, 27, "extranonce2 of the wrong length");
                    continue;
                };
                if time != j.job.time_start {
                    reply(out, &rid, false, 26, "time must be the job's time_start on this gate");
                    continue;
                }
                let Ok(nonce) = u32::try_from(nonce) else {
                    reply(out, &rid, false, 20, "bad nonce");
                    continue;
                };
                // the reference header (all-zero extranonce): this session's nonce range, shares and solutions; any other
                // extranonce (F-02, the range is used up): solutions only
                let reference = en2.iter().all(|&b| b == 0);
                let i = slot.unwrap_or(0) as u64;
                if reference && (nonce as u64) >> (32 - sh.bits) != i {
                    reply(out, &rid, false, 20, "nonce outside the range of this session");
                    continue;
                }
                if !seen.insert((jid.clone(), en2.clone(), nonce)) {
                    reply(out, &rid, false, 22, "duplicate attempt");
                    continue;
                }
                if seen.len() > 100_000 {
                    seen.clear();
                }
                let Some(ds) = refc::resolve_dataset(&sh.ds.sets, j.slot, j.model.arch).ok().map(|i| &sh.ds.sets[i]) else {
                    reply(out, &rid, false, 20, "unknown dataset");
                    continue;
                };
                let header = j.job.header(&en2, j.job.time_start, 0);
                let t = match refc::ticket_of(&mut sc, &j.model, ds, &header, nonce) {
                    Ok(t) => t,
                    Err(e) => {
                        reply(out, &rid, false, 20, &e);
                        continue;
                    }
                };
                let need = if reference { &j.job.share_target } else { &j.job.target };
                if !proto::admits(need, &t) {
                    sh.bad.fetch_add(1, Ordering::Relaxed);
                    bad += 1;
                    reply(out, &rid, false, 23, if reference { "ticket above the share target" } else { "off the reference header only a solution is accepted" });
                    if bad > 20 {
                        bail!("too many tickets above the target");
                    }
                    continue;
                }
                let block = proto::admits(&j.job.target, &t);
                if reference {
                    sh.shares.fetch_add(1, Ordering::Relaxed);
                    *sh.per_worker.lock().unwrap().entry(worker.clone()).or_default() += 1;
                }
                if block {
                    sh.blocks.fetch_add(1, Ordering::Relaxed);
                    println!("gate: BLOCK from {worker}: height {} nonce {nonce}{}", j.job.height, if reference { "" } else { " (off the reference header)" });
                }
                let _ = sh.find_tx.lock().unwrap().send(Find { job: j.clone(), extranonce: en2, time, nonce, ticket: t, share: reference, block, fee: false, found: std::time::Instant::now(), checked: std::time::Instant::now() });
                reply(out, &rid, true, 0, "");
            }
            "mining.extranonce.subscribe" | "mining.hashrate" => reply(out, &rid, true, 0, ""),
            m => reply(out, &rid, false, 20, &format!("unknown method {m}")),
        }
    }
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |x, (p, q)| x | (p ^ q)) == 0
}

/// reads a token file (first line, trimmed)
pub fn read_token(path: &str) -> Result<String> {
    let t = std::fs::read_to_string(path).with_context(|| format!("cannot read the gate token file {path}"))?;
    Ok(t.lines().next().unwrap_or("").trim().to_string())
}

#[allow(dead_code)]
fn _unused(s: &Sess) -> &str {
    &s.worker
}
