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
    /// the entry in use (`pools[entry]`, set by the connection loop; `--pool` list, the first one is the primary)
    pub addr: String,
    pub pools: Vec<String>,
    /// wallet text of the login for a pool address (login_wallet(pool, wallet, worker) of the user / of the fee)
    pub wallet_fn: Arc<dyn Fn(&str) -> String + Send + Sync>,
    pub link: Arc<crate::failover::Link>,
    /// the fee login: follows the user's entry
    pub follow: Option<Arc<crate::failover::Link>>,
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

const SILENCE_LIMIT: Duration = Duration::from_secs(240); // until the pool answered a probe on this connection (silence.rs); then 60 s

/// The connection loop of one login (failover.rs): the entries of the `--pool` list in turn, back to the primary on request.
/// A single pool reconnects with exponential backoff as before; the fee login always retries quickly (its slot is short).
pub fn spawn(l: Login, src: Src, tx: Sender<(Src, Event)>, rx: Receiver<Submit>, stats: Arc<Stats>) {
    std::thread::Builder::new()
        .name(format!("stratum-{}", l.name))
        .spawn(move || {
            let mut l = l;
            let lp = crate::failover::Loop {
                name: l.name,
                link: l.link.clone(),
                follow: l.follow.clone(),
                stop: l.stop.clone(),
                enabled: l.enabled.clone(),
                fee: src == Src::Fee,
                label: Box::new({
                    let pools = l.pools.clone();
                    move |e| crate::failover::label(&pools[e.min(pools.len() - 1)])
                }),
            };
            crate::failover::run(
                lp,
                &mut l,
                |l, e| {
                    l.addr = l.pools[e].clone();
                    l.wallet = (l.wallet_fn)(&l.addr);
                    l.fallback.store(false, Ordering::Relaxed);
                },
                |l| session(l, src, &tx, &rx, &stats),
                |_| {
                    stats.connected.store(0, Ordering::Relaxed);
                    let _ = tx.send((src, Event::Disconnected));
                    while rx.try_recv().is_ok() {} // drop submits that belong to the dead connection
                },
                |_| while rx.try_recv().is_ok() {},
            );
        })
        .expect("spawn stratum thread");
}

/// `--pool-check`: the failback check (connect + login + a job) of every entry of the list.
pub fn check(pools: &[String], wallet: &str, worker: &str, password: &str, timeout: Duration) -> Vec<Result<(), String>> {
    let (w, wk) = (wallet.to_string(), worker.to_string());
    let wallet_fn: Arc<dyn Fn(&str) -> String + Send + Sync> = Arc::new(move |p: &str| login_wallet(p, &w, &wk).0);
    let l = Login {
        name: "check",
        addr: pools[0].clone(),
        pools: pools.to_vec(),
        wallet: wallet_fn(&pools[0]),
        wallet_fn,
        link: crate::failover::Link::new(1),
        follow: None,
        fallback_wallet: None,
        fallback: AtomicBool::new(false),
        worker: worker.to_string(),
        password: password.to_string(),
        enabled: Arc::new(AtomicBool::new(true)),
        stop: Arc::new(AtomicBool::new(false)),
    };
    (0..pools.len()).map(|e| probe(&l, e, timeout)).collect()
}

/// One check of entry `entry` of the list (failback): connect + login + a usable job, nothing is submitted. The session code is
/// the mining one, on a scratch channel.
pub fn probe(l: &Login, entry: usize, timeout: Duration) -> Result<(), String> {
    let addr = l.pools[entry].clone();
    let p = Login {
        name: "probe",
        wallet: (l.wallet_fn)(&addr),
        addr,
        pools: l.pools.clone(),
        wallet_fn: l.wallet_fn.clone(),
        link: crate::failover::Link::new(1),
        follow: None,
        fallback_wallet: l.fallback_wallet.clone(),
        fallback: AtomicBool::new(false),
        worker: l.worker.clone(),
        password: l.password.clone(),
        enabled: Arc::new(AtomicBool::new(true)),
        stop: Arc::new(AtomicBool::new(false)),
    };
    crate::failover::probe_session::<Event, Submit>(
        timeout,
        move |en, tx, rx| {
            let p = Login { enabled: en, ..p };
            session(&p, Src::User, &tx, &rx, &Stats::default())
        },
        |e| matches!(e, Event::Job(_)),
    )
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

/// Kryptex (`*.kryptex.network`): also takes a Kryptex account username instead of a PRL address (payouts in BTC/USDT,
/// auto-conversion); the worker then follows a slash: `username/rig1`.
pub fn is_kryptex(pool: &str) -> bool {
    host_in(pool, "kryptex.network")
}

/// A Kryptex account username (not a `prl1...` address): letters, digits, `_` and `-`.
pub fn is_kryptex_username(wallet: &str) -> bool {
    !wallet.is_empty()
        && !wallet.get(..4).is_some_and(|p| p.eq_ignore_ascii_case("prl1"))
        && wallet.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
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
/// field; a Kryptex account username gets it as `username/worker`. Returns the wallet and a note for the log.
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
    if worker.is_empty() || w.contains('.') || w.contains('/') {
        (w.to_string(), note)
    } else if is_kryptex(pool) && is_kryptex_username(w) {
        (format!("{w}/{worker}"), note)
    } else {
        (format!("{w}.{worker}"), note)
    }
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
    let mut sil = crate::silence::Silence::new(SILENCE_LIMIT);
    let mut drain = crate::failover::Drain::new();
    loop {
        if l.stop.load(Ordering::Relaxed) || !l.enabled.load(Ordering::Relaxed) {
            return Ok(());
        }
        if let Some(idle) = sil.expired() {
            return Err(crate::silence::Silence::error(idle));
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
            if drain.frozen() {
                continue; // a switch to another pool is under way: this share belongs to a job of the old pool
            }
            pending.insert(s.id, s.job_id);
            sock.write_all(s.line.as_bytes())?;
            sock.write_all(b"\n")?;
        }
        // a switch to another pool (failback): the shares found meanwhile went out above, wait for the pool's answers, then end
        if drain.check(&l.link, pending.is_empty()) {
            return Ok(());
        }
        if sil.probe_due() {
            // no keepalive in this protocol: after 20 s of silence ask something the pool answers ("method not supported" is an answer)
            sil.synthetic_sent();
            sock.write_all(crate::silence::Silence::probe_line().as_bytes())?;
        }
        match reader.read_line(&mut buf) {
            Ok(0) => return Ok(()),
            Ok(_) => sil.rx(),
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
                if id == crate::silence::PROBE_ID {
                    sil.answered(); // any answer, an error too
                } else if let Some(job_id) = pending.remove(&id) {
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
    // goal profit (auto-lock): the network target (nbits, header bytes 72..76) and the height of the job
    if let Some(h) = p.get("height").and_then(Value::as_u64) {
        crate::autolock::note_net("pearl", h, crate::profit::work_nbits(u32::from_le_bytes([header[72], header[73], header[74], header[75]])));
    }
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
    fn login_wallet_kryptex_username() {
        let k = "prl-eu.kryptex.network:7048";
        assert_eq!(login_wallet(k, "krxAbCd1234", "rig1").0, "krxAbCd1234/rig1");
        assert_eq!(login_wallet(k, "krxAbCd1234/own", "rig1").0, "krxAbCd1234/own");
        assert_eq!(login_wallet(k, "krxAbCd1234", "").0, "krxAbCd1234");
        assert_eq!(login_wallet(k, "PRL1abcdefghijklmnopqrstuvwxyz", "rig1").0, "PRL1abcdefghijklmnopqrstuvwxyz.rig1");
        // only Kryptex takes a username: other pools keep wallet.worker
        assert_eq!(login_wallet("pearl.example.org:3333", "krxAbCd1234", "rig1").0, "krxAbCd1234.rig1");
        assert!(is_kryptex_username("user_name-1") && !is_kryptex_username("prl1abc") && !is_kryptex_username("a.b"));
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

/// Failover / failback against local mock pools (the real connection loop, session code, prober and drain; no GPU).
#[cfg(test)]
mod failover_tests {
    use super::*;
    use crate::failover::{self, Link};
    use std::io::{BufRead, BufReader, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::Mutex;
    use std::sync::mpsc;

    /// A Pearl stratum mock: login, one job per login, every submit accepted. `stop()` = the pool goes down (open connections are cut).
    struct Mock {
        port: u16,
        alive: Arc<AtomicBool>,
        conns: Arc<Mutex<Vec<TcpStream>>>,
        logins: Arc<Mutex<Vec<String>>>,
        submits: Arc<AtomicU64>,
        /// what a submit gets: 0 accepted, 1 rejected, 2 no answer at all
        mode: Arc<std::sync::atomic::AtomicU8>,
    }

    fn free_port() -> u16 {
        TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
    }

    impl Mock {
        fn new(port: u16) -> Mock {
            Mock { port, alive: Arc::new(AtomicBool::new(false)), conns: Arc::new(Mutex::new(vec![])), logins: Arc::new(Mutex::new(vec![])), submits: Arc::new(AtomicU64::new(0)), mode: Arc::new(std::sync::atomic::AtomicU8::new(0)) }
        }
        fn start(&self) {
            let l = TcpListener::bind(("127.0.0.1", self.port)).expect("bind mock port");
            l.set_nonblocking(true).unwrap();
            self.alive.store(true, Ordering::SeqCst);
            let (alive, conns, logins, submits, mode) = (self.alive.clone(), self.conns.clone(), self.logins.clone(), self.submits.clone(), self.mode.clone());
            std::thread::spawn(move || {
                while alive.load(Ordering::SeqCst) {
                    match l.accept() {
                        Ok((s, _)) => {
                            s.set_nonblocking(false).unwrap();
                            conns.lock().unwrap().push(s.try_clone().unwrap());
                            let (logins, submits, mode) = (logins.clone(), submits.clone(), mode.clone());
                            std::thread::spawn(move || {
                                let w = Arc::new(Mutex::new(s.try_clone().unwrap()));
                                let send = |w: &Mutex<TcpStream>, v: Value| {
                                    let _ = w.lock().unwrap().write_all(format!("{v}
").as_bytes());
                                };
                                for line in BufReader::new(s).lines().map_while(Result::ok) {
                                    let v: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
                                    match v["method"].as_str() {
                                        Some("mining.subscribe") => {
                                            send(&w, json!({"jsonrpc":"2.0","id":1,"result":[]}));
                                        }
                                        Some("mining.authorize") => {
                                            logins.lock().unwrap().push(format!("{}", v["params"]["wallet"].as_str().unwrap_or("")));
                                            send(&w, json!({"jsonrpc":"2.0","id":2,"result":true}));
                                            let job = json!({"jsonrpc":"2.0","method":"mining.notify","params":{"job_id":"j1","header":"00".repeat(76),"target":"ff".repeat(32),"difficulty":1.0}});
                                            send(&w, job);
                                        }
                                        Some("mining.submit") => {
                                            submits.fetch_add(1, Ordering::SeqCst);
                                            // a pool needs a moment to answer: the switch must wait for it
                                            let m = mode.load(Ordering::SeqCst);
                                            if m == 2 {
                                                continue; // silent: no answer
                                            }
                                            let (w2, id) = (w.clone(), v["id"].clone());
                                            std::thread::spawn(move || {
                                                std::thread::sleep(Duration::from_millis(150));
                                                let r = if m == 1 { json!({"jsonrpc":"2.0","id":id,"result":null,"error":{"code":20,"message":"rejected"}}) } else { json!({"jsonrpc":"2.0","id":id,"result":true}) };
                                                let _ = w2.lock().unwrap().write_all(format!("{r}
").as_bytes());
                                            });
                                        }
                                        _ => {}
                                    }
                                }
                            });
                        }
                        Err(_) => std::thread::sleep(Duration::from_millis(10)),
                    }
                }
            });
        }
        fn stop(&self) {
            self.alive.store(false, Ordering::SeqCst);
            for c in self.conns.lock().unwrap().drain(..) {
                let _ = c.shutdown(std::net::Shutdown::Both);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        fn logins(&self) -> Vec<String> {
            self.logins.lock().unwrap().clone()
        }
    }

    fn login(name: &'static str, pools: &[String], wallet: &str, link: Arc<Link>, follow: Option<Arc<Link>>, enabled: Arc<AtomicBool>, stop: Arc<AtomicBool>) -> Login {
        let w = wallet.to_string();
        let wallet_fn: Arc<dyn Fn(&str) -> String + Send + Sync> = Arc::new(move |_p: &str| format!("{w}.rig"));
        Login {
            name,
            addr: pools[0].clone(),
            pools: pools.to_vec(),
            wallet: wallet_fn(&pools[0]),
            wallet_fn,
            link,
            follow,
            fallback_wallet: None,
            fallback: AtomicBool::new(false),
            worker: "rig".into(),
            password: "x".into(),
            enabled,
            stop,
        }
    }

    fn wait_for(what: &str, secs: u64, mut f: impl FnMut() -> bool) {
        let t = Instant::now();
        while !f() {
            assert!(t.elapsed() < Duration::from_secs(secs), "timeout waiting for: {what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    static GLOBAL: Mutex<()> = Mutex::new(());

    /// the user's session against pool A (mode `mode`) and a good pool B, with the accepted-shares watchdog ticking like the status loop
    /// does; `expect` = shares expected per second at the measured hash rate
    fn watchdog_run(mode: u8, expect: f64, secs: u64) -> (Option<usize>, u64, u64) {
        failover::set_test_timing(0.0, 0.4, 100); // no failback
        failover::set_watchdog_secs(3.0, 2.0, 1.0);
        let (pa, pb) = (free_port(), free_port());
        let (a, b) = (Mock::new(pa), Mock::new(pb));
        a.mode.store(mode, Ordering::SeqCst);
        a.start();
        b.start();
        let pools = vec![format!("127.0.0.1:{pa}"), format!("127.0.0.1:{pb}")];
        let stop = Arc::new(AtomicBool::new(false));
        let (ev_tx, _ev_rx) = mpsc::channel::<(Src, Event)>();
        let (sub_tx, sub_rx) = mpsc::channel::<Submit>();
        let stats = Arc::new(Stats::default());
        let link = Link::new(2);
        spawn(login("user", &pools, "UW", link.clone(), None, Arc::new(AtomicBool::new(true)), stop.clone()), Src::User, ev_tx, sub_rx, stats.clone());
        {
            let (stop, stats, link) = (stop.clone(), stats.clone(), link.clone());
            std::thread::spawn(move || {
                let mut w = failover::Watch::default();
                while !stop.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(100));
                    let allowed = stats.connected.load(Ordering::SeqCst) != 0;
                    failover::watch_tick(&mut w, &link, "user", 0.1, allowed, 1e9, Some(1e9 / expect), stats.accepted.load(Ordering::SeqCst), stats.rejected.load(Ordering::SeqCst), stats.submitted.load(Ordering::SeqCst));
                }
            });
        }
        {
            let (stop, stats) = (stop.clone(), stats.clone());
            std::thread::spawn(move || {
                let mut id = 100u64;
                while !stop.load(Ordering::SeqCst) {
                    id += 1;
                    let line = json!({"jsonrpc":"2.0","id":id,"method":"mining.submit","params":["j1","00"]}).to_string();
                    stats.submitted.fetch_add(1, Ordering::SeqCst);
                    let _ = sub_tx.send(Submit { id, job_id: "j1".into(), line });
                    std::thread::sleep(Duration::from_millis(100));
                }
            });
        }
        std::thread::sleep(Duration::from_secs(secs));
        let r = (link.entry(), stats.accepted.load(Ordering::SeqCst), b.submits.load(Ordering::SeqCst));
        stop.store(true, Ordering::SeqCst);
        a.stop();
        b.stop();
        r
    }

    #[test]
    fn watchdog_leaves_a_pool_that_rejects_or_ignores_shares() {
        let _g = GLOBAL.lock().unwrap_or_else(|e| e.into_inner());
        for mode in [1u8, 2u8] {
            // 10 shares expected per second: the 3 s window is plenty
            let (entry, acc, on_b) = watchdog_run(mode, 10.0, 25);
            assert_eq!(entry, Some(1), "mode {mode}: the miner left the pool that credits nothing");
            assert!(acc >= 3 && on_b >= 3, "mode {mode}: shares are accepted on the backup (acc {acc}, on B {on_b})");
        }
    }

    #[test]
    fn watchdog_no_false_alarm_on_rare_shares_or_a_normal_pool() {
        let _g = GLOBAL.lock().unwrap_or_else(|e| e.into_inner());
        // a silent pool, but only ~0.001 shares expected per second (big difficulty): the window must not fire
        let (entry, _, _) = watchdog_run(2, 0.001, 12);
        assert_eq!(entry, Some(0), "rare shares: no switch");
        // a normal pool: never leaves
        let (entry, acc, _) = watchdog_run(0, 10.0, 10);
        assert_eq!(entry, Some(0));
        assert!(acc >= 20);
    }

    #[test]
    fn failover_then_failback_keeps_shares_and_fee_follows() {
        let _g = GLOBAL.lock().unwrap_or_else(|e| e.into_inner());
        failover::set_test_timing(0.02, 0.4, 100);
        let (pa, pb) = (free_port(), free_port());
        let (a, b) = (Mock::new(pa), Mock::new(pb));
        b.start(); // the primary (a) is down
        let pools = vec![format!("127.0.0.1:{pa}"), format!("127.0.0.1:{pb}")];
        let stop = Arc::new(AtomicBool::new(false));
        let (ev_tx, ev_rx) = mpsc::channel::<(Src, Event)>();
        let (user_sub_tx, user_sub_rx) = mpsc::channel::<Submit>();
        let (_fee_sub_tx, fee_sub_rx) = mpsc::channel::<Submit>();
        let stats = [Arc::new(Stats::default()), Arc::new(Stats::default())];
        let user_link = Link::new(2);
        let fee_enabled = Arc::new(AtomicBool::new(false));
        spawn(login("user", &pools, "UW", user_link.clone(), None, Arc::new(AtomicBool::new(true)), stop.clone()), Src::User, ev_tx.clone(), user_sub_rx, stats[0].clone());
        spawn(login("fee", &pools, "FW", Link::new(2), Some(user_link.clone()), fee_enabled.clone(), stop.clone()), Src::Fee, ev_tx, fee_sub_rx, stats[1].clone());
        {
            let probe_login = login("probe", &pools, "UW", Link::new(1), None, Arc::new(AtomicBool::new(true)), stop.clone());
            let p2 = pools.clone();
            failover::spawn_prober("user", user_link.clone(), stop.clone(), move |e| p2[e].clone(), move |e| probe(&probe_login, e, Duration::from_secs(5)));
        }
        let jobs = Arc::new(AtomicU64::new(0));
        {
            let jobs = jobs.clone();
            std::thread::spawn(move || {
                while let Ok((src, ev)) = ev_rx.recv() {
                    if src == Src::User && matches!(ev, Event::Job(_)) {
                        jobs.fetch_add(1, Ordering::SeqCst);
                    }
                }
            });
        }
        // 1. failover: the primary is down, the backup takes the user
        wait_for("a job from the backup", 20, || jobs.load(Ordering::SeqCst) >= 1);
        assert_eq!(user_link.entry(), Some(1));
        assert_eq!(b.logins(), vec!["UW.rig"]);
        assert!(a.logins().is_empty());
        // shares go on: one per 40 ms from a background thread for the whole test
        let sent = Arc::new(AtomicU64::new(0));
        let run = Arc::new(AtomicBool::new(true));
        {
            let (sent, run) = (sent.clone(), run.clone());
            std::thread::spawn(move || {
                let mut id = 100u64;
                while run.load(Ordering::SeqCst) {
                    id += 1;
                    let line = json!({"jsonrpc":"2.0","id":id,"method":"mining.submit","params":["j1","00"]}).to_string();
                    let _ = user_sub_tx.send(Submit { id, job_id: "j1".into(), line });
                    sent.fetch_add(1, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(40));
                }
            });
        }
        wait_for("shares accepted on the backup", 10, || stats[0].accepted.load(Ordering::SeqCst) >= 3);
        // the fee slot on the backup: the fee login goes to the pool the user is on
        fee_enabled.store(true, Ordering::SeqCst);
        wait_for("the fee login on the backup", 10, || b.logins().iter().any(|l| l == "FW.rig"));
        assert!(a.logins().is_empty());
        fee_enabled.store(false, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(300));
        // 2. the primary comes back: two good checks, the user goes back to it (the drain waits for the answers of the shares in flight)
        a.start();
        wait_for("failback to the primary", 30, || user_link.entry() == Some(0) && a.logins().iter().any(|l| l == "UW.rig"));
        wait_for("shares accepted on the primary", 10, || a.submits.load(Ordering::SeqCst) >= 3);
        assert_eq!(user_link.failbacks.load(Ordering::Relaxed), 1);
        // the fee follows the user back
        let before = b.logins().iter().filter(|l| *l == "FW.rig").count();
        fee_enabled.store(true, Ordering::SeqCst);
        wait_for("the fee login on the primary", 10, || a.logins().iter().any(|l| l == "FW.rig"));
        assert_eq!(b.logins().iter().filter(|l| *l == "FW.rig").count(), before, "the fee must not reconnect to the backup");
        fee_enabled.store(false, Ordering::SeqCst);
        // every share a pool received was answered and counted (the switch did not cut shares in flight)
        run.store(false, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(800));
        let got = a.submits.load(Ordering::SeqCst) + b.submits.load(Ordering::SeqCst);
        let counted = stats[0].accepted.load(Ordering::SeqCst) + stats[0].rejected.load(Ordering::SeqCst);
        assert!(got >= 6 && sent.load(Ordering::SeqCst) >= got);
        assert_eq!(counted, got, "shares received by the pools = shares answered ({got} received: a {} b {}; accepted {} rejected {})", a.submits.load(Ordering::SeqCst), b.submits.load(Ordering::SeqCst), stats[0].accepted.load(Ordering::SeqCst), stats[0].rejected.load(Ordering::SeqCst));
        assert_eq!(stats[0].rejected.load(Ordering::SeqCst), 0);
        stop.store(true, Ordering::SeqCst);
        a.stop();
        b.stop();
    }
}
