// FreeForGem — see LICENSE
//! PROX pool client: the Proxima stratum of suprnova / proximamine (newline-delimited JSON-RPC over TCP or TLS), hash jobs
//! `txid-blake2b` (library upgrade 6).  One thread per login (user, dev fee); jobs go to the miner through a channel.
//!   -> mining.subscribe [agent]            <- [subscriptions, extranonce1 (2 bytes hex), extranonce2_size 6]
//!   -> mining.authorize [ADDR.WORKER, password / token]
//!   <- mining.set_difficulty [2^k]          (applies from the next notify)
//!   <- mining.notify [job_id, "txid-blake2b", prefix_hex, prefix_len, slot, ttl_ms, clean]
//!   <- client.show_message [text]           ("mining paused: no job" = the current job is void until the next notify)
//!   -> mining.submit [login, job_id, extranonce2 (12 hex), hash (64 hex)]   errors: 20 other, 21 stale, 22 duplicate,
//!                                                                           23 low difficulty, 24 unauthorized, 25 not subscribed
//! nonce = extranonce1 (2 bytes) || extranonce2 (6 bytes), big-endian in the message.
//! The user agent is this miner's own name and version (FreeForGem/<version>), never another miner's.
use crate::noid::pool::{Conn, PoolUrl, connect};
use crate::stratum::{Src, Stats};
use serde_json::{Value, json};
use std::io::ErrorKind;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

pub const AGENT: &str = concat!("FreeForGem/", env!("CARGO_PKG_VERSION"));
/// the limit until the pool answered a probe on this connection; after that a silence of crate::silence::FAST (60 s) ends it
const SILENCE_LIMIT: Duration = Duration::from_secs(180);

#[derive(Clone, Debug)]
pub struct PJob {
    pub job_id: String,
    pub prefix: Vec<u8>, // the message without the 8-byte nonce
    pub k: u32,          // zero low bits of h[24..32] a share needs
    pub en1: u16,        // pool: the top 2 nonce bytes (extranonce1)
    pub pool: bool,      // false: solo (seeker), the nonce is free
    pub slot: i64,
    pub clean: bool,
    pub expires: Option<Instant>,
    pub beat: Option<[u8; 26]>, // solo: the best work key the node knew when the job came (a solution must be smaller)
}

pub enum Event {
    Job(PJob),
    Pause(String),
    Disconnected,
}

pub struct Submit {
    pub id: u64,
    pub job_id: String,
    pub nonce: u64,
    pub hash: [u8; 32],
    pub gpu: i32,
}

pub struct Login {
    pub name: &'static str,
    /// the entry in use (`urls[entry]`, set by the connection loop; the first one is the primary)
    pub url: PoolUrl,
    pub urls: Vec<PoolUrl>,
    /// the password of every entry (the dev account's secret differs per pool)
    pub passes: Vec<String>,
    pub link: Arc<crate::failover::Link>,
    /// the fee login: follows the user's entry
    pub follow: Option<Arc<crate::failover::Link>>,
    pub login: String,
    pub password: String,
    pub enabled: Arc<AtomicBool>,
    pub stop: Arc<AtomicBool>,
    /// authorize / login refused by the pool (counted; the dev fee falls back to proximamine on a refused fee login)
    pub refused: Arc<AtomicU64>,
    /// hashes per second to report (InnovLab keepalive), set by the miner
    pub hashrate: Arc<AtomicU64>,
}

pub fn log(l: &Login, s: &str) {
    eprintln!("[prox-pool:{}] {s}", l.name);
}

pub type Session = fn(&Login, Src, &Sender<(Src, Event)>, &Receiver<Submit>, &Stats) -> std::io::Result<()>;

/// The connection loop (failover.rs: the entries of the `--pool` list in turn, back to the primary on request, enable / disable)
/// around one protocol's `session`.
pub fn spawn_with(l: Login, src: Src, tx: Sender<(Src, Event)>, rx: Receiver<Submit>, stats: Arc<Stats>, session: Session) {
    std::thread::Builder::new()
        .name(format!("prox-pool-{}", l.name))
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
                    let urls = l.urls.clone();
                    move |e| format!("{}:{}", urls[e.min(urls.len() - 1)].host, urls[e.min(urls.len() - 1)].port)
                }),
            };
            crate::failover::run(
                lp,
                &mut l,
                |l, e| {
                    l.url = l.urls[e].clone();
                    l.password = l.passes[e].clone();
                },
                |l| session(l, src, &tx, &rx, &stats),
                |_| {
                    stats.connected.store(0, Ordering::Relaxed);
                    let _ = tx.send((src, Event::Disconnected));
                    while rx.try_recv().is_ok() {}
                },
                |_| while rx.try_recv().is_ok() {},
            );
        })
        .expect("spawn PROX pool thread");
}

/// `--pool-check`: the failback check (connect + login + a job) of every entry of the list.
pub fn check(urls: &[PoolUrl], login: &str, password: &str, timeout: Duration, session: Session) -> Vec<Result<(), String>> {
    let l = Login {
        name: "check",
        url: urls[0].clone(),
        urls: urls.to_vec(),
        passes: vec![password.to_string(); urls.len()],
        link: crate::failover::Link::new(1),
        follow: None,
        login: login.to_string(),
        password: password.to_string(),
        enabled: Arc::new(AtomicBool::new(true)),
        stop: Arc::new(AtomicBool::new(false)),
        refused: Arc::new(AtomicU64::new(0)),
        hashrate: Arc::new(AtomicU64::new(0)),
    };
    (0..urls.len()).map(|e| probe(&l, e, timeout, session)).collect()
}

/// One check of entry `entry` (failback): connect + login + a usable job, nothing is submitted (the mining session on a scratch channel).
pub fn probe(l: &Login, entry: usize, timeout: Duration, session: Session) -> Result<(), String> {
    let p = Login {
        name: "probe",
        url: l.urls[entry].clone(),
        urls: l.urls.clone(),
        passes: l.passes.clone(),
        link: crate::failover::Link::new(1),
        follow: None,
        login: l.login.clone(),
        password: l.passes[entry].clone(),
        enabled: Arc::new(AtomicBool::new(true)),
        stop: Arc::new(AtomicBool::new(false)),
        refused: Arc::new(AtomicU64::new(0)),
        hashrate: Arc::new(AtomicU64::new(0)),
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

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// k of a set_difficulty value (a power of two), None otherwise
fn k_of(d: f64) -> Option<u32> {
    (0..64).find(|&i| d == 2f64.powi(i)).map(|i| i as u32)
}

pub fn session_stratum(l: &Login, src: Src, tx: &Sender<(Src, Event)>, rx: &Receiver<Submit>, stats: &Stats) -> std::io::Result<()> {
    log(l, &format!("connecting to {} as {} (agent {AGENT})", l.url, l.login));
    let mut c: Conn = connect(&l.url)?;
    stats.connected.store(1, Ordering::Relaxed);
    for h in [json!({"id":1,"method":"mining.subscribe","params":[AGENT]}), json!({"id":2,"method":"mining.authorize","params":[l.login, l.password]})] {
        c.send(format!("{h}\n").as_bytes())?;
    }
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 16384];
    let mut pending: std::collections::HashMap<u64, (String, Instant, i32)> = Default::default();
    let mut valid: Vec<String> = Vec::new(); // jobs the pool still takes shares for (a clean job / pause voids the older ones)
    let (mut en1, mut authed, mut diff_k): (Option<u16>, bool, Option<u32>) = (None, false, None);
    let mut sil = crate::silence::Silence::new(SILENCE_LIMIT);
    let mut seen = 0;
    let mut drain = crate::failover::Drain::new();
    loop {
        if l.stop.load(Ordering::Relaxed) || !l.enabled.load(Ordering::Relaxed) {
            return Ok(());
        }
        if let Some(idle) = sil.expired() {
            return Err(crate::silence::Silence::error(idle));
        }
        if pending.len() > 256 {
            pending.retain(|_, (_, t, _)| t.elapsed() < Duration::from_secs(120));
        }
        while let Ok(s) = rx.try_recv() {
            if drain.frozen() {
                stats.submitted.fetch_sub(1, Ordering::Relaxed);
                continue; // a switch to another pool is under way: this share belongs to a job of the old pool
            }
            if !authed || !valid.contains(&s.job_id) {
                stats.submitted.fetch_sub(1, Ordering::Relaxed);
                stats.stale_drop.fetch_add(1, Ordering::Relaxed);
                log(l, &format!("share for job {} dropped, not sent: the pool voided that job (new clean job / pause)", s.job_id));
                continue;
            }
            let en2 = format!("{:012x}", s.nonce & 0xffff_ffff_ffff);
            let line = json!({"id": s.id, "method": "mining.submit", "params": [l.login, s.job_id, en2, hex(&s.hash)]});
            pending.insert(s.id, (s.job_id, Instant::now(), s.gpu));
            c.send(format!("{line}\n").as_bytes())?;
        }
        // a switch to another pool (failback): the shares found meanwhile went out above, wait for the pool's answers, then end
        if drain.check(&l.link, pending.is_empty()) {
            return Ok(());
        }
        // no keepalive in this protocol: after 20 s of silence ask something the pool answers (an unknown method gets an error)
        if sil.probe_due() {
            sil.synthetic_sent();
            c.send(crate::silence::Silence::probe_line().as_bytes())?;
        }
        match c.recv(&mut chunk)? {
            Some(0) => return Ok(()),
            Some(n) => {
                buf.extend_from_slice(&chunk[..n]);
                sil.rx();
            }
            None => {}
        }
        while let Some(p) = buf.iter().position(|&b| b == b'\n') {
            let raw: Vec<u8> = buf.drain(..=p).collect();
            let line = String::from_utf8_lossy(&raw);
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(line) else {
                log(l, &format!("unparsable: {line}"));
                continue;
            };
            let method = v.get("method").and_then(Value::as_str).unwrap_or("");
            let p = &v["params"];
            match method {
                "mining.set_difficulty" => match p.get(0).and_then(Value::as_f64).and_then(k_of) {
                    Some(k) => {
                        if diff_k != Some(k) {
                            log(l, &format!("difficulty k={k} (2^{k})"));
                        }
                        diff_k = Some(k);
                    }
                    None => log(l, &format!("difficulty is not a power of two, ignored: {line}")),
                },
                "mining.notify" => {
                    if seen < 1 {
                        seen += 1;
                        log(l, &format!("RAW JOB: {}", if line.len() > 300 { format!("{}...", &line[..300]) } else { line.to_string() }));
                    }
                    let kind = p.get(1).and_then(Value::as_str).unwrap_or("");
                    if kind != "txid-blake2b" {
                        log(l, &format!("job type {kind:?} is not supported (only txid-blake2b hash jobs since slot 196000): waiting for a hash job"));
                        valid.clear();
                        let _ = tx.send((src, Event::Pause(format!("unsupported job type {kind:?}"))));
                        continue;
                    }
                    let (Some(e1), Some(k)) = (en1, diff_k) else {
                        log(l, "job before subscribe / difficulty, ignored");
                        continue;
                    };
                    let job_id = p.get(0).map(|x| x.as_str().map(String::from).unwrap_or_else(|| x.to_string())).unwrap_or_default();
                    let pre = p.get(2).and_then(Value::as_str).and_then(|s| crate::stratum::hex_decode(s.trim()));
                    let plen = p.get(3).and_then(Value::as_u64).unwrap_or(0) as usize;
                    let Some(prefix) = pre.filter(|x| !x.is_empty() && x.len() == plen && x.len() <= 4000) else {
                        log(l, &format!("bad hash job (prefix / prefix_len): {}", &line[..line.len().min(200)]));
                        continue;
                    };
                    let ttl = p.get(5).and_then(Value::as_i64).unwrap_or(0);
                    let clean = p.get(6).and_then(Value::as_bool).unwrap_or(true);
                    if clean {
                        valid.clear();
                    }
                    valid.retain(|x| x != &job_id);
                    valid.push(job_id.clone());
                    if valid.len() > 8 {
                        valid.remove(0);
                    }
                    let j = PJob {
                        job_id,
                        prefix,
                        k,
                        en1: e1,
                        pool: true,
                        slot: p.get(4).and_then(Value::as_i64).unwrap_or(0),
                        clean,
                        expires: (ttl > 0).then(|| Instant::now() + Duration::from_millis(ttl as u64)),
                        beat: None,
                    };
                    let _ = tx.send((src, Event::Job(j)));
                }
                "client.show_message" => {
                    let m = p.get(0).and_then(Value::as_str).unwrap_or("").to_string();
                    log(l, &format!("pool message: {m}"));
                    if m.to_ascii_lowercase().contains("paused") {
                        valid.clear();
                        let _ = tx.send((src, Event::Pause(m)));
                    }
                }
                "" => {
                    let id = v.get("id").and_then(Value::as_u64).unwrap_or(0);
                    let err = &v["error"];
                    let ok = v.get("result").and_then(Value::as_bool).unwrap_or(false) && err.is_null();
                    if id == crate::silence::PROBE_ID {
                        sil.answered(); // any answer, an error too
                    } else if id == 1 {
                        let r = &v["result"];
                        let e1 = r.get(1).and_then(Value::as_str).and_then(crate::stratum::hex_decode);
                        let sz = r.get(2).and_then(Value::as_u64);
                        match (e1, sz) {
                            (Some(e), Some(6)) if e.len() == 2 && err.is_null() => {
                                en1 = Some(u16::from_be_bytes([e[0], e[1]]));
                                log(l, &format!("subscribed, extranonce1 {}", hex(&e)));
                            }
                            _ => return Err(std::io::Error::other(format!("subscribe failed or unsupported extranonce layout (need 2 + 6 bytes): {line}"))),
                        }
                    } else if id == 2 {
                        if !ok {
                            l.refused.fetch_add(1, Ordering::Relaxed);
                            return Err(std::io::Error::new(ErrorKind::PermissionDenied, format!("authorize failed: {line}")));
                        }
                        authed = true;
                        log(l, "authorized");
                    } else if let Some((job_id, sent, gpu)) = pending.remove(&id) {
                        let ms = sent.elapsed().as_millis();
                        if ok {
                            stats.accepted.fetch_add(1, Ordering::Relaxed);
                            log(l, &format!("share ACCEPTED (id {id}, job {job_id}, GPU{gpu}, {ms} ms)"));
                        } else {
                            stats.rejected.fetch_add(1, Ordering::Relaxed);
                            let code = err.get(0).and_then(Value::as_i64).unwrap_or(0);
                            if code == 21 {
                                stats.stale_rej.fetch_add(1, Ordering::Relaxed);
                            } else if code == 23 {
                                stats.low_diff.fetch_add(1, Ordering::Relaxed);
                            }
                            log(l, &format!("share REJECTED (id {id}, job {job_id}, GPU{gpu}, code {code}, {ms} ms): {line}"));
                        }
                    } else {
                        log(l, &format!("reply: {line}"));
                    }
                }
                _ => log(l, &format!("other: {line}")),
            }
        }
        if buf.len() > 1 << 20 {
            return Err(std::io::Error::other("line longer than 1 MB from the pool"));
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn k_of_powers() {
        assert_eq!(super::k_of(2f64.powi(31)), Some(31));
        assert_eq!(super::k_of(1.0), Some(0));
        assert_eq!(super::k_of(3.0), None);
    }
}
