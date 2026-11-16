// FreeForGem — see LICENSE
//! PROX solo mining through `proxi node mine` with an external nonce seeker, protocol version 2 (Proxima kb/external_nonce_seeker.md
//! sections 3 and 3.7).  This miner is the seeker (HTTP client, Bearer token = the node's mine.seeker.token):
//!   GET  /seeker/job?after=<id>&wait=<ms>   long poll: {id, version: 2, message, nonce_offset, k, beat, ttl_ms, pred, slot}; id 0 = no job
//!   POST /seeker/result {id, nonce, beta, attempts, seeker}  -> 200 {accepted, beat} | 409 stale | 422 not a solution
//!   POST /seeker/report {id, attempts, seeker}                about once a second
//! The node verifies every solution itself; a solution must have k zero bits AND a work key below the current `beat`.
//! Counters: accepted = new best, rejected = 422 (+ 409 in stale_rej), skipped = valid but not below the beat (someone was faster).
use super::pool::{Event, PJob, Submit, hex};
use crate::stratum::{Src, Stats};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub struct Node {
    pub host: String,
    pub port: u16,
    pub token: String,
    pub name: String, // seeker name shown by the node
}

impl std::fmt::Display for Node {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "http://{}:{}", self.host, self.port)
    }
}

pub fn parse_url(s: &str) -> Result<(String, u16), String> {
    let r = s.trim().strip_prefix("http://").ok_or_else(|| format!("--solo {s}: needs http://HOST:PORT (the node's mine.seeker.listen)"))?;
    let r = r.trim_end_matches('/');
    let (h, p) = r.rsplit_once(':').ok_or_else(|| format!("--solo {s}: needs HOST:PORT"))?;
    Ok((h.trim_start_matches('[').trim_end_matches(']').to_string(), p.parse().map_err(|_| format!("--solo {s}: bad port"))?))
}

/// The best work key known per job id (only ever goes down); the GPU workers check every candidate against it.
#[derive(Default)]
pub struct Beat(Mutex<Option<(String, [u8; 26])>>);
impl Beat {
    pub fn get(&self, id: &str) -> Option<[u8; 26]> {
        self.0.lock().unwrap().as_ref().filter(|b| b.0 == id).map(|b| b.1)
    }
    fn set(&self, id: &str, hexbeat: &str) {
        let Some(b) = crate::stratum::hex_decode(hexbeat.trim()).filter(|b| b.len() == 26) else { return };
        let b: [u8; 26] = b.try_into().unwrap();
        let mut g = self.0.lock().unwrap();
        if g.as_ref().is_some_and(|x| x.0 == id && x.1 <= b) {
            return;
        }
        *g = Some((id.to_string(), b));
    }
}

fn log(s: &str) {
    eprintln!("[prox-solo] {s}");
}

/// One HTTP/1.1 request (Connection: close); (status, body).
fn request(n: &Node, method: &str, path: &str, body: &str, timeout: Duration) -> std::io::Result<(u16, String)> {
    let addr = (n.host.as_str(), n.port).to_socket_addrs()?.next().ok_or_else(|| std::io::Error::other("address does not resolve"))?;
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(5))?;
    s.set_read_timeout(Some(timeout))?;
    s.set_write_timeout(Some(Duration::from_secs(10)))?;
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {}:{}\r\nUser-Agent: {}\r\nAccept: application/json\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        n.host,
        n.port,
        super::pool::AGENT,
        n.token,
        body.len()
    );
    s.write_all(req.as_bytes())?;
    let mut resp = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match s.read(&mut chunk) {
            Ok(0) => break,
            Ok(k) => resp.extend_from_slice(&chunk[..k]),
            Err(e) => return Err(e),
        }
        // complete once the headers and Content-Length bytes are in
        if let Some(p) = resp.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&resp[..p]).to_ascii_lowercase();
            if let Some(cl) = head.lines().find_map(|l| l.strip_prefix("content-length:").and_then(|v| v.trim().parse::<usize>().ok())) {
                if resp.len() >= p + 4 + cl {
                    break;
                }
            }
        }
    }
    let p = resp.windows(4).position(|w| w == b"\r\n\r\n").ok_or_else(|| std::io::Error::other("bad HTTP reply"))?;
    let head = String::from_utf8_lossy(&resp[..p]).to_string();
    let status: u16 = head.split_whitespace().nth(1).and_then(|x| x.parse().ok()).ok_or_else(|| std::io::Error::other("bad HTTP status line"))?;
    Ok((status, String::from_utf8_lossy(&resp[p + 4..]).to_string()))
}

/// Job id as JSON (numbers stay numbers)
fn id_json(id: &str) -> Value {
    serde_json::from_str::<Value>(id).ok().filter(Value::is_number).unwrap_or_else(|| Value::String(id.to_string()))
}

/// Starts the long-poll thread (jobs -> `tx`) and the sender thread (solutions from `rx`, attempt reports).
pub fn spawn(node: Node, tx: Sender<(Src, Event)>, rx: Receiver<Submit>, stats: Arc<Stats>, beat: Arc<Beat>, attempts: Arc<AtomicU64>, stop: Arc<AtomicBool>) {
    let node = Arc::new(node);
    let cur_id = Arc::new(Mutex::new(String::from("0")));
    {
        let (node, stats, beat, stop, cur_id) = (node.clone(), stats.clone(), beat.clone(), stop.clone(), cur_id.clone());
        std::thread::Builder::new().name("prox-solo-poll".into()).spawn(move || poll(&node, &tx, &stats, &beat, &stop, &cur_id)).expect("spawn");
    }
    std::thread::Builder::new().name("prox-solo-send".into()).spawn(move || sender(&node, &rx, &stats, &beat, &attempts, &stop, &cur_id)).expect("spawn");
}

fn poll(n: &Node, tx: &Sender<(Src, Event)>, stats: &Stats, beat: &Beat, stop: &AtomicBool, cur_id: &Mutex<String>) {
    let mut after = String::from("0");
    let (mut had_err, mut backoff, mut last_warn) = (false, 1u64, String::new());
    log(&format!("node {n}, seeker name {:?}", n.name));
    while !stop.load(Ordering::Relaxed) {
        let t0 = Instant::now();
        let r = request(n, "GET", &format!("/seeker/job?after={after}&wait=5000"), "", Duration::from_secs(20));
        let (st, body) = match r {
            Ok(x) => x,
            Err(e) => {
                if !had_err {
                    log(&format!("node not reachable: {e} (retrying)"));
                }
                had_err = true;
                stats.connected.store(0, Ordering::Relaxed);
                let _ = tx.send((Src::User, Event::Disconnected));
                std::thread::sleep(Duration::from_secs(backoff));
                backoff = (backoff * 2).min(10);
                continue;
            }
        };
        if st == 401 {
            log("401 from the node: wrong --token (the node's mine.seeker.token)");
            stats.connected.store(0, Ordering::Relaxed);
            std::thread::sleep(Duration::from_secs(5));
            continue;
        }
        if st != 200 {
            log(&format!("GET /seeker/job: HTTP {st} {}", &body[..body.len().min(100)]));
            std::thread::sleep(Duration::from_secs(2));
            continue;
        }
        backoff = 1;
        if had_err || stats.connected.load(Ordering::Relaxed) == 0 {
            log(&format!("connected to {n}"));
        }
        had_err = false;
        stats.connected.store(1, Ordering::Relaxed);
        let Ok(j) = serde_json::from_str::<Value>(&body) else {
            log(&format!("bad JSON from the node: {}", &body[..body.len().min(100)]));
            std::thread::sleep(Duration::from_millis(500));
            continue;
        };
        let id = match &j["id"] {
            Value::String(s) => s.clone(),
            v => v.to_string(),
        };
        if id.is_empty() || id == "0" || id == "null" {
            if after != "0" {
                let _ = tx.send((Src::User, Event::Pause("no job (node waiting)".into())));
            }
            after = "0".into();
            *cur_id.lock().unwrap() = "0".into();
            continue;
        }
        if let Some(b) = j["beat"].as_str() {
            beat.set(&id, b);
        }
        let ttl = j["ttl_ms"].as_i64().unwrap_or(0);
        if id == after {
            // the same job again (long poll timed out): only a fresh deadline / beat
            continue;
        }
        after = id.clone();
        let msg = j["message"].as_str().and_then(crate::stratum::hex_decode).unwrap_or_default();
        let version = j["version"].as_i64().unwrap_or(0);
        if version != 2 || msg.is_empty() || msg.len() > 4000 {
            if last_warn != id {
                log(&format!("job {id} is not a version 2 hash job (version {version}, message {} B): idle until one comes", msg.len()));
            }
            last_warn = id;
            let _ = tx.send((Src::User, Event::Pause("unsupported node job".into())));
            continue;
        }
        if let Some(off) = j["nonce_offset"].as_u64() {
            if off as usize != msg.len() {
                log(&format!("job {id}: nonce_offset {off} != message length {} (unsupported layout)", msg.len()));
                let _ = tx.send((Src::User, Event::Pause("unsupported layout".into())));
                continue;
            }
        }
        *cur_id.lock().unwrap() = id.clone();
        let job = PJob {
            job_id: id.clone(),
            prefix: msg,
            k: j["k"].as_u64().unwrap_or(64) as u32,
            en1: 0,
            pool: false,
            slot: j["slot"].as_i64().unwrap_or(0),
            clean: true,
            expires: (ttl > 0).then(|| t0 + Duration::from_millis(ttl as u64)),
            beat: beat.get(&id),
        };
        let _ = tx.send((Src::User, Event::Job(job)));
    }
}

fn sender(n: &Node, rx: &Receiver<Submit>, stats: &Stats, beat: &Beat, attempts: &AtomicU64, stop: &AtomicBool, cur_id: &Mutex<String>) {
    let mut last_report = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        if let Ok(s) = rx.recv_timeout(Duration::from_millis(200)) {
            let a = attempts.swap(0, Ordering::Relaxed);
            let body = json!({"id": id_json(&s.job_id), "nonce": hex(&s.nonce.to_be_bytes()), "beta": hex(&s.hash), "attempts": a, "seeker": n.name}).to_string();
            let t = Instant::now();
            match request(n, "POST", "/seeker/result", &body, Duration::from_secs(10)) {
                Err(e) => {
                    attempts.fetch_add(a, Ordering::Relaxed);
                    stats.submitted.fetch_sub(1, Ordering::Relaxed);
                    stats.stale_drop.fetch_add(1, Ordering::Relaxed);
                    log(&format!("cannot send the solution: {e}"));
                }
                Ok((200, resp)) => {
                    let v: Value = serde_json::from_str(&resp).unwrap_or(Value::Null);
                    if let Some(b) = v["beat"].as_str() {
                        beat.set(&s.job_id, b);
                    }
                    if v["accepted"].as_bool() == Some(true) {
                        stats.accepted.fetch_add(1, Ordering::Relaxed);
                        log(&format!("solution ACCEPTED (new best) job {} GPU{} {} ms", s.job_id, s.gpu, t.elapsed().as_millis()));
                    } else {
                        stats.skipped.fetch_add(1, Ordering::Relaxed);
                        log(&format!("solution valid but not below the current best (someone was faster), job {}", s.job_id));
                    }
                }
                Ok((409, resp)) => {
                    stats.rejected.fetch_add(1, Ordering::Relaxed);
                    stats.stale_rej.fetch_add(1, Ordering::Relaxed);
                    log(&format!("solution stale (job over), job {}: {}", s.job_id, resp.trim()));
                }
                Ok((st, resp)) => {
                    stats.rejected.fetch_add(1, Ordering::Relaxed);
                    log(&format!("solution REJECTED (HTTP {st}) job {} GPU{}: {}", s.job_id, s.gpu, resp.trim()));
                }
            }
        }
        if last_report.elapsed() >= Duration::from_secs(1) {
            last_report = Instant::now();
            let a = attempts.swap(0, Ordering::Relaxed);
            if a > 0 {
                let id = cur_id.lock().unwrap().clone();
                let body = json!({"id": id_json(&id), "attempts": a, "seeker": n.name}).to_string();
                if !matches!(request(n, "POST", "/seeker/report", &body, Duration::from_secs(5)), Ok((200, _))) {
                    attempts.fetch_add(a, Ordering::Relaxed);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn urls_and_ids() {
        assert_eq!(super::parse_url("http://127.0.0.1:8100/").unwrap(), ("127.0.0.1".to_string(), 8100));
        assert!(super::parse_url("127.0.0.1:8100").is_err());
        assert_eq!(super::id_json("1731650000123").to_string(), "1731650000123");
        assert_eq!(super::id_json("abc").to_string(), "\"abc\"");
    }
}
