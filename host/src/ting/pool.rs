// FFGT — see LICENSE
//! Ting stratum client (crypto-connection.net, Miningcore family `ting`; the protocol of the pool's open-source miner,
//! gacanga/crypto-connection-miner, README "Ting stratum", and the same the ffgt-gate serves):
//!   >> mining.subscribe ["ffgt/<version>"]          << [[["mining.set_difficulty",S],["mining.notify",S]], EXTRANONCE1 (8 hex), 12]
//!   >> mining.authorize ["ACCOUNT.worker", PASS]    << true
//!   << mining.set_difficulty [d]                    share target = 2^224 / d (difficulty 1 = about 2^32 hashes)
//!   << mining.notify [job id, header[0..278] hex, height, network target (64 hex, big-endian), clean]
//!   >> mining.submit ["ACCOUNT.worker", job id, NONCE (32 hex: the 16 nonce bytes as in the header)]   << true | error
//! The nonce's bytes 12..15 are the pool's EXTRANONCE1; FFGT puts its GPU slot and counters in bytes 0..5 and 10..11 and
//! enumerates bytes 6..9 (word 10 of the last SHA-256 block, kernel t1p).
//! Several entries (`--pool a,b`): failover to the next when a pool is unusable, back to the first after `failback` minutes
//! on a backup (checked by a fresh login).  Dead link: no line from the pool for 150 s while connected = reconnect.
use super::node::{Event, Job, Ring, Submit, Template};
use super::pow;
use crate::stratum::{Src, Stats};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const DEFAULT_POOL: &str = "crypto-connection.net:4073";
const SILENT: Duration = Duration::from_secs(150);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Url {
    pub host: String,
    pub port: u16,
}
impl std::fmt::Display for Url {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.host, self.port)
    }
}

/// `stratum+tcp://HOST:PORT`, `tcp://HOST:PORT` or `HOST:PORT` (TLS: not offered by the Ting pools seen so far).
pub fn parse_url(s: &str) -> Result<Url, String> {
    let s = s.trim();
    if s.starts_with("stratum+ssl") || s.starts_with("ssl://") || s.starts_with("tls://") {
        return Err(format!("{s}: TLS is not supported for Ting pools (crypto-connection.net speaks plain TCP: stratum+tcp://HOST:PORT)"));
    }
    let r = s.split_once("://").map_or(s, |x| x.1).trim_end_matches('/');
    let (h, p) = r.rsplit_once(':').ok_or_else(|| format!("{s}: needs HOST:PORT"))?;
    Ok(Url { host: h.trim_start_matches('[').trim_end_matches(']').to_string(), port: p.parse().map_err(|_| format!("{s}: bad port"))? })
}

/// Big-endian share target 2^224 / d (as the pool's miner computes it; d <= 0 = everything).
pub fn diff_to_target(d: f64) -> [u8; 32] {
    let mut be = [0u8; 32];
    let d = if d > 0.0 { d } else { 1e-12 };
    let mut x = 4294967296.0 / d; // target / 2^192
    if x >= 18446744073709551615.0 {
        return [0xff; 32];
    }
    for l in 0..4 {
        let f = x.floor();
        let v = f as u64;
        be[8 * l..8 * l + 8].copy_from_slice(&v.to_be_bytes());
        x = (x - f) * 18446744073709551616.0;
    }
    be
}

pub struct Login {
    pub name: &'static str, // "user" | "fee"
    pub src: Src,
    pub urls: Vec<Url>,
    pub login: String, // ACCOUNT.worker
    pub pass: String,
    pub enabled: Arc<AtomicBool>, // connect only while wanted (the fee login: around its slot)
    pub stop: Arc<AtomicBool>,
    pub failback_min: f64,
    pub jgen: Arc<AtomicU64>, // this login's job generation (a clean job / new height: older shares are stale)
    pub entry: Arc<AtomicU64>, // the entry in use (for the stats line)
}

fn log(tag: &str, s: &str) {
    println!("[pool:{tag}] {s}");
}

pub fn spawn(l: Login, tx: Sender<(Src, Event)>, rx: Receiver<Submit>, stats: Arc<Stats>) {
    std::thread::Builder::new().name(format!("ting-pool-{}", l.name)).spawn(move || run(l, tx, rx, stats)).expect("thread");
}

enum End {
    Stop,
    Disabled,
    Failed(String),
    Failback,
}

fn run(l: Login, tx: Sender<(Src, Event)>, rx: Receiver<Submit>, stats: Arc<Stats>) {
    let mut entry = 0usize;
    let mut backoff = 1u64;
    let mut fails_here = 0u32;
    while !l.stop.load(Ordering::Relaxed) {
        if !l.enabled.load(Ordering::Relaxed) {
            while rx.try_recv().is_ok() {
                stats.stale_drop.fetch_add(1, Ordering::Relaxed);
            }
            std::thread::sleep(Duration::from_millis(200));
            continue;
        }
        l.entry.store(entry as u64, Ordering::Relaxed);
        let t0 = Instant::now();
        let end = session(&l, entry, &tx, &rx, &stats);
        stats.connected.store(0, Ordering::Relaxed);
        let _ = tx.send((l.src, Event::Disconnected));
        match end {
            End::Stop => return,
            End::Disabled => {
                entry = 0;
                continue;
            }
            End::Failback => {
                log(l.name, &format!("back to the primary {}", l.urls[0]));
                entry = 0;
                fails_here = 0;
                continue;
            }
            End::Failed(why) => {
                log(l.name, &format!("{}: {why}", l.urls[entry]));
                if t0.elapsed() > Duration::from_secs(60) {
                    backoff = 1;
                    fails_here = 0;
                }
                fails_here += 1;
                if l.urls.len() > 1 && fails_here >= 2 {
                    entry = (entry + 1) % l.urls.len();
                    fails_here = 0;
                    log(l.name, &format!("failover to pool {} of {}: {}", entry + 1, l.urls.len(), l.urls[entry]));
                    backoff = 1;
                    continue;
                }
                let until = Instant::now() + Duration::from_secs(backoff);
                while Instant::now() < until && !l.stop.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(100));
                }
                backoff = (backoff * 2).min(30);
            }
        }
    }
}

/// A login on `u` that gets a job within `timeout` (the failback probe).
pub fn probe(u: &Url, login: &str, pass: &str, timeout: Duration) -> Result<(), String> {
    let mut c = Conn::open(u, timeout)?;
    c.send(&json!({"id":1,"method":"mining.subscribe","params":[agent()]}))?;
    c.send(&json!({"id":2,"method":"mining.authorize","params":[login, pass]}))?;
    let t = Instant::now();
    let mut authorized = false;
    while t.elapsed() < timeout {
        if let Some(v) = c.line()? {
            if v["id"] == 2 {
                if v["result"] != true {
                    return Err(format!("authorize refused: {}", v["error"]));
                }
                authorized = true;
            }
            if authorized && v["method"] == "mining.notify" {
                return Ok(());
            }
        }
    }
    Err("no job in time".into())
}

pub fn agent() -> String {
    format!("ffgt/{}", env!("CARGO_PKG_VERSION"))
}

struct Conn {
    w: TcpStream,
    r: BufReader<TcpStream>,
    buf: String,
}
impl Conn {
    fn open(u: &Url, timeout: Duration) -> Result<Conn, String> {
        let addr = (u.host.as_str(), u.port).to_socket_addrs().map_err(|e| format!("resolve: {e}"))?.next().ok_or("no address")?;
        let s = TcpStream::connect_timeout(&addr, timeout).map_err(|e| format!("connect: {e}"))?;
        let _ = s.set_nodelay(true);
        s.set_read_timeout(Some(Duration::from_millis(200))).map_err(|e| e.to_string())?;
        let r = BufReader::new(s.try_clone().map_err(|e| e.to_string())?);
        Ok(Conn { w: s, r, buf: String::new() })
    }
    fn send(&mut self, v: &Value) -> Result<(), String> {
        let mut l = v.to_string();
        l.push('\n');
        self.w.write_all(l.as_bytes()).map_err(|e| format!("send: {e}"))
    }
    /// One JSON line, or None after the read timeout (partial lines are kept).
    fn line(&mut self) -> Result<Option<Value>, String> {
        match self.r.read_line(&mut self.buf) {
            Ok(0) => Err("connection closed by the pool".into()),
            Ok(_) if self.buf.ends_with('\n') => {
                let l = std::mem::take(&mut self.buf);
                serde_json::from_str(l.trim()).map(Some).map_err(|e| format!("bad JSON from the pool ({e}): {}", &l[..l.len().min(120)]))
            }
            Ok(_) => Ok(None),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => Ok(None),
            Err(e) => Err(format!("read: {e}")),
        }
    }
}

fn session(l: &Login, entry: usize, tx: &Sender<(Src, Event)>, rx: &Receiver<Submit>, stats: &Stats) -> End {
    let u = &l.urls[entry];
    let mut c = match Conn::open(u, Duration::from_secs(10)) {
        Ok(c) => c,
        Err(e) => return End::Failed(e),
    };
    if let Err(e) = c.send(&json!({"id":1,"method":"mining.subscribe","params":[agent()]})).and_then(|_| c.send(&json!({"id":2,"method":"mining.authorize","params":[l.login, l.pass]}))) {
        return End::Failed(e);
    }
    let started = Instant::now();
    let mut en1: Option<[u8; 4]> = None;
    let mut diff = 1.0f64;
    let mut authorized = false;
    let mut last_rx = Instant::now();
    let mut height = 0u64;
    let mut pending: HashMap<u64, (Instant, String)> = HashMap::new();
    let mut next_id = 10u64;
    let mut cur_job: Option<String> = None;
    // 0.2.2: the jobs of this height that share the last block (same header bytes 244..277, targets, extranonce1) are
    // searched as one group; a clean job / a new height / another share target starts it afresh
    let mut ring = Ring::new(if super::grouping() { super::node::GROUP_KEEP } else { 1 });
    loop {
        if l.stop.load(Ordering::Relaxed) {
            return End::Stop;
        }
        if !l.enabled.load(Ordering::Relaxed) {
            return End::Disabled;
        }
        if entry > 0 && l.failback_min > 0.0 && started.elapsed().as_secs_f64() >= l.failback_min * 60.0 {
            if probe(&l.urls[0], &l.login, &l.pass, Duration::from_secs(15)).is_ok() {
                return End::Failback;
            }
        }
        // shares
        while let Ok(s) = rx.try_recv() {
            let fresh = s.job.tgen == l.jgen.load(Ordering::SeqCst) && authorized;
            if !fresh {
                stats.stale_drop.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            let id = next_id;
            next_id += 1;
            let jid = s.job.tpl.job_id.clone();
            if c.send(&json!({"id":id,"method":"mining.submit","params":[l.login, jid, pow::hex(&s.nonce)]})).is_err() {
                stats.stale_drop.fetch_add(1, Ordering::Relaxed);
                return End::Failed("send failed".into());
            }
            stats.submitted.fetch_add(1, Ordering::Relaxed);
            let block = pow::meets(&s.id, &s.job.tpl.target);
            if block {
                log(l.name, &format!("share is a BLOCK candidate: {} (height {}, GPU{})", pow::hex(&s.id), s.job.tpl.height, s.gpu));
            }
            pending.insert(id, (Instant::now(), format!("GPU{} job {jid}", s.gpu)));
        }
        let v = match c.line() {
            Ok(Some(v)) => v,
            Ok(None) => {
                if last_rx.elapsed() > SILENT {
                    return End::Failed(format!("no line from the pool for {} s (dead link)", SILENT.as_secs()));
                }
                if !authorized && started.elapsed() > Duration::from_secs(20) {
                    return End::Failed("no authorize answer in 20 s".into());
                }
                continue;
            }
            Err(e) => return End::Failed(e),
        };
        last_rx = Instant::now();
        let method = v["method"].as_str().unwrap_or("");
        let id = v["id"].as_u64();
        match (method, id) {
            ("", Some(1)) => {
                let r = &v["result"];
                let e1 = r[1].as_str().and_then(pow::unhex).filter(|b| b.len() == 4);
                if r[2].as_u64() != Some(12) || e1.is_none() {
                    return End::Failed(format!("subscribe answer without a 4-byte extranonce1 and extranonce2 size 12: {v}"));
                }
                en1 = e1.map(|b| b.try_into().unwrap());
            }
            ("", Some(2)) => {
                if v["result"] != true {
                    return End::Failed(format!("authorize refused ({}): check the account (64 hex digits) and worker", v["error"]));
                }
                authorized = true;
                stats.connected.store(1, Ordering::Relaxed);
                log(l.name, &format!("logged in to {u} as {} (extranonce1 {})", l.login, en1.map_or("?".into(), |e| pow::hex(&e))));
            }
            ("mining.set_difficulty", _) => {
                if let Some(d) = v["params"][0].as_f64() {
                    diff = d;
                }
            }
            ("mining.notify", _) => {
                let p = &v["params"];
                let (Some(jid), Some(hx), Some(h), Some(tx_)) = (p[0].as_str(), p[1].as_str(), p[2].as_u64(), p[3].as_str()) else {
                    log(l.name, &format!("bad mining.notify: {v}"));
                    continue;
                };
                let (Some(pre), Some(net)) = (pow::unhex(hx).filter(|b| b.len() == pow::NONCE_OFFSET), pow::unhex(tx_).filter(|b| b.len() == 32)) else {
                    log(l.name, &format!("bad mining.notify fields (header {} hex, target {} hex)", hx.len(), tx_.len()));
                    continue;
                };
                let Some(e1) = en1 else { continue };
                let clean = p[4].as_bool().unwrap_or(false);
                if clean || h != height {
                    l.jgen.fetch_add(1, Ordering::SeqCst);
                }
                height = h;
                let mut header = [0u8; pow::HEADER_LEN];
                header[..pow::NONCE_OFFSET].copy_from_slice(&pre);
                let tpl = Template {
                    id: format!("{}:{jid}", l.name),
                    job_id: jid.to_string(),
                    height: h,
                    parent: String::new(),
                    header,
                    target: net.try_into().unwrap(),
                    share: diff_to_target(diff),
                    diff,
                    en1: Some(e1),
                    txs: 0,
                    got: Instant::now(),
                };
                if cur_job.as_deref() != Some(jid) {
                    cur_job = Some(jid.to_string());
                }
                if clean {
                    ring.clear();
                }
                if let Some(job) = ring.push(Job::new(l.src, tpl, l.jgen.load(Ordering::SeqCst))) {
                    let _ = tx.send((l.src, Event::Job(job)));
                }
            }
            ("", Some(i)) if pending.contains_key(&i) => {
                let (t, what) = pending.remove(&i).unwrap();
                if v["result"] == true {
                    stats.accepted.fetch_add(1, Ordering::Relaxed);
                    log(l.name, &format!("share ACCEPTED ({what}, d={diff}, {} ms)", t.elapsed().as_millis()));
                } else {
                    stats.rejected.fetch_add(1, Ordering::Relaxed);
                    let err = v["error"].to_string();
                    if err.contains("stale") || err.contains("21") || err.contains("job not found") {
                        stats.stale_rej.fetch_add(1, Ordering::Relaxed);
                    }
                    log(l.name, &format!("share REJECTED ({what}): {err}"));
                }
            }
            ("client.reconnect", _) => return End::Failed("the pool asked to reconnect".into()),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn targets() {
        // d = 1: 2^224 = 00000000 00000001 00..
        let t = diff_to_target(1.0);
        assert_eq!(pow::hex(&t[..8]), "0000000100000000");
        let t = diff_to_target(4.0);
        assert_eq!(pow::hex(&t[..8]), "0000000040000000");
        let t = diff_to_target(0.5);
        assert_eq!(pow::hex(&t[..8]), "0000000200000000");
    }
    #[test]
    fn urls() {
        assert_eq!(parse_url("stratum+tcp://crypto-connection.net:4073").unwrap(), Url { host: "crypto-connection.net".into(), port: 4073 });
        assert_eq!(parse_url("crypto-connection.net:4074").unwrap().port, 4074);
        assert!(parse_url("stratum+ssl://x:1").is_err());
    }
}
