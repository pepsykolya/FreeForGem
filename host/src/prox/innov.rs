// FreeForGem — see LICENSE
//! PROX on the InnovLab pool (prox.innovlab.cc, PPLNS): its own JSON-lines protocol over TLS (port 19701) or TCP, ported from our
//! ffgprox host/pool_innov.h (39/39 shares accepted live there).
//!   -> {"id":1,"method":"login","params":{"agent":"..","user":"<holder id 64 hex>.<worker>","pass":"x"}}
//!   <- {"id":1,"result":{"proto":1,"session":".."}}
//!   <- {"method":"job","params":{"job_id":"..","message":"<hex>","share_k":32,"share_beat":"<52 hex>","k_net":47,"ttl_ms":8172,"clean":true}}
//!   -> {"id":N,"method":"submit","params":{"job_id":"..","nonce":"<16 hex, big-endian>"}}   <- {"id":N,"result":{"accepted":true}}
//!   -> {"id":N,"method":"keepalive","params":{"hashrate":<H/s>}}                               <- {"id":N,"result":"ok"}
//! The message is hashed as message || nonce_be8 (as a solo seeker job); a share needs share_k zero bits and a work key below
//! share_beat (all ff = none).  The whole nonce is ours (random base per job, layout as for solo).
use super::pool::{AGENT, Event, Login, PJob, Submit, log};
use crate::noid::pool::connect;
use crate::stratum::{Src, Stats};
use serde_json::{Value, json};
use std::io::ErrorKind;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

/// the limit until the pool answered a keepalive on this connection; after that a silence of crate::silence::FAST (60 s) ends it
const SILENCE_LIMIT: Duration = Duration::from_secs(180);
const KEEPALIVE: Duration = Duration::from_secs(20);

/// `innov+tls://` / `innov+tcp://` (explicit) or an InnovLab host: the InnovLab protocol; the URL for the transport parser.
pub fn transport_url(s: &str) -> Option<String> {
    let t = s.trim();
    let low = t.to_ascii_lowercase();
    if let Some(r) = low.strip_prefix("innov+tls://").or_else(|| low.strip_prefix("innov+ssl://")).or_else(|| low.strip_prefix("innov://")) {
        return Some(format!("stratum+ssl://{}", &t[t.len() - r.len()..]));
    }
    if let Some(r) = low.strip_prefix("innov+tcp://") {
        return Some(format!("stratum+tcp://{}", &t[t.len() - r.len()..]));
    }
    let host = low.split_once("://").map_or(low.as_str(), |x| x.1);
    host.contains("innovlab").then(|| t.to_string())
}

pub fn session(l: &Login, src: Src, tx: &Sender<(Src, Event)>, rx: &Receiver<Submit>, stats: &Stats) -> std::io::Result<()> {
    log(l, &format!("connecting to {} as {} (InnovLab protocol, agent {AGENT})", l.url, l.login));
    let mut c = connect(&l.url)?;
    stats.connected.store(1, Ordering::Relaxed);
    let hello = json!({"id":1,"method":"login","params":{"agent": AGENT, "user": l.login, "pass": l.password}});
    c.send(format!("{hello}\n").as_bytes())?;
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 16384];
    let mut pending: std::collections::HashMap<u64, (String, Instant, i32)> = Default::default();
    let mut valid: Vec<String> = Vec::new();
    let mut authed = false;
    let mut next_id = 100u64;
    let mut last_ka = Instant::now();
    let mut sil = crate::silence::Silence::new(SILENCE_LIMIT);
    let mut drain = crate::failover::Drain::new();
    loop {
        if l.stop.load(Ordering::Relaxed) || !l.enabled.load(Ordering::Relaxed) {
            return Ok(());
        }
        if let Some(idle) = sil.expired() {
            return Err(crate::silence::Silence::error(idle));
        }
        while let Ok(s) = rx.try_recv() {
            if drain.frozen() {
                stats.submitted.fetch_sub(1, Ordering::Relaxed);
                continue; // a switch to another pool is under way: this share belongs to a job of the old pool
            }
            if !authed || !valid.contains(&s.job_id) {
                stats.submitted.fetch_sub(1, Ordering::Relaxed);
                stats.stale_drop.fetch_add(1, Ordering::Relaxed);
                log(l, &format!("share for job {} dropped, not sent: the pool replaced that job", s.job_id));
                continue;
            }
            let line = json!({"id": s.id, "method": "submit", "params": {"job_id": s.job_id, "nonce": format!("{:016x}", s.nonce)}});
            pending.insert(s.id, (s.job_id, Instant::now(), s.gpu));
            c.send(format!("{line}\n").as_bytes())?;
        }
        // a switch to another pool (failback): the shares found meanwhile went out above, wait for the pool's answers, then end
        if drain.check(&l.link, pending.is_empty()) {
            return Ok(());
        }
        if authed && last_ka.elapsed() >= KEEPALIVE {
            last_ka = Instant::now();
            next_id += 1;
            let ka = json!({"id": next_id + 1_000_000, "method": "keepalive", "params": {"hashrate": l.hashrate.load(Ordering::Relaxed)}});
            sil.probe_sent();
            c.send(format!("{ka}\n").as_bytes())?;
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
            match method {
                "job" => {
                    if !authed {
                        continue;
                    }
                    let p = &v["params"];
                    let job_id = match &p["job_id"] {
                        Value::String(s) => s.clone(),
                        x => x.to_string(),
                    };
                    let msg = p["message"].as_str().and_then(crate::stratum::hex_decode).unwrap_or_default();
                    let k = p["share_k"].as_u64().unwrap_or(0);
                    if job_id.is_empty() || msg.is_empty() || msg.len() > 4000 || !(1..=63).contains(&k) {
                        log(l, &format!("bad job (message {} B, share_k {k}): {}", msg.len(), &line[..line.len().min(200)]));
                        let _ = tx.send((src, Event::Pause("bad job".into())));
                        valid.clear();
                        continue;
                    }
                    let beat = p["share_beat"].as_str().and_then(crate::stratum::hex_decode).filter(|b| b.len() == 26 && b.iter().any(|&x| x != 0xff));
                    let ttl = p["ttl_ms"].as_i64().unwrap_or(0);
                    let clean = p["clean"].as_bool().unwrap_or(true);
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
                        prefix: msg,
                        k: k as u32,
                        en1: 0,
                        pool: false,
                        slot: 0, // InnovLab sends the network difficulty (k_net), no slot
                        clean,
                        expires: (ttl > 0).then(|| Instant::now() + Duration::from_millis(ttl as u64)),
                        beat: beat.map(|b| b.try_into().unwrap()),
                    };
                    let _ = tx.send((src, Event::Job(j)));
                }
                "client.show_message" | "message" => log(l, &format!("pool message: {}", &line[..line.len().min(200)])),
                "" => {
                    let id = v.get("id").and_then(Value::as_u64).unwrap_or(0);
                    let err = &v["error"];
                    let res = &v["result"];
                    if id == 1 {
                        if !err.is_null() || res.is_null() || res.as_bool() == Some(false) {
                            l.refused.fetch_add(1, Ordering::Relaxed);
                            return Err(std::io::Error::new(ErrorKind::PermissionDenied, format!("login failed: {line}")));
                        }
                        authed = true;
                        log(l, &format!("logged in (proto {}, session {})", res["proto"], res["session"].as_str().unwrap_or("-")));
                    } else if id > 1_000_000 {
                        sil.answered(); // the keepalive answer: from now on this pool is judged by the 60 s limit
                    } else if let Some((job_id, sent, gpu)) = pending.remove(&id) {
                        let ms = sent.elapsed().as_millis();
                        if res["accepted"].as_bool() == Some(true) {
                            stats.accepted.fetch_add(1, Ordering::Relaxed);
                            log(l, &format!("share ACCEPTED (id {id}, job {job_id}, GPU{gpu}, {ms} ms)"));
                        } else {
                            stats.rejected.fetch_add(1, Ordering::Relaxed);
                            let why = if err.is_null() { res["reason"].as_str().unwrap_or(line).to_string() } else { err.to_string() };
                            let lw = why.to_ascii_lowercase();
                            if ["stale", "expired", "unknown job", "late"].iter().any(|x| lw.contains(x)) {
                                stats.stale_rej.fetch_add(1, Ordering::Relaxed);
                            } else if lw.contains("difficulty") || lw.contains("low") {
                                stats.low_diff.fetch_add(1, Ordering::Relaxed);
                            }
                            log(l, &format!("share REJECTED (id {id}, job {job_id}, GPU{gpu}, {ms} ms): {why}"));
                        }
                    }
                    // keepalive answers: nothing to do
                }
                _ => log(l, &format!("other: {}", &line[..line.len().min(200)])),
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
    fn urls() {
        assert_eq!(super::transport_url("innov+tls://eu3.innovlab.cc:19701").as_deref(), Some("stratum+ssl://eu3.innovlab.cc:19701"));
        assert_eq!(super::transport_url("innov+tcp://127.0.0.1:19777").as_deref(), Some("stratum+tcp://127.0.0.1:19777"));
        assert_eq!(super::transport_url("stratum+ssl://global.innovlab.cc:19701").as_deref(), Some("stratum+ssl://global.innovlab.cc:19701"));
        assert_eq!(super::transport_url("stratum+ssl://pool.proximamine.com:3334"), None);
    }
}
