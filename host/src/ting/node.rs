// FFGT — see LICENSE
//! Solo mining on a local Ting node through its control channel (node/wiki/node/security.md, node/src/transport.rs):
//! TCP on a loopback address (default 127.0.0.1:18440), one connection per request, frames = u32 little-endian length + JSON:
//!   1. client -> {"nonce": 32 random bytes hex}
//!   2. node   -> {"nonce": its 32 bytes, "proof": HMAC(token, "ting-control-node" || client nonce || node nonce)}
//!      (checked before anything else is sent: a listener that is not this node learns nothing)
//!   3. client -> {"request": "{\"method\":…,\"params\":…}", "proof": HMAC(token, "ting-control-client" || node nonce ||
//!      client nonce || request bytes)}
//!   node   -> {"ok": true, "result": …} | {"ok": false, "error": "unauthorized" | "unknown or expired template" | …}
//! HMAC-SHA-256 keyed with the 64 hex characters of `control.token` (the node's data directory; the miner needs read access,
//! e.g. a group such as ting-ctl).  Mining uses `status`, `template` and `solution` exactly as `ting template` / `ting solution`
//! do; the node keeps 8 templates per tip and drops all of them when the tip (or the producer data) changes.
use super::pow;
use crate::stratum::{Src, Stats};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const DEFAULT_ADDR: &str = "127.0.0.1:18440";
const NODE_PROOF: &[u8] = b"ting-control-node";
const CLIENT_PROOF: &[u8] = b"ting-control-client";
const FRAME_MAX: usize = 16 << 20;

/// How to reach the node: control address and token.
#[derive(Clone)]
pub struct Node {
    pub addr: SocketAddr,
    pub token: String,
    pub token_src: String, // where the token came from (for messages; never the token itself)
}

impl std::fmt::Display for Node {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ting node {} (control channel)", self.addr)
    }
}

/// The node to mine on: `--node HOST:PORT` (loopback; default 127.0.0.1:18440), the token from `--node-token-file FILE`, or
/// `control.token` in `--node-dir DIR` (default: the `ting` default data directory HOME/.ting, USERPROFILE\.ting on Windows,
/// then /var/lib/ting).
pub fn resolve(addr: Option<&str>, dir: Option<&str>, token_file: Option<&str>, allow_remote: bool) -> Result<Node> {
    let a = addr.unwrap_or(DEFAULT_ADDR);
    let sa = a.to_socket_addrs().with_context(|| format!("--node {a}: not HOST:PORT"))?.next().with_context(|| format!("--node {a}: does not resolve"))?;
    ensure!(
        sa.ip().is_loopback() || allow_remote,
        "--node {a}: the Ting control channel listens on loopback only (the node refuses any other address); use 127.0.0.1:PORT, a tunnel, or ffgt-gate"
    );
    let path: PathBuf = match (token_file, dir) {
        (Some(f), _) => PathBuf::from(f),
        (None, Some(d)) => PathBuf::from(d).join("control.token"),
        (None, None) => {
            let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from);
            let mut c: Vec<PathBuf> = home.into_iter().map(|h| h.join(".ting").join("control.token")).collect();
            c.push(PathBuf::from("/var/lib/ting/control.token"));
            c.into_iter().find(|p| p.exists()).unwrap_or_else(|| PathBuf::from(".ting/control.token"))
        }
    };
    let token = std::fs::read_to_string(&path)
        .with_context(|| format!("cannot read the node's control token {} (give --node-dir DATADIR or --node-token-file FILE; the miner's user needs read access, e.g. the group of the file)", path.display()))?
        .trim()
        .to_string();
    ensure!(token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit()), "{}: not a Ting control token (64 hex digits expected)", path.display());
    Ok(Node { addr: sa, token, token_src: path.display().to_string() })
}

/// HMAC-SHA-256 (RFC 2104) of the concatenated parts, keyed with the token's characters (64 bytes: one key block).
fn mac(token: &str, parts: &[&[u8]]) -> [u8; 32] {
    let mut key = [0u8; 64];
    let t = token.as_bytes();
    if t.len() > 64 {
        key[..32].copy_from_slice(&Sha256::digest(t));
    } else {
        key[..t.len()].copy_from_slice(t);
    }
    let mut inner = Sha256::new();
    inner.update(key.map(|b| b ^ 0x36));
    for p in parts {
        inner.update(p);
    }
    let mut outer = Sha256::new();
    outer.update(key.map(|b| b ^ 0x5c));
    outer.update(inner.finalize());
    outer.finalize().into()
}

fn write_frame(s: &mut TcpStream, v: &Value) -> Result<()> {
    let b = serde_json::to_vec(v)?;
    let mut f = Vec::with_capacity(4 + b.len());
    f.extend((b.len() as u32).to_le_bytes());
    f.extend(b);
    s.write_all(&f)?;
    Ok(())
}

fn read_frame(s: &mut TcpStream) -> Result<Value> {
    let mut l = [0u8; 4];
    s.read_exact(&mut l)?;
    let n = u32::from_le_bytes(l) as usize;
    ensure!(n > 0 && n <= FRAME_MAX, "control frame of {n} bytes");
    let mut b = vec![0u8; n];
    s.read_exact(&mut b)?;
    Ok(serde_json::from_slice(&b)?)
}

/// Error text of a node answer {"ok": false, "error": …}.
#[derive(Debug)]
pub struct NodeError(pub String);
impl std::fmt::Display for NodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for NodeError {}

/// One control request: handshake, request, answer.  Err(NodeError) for a refusal by the node, other errors for transport.
pub fn call(n: &Node, method: &str, params: Value, timeout: Duration) -> Result<Value> {
    let mut s = TcpStream::connect_timeout(&n.addr, Duration::from_secs(5)).with_context(|| format!("connect {}", n.addr))?;
    s.set_nodelay(true)?;
    s.set_read_timeout(Some(timeout))?;
    s.set_write_timeout(Some(Duration::from_secs(5)))?;
    let ours: [u8; 32] = rand::random();
    write_frame(&mut s, &json!({ "nonce": pow::hex(&ours) }))?;
    let hello = read_frame(&mut s).context("control handshake")?;
    let theirs: [u8; 32] = match hello["nonce"].as_str().and_then(pow::unhex).and_then(|v| v.try_into().ok()) {
        Some(t) => t,
        None => bail!("control handshake failed: {}", hello["error"].as_str().unwrap_or("no nonce")),
    };
    let want = mac(&n.token, &[NODE_PROOF, &ours, &theirs]);
    let got = hello["proof"].as_str().and_then(pow::unhex).unwrap_or_default();
    // not secret-dependent timing on our side: the proof is the node's; compare in full anyway
    ensure!(got.len() == 32 && got.iter().zip(want.iter()).fold(0u8, |a, (x, y)| a | (x ^ y)) == 0, "the control listener at {} did not prove it holds the token from {} (another node, or a stale token)", n.addr, n.token_src);
    let req = json!({ "method": method, "params": params }).to_string();
    let proof = mac(&n.token, &[CLIENT_PROOF, &theirs, &ours, req.as_bytes()]);
    write_frame(&mut s, &json!({ "request": req, "proof": pow::hex(&proof) }))?;
    let r = read_frame(&mut s).with_context(|| format!("control answer to {method}"))?;
    if r["ok"] != true {
        return Err(NodeError(r["error"].as_str().unwrap_or("invalid control reply").to_string()).into());
    }
    Ok(r["result"].clone())
}

/// One block template: a header complete but for the nonce.
#[derive(Clone, Debug)]
pub struct Template {
    pub id: String,
    pub job_id: String,         // the pool's job id (solo: the template id)
    pub share: [u8; 32],        // share target (solo: the block target)
    pub diff: f64,              // pool share difficulty (solo: 0)
    pub en1: Option<[u8; 4]>,   // the pool's extranonce1 = nonce bytes 12..15 (solo: none)
    pub height: u64,
    pub parent: String,
    pub header: [u8; pow::HEADER_LEN],
    pub target: [u8; 32],
    pub txs: usize,
    pub got: Instant,
}

pub fn template(n: &Node) -> Result<Template> {
    let v = call(n, "template", json!({}), Duration::from_secs(35))?;
    let m = &v["mining"];
    ensure!(
        m["nonce_offset"].as_u64() == Some(pow::NONCE_OFFSET as u64) && m["nonce_bytes"].as_u64() == Some(16) && m["nonce_endian"].as_str() == Some("little"),
        "the node's template has another nonce layout ({m}): this miner knows offset 278, 16 bytes, little-endian"
    );
    let header: [u8; pow::HEADER_LEN] = v["header_hex"].as_str().and_then(pow::unhex).and_then(|b| b.try_into().ok()).context("template: header_hex is not 294 bytes")?;
    let target: [u8; 32] = m["target_hex"].as_str().and_then(pow::unhex).and_then(|b| b.try_into().ok()).context("template: target_hex is not 32 bytes")?;
    let id = v["id"].as_str().context("template: no id")?.to_string();
    Ok(Template {
        job_id: id.clone(),
        share: target,
        diff: 0.0,
        en1: None,
        id,
        height: v["height"].as_u64().unwrap_or(0),
        parent: v["parent"].as_str().unwrap_or("").to_string(),
        header,
        target,
        txs: v["transactions"].as_array().map_or(0, |a| a.len()),
        got: Instant::now(),
    })
}

/// The tip (id, height) and whether the node is still catching up (`replaying` or best header above the height).
pub fn tip(n: &Node) -> Result<(String, u64, bool, Value)> {
    let v = call(n, "status", json!({}), Duration::from_secs(10))?;
    let h = v["height"].as_u64().unwrap_or(0);
    let best = v["best_height"].as_u64().unwrap_or(h);
    let syncing = v["replaying"] == true || best > h + 2;
    Ok((v["tip"].as_str().unwrap_or("").to_string(), h, syncing, v))
}

/// The payout account the node's blocks pay (producer data `OCP1…` + 32-byte account), if any.
pub fn payout(status: &Value) -> Option<String> {
    let p = status["producer_hex"].as_str()?;
    // ting miner payout writes 44 bytes: "OCP1" 01 00 00 00 20 00 00 00 + account (node/src/payout.rs)
    (p.len() == 88 && p.starts_with("4f435031")).then(|| p[24..].to_string())
}

/// Producer data that pays `account` (64 hex): "OCP1", version 1, length 32, the account (node/src/payout.rs).
pub fn producer_for(account: &str) -> String {
    format!("4f4350310100000020000000{}", account.to_ascii_lowercase())
}

/// Sets the node's producer data (all its templates are dropped by the node) and reads it back.
pub fn set_producer(n: &Node, hex: &str) -> Result<()> {
    call(n, "miner", json!({"action": "producer", "producer": hex}), Duration::from_secs(10))?;
    let (_, _, _, st) = tip(n)?;
    let now = st["producer_hex"].as_str().unwrap_or("");
    ensure!(now.eq_ignore_ascii_case(hex), "the node's producer data reads back as {now:?}, not {hex}");
    Ok(())
}

/// The crash-safe marker of a producer switch (solo dev-fee fallback): written BEFORE the node is switched to the developer's
/// account, removed after the owner's producer data is back and read back.  Found at start = put back first.
pub fn marker_path(dir: &std::path::Path) -> PathBuf {
    dir.join("ting-fee-producer.json")
}

pub fn write_marker(dir: &std::path::Path, n: &Node, original: &str) -> Result<()> {
    let _ = std::fs::create_dir_all(dir);
    let p = marker_path(dir);
    let tmp = p.with_extension("tmp");
    std::fs::write(&tmp, json!({"node": n.addr.to_string(), "producer": original, "at": crate::uv::now()}).to_string())?;
    std::fs::rename(&tmp, &p)?;
    Ok(())
}

/// A marker left by a crashed run: put the owner's producer data back on that node (when it is this node).
pub fn recover_marker(dir: &std::path::Path, n: &Node) -> Result<()> {
    let p = marker_path(dir);
    let Ok(t) = std::fs::read_to_string(&p) else { return Ok(()) };
    let v: Value = serde_json::from_str(&t).unwrap_or(Value::Null);
    let (node, prod) = (v["node"].as_str().unwrap_or(""), v["producer"].as_str().unwrap_or(""));
    if node != n.addr.to_string() {
        println!("devfee: marker {} names another node ({node}); left alone", p.display());
        return Ok(());
    }
    println!("devfee: the last run ended inside a node fee slot: putting the node's producer data back to {prod}");
    set_producer(n, prod)?;
    std::fs::remove_file(&p)?;
    println!("devfee: producer data restored and read back; marker removed");
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------------
// the solo threads: work (status poll + templates) and solutions
// ---------------------------------------------------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Job {
    pub src: Src,
    pub tpl: Arc<Template>,
    pub solved: Arc<AtomicBool>, // a block for this template went to the node: no second one (and the GPUs move on)
    pub tgen: u64, // tip generation: a solution for an older one is stale
    /// 0.2.2 template grouping: older templates (pool: jobs) of the same tip that share the last SHA-256 block (header bytes
    /// 244..277 + the nonce) and the targets, newest first; the GPUs search them together with this one (one message
    /// schedule per nonce, see tinggpu.h).  Each member keeps its own id / solved flag.
    pub group: Arc<Vec<Job>>,
}

impl Job {
    pub fn new(src: Src, tpl: Template, tgen: u64) -> Job {
        Job { src, tpl: Arc::new(tpl), solved: Arc::new(AtomicBool::new(false)), tgen, group: Arc::new(Vec::new()) }
    }
    /// This job without its group (a ring member).
    pub fn alone(&self) -> Job {
        Job { group: Arc::new(Vec::new()), ..self.clone() }
    }
}

/// The first header byte of the last SHA-256 block (message byte 256 = 12-byte tag + 244).
pub const SHARED_TAIL: usize = 244;

/// Two templates can be searched as one group: the same last block (header bytes 244..277; the nonce is ours), the same
/// network and share targets, the same pool extranonce1, the same height.
pub fn compatible(a: &Template, b: &Template) -> bool {
    a.header[SHARED_TAIL..pow::NONCE_OFFSET] == b.header[SHARED_TAIL..pow::NONCE_OFFSET] && a.target == b.target && a.share == b.share && a.en1 == b.en1 && a.height == b.height
}

/// The templates (pool: jobs) of the current tip kept for grouping, newest last.  The node keeps 8 templates per tip and
/// evicts the oldest when a 9th is made: with the next request in flight only 7 are certainly alive, so at most 7 (`cap`).
pub struct Ring {
    jobs: std::collections::VecDeque<Job>,
    pub cap: usize,
}

impl Ring {
    pub fn new(cap: usize) -> Ring {
        Ring { jobs: Default::default(), cap: cap.max(1) }
    }
    pub fn clear(&mut self) {
        self.jobs.clear();
    }
    /// Adds a job (another generation, or a template that cannot join the group, restarts the ring); a template id already
    /// in the ring is not added again (the node keeps it in its place).  Returns the job to publish: the newest with the
    /// others (newest first) as its group; None = a duplicate of a kept template.
    pub fn push(&mut self, j: Job) -> Option<Job> {
        if let Some(last) = self.jobs.back() {
            if last.tgen != j.tgen || !compatible(&last.tpl, &j.tpl) {
                self.jobs.clear();
            }
        }
        if self.jobs.iter().any(|o| o.tpl.id == j.tpl.id) {
            return None;
        }
        self.jobs.push_back(j.alone());
        while self.jobs.len() > self.cap {
            self.jobs.pop_front();
        }
        let n = self.jobs.len();
        let group: Vec<Job> = self.jobs.iter().rev().skip(1).filter(|o| !o.solved.load(Ordering::Relaxed)).cloned().collect();
        let newest = self.jobs[n - 1].clone();
        Some(Job { group: Arc::new(group), ..newest })
    }
}

/// Whether template grouping is on (`--no-group` / FFGT_NO_GROUP=1 turn it off).
pub fn grouping() -> bool {
    super::grouping()
}

pub enum Event {
    Job(Job),
    Pause(String),
    Disconnected,
}

pub struct Submit {
    pub job: Job,
    pub nonce: [u8; 16],
    pub id: [u8; 32],
    pub gpu: i32,
}

fn log(s: &str) {
    println!("[node] {s}");
}

/// How often the tip is polled (a new block elsewhere: the next template within this time; GPUs pick it up at their next call)
pub const POLL: Duration = Duration::from_millis(250);
/// A fresh template on the same tip after this long (new transactions, a later timestamp)
pub const REFRESH: Duration = Duration::from_secs(20);
/// 0.2.2 with template grouping: a template per second (the node keeps 8 per tip; groups of up to 7)
pub const REFRESH_GROUP: Duration = Duration::from_secs(1);

pub fn refresh() -> Duration {
    if grouping() { REFRESH_GROUP } else { REFRESH }
}

/// Templates kept for a group: 7 (one request may be in flight while the GPUs search the others)
pub const GROUP_KEEP: usize = 7;

pub struct Shared {
    pub tip_gen: Arc<AtomicU64>,
    pub blocks: Mutex<Vec<(u64, String)>>, // (height, block id) of the blocks this run got accepted
    pub kick: AtomicBool,                  // fetch a template now (our block was accepted, or a template was refused)
    pub keep: std::sync::atomic::AtomicUsize, // templates kept for a group (lowered when an older one turned out evicted)
}

pub fn spawn(n: Node, tx: Sender<(Src, Event)>, rx: Receiver<Submit>, stats: Arc<Stats>, sh: Arc<Shared>, stop: Arc<AtomicBool>) {
    {
        let (n, stats, sh, stop) = (n.clone(), stats.clone(), sh.clone(), stop.clone());
        std::thread::Builder::new().name("ting-work".into()).spawn(move || work(&n, &tx, &stats, &sh, &stop)).expect("thread");
    }
    std::thread::Builder::new().name("ting-submit".into()).spawn(move || sender(&n, &rx, &stats, &sh, &stop)).expect("thread");
}

fn work(n: &Node, tx: &Sender<(Src, Event)>, stats: &Stats, sh: &Shared, stop: &AtomicBool) {
    let mut cur_tip = String::new();
    let mut last_tpl: Option<Instant> = None;
    let mut had_err = false;
    let mut syncing_said = false;
    let mut force = false;
    let mut ring = Ring::new(if grouping() { sh.keep.load(Ordering::Relaxed) } else { 1 });
    while !stop.load(Ordering::Relaxed) {
        let t0 = Instant::now();
        match tip(n) {
            Err(e) => {
                if !had_err {
                    log(&format!("node not reachable: {e:#} (retrying)"));
                }
                had_err = true;
                stats.connected.store(0, Ordering::Relaxed);
                let _ = tx.send((Src::User, Event::Disconnected));
                std::thread::sleep(Duration::from_secs(2));
                continue;
            }
            Ok((t, h, syncing, _)) => {
                if had_err || stats.connected.load(Ordering::Relaxed) == 0 {
                    log(&format!("connected to {n}: height {h}"));
                }
                had_err = false;
                stats.connected.store(1, Ordering::Relaxed);
                if syncing {
                    if !syncing_said {
                        log(&format!("the node is catching up (height {h}): waiting, a block on an old tip would be lost"));
                        let _ = tx.send((Src::User, Event::Pause("node syncing".into())));
                    }
                    syncing_said = true;
                    cur_tip.clear();
                    std::thread::sleep(Duration::from_secs(1));
                    continue;
                }
                syncing_said = false;
                let changed = t != cur_tip;
                if sh.kick.swap(false, Ordering::SeqCst) {
                    force = true;
                }
                if changed || force || last_tpl.is_none_or(|l| l.elapsed() >= refresh()) {
                    match template(n) {
                        Ok(tpl) => {
                            if changed {
                                sh.tip_gen.fetch_add(1, Ordering::SeqCst);
                                cur_tip = t;
                            }
                            if force {
                                // our block accepted / a solution refused / the producer data switched (node fee slot): the
                                // node's templates changed, start the group afresh
                                ring.clear();
                            }
                            force = false;
                            last_tpl = Some(Instant::now());
                            if grouping() {
                                ring.cap = sh.keep.load(Ordering::Relaxed).max(1);
                            }
                            if let Some(job) = ring.push(Job::new(Src::User, tpl, sh.tip_gen.load(Ordering::SeqCst))) {
                                let _ = tx.send((Src::User, Event::Job(job)));
                            }
                        }
                        Err(e) => {
                            log(&format!("template: {e:#}"));
                            force = true;
                            std::thread::sleep(Duration::from_millis(500));
                        }
                    }
                }
            }
        }
        let el = t0.elapsed();
        if el < POLL {
            std::thread::sleep(POLL - el);
        }
    }
}

fn sender(n: &Node, rx: &Receiver<Submit>, stats: &Stats, sh: &Shared, stop: &AtomicBool) {
    while !stop.load(Ordering::Relaxed) {
        let Ok(s) = rx.recv_timeout(Duration::from_millis(200)) else { continue };
        let t = Instant::now();
        let nonce = pow::nonce_u128(&s.nonce);
        let tpl = &s.job.tpl;
        match call(n, "solution", json!({ "template": tpl.id, "nonce": nonce.to_string() }), Duration::from_secs(35)) {
            Ok(v) => {
                if v["canonical"] == false {
                    stats.rejected.fetch_add(1, Ordering::Relaxed);
                    stats.stale_rej.fetch_add(1, Ordering::Relaxed);
                    log(&format!("block {} at height {} imported but NOT on the best chain (a competing block won): {}", pow::hex(&s.id), tpl.height, v));
                } else {
                    stats.accepted.fetch_add(1, Ordering::Relaxed);
                    sh.blocks.lock().unwrap().push((tpl.height, pow::hex(&s.id)));
                    log(&format!("BLOCK ACCEPTED height {} id {} (GPU{}, nonce {nonce}, {} ms)", tpl.height, pow::hex(&s.id), s.gpu, t.elapsed().as_millis()));
                    sh.kick.store(true, Ordering::SeqCst);
                }
            }
            Err(e) => {
                stats.rejected.fetch_add(1, Ordering::Relaxed);
                let msg = format!("{e:#}");
                sh.kick.store(true, Ordering::SeqCst);
                if msg.contains("unknown or expired template") {
                    stats.stale_rej.fetch_add(1, Ordering::Relaxed);
                    let new_tip = s.job.tgen != sh.tip_gen.load(Ordering::SeqCst) || tip(n).is_ok_and(|x| x.1 >= tpl.height);
                    if new_tip {
                        log(&format!("solution stale: template {} (height {}) is gone (new tip): {msg}", &tpl.id[..tpl.id.len().min(16)], tpl.height));
                    } else {
                        // the tip stood but the template was evicted: another client takes templates from this node too (a
                        // second miner / ffgt-gate on it) - keep fewer for the group
                        let k = sh.keep.load(Ordering::Relaxed);
                        if k > 2 {
                            sh.keep.store(k - 2, Ordering::Relaxed);
                        }
                        log(&format!("solution LOST: template {} (height {}) was evicted by the node while the tip stood (does another miner or ffgt-gate take templates from this node?): groups keep {} templates from now", &tpl.id[..tpl.id.len().min(16)], tpl.height, sh.keep.load(Ordering::Relaxed)));
                    }
                } else {
                    log(&format!("solution REJECTED height {} GPU{}: {msg}", tpl.height, s.gpu));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn hmac_rfc4231_case2() {
        assert_eq!(super::pow::hex(&super::mac("Jefe", &[b"what do ya ", b"want for nothing?"])), "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
    }
    fn tpl(id: &str, height: u64, tail: u8, front: u8) -> super::Template {
        let mut header = [front; super::pow::HEADER_LEN];
        header[super::SHARED_TAIL..].fill(tail);
        super::Template { id: id.into(), job_id: id.into(), share: [0x0f; 32], diff: 0.0, en1: None, height, parent: String::new(), header, target: [0x0f; 32], txs: 0, got: std::time::Instant::now() }
    }

    #[test]
    fn ring_groups_compatible_templates_of_one_tip() {
        use super::{Job, Ring};
        use crate::stratum::Src;
        let mut r = Ring::new(3);
        let j = r.push(Job::new(Src::User, tpl("a", 5, 1, 1), 1)).unwrap();
        assert!(j.group.is_empty());
        let j = r.push(Job::new(Src::User, tpl("b", 5, 1, 2), 1)).unwrap();
        assert_eq!(j.group.iter().map(|x| x.tpl.id.as_str()).collect::<Vec<_>>(), ["a"]);
        assert!(r.push(Job::new(Src::User, tpl("b", 5, 1, 2), 1)).is_none(), "a template id already kept is not added again");
        r.push(Job::new(Src::User, tpl("c", 5, 1, 3), 1));
        let j = r.push(Job::new(Src::User, tpl("d", 5, 1, 4), 1)).unwrap();
        assert_eq!(j.group.iter().map(|x| x.tpl.id.as_str()).collect::<Vec<_>>(), ["c", "b"], "newest first, at most cap in all");
        // another last block (producer data switched): the group starts afresh
        let j = r.push(Job::new(Src::User, tpl("e", 5, 2, 4), 1)).unwrap();
        assert!(j.group.is_empty());
        // another tip generation
        r.push(Job::new(Src::User, tpl("f", 5, 2, 5), 1));
        let j = r.push(Job::new(Src::User, tpl("g", 6, 2, 5), 2)).unwrap();
        assert!(j.group.is_empty());
        // a solved member is left out
        let k = r.push(Job::new(Src::User, tpl("h", 6, 2, 6), 2)).unwrap();
        k.group[0].solved.store(true, std::sync::atomic::Ordering::SeqCst);
        let j = r.push(Job::new(Src::User, tpl("i", 6, 2, 7), 2)).unwrap();
        assert_eq!(j.group.iter().map(|x| x.tpl.id.as_str()).collect::<Vec<_>>(), ["h"]);
    }

    #[test]
    fn payout_from_producer() {
        let s = serde_json::json!({"producer_hex": "4f43503101000000200000001111111111111111111111111111111111111111111111111111111111111111"});
        assert_eq!(super::payout(&s).as_deref(), Some("1111111111111111111111111111111111111111111111111111111111111111"));
    }
}
