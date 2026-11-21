// FFGT — see LICENSE
//! ffgt-gate (`ffgt --gate CONFIG.json`): a small stratum server beside one or more Ting nodes.  Rigs connect to it as to the
//! crypto-connection.net pool (the same Ting stratum, pool.rs) over LAN / tailscale; the gate maps each rig (worker) to a node,
//! so different rigs mine to different payout accounts (each node its own data dir, ports and `ting miner payout`).  The
//! nodes' control tokens never leave this machine: the gate reads them and talks to the nodes on loopback.
//!
//! CONFIG.json:
//!   { "listen": "100.64.0.1:4073",                      // LAN / tailscale address (never 0.0.0.0 on a public host)
//!     "difficulty": 4,                                  // start share difficulty (vardiff: ~10 s per share, 0.1 .. 100000)
//!     "nodes":   { "main": {"control": "127.0.0.1:18440", "token_file": "/var/lib/ting/control.token"},
//!                  "b":    {"control": "127.0.0.1:18470", "token_file": "/var/lib/ting-b/control.token"} },
//!     "workers": { "rig02": {"node": "main", "password": "<per-rig secret>"},
//!                  "rig05": {"node": "b",    "password": "<another secret>"} },
//!     "api": "127.0.0.1:4080" }                         // optional: GET /summary (workers, nodes, blocks)
//! A rig logs in with `mining.authorize ["<anything>.<worker>", "<its password>"]` (the part before the dot is ignored: a pool
//! wallet field may hold anything); unknown workers and wrong passwords are refused.
//! Jobs: per node the gate polls `status` every 250 ms and takes a template on a new tip (clean job) or every 20 s; job =
//! [id, header[0..278], height, network target, clean].  Each connection gets its own 4-byte extranonce1 (nonce bytes
//! 12..15).  A share is re-hashed here; one that meets the network target goes to its node as `solution`.
//! 0.2.2 (template grouping): a template per second; every new one goes out as a job with clean = false, so a rig's FFGT
//! keeps up to 7 jobs of the tip (its Ring, node.rs) and searches them together; a block on any of them goes to the node
//! with that job's template id.  The node keeps 8 templates per tip: no other client may take templates from a node a
//! gate serves (a second miner on it evicts the rigs' templates).
use super::node::{self, Template};
use super::pool::diff_to_target;
use super::pow;
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

const KEEP_JOBS: usize = 16;
/// jobs re-sent to a rig after a difficulty change (its group restarts with the new share target): its Ring size
const RESEND: usize = super::node::GROUP_KEEP;
/// a share found just before a difficulty change still counts at the previous difficulty for this long
const DIFF_GRACE: Duration = Duration::from_secs(10);
const TARGET_SECS: f64 = 10.0;
const RETARGET: Duration = Duration::from_secs(90);

struct GJob {
    id: String,
    seq: u64,
    tpl: Arc<Template>,
    clean: bool,
}

struct NodeState {
    name: String,
    node: node::Node,
    jobs: RwLock<VecDeque<Arc<GJob>>>, // newest last
    version: AtomicU64,
    up: AtomicBool,
    blocks: Mutex<Vec<(u64, String, String)>>, // (height, id, worker)
    rejected: AtomicU64,
}

#[derive(Default)]
struct WorkerStat {
    shares: u64,
    rejected: u64,
    work: f64, // sum of share difficulties x 2^32 (hashes)
    since: Option<Instant>,
    last: Option<Instant>,
    blocks: u64,
    conns: u32,
    recent: VecDeque<(Instant, f64)>, // the shares of the last RECENT (time, hashes): the current rate, without the gaps
}

/// window of the recent hash rate estimate (the lifetime one counts the time a rig was away, e.g. on another coin)
const RECENT: Duration = Duration::from_secs(900);

struct Gate {
    nodes: HashMap<String, Arc<NodeState>>,
    workers: HashMap<String, (String, String)>, // worker -> (node, password)
    diff0: f64,
    en1: AtomicU32,
    job_seq: AtomicU64,
    stats: Mutex<HashMap<String, WorkerStat>>,
    started: Instant,
}

fn log(s: &str) {
    println!("[gate] {s}");
}

pub fn run(config: &str) -> Result<()> {
    let c: Value = serde_json::from_str(&std::fs::read_to_string(config).with_context(|| format!("read {config}"))?).with_context(|| format!("{config}: not JSON"))?;
    // one address or a list (LAN and tailscale)
    let listen: Vec<String> = match &c["listen"] {
        Value::String(x) => vec![x.clone()],
        Value::Array(a) => a.iter().filter_map(|x| x.as_str().map(String::from)).collect(),
        _ => bail!("config: \"listen\" missing"),
    };
    ensure!(!listen.is_empty(), "config: \"listen\" is empty");
    for a in &listen {
        ensure!(!a.starts_with("0.0.0.0") || c["allow_any"] == true, "config: listen on a LAN / tailscale address, not 0.0.0.0 (\"allow_any\": true to override)");
    }
    let mut nodes = HashMap::new();
    for (name, v) in c["nodes"].as_object().context("config: \"nodes\" missing")? {
        let n = node::resolve(v["control"].as_str(), None, v["token_file"].as_str(), false).with_context(|| format!("node {name}"))?;
        let (_, h, _, st) = node::tip(&n).with_context(|| format!("node {name} ({}) does not answer", n.addr))?;
        log(&format!("node {name}: {} height {h}, blocks pay {}", n.addr, node::payout(&st).unwrap_or_else(|| "NOTHING (no payout set!)".into())));
        ensure!(st["producer_hex"].as_str().is_some_and(|p| !p.is_empty()), "node {name} has no payout account (`ting miner payout <account>`)");
        nodes.insert(
            name.clone(),
            Arc::new(NodeState { name: name.clone(), node: n, jobs: RwLock::new(VecDeque::new()), version: AtomicU64::new(0), up: AtomicBool::new(false), blocks: Mutex::new(vec![]), rejected: AtomicU64::new(0) }),
        );
    }
    let mut workers = HashMap::new();
    for (w, v) in c["workers"].as_object().context("config: \"workers\" missing")? {
        let n = v["node"].as_str().context("worker without \"node\"")?;
        ensure!(nodes.contains_key(n), "worker {w}: unknown node {n}");
        let pw = v["password"].as_str().unwrap_or("");
        ensure!(pw.len() >= 8, "worker {w}: a password of at least 8 characters, please");
        workers.insert(w.clone(), (n.to_string(), pw.to_string()));
    }
    let g = Arc::new(Gate { nodes, workers, diff0: c["difficulty"].as_f64().unwrap_or(4.0), en1: AtomicU32::new(rand::random::<u32>() & 0xffff_0000), job_seq: AtomicU64::new(1), stats: Mutex::new(HashMap::new()), started: Instant::now() });
    for ns in g.nodes.values() {
        let (g2, ns) = (g.clone(), ns.clone());
        std::thread::Builder::new().name(format!("gate-node-{}", ns.name)).spawn(move || poll(&g2, &ns))?;
    }
    if let Some(a) = c["api"].as_str() {
        let a = a.to_string();
        let g2 = g.clone();
        std::thread::Builder::new().name("gate-api".into()).spawn(move || api(&a, &g2))?;
    }
    let mut ls = Vec::new();
    for a in &listen {
        ls.push(TcpListener::bind(a).with_context(|| format!("listen {a}"))?);
    }
    log(&format!("{} {}: stratum on {}, {} node(s), {} worker(s)", crate::PRODUCT, env!("CARGO_PKG_VERSION"), listen.join(", "), g.nodes.len(), g.workers.len()));
    let mut hs = Vec::new();
    for l in ls {
        let g = g.clone();
        hs.push(std::thread::spawn(move || {
            for s in l.incoming() {
                let Ok(s) = s else { continue };
                let g2 = g.clone();
                let _ = std::thread::Builder::new().name("gate-conn".into()).spawn(move || {
                    let peer = s.peer_addr().map(|a| a.to_string()).unwrap_or_default();
                    if let Err(e) = conn(&g2, s) {
                        log(&format!("{peer}: {e:#}"));
                    }
                });
            }
        }));
    }
    for h in hs {
        let _ = h.join();
    }
    Ok(())
}

fn poll(g: &Gate, ns: &NodeState) {
    let mut cur_tip = String::new();
    let mut last: Option<Instant> = None;
    loop {
        match node::tip(&ns.node) {
            Err(e) => {
                if ns.up.swap(false, Ordering::SeqCst) {
                    log(&format!("node {}: not reachable: {e:#}", ns.name));
                }
                std::thread::sleep(Duration::from_secs(2));
                continue;
            }
            Ok((t, h, syncing, _)) => {
                if !ns.up.swap(true, Ordering::SeqCst) {
                    log(&format!("node {}: up, height {h}", ns.name));
                }
                if syncing {
                    std::thread::sleep(Duration::from_secs(1));
                    continue;
                }
                let changed = t != cur_tip;
                if changed || last.is_none_or(|l| l.elapsed() >= node::refresh()) {
                    match node::template(&ns.node) {
                        Ok(tpl) => {
                            cur_tip = t;
                            last = Some(Instant::now());
                            let mut q = ns.jobs.write().unwrap();
                            if changed {
                                q.clear();
                            } else if q.iter().any(|j| j.tpl.id == tpl.id) {
                                // the node handed out the same template again (nothing changed): no new job
                                drop(q);
                                std::thread::sleep(node::POLL);
                                continue;
                            }
                            let seq = g.job_seq.fetch_add(1, Ordering::Relaxed);
                            let id = format!("{seq:08x}");
                            q.push_back(Arc::new(GJob { id, seq, tpl: Arc::new(tpl), clean: changed }));
                            while q.len() > KEEP_JOBS {
                                q.pop_front();
                            }
                            drop(q);
                            ns.version.fetch_add(1, Ordering::SeqCst);
                        }
                        Err(e) => log(&format!("node {}: template: {e:#}", ns.name)),
                    }
                }
            }
        }
        std::thread::sleep(node::POLL);
    }
}

fn send(w: &mut TcpStream, v: &Value) -> Result<()> {
    let mut l = v.to_string();
    l.push('\n');
    w.write_all(l.as_bytes())?;
    Ok(())
}

fn notify(w: &mut TcpStream, j: &GJob, clean: bool) -> Result<()> {
    send(w, &json!({"jsonrpc":"2.0","method":"mining.notify","params":[j.id, pow::hex(&j.tpl.header[..pow::NONCE_OFFSET]), j.tpl.height, pow::hex(&j.tpl.target), clean],"id":null}))
}

fn conn(g: &Gate, s: TcpStream) -> Result<()> {
    let peer = s.peer_addr()?.to_string();
    s.set_nodelay(true)?;
    s.set_read_timeout(Some(Duration::from_millis(100)))?;
    let mut w = s.try_clone()?;
    let mut r = BufReader::new(s);
    let en1 = g.en1.fetch_add(1, Ordering::Relaxed).to_be_bytes();
    let mut worker: Option<(String, Arc<NodeState>)> = None;
    let mut diff = g.diff0;
    let mut prev_diff = diff;
    let mut diff_at: Option<Instant> = None;
    let mut seen_ver = 0u64;
    let mut sent_seq = 0u64; // the newest job sent to this rig
    let mut buf = String::new();
    let mut last_rx = Instant::now();
    let mut dup: HashSet<[u8; 16]> = HashSet::new();
    let (mut win_start, mut win_shares) = (Instant::now(), 0u32);
    let res = (|| -> Result<()> {
        loop {
            // new job of this rig's node
            if let Some((_, ns)) = &worker {
                let v = ns.version.load(Ordering::SeqCst);
                if v != seen_ver {
                    seen_ver = v;
                    // every job not yet sent, oldest first (grouping: the rig keeps them all); from the last clean one on,
                    // and on a fresh connection the tip's newest RESEND (the first of them clean)
                    let q: Vec<Arc<GJob>> = ns.jobs.read().unwrap().iter().cloned().collect();
                    let mut new: Vec<Arc<GJob>> = q.into_iter().filter(|j| j.seq > sent_seq).collect();
                    if let Some(c) = new.iter().rposition(|j| j.clean) {
                        new.drain(..c);
                    }
                    let fresh = sent_seq == 0;
                    let keep = if super::grouping() { RESEND } else { 1 };
                    if new.len() > keep {
                        new.drain(..new.len() - keep);
                    }
                    for (k, j) in new.iter().enumerate() {
                        let clean = j.clean || (fresh && k == 0);
                        if clean {
                            dup.clear();
                        }
                        notify(&mut w, j, clean)?;
                        sent_seq = j.seq;
                    }
                }
                // vardiff
                if win_start.elapsed() >= RETARGET {
                    let el = win_start.elapsed().as_secs_f64();
                    let rate = win_shares as f64 / el; // shares per second at `diff`
                    let want = if win_shares == 0 { diff / 4.0 } else { diff * rate * TARGET_SECS };
                    let nd = want.clamp(diff / 4.0, diff * 4.0).clamp(0.1, 100000.0);
                    if (nd / diff - 1.0).abs() > 0.3 {
                        prev_diff = diff;
                        diff_at = Some(Instant::now());
                        diff = (nd * 1000.0).round() / 1000.0;
                        send(&mut w, &json!({"jsonrpc":"2.0","method":"mining.set_difficulty","params":[diff],"id":null}))?;
                        // the rig takes the new share target with the next job: re-send the tip's jobs (its group
                        // restarts with the new target and gets all of them back)
                        let q: Vec<Arc<GJob>> = ns.jobs.read().unwrap().iter().cloned().collect();
                        let keep = if super::grouping() { RESEND } else { 1 };
                        for j in q.iter().skip(q.len().saturating_sub(keep)) {
                            notify(&mut w, j, false)?;
                        }
                    }
                    (win_start, win_shares) = (Instant::now(), 0);
                }
            }
            match r.read_line(&mut buf) {
                Ok(0) => return Ok(()),
                Ok(_) if buf.ends_with('\n') => {}
                Ok(_) => continue,
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {
                    if last_rx.elapsed() > Duration::from_secs(600) {
                        bail!("silent for 10 min");
                    }
                    continue;
                }
                Err(e) => return Err(e.into()),
            }
            last_rx = Instant::now();
            let line = std::mem::take(&mut buf);
            let v: Value = serde_json::from_str(line.trim()).context("bad JSON")?;
            let id = v["id"].clone();
            match v["method"].as_str().unwrap_or("") {
                "mining.subscribe" => send(&mut w, &json!({"id":id,"result":[[["mining.set_difficulty","g"],["mining.notify","g"]], pow::hex(&en1), 12],"error":null}))?,
                "mining.authorize" => {
                    let login = v["params"][0].as_str().unwrap_or("");
                    let pass = v["params"][1].as_str().unwrap_or("");
                    let name = login.rsplit_once('.').map_or(login, |x| x.1).to_string();
                    let ok = g.workers.get(&name).filter(|(_, p)| constant_eq(p, pass)).map(|(n, _)| g.nodes[n].clone());
                    match ok {
                        Some(ns) => {
                            send(&mut w, &json!({"id":id,"result":true,"error":null}))?;
                            log(&format!("{peer}: worker {name} -> node {} (extranonce1 {})", ns.name, pow::hex(&en1)));
                            g.stats.lock().unwrap().entry(name.clone()).or_default().conns += 1;
                            send(&mut w, &json!({"jsonrpc":"2.0","method":"mining.set_difficulty","params":[diff],"id":null}))?;
                            seen_ver = 0;
                            worker = Some((name, ns));
                        }
                        None => {
                            log(&format!("{peer}: refused login {name:?} (unknown worker or wrong password)"));
                            send(&mut w, &json!({"id":id,"result":false,"error":[24,"unauthorized worker",null]}))?;
                            std::thread::sleep(Duration::from_secs(1));
                            bail!("unauthorized");
                        }
                    }
                }
                "mining.submit" => {
                    let Some((name, ns)) = &worker else {
                        send(&mut w, &json!({"id":id,"result":false,"error":[24,"unauthorized worker",null]}))?;
                        continue;
                    };
                    let jid = v["params"][1].as_str().unwrap_or("");
                    let nonce: Option<[u8; 16]> = v["params"][2].as_str().and_then(pow::unhex).and_then(|b| b.try_into().ok());
                    let job = ns.jobs.read().unwrap().iter().find(|j| j.id == jid).cloned();
                    let mut credit = diff;
                    let verdict: Result<(), (i32, &str)> = (|| {
                        let job = job.as_ref().ok_or((21, "job not found (stale)"))?;
                        let nonce = nonce.ok_or((20, "bad nonce (32 hex)"))?;
                        if nonce[12..16] != en1 {
                            return Err((20, "nonce bytes 12..15 are not this connection's extranonce1"));
                        }
                        if !dup.insert(nonce) {
                            return Err((22, "duplicate share"));
                        }
                        let bid = pow::block_id(&job.tpl.header, &nonce);
                        if !pow::meets(&bid, &diff_to_target(diff)) {
                            // found before the rig took a raised difficulty: counts at the previous one for a while
                            let grace = diff_at.is_some_and(|t| t.elapsed() < DIFF_GRACE) && prev_diff < diff && pow::meets(&bid, &diff_to_target(prev_diff));
                            if !grace {
                                return Err((23, "low difficulty share"));
                            }
                            credit = prev_diff;
                        }
                        if pow::meets(&bid, &job.tpl.target) {
                            let n = pow::nonce_u128(&nonce);
                            match node::call(&ns.node, "solution", json!({"template": job.tpl.id, "nonce": n.to_string()}), Duration::from_secs(35)) {
                                Ok(r) => {
                                    log(&format!("BLOCK from {name}: height {} id {} -> node {}: canonical {}", job.tpl.height, pow::hex(&bid), ns.name, r["canonical"]));
                                    ns.blocks.lock().unwrap().push((job.tpl.height, pow::hex(&bid), name.clone()));
                                    g.stats.lock().unwrap().entry(name.clone()).or_default().blocks += 1;
                                }
                                Err(e) => {
                                    ns.rejected.fetch_add(1, Ordering::Relaxed);
                                    log(&format!("block from {name} refused by node {}: {e:#}", ns.name));
                                }
                            }
                        }
                        Ok(())
                    })();
                    let mut st = g.stats.lock().unwrap();
                    let ws = st.entry(name.clone()).or_default();
                    match verdict {
                        Ok(()) => {
                            win_shares += 1;
                            ws.shares += 1;
                            ws.work += credit * 4294967296.0;
                            ws.recent.push_back((Instant::now(), credit * 4294967296.0));
                            while ws.recent.front().is_some_and(|x| x.0.elapsed() > RECENT) {
                                ws.recent.pop_front();
                            }
                            ws.since.get_or_insert(Instant::now());
                            ws.last = Some(Instant::now());
                            drop(st);
                            send(&mut w, &json!({"id":id,"result":true,"error":null}))?;
                        }
                        Err((code, msg)) => {
                            ws.rejected += 1;
                            drop(st);
                            send(&mut w, &json!({"id":id,"result":null,"error":[code,msg,null]}))?;
                        }
                    }
                }
                "mining.extranonce.subscribe" | "mining.ping" => send(&mut w, &json!({"id":id,"result":true,"error":null}))?,
                m => send(&mut w, &json!({"id":id,"result":null,"error":[20,format!("unknown method {m}"),null]}))?,
            }
        }
    })();
    if let Some((name, _)) = &worker {
        if let Some(ws) = g.stats.lock().unwrap().get_mut(name) {
            ws.conns = ws.conns.saturating_sub(1);
        }
        log(&format!("{peer}: worker {name} disconnected"));
    }
    res
}

fn constant_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |x, (p, q)| x | (p ^ q)) == 0
}

fn summary(g: &Gate) -> Value {
    let st = g.stats.lock().unwrap();
    let workers: Value = st
        .iter()
        .map(|(n, w)| {
            let el = w.since.map_or(0.0, |s| s.elapsed().as_secs_f64()).max(1.0);
            (n.clone(), json!({"node": g.workers.get(n).map(|x| x.0.clone()), "connections": w.conns, "shares": w.shares, "rejected": w.rejected, "blocks": w.blocks, "hashrate_ghs_est": (w.work / el / 1e9 * 1000.0).round() / 1000.0, "hashrate_ghs_15min": recent_ghs(w), "last_share_s": w.last.map(|l| l.elapsed().as_secs())}))
        })
        .collect::<serde_json::Map<_, _>>()
        .into();
    let nodes: Value = g
        .nodes
        .values()
        .map(|ns| {
            let j = ns.jobs.read().unwrap().back().map(|j| j.tpl.height);
            (ns.name.clone(), json!({"control": ns.node.addr.to_string(), "up": ns.up.load(Ordering::Relaxed), "height": j, "blocks": ns.blocks.lock().unwrap().iter().map(|b| json!({"height": b.0, "id": b.1, "worker": b.2})).collect::<Vec<_>>(), "refused": ns.rejected.load(Ordering::Relaxed)}))
        })
        .collect::<serde_json::Map<_, _>>()
        .into();
    json!({"gate": crate::PRODUCT, "version": env!("CARGO_PKG_VERSION"), "uptime": g.started.elapsed().as_secs(), "workers": workers, "nodes": nodes})
}

fn api(bind: &str, g: &Gate) {
    let Ok(l) = TcpListener::bind(bind) else {
        log(&format!("api: cannot listen on {bind}"));
        return;
    };
    log(&format!("api: http://{bind}/summary"));
    for c in l.incoming().flatten() {
        let mut c = c;
        let _ = c.set_read_timeout(Some(Duration::from_secs(2)));
        let mut b = [0u8; 1024];
        let _ = std::io::Read::read(&mut c, &mut b);
        let body = summary(g).to_string();
        let _ = c.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes());
    }
}

/// The rate of the last RECENT of shares (GH/s): what the rig mines now, not since the gate started.
fn recent_ghs(w: &WorkerStat) -> f64 {
    let r: f64 = w.recent.iter().filter(|x| x.0.elapsed() <= RECENT).map(|x| x.1).sum();
    let span = w.since.map_or(1.0, |s| s.elapsed().as_secs_f64()).clamp(1.0, RECENT.as_secs_f64());
    (r / span / 1e9 * 1000.0).round() / 1000.0
}
