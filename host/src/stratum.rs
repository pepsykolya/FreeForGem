// FreeForGemPear — see LICENSE
//! Minimal Pearl stratum client: newline-delimited JSON-RPC 2.0 over plain TCP, auto-reconnect with backoff.
//! One thread per login (user, dev fee); jobs are handed to the miner through a channel.
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Miner name shown by pools (LuckyPool reads it from the `agent` field of mining.authorize).
pub const AGENT: &str = concat!("FreeForGemPear/", env!("CARGO_PKG_VERSION"));

#[derive(Debug, Clone)]
pub struct PoolJob {
    pub job_id: String,
    pub header: Vec<u8>,
    pub target: Option<String>,
    pub difficulty: Option<f64>,
    pub cert_version: u32,
    pub gzip: bool,
}

pub enum Event {
    Job(PoolJob),
    SetDifficulty(f64),
    /// The connection was lost (the miner keeps the last job; shares of the dead connection are dropped).
    Disconnected,
}

pub struct Submit {
    pub id: u64,
    pub job_id: String,
    pub line: String,
}

#[derive(Default)]
pub struct Stats {
    pub submitted: AtomicU64,
    pub accepted: AtomicU64,
    pub rejected: AtomicU64,
    pub low_diff: AtomicU64,
    pub connected: AtomicU64,
    /// NOID: rejected as stale by the pool ("job not found" / "stale"), part of `rejected`
    pub stale_rej: AtomicU64,
    /// NOID: shares for a job the pool had already voided (pause / clean job), dropped before sending (not in `submitted`)
    pub stale_drop: AtomicU64,
    /// NOID: easier surplus shares not sent while the pool's replies lag (bounded backlog), not in `submitted`
    pub skipped: AtomicU64,
}

/// Which login a job / share belongs to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Src {
    User = 0,
    Fee = 1,
}

impl Src {
    pub fn idx(self) -> usize {
        self as usize
    }
}

/// Connection parameters of one stratum login.
pub struct Login {
    pub name: &'static str, // "user" / "fee": log prefix
    pub addr: String,
    pub wallet: String,
    /// Used instead of `wallet` once the pool has refused it (fee login: plain PRL when the PRL+NOCK login is refused).
    pub fallback_wallet: Option<String>,
    pub fallback: AtomicBool,
    pub worker: String,
    /// Stratum password (Kryptex reads a custom share difficulty `d=VALUE` from it).
    pub password: String,
    /// The connection is only kept while this is true (the fee login is enabled around its slot only).
    pub enabled: Arc<AtomicBool>,
    pub stop: Arc<AtomicBool>,
}

/// No line from the pool for this long: the connection is considered dead (half-open TCP) and is re-established.
/// The pool has no stratum-level keepalive (it answers unknown methods with "method not supported") but sends a
/// new job every ~30-60 s, so silence is a reliable sign of a dead link; TCP keepalive is enabled as well.
/// Proofs are sent gzip-compressed (~200 KB -> ~5 KB) when the pool agreed to it in the handshake: the miner sends
/// `"type":"v2"` in mining.authorize and a pool that decompresses proofs answers `"type":"v2"` (the Pearl stratum gzip
/// protocol, used by Kryptex and LuckyPool; gzip = zlib wbits 31). Without that answer (or after a rejected compressed proof)
/// proofs go uncompressed: such a pool would take a compressed proof as raw and silently not credit it.
/// A job that lists `proof_encodings` with "gzip" also enables it. FFGP_PROOF_GZIP=on/off forces the choice.
static GZIP_REFUSED: AtomicBool = AtomicBool::new(false);

const SILENCE_LIMIT: Duration = Duration::from_secs(240);
const BACKOFF_MIN: Duration = Duration::from_secs(2);
const BACKOFF_MAX: Duration = Duration::from_secs(60);

pub fn spawn(l: Login, src: Src, tx: Sender<(Src, Event)>, rx: Receiver<Submit>, stats: Arc<Stats>) {
    std::thread::Builder::new()
        .name(format!("stratum-{}", l.name))
        .spawn(move || {
            let mut backoff = BACKOFF_MIN;
            while !l.stop.load(Ordering::Relaxed) {
                if !l.enabled.load(Ordering::Relaxed) {
                    while rx.try_recv().is_ok() {}
                    std::thread::sleep(Duration::from_millis(200));
                    continue;
                }
                let t = Instant::now();
                match session(&l, src, &tx, &rx, &stats) {
                    Ok(()) => log(&l, "connection closed"),
                    Err(e) => log(&l, &format!("{e}")),
                }
                stats.connected.store(0, Ordering::Relaxed);
                let _ = tx.send((src, Event::Disconnected));
                while rx.try_recv().is_ok() {} // drop submits that belong to the dead connection
                if !l.enabled.load(Ordering::Relaxed) || l.stop.load(Ordering::Relaxed) {
                    continue;
                }
                // exponential backoff; a session that lived a while starts over from the minimum (the fee
                // login always retries quickly: its slot is short)
                if t.elapsed() > Duration::from_secs(60) {
                    backoff = BACKOFF_MIN;
                }
                let wait = if src == Src::Fee { BACKOFF_MIN } else { backoff };
                log(&l, &format!("reconnecting in {}s", wait.as_secs()));
                let until = Instant::now() + wait;
                while Instant::now() < until && l.enabled.load(Ordering::Relaxed) && !l.stop.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(100));
                }
                backoff = (backoff * 2).min(BACKOFF_MAX);
            }
        })
        .expect("spawn stratum thread");
}

/// LuckyPool (`*.luckypool.io`): takes the worker in its own authorize field and understands PRL+NOCK merge-mining wallets.
pub fn is_luckypool(pool: &str) -> bool {
    let host = pool.rsplit_once(':').map_or(pool, |(h, _)| h).trim_end_matches('.').to_ascii_lowercase();
    host == "luckypool.io" || host.ends_with(".luckypool.io")
}

/// HimPool (`*.himpool.com`) and AlphaPool (`*.alphapool.tech`): merge-mine NACRE (and HimPool also NOCK) from a
/// `PRL+NACR[+NOCK]` login, with the worker appended after all addresses (`prl1...+nacr1p....rig1`).
pub fn is_merge_pool(pool: &str) -> bool {
    host_in(pool, "himpool.com") || is_alphapool(pool)
}

/// AlphaPool (`*.alphapool.tech`) wants mining.authorize first: a mining.subscribe gets error 24 and the connection is closed.
pub fn is_alphapool(pool: &str) -> bool {
    host_in(pool, "alphapool.tech")
}

fn host_in(pool: &str, domain: &str) -> bool {
    let host = pool.rsplit_once(':').map_or(pool, |(h, _)| h).trim_end_matches('.').to_ascii_lowercase();
    host == domain || host.ends_with(&format!(".{domain}"))
}

/// `--pool` as HOST:PORT. Drops a `stratum+tcp://`-style prefix copied from a pool's page. TLS schemes are refused with a
/// clear error: the client speaks plain TCP only, so a TLS port would just fail the handshake.
pub fn pool_addr(pool: &str) -> Result<String, String> {
    let p = pool.trim();
    let addr = match p.split_once("://") {
        None => p,
        Some((scheme, rest)) => match scheme.to_ascii_lowercase().as_str() {
            "stratum+tcp" | "stratum" | "tcp" => rest,
            "stratum+ssl" | "stratum+tls" | "ssl" | "tls" => {
                return Err(format!("--pool {p}: SSL/TLS is not supported, use the pool's plain TCP port (for example Kryptex 7048, not 8048)"));
            }
            _ => return Err(format!("--pool {p}: unknown scheme '{scheme}://', use HOST:PORT")),
        },
    };
    Ok(addr.trim_end_matches('/').to_string())
}

/// Wallet string sent in mining.authorize. LuckyPool: as given. HimPool/AlphaPool: the `+NACR` (`+NOCK`) parts are kept
/// (merge mining). Other pools (Kryptex): a `+...` part is dropped (such a login is refused). Outside LuckyPool the worker
/// is appended as `wallet.worker`, because those pools read the worker name from the login string, not from the `worker`
/// field. Returns the wallet and a note for the log.
pub fn login_wallet(pool: &str, wallet: &str, worker: &str) -> (String, Option<String>) {
    if is_luckypool(pool) {
        return (wallet.to_string(), None);
    }
    let mut note = None;
    let mut w = wallet;
    if let Some((prl, _)) = wallet.split_once('+') {
        if is_merge_pool(pool) {
            note = Some(format!("merge mining: PRL to {prl}, +{} on this pool", wallet[prl.len() + 1..].replace('+', " +")));
        } else {
            note = Some(format!("merge mining (+NOCK/+NACR) works on LuckyPool, HimPool and AlphaPool only: mining PRL to {prl} on this pool, the + part is ignored"));
            w = prl;
        }
    }
    if worker.is_empty() || w.contains('.') || w.contains('/') { (w.to_string(), note) } else { (format!("{w}.{worker}"), note) }
}

fn wallet(l: &Login) -> &str {
    match &l.fallback_wallet {
        Some(w) if l.fallback.load(Ordering::Relaxed) => w,
        _ => &l.wallet,
    }
}

fn log(l: &Login, s: &str) {
    eprintln!("[stratum:{}] {s}", l.name);
}

fn connect(addr: &str) -> std::io::Result<TcpStream> {
    let mut last = None;
    for a in addr.to_socket_addrs()? {
        match TcpStream::connect_timeout(&a, Duration::from_secs(10)) {
            Ok(s) => return Ok(s),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| std::io::Error::new(ErrorKind::NotFound, "address does not resolve")))
}

fn session(l: &Login, src: Src, tx: &Sender<(Src, Event)>, rx: &Receiver<Submit>, stats: &Stats) -> std::io::Result<()> {
    log(l, &format!("connecting to {} as {}", l.addr, l.worker));
    let mut sock = connect(&l.addr)?;
    sock.set_read_timeout(Some(Duration::from_millis(100)))?;
    sock.set_write_timeout(Some(Duration::from_secs(20)))?;
    sock.set_nodelay(true)?;
    let ka = socket2::TcpKeepalive::new().with_time(Duration::from_secs(30)).with_interval(Duration::from_secs(10));
    let _ = socket2::SockRef::from(&sock).set_tcp_keepalive(&ka);
    stats.connected.store(1, Ordering::Relaxed);
    // subscribe and authorize back-to-back (LuckyPool drops us otherwise); AlphaPool: authorize only
    let mut hello = vec![
        json!({"jsonrpc":"2.0","id":1,"method":"mining.subscribe","params":[AGENT]}),
        json!({"jsonrpc":"2.0","id":2,"method":"mining.authorize","params":{"wallet":wallet(l),"worker":l.worker,"pass":l.password,"password":l.password,"agent":AGENT,"type":"v2"}}),
    ];
    if is_alphapool(&l.addr) {
        hello.remove(0);
    }
    for h in &hello {
        sock.write_all(format!("{h}\n").as_bytes())?;
    }
    let mut reader = BufReader::new(sock.try_clone()?);
    let mut buf = String::new();
    let mut pending: HashMap<u64, String> = HashMap::new();
    let mut seen = 0;
    let mut v2 = false; // pool confirmed gzip proofs ("type":"v2" in the authorize reply)
    // jobs that arrive before the authorize reply wait for it: once a pool confirmed v2, every proof must be gzipped
    let mut authorized = false;
    let mut early: Vec<Value> = Vec::new();
    let started = Instant::now();
    let mut last_rx = Instant::now();
    loop {
        if l.stop.load(Ordering::Relaxed) || !l.enabled.load(Ordering::Relaxed) {
            return Ok(());
        }
        if last_rx.elapsed() > SILENCE_LIMIT {
            return Err(std::io::Error::new(ErrorKind::TimedOut, format!("no data from the pool for {}s", SILENCE_LIMIT.as_secs())));
        }
        // a pool that never answers authorize: go on after 5 s, uncompressed
        if !authorized && started.elapsed() > Duration::from_secs(5) {
            authorized = true;
            log(l, "no authorize reply after 5 s: mining with uncompressed proofs");
            for e in early.drain(..) {
                if let Some(j) = parse_job(&e, false) {
                    let _ = tx.send((src, Event::Job(j)));
                }
            }
        }
        // outgoing submits
        while let Ok(s) = rx.try_recv() {
            pending.insert(s.id, s.job_id);
            sock.write_all(s.line.as_bytes())?;
            sock.write_all(b"\n")?;
        }
        match reader.read_line(&mut buf) {
            Ok(0) => return Ok(()),
            Ok(_) => last_rx = Instant::now(),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                if !buf.ends_with('\n') {
                    continue; // partial line stays in buf
                }
            }
            Err(e) => return Err(e),
        }
        if !buf.ends_with('\n') {
            continue;
        }
        let line = std::mem::take(&mut buf);
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            log(l, &format!("unparsable: {line}"));
            continue;
        };
        let method = v.get("method").and_then(Value::as_str).unwrap_or("");
        match method {
            "mining.notify" | "job" => {
                if seen < 3 {
                    seen += 1;
                    log(l, &format!("RAW JOB: {line}"));
                }
                if !authorized {
                    early.truncate(0); // only the newest job matters
                    early.push(v.clone());
                    continue;
                }
                match parse_job(&v, v2) {
                    Some(j) => {
                        let _ = tx.send((src, Event::Job(j)));
                    }
                    None => log(l, &format!("job not understood: {line}")),
                }
            }
            "mining.set_difficulty" => {
                log(l, &format!("set_difficulty: {line}"));
                let p = &v["params"];
                let d = p.get(0).or_else(|| p.get("difficulty")).and_then(Value::as_f64);
                if let Some(d) = d {
                    let _ = tx.send((src, Event::SetDifficulty(d)));
                }
            }
            "" => {
                let id = v.get("id").and_then(Value::as_u64).unwrap_or(0);
                let ok = v.get("result").map(|r| r.as_bool().unwrap_or(!r.is_null())).unwrap_or(false) && v.get("error").map_or(true, Value::is_null);
                if let Some(job_id) = pending.remove(&id) {
                    if ok {
                        stats.accepted.fetch_add(1, Ordering::Relaxed);
                        log(l, &format!("share ACCEPTED (id {id}, job {job_id}): {line}"));
                    } else {
                        stats.rejected.fetch_add(1, Ordering::Relaxed);
                        let low = line.to_lowercase();
                        if low.contains("low") || low.contains("difficulty") {
                            stats.low_diff.fetch_add(1, Ordering::Relaxed);
                        }
                        log(l, &format!("share REJECTED (id {id}, job {job_id}): {line}"));
                        if (low.contains("gzip") || low.contains("encod") || low.contains("decompress") || low.contains("decode"))
                            && !GZIP_REFUSED.swap(true, Ordering::Relaxed)
                        {
                            log(l, "the pool does not take gzip proofs: sending them uncompressed from the next job");
                        }
                    }
                } else if id == 2 && !ok {
                    if l.fallback_wallet.is_some() && !l.fallback.swap(true, Ordering::Relaxed) {
                        log(l, "the pool refused the merged wallet: logging in with the PRL wallet only from now on");
                    }
                    return Err(std::io::Error::new(ErrorKind::PermissionDenied, format!("authorize failed: {line}")));
                } else if id == 1 && !ok {
                    // some pools (LuckyPool) do not implement mining.subscribe; the login goes on without it.
                    // Logged without the raw reply: its "error" object would look like a failure in the Windows window.
                    let msg = v["error"].get("message").or_else(|| v["error"].get("msg")).and_then(Value::as_str).unwrap_or("not supported");
                    log(l, &format!("pool skips mining.subscribe ({msg}): normal for this pool, logging in"));
                } else {
                    log(l, &format!("reply: {line}"));
                    if id == 2 {
                        v2 = v.get("type").and_then(Value::as_str) == Some("v2");
                        log(l, if v2 { "pool confirmed gzip proofs (type v2)" } else { "pool did not confirm gzip (no type v2): proofs go uncompressed" });
                        authorized = true;
                        for e in early.drain(..) {
                            match parse_job(&e, v2) {
                                Some(j) => {
                                    let _ = tx.send((src, Event::Job(j)));
                                }
                                None => log(l, &format!("job not understood: {e}")),
                            }
                        }
                    }
                }
            }
            _ => log(l, &format!("other: {line}")),
        }
    }
}

fn parse_job(v: &Value, v2: bool) -> Option<PoolJob> {
    let p = v.get("params")?;
    if !p.is_object() {
        return None;
    }
    let job_id = match &p["job_id"] {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => return None,
    };
    let header = hex_decode(p["header"].as_str()?)?;
    if header.len() != 76 {
        return None;
    }
    let difficulty = p.get("difficulty").or_else(|| p.get("diff")).and_then(Value::as_f64).or_else(|| {
        let (_, suf) = job_id.rsplit_once('_')?;
        suf.parse::<f64>().ok()
    });
    let encodings = p["proof_encodings"].as_array();
    let gzip = match std::env::var("FFGP_PROOF_GZIP").as_deref() {
        Ok("on") => true,
        Ok("off") => false,
        _ => (v2 || encodings.map_or(false, |a| a.iter().any(|e| e == "gzip"))) && !GZIP_REFUSED.load(Ordering::Relaxed),
    };
    Some(PoolJob {
        job_id,
        header,
        target: p["target"].as_str().map(|s| s.trim_start_matches("0x").to_string()),
        difficulty,
        cert_version: p["cert_version"].as_u64().unwrap_or(3) as u32,
        gzip,
    })
}

pub fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len() / 2).map(|i| u8::from_str_radix(s.get(2 * i..2 * i + 2)?, 16).ok()).collect()
}

#[cfg(test)]
mod login_tests {
    use super::*;

    #[test]
    fn login_wallet_per_pool() {
        let p = "prl1abcdefghijklmnopqrstuvwxyz";
        let n = "DZgTtG2fuAYQSBbbV1GZ1ab4UfTpKtkm";
        let pn = format!("{p}+{n}");
        assert_eq!(login_wallet("pearl-eu2.luckypool.io:3360", &pn, "rig1"), (pn.clone(), None));
        assert_eq!(login_wallet("pearl-eu2.luckypool.io:3360", p, "rig1").0, p);
        assert_eq!(login_wallet("prl-eu.kryptex.network:7048", p, "rig1").0, format!("{p}.rig1"));
        let (w, note) = login_wallet("prl-eu.kryptex.network:7048", &pn, "rig1");
        assert_eq!(w, format!("{p}.rig1"));
        assert!(note.is_some());
        assert_eq!(login_wallet("prl-eu.kryptex.network:7048", &format!("{p}/own"), "rig1").0, format!("{p}/own"));
        assert_eq!(login_wallet("prl-eu.kryptex.network:7048", p, "").0, p);
    }

    #[test]
    fn login_wallet_nacr_merge() {
        let p = "prl1abcdefghijklmnopqrstuvwxyz";
        let pa = format!("{p}+nacr1pqrstuvwxyz");
        let pan = format!("{pa}+DZgTtG2fuAYQSBbbV1GZ1ab4UfTpKtkm");
        for pool in ["eu.himpool.com:1480", "EU1.AlphaPool.tech:5571", "himpool.com:1481"] {
            let (w, note) = login_wallet(pool, &pa, "rig1");
            assert_eq!(w, format!("{pa}.rig1"), "{pool}");
            assert!(note.unwrap().contains("+nacr1p"), "{pool}");
            assert_eq!(login_wallet(pool, p, "rig1"), (format!("{p}.rig1"), None), "{pool}");
        }
        assert_eq!(login_wallet("ca.himpool.com:1480", &pan, "rig1").0, format!("{pan}.rig1"));
        assert_eq!(login_wallet("prl-eu.kryptex.network:7048", &pa, "rig1").0, format!("{p}.rig1"));
        assert!(!is_merge_pool("himpool.com.evil.net:1480"));
        assert!(!is_merge_pool("nothimpool.com:1480"));
    }

    #[test]
    fn pool_addr_schemes() {
        let k = "prl-eu.kryptex.network:7048";
        assert_eq!(pool_addr(k).unwrap(), k);
        assert_eq!(pool_addr(&format!("stratum+tcp://{k}")).unwrap(), k);
        assert_eq!(pool_addr(&format!("Stratum+TCP://{k}/")).unwrap(), k);
        assert_eq!(pool_addr(&format!(" tcp://{k} ")).unwrap(), k);
        assert_eq!(pool_addr("stratum://pearl-eu2.luckypool.io:3360").unwrap(), "pearl-eu2.luckypool.io:3360");
        assert!(pool_addr("stratum+ssl://prl-eu.kryptex.network:8048").unwrap_err().contains("not supported"));
        assert!(pool_addr("ssl://prl-eu.kryptex.network:8048").is_err());
        assert!(pool_addr("http://prl-eu.kryptex.network:7048").unwrap_err().contains("unknown scheme"));
    }
}
