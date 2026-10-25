// FreeForGemPear — see LICENSE
//! NOID pool client: `parano1d-stratum-v1` (newline-delimited JSON-RPC) over TLS or plain TCP, auto-reconnect with backoff.
//! One thread per login (user, dev fee); jobs go to the miner through a channel.
//!
//! Protocol (InnovLab / Suprnova NOID pools, checked against a live pool):
//!   -> mining.subscribe [agent]          <- {nonce_bits: 64, protocol: "parano1d-stratum-v1", session_namespace: <8 bytes hex>}
//!   -> mining.authorize [ADDR.WORKER, password]
//!   <- mining.notify [{job_id, pow_fields_hex (16 fields x 16 B LE), nonce_field_index: 10, nonce_prefix_hex, nonce_bits,
//!                      share_target_hex, block_target_hex, work_domain_id, expires_in_seconds, clean, height, ...}]
//!   <- mining.pause [{reason}]           (the current job is void; a new notify follows)
//!   -> mining.submit [job_id, nonce (16 B LE hex: low nonce_bits/8 bytes ours, the rest = nonce_prefix), work_domain_id]
//!   -> mining.hashrate [H/s]
use crate::stratum::{Src, Stats};
use serde_json::{Value, json};
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const PROTOCOL: &str = "parano1d-stratum-v1";
const SILENCE_LIMIT: Duration = Duration::from_secs(300);
const BACKOFF_MIN: Duration = Duration::from_secs(2);
const BACKOFF_MAX: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, PartialEq)]
pub struct PoolUrl {
    pub host: String,
    pub port: u16,
    pub tls: bool,
    pub insecure: bool, // TLS without certificate verification (?insecure in the URL)
}

impl std::fmt::Display for PoolUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}://{}:{}{}", if self.tls { "stratum+ssl" } else { "stratum+tcp" }, self.host, self.port, if self.insecure { "?insecure" } else { "" })
    }
}

/// `stratum+ssl://` / `stratum+tls://` / `ssl://` / `tls://` = TLS; `stratum+tcp://` / `tcp://` / `stratum://` = plain TCP; a bare
/// HOST:PORT = TLS (NOID pools are TLS).  `?insecure` turns off the certificate check (self-signed pool certificates).
pub fn parse_url(s: &str) -> Result<PoolUrl, String> {
    let s = s.trim();
    let (scheme, rest) = match s.split_once("://") {
        Some((a, b)) => (a.to_ascii_lowercase(), b),
        None => (String::new(), s),
    };
    let tls = match scheme.as_str() {
        "" | "stratum+ssl" | "stratum+tls" | "ssl" | "tls" | "stratum2+ssl" => true,
        "stratum+tcp" | "tcp" | "stratum" => false,
        x => return Err(format!("--pool {s}: unknown scheme '{x}://' (stratum+ssl://HOST:PORT or stratum+tcp://HOST:PORT)")),
    };
    let (addr, query) = rest.split_once('?').unwrap_or((rest, ""));
    let addr = addr.trim_end_matches('/');
    let insecure = query.split('&').any(|q| q.eq_ignore_ascii_case("insecure") || q.eq_ignore_ascii_case("insecure=1"));
    let (host, port) = addr.rsplit_once(':').ok_or_else(|| format!("--pool {s}: needs HOST:PORT"))?;
    let port: u16 = port.parse().map_err(|_| format!("--pool {s}: bad port {port:?}"))?;
    let host = host.trim_start_matches('[').trim_end_matches(']').to_string();
    if host.is_empty() {
        return Err(format!("--pool {s}: empty host"));
    }
    Ok(PoolUrl { host, port, tls, insecure })
}

#[derive(Clone, Debug)]
pub struct NJob {
    pub job_id: String,
    pub fields: [u8; 256],
    pub nonce_field: usize,
    pub prefix: Vec<u8>, // high bytes of the 16-byte nonce field (session namespace)
    pub own_bytes: usize, // low bytes of the nonce that are ours (nonce_bits / 8)
    pub share_target: [u8; 32],
    pub block_target: Option<[u8; 32]>,
    pub work_domain_id: String,
    pub height: u64,
    #[allow(dead_code)]
    pub clean: bool,
    pub expires: Option<Instant>,
}

pub enum Event {
    Job(NJob),
    Pause(String),
    Disconnected,
}

pub struct Submit {
    pub id: u64,
    pub job_id: String,
    pub line: String,
}

pub struct Login {
    pub name: &'static str,
    pub url: PoolUrl,
    pub login: String,
    pub password: String,
    pub enabled: Arc<AtomicBool>,
    pub stop: Arc<AtomicBool>,
    /// hashes per second to report (mining.hashrate), set by the miner
    pub hashrate: Arc<AtomicU64>,
}

fn log(l: &Login, s: &str) {
    eprintln!("[noid-pool:{}] {s}", l.name);
}

pub fn spawn(l: Login, src: Src, tx: Sender<(Src, Event)>, rx: Receiver<Submit>, stats: Arc<Stats>) {
    std::thread::Builder::new()
        .name(format!("noid-pool-{}", l.name))
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
                while rx.try_recv().is_ok() {}
                if !l.enabled.load(Ordering::Relaxed) || l.stop.load(Ordering::Relaxed) {
                    continue;
                }
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
        .expect("spawn NOID pool thread");
}

// ---------------- transport ----------------

#[derive(Debug)]
struct NoVerify(Arc<rustls::crypto::CryptoProvider>);

impl rustls::client::danger::ServerCertVerifier for NoVerify {
    fn verify_server_cert(
        &self,
        _e: &rustls::pki_types::CertificateDer<'_>,
        _i: &[rustls::pki_types::CertificateDer<'_>],
        _n: &rustls::pki_types::ServerName<'_>,
        _o: &[u8],
        _t: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        m: &[u8],
        c: &rustls::pki_types::CertificateDer<'_>,
        d: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(m, c, d, &self.0.signature_verification_algorithms)
    }
    fn verify_tls13_signature(
        &self,
        m: &[u8],
        c: &rustls::pki_types::CertificateDer<'_>,
        d: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(m, c, d, &self.0.signature_verification_algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

fn tls_config(insecure: bool) -> Arc<rustls::ClientConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let b = rustls::ClientConfig::builder_with_provider(provider.clone()).with_safe_default_protocol_versions().expect("TLS versions");
    let cfg = if insecure {
        b.dangerous().with_custom_certificate_verifier(Arc::new(NoVerify(provider))).with_no_client_auth()
    } else {
        let roots = rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
        b.with_root_certificates(roots).with_no_client_auth()
    };
    Arc::new(cfg)
}

enum Conn {
    Plain(TcpStream),
    Tls(Box<rustls::ClientConnection>, TcpStream),
}

impl Conn {
    /// Plain bytes from the pool; Ok(None) = nothing within the socket's read timeout, Ok(Some(0)) = closed.
    fn recv(&mut self, b: &mut [u8]) -> std::io::Result<Option<usize>> {
        let quiet = |e: &std::io::Error| matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted);
        match self {
            Conn::Plain(s) => match s.read(b) {
                Ok(n) => Ok(Some(n)),
                Err(e) if quiet(&e) => Ok(None),
                Err(e) => Err(e),
            },
            Conn::Tls(c, s) => {
                // plaintext already decrypted first
                match c.reader().read(b) {
                    Ok(n) => return Ok(Some(n)),
                    Err(e) if e.kind() == ErrorKind::WouldBlock => {}
                    Err(e) => return Err(e),
                }
                match c.read_tls(s) {
                    Ok(0) => return Ok(Some(0)),
                    Ok(_) => {}
                    Err(e) if quiet(&e) => return Ok(None),
                    Err(e) => return Err(e),
                }
                c.process_new_packets().map_err(|e| std::io::Error::other(format!("TLS: {e}")))?;
                while c.wants_write() {
                    c.write_tls(s)?;
                }
                match c.reader().read(b) {
                    Ok(n) => Ok(Some(n)),
                    Err(e) if e.kind() == ErrorKind::WouldBlock => Ok(None),
                    Err(e) => Err(e),
                }
            }
        }
    }

    fn send(&mut self, b: &[u8]) -> std::io::Result<()> {
        match self {
            Conn::Plain(s) => s.write_all(b),
            Conn::Tls(c, s) => {
                c.writer().write_all(b)?;
                while c.wants_write() {
                    c.write_tls(s)?;
                }
                Ok(())
            }
        }
    }
}

fn connect(u: &PoolUrl) -> std::io::Result<Conn> {
    let mut last = None;
    let mut sock = None;
    for a in (u.host.as_str(), u.port).to_socket_addrs()? {
        match TcpStream::connect_timeout(&a, Duration::from_secs(10)) {
            Ok(s) => {
                sock = Some(s);
                break;
            }
            Err(e) => last = Some(e),
        }
    }
    let sock = match sock {
        Some(s) => s,
        None => return Err(last.unwrap_or_else(|| std::io::Error::new(ErrorKind::NotFound, "address does not resolve"))),
    };
    sock.set_nodelay(true)?;
    let ka = socket2::TcpKeepalive::new().with_time(Duration::from_secs(30)).with_interval(Duration::from_secs(10));
    let _ = socket2::SockRef::from(&sock).set_tcp_keepalive(&ka);
    if !u.tls {
        sock.set_read_timeout(Some(Duration::from_millis(100)))?;
        sock.set_write_timeout(Some(Duration::from_secs(20)))?;
        return Ok(Conn::Plain(sock));
    }
    let name = rustls::pki_types::ServerName::try_from(u.host.clone()).map_err(|e| std::io::Error::new(ErrorKind::InvalidInput, format!("TLS name {}: {e}", u.host)))?;
    let mut c = rustls::ClientConnection::new(tls_config(u.insecure), name).map_err(|e| std::io::Error::other(format!("TLS: {e}")))?;
    let mut sock = sock;
    sock.set_read_timeout(Some(Duration::from_secs(15)))?;
    sock.set_write_timeout(Some(Duration::from_secs(20)))?;
    while c.is_handshaking() {
        c.complete_io(&mut sock).map_err(|e| {
            let hint = if e.to_string().to_lowercase().contains("certificate") { " (a self-signed pool certificate? add ?insecure to the pool URL)" } else { "" };
            std::io::Error::new(e.kind(), format!("TLS handshake with {}:{}: {e}{hint}", u.host, u.port))
        })?;
    }
    sock.set_read_timeout(Some(Duration::from_millis(100)))?;
    Ok(Conn::Tls(Box::new(c), sock))
}

// ---------------- session ----------------

fn hexd(s: &str) -> Option<Vec<u8>> {
    crate::stratum::hex_decode(s.trim().trim_start_matches("0x"))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn session(l: &Login, src: Src, tx: &Sender<(Src, Event)>, rx: &Receiver<Submit>, stats: &Stats) -> std::io::Result<()> {
    log(l, &format!("connecting to {} as {}", l.url, l.login));
    let mut c = connect(&l.url)?;
    stats.connected.store(1, Ordering::Relaxed);
    let hello = [
        json!({"id":1,"method":"mining.subscribe","params":[crate::stratum::AGENT]}),
        json!({"id":2,"method":"mining.authorize","params":[l.login, l.password]}),
    ];
    for h in &hello {
        c.send(format!("{h}\n").as_bytes())?;
    }
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 16384];
    let mut pending: std::collections::HashMap<u64, String> = Default::default();
    let mut next_id = 100u64;
    let mut namespace: Option<Vec<u8>> = None;
    let mut nonce_bits = 64u32;
    let mut last_rx = Instant::now();
    let mut last_hr = Instant::now();
    let mut seen = 0;
    loop {
        if l.stop.load(Ordering::Relaxed) || !l.enabled.load(Ordering::Relaxed) {
            return Ok(());
        }
        if last_rx.elapsed() > SILENCE_LIMIT {
            return Err(std::io::Error::new(ErrorKind::TimedOut, format!("no data from the pool for {}s", SILENCE_LIMIT.as_secs())));
        }
        while let Ok(s) = rx.try_recv() {
            pending.insert(s.id, s.job_id);
            c.send(format!("{}\n", s.line).as_bytes())?;
        }
        if last_hr.elapsed() >= Duration::from_secs(30) {
            last_hr = Instant::now();
            let hr = l.hashrate.load(Ordering::Relaxed);
            if hr > 0 {
                next_id += 1;
                c.send(format!("{}\n", json!({"id": next_id, "method": "mining.hashrate", "params": [hr]})).as_bytes())?;
            }
        }
        match c.recv(&mut chunk)? {
            Some(0) => return Ok(()),
            Some(n) => {
                buf.extend_from_slice(&chunk[..n]);
                last_rx = Instant::now();
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
                "mining.notify" => {
                    if seen < 2 {
                        seen += 1;
                        log(l, &format!("RAW JOB: {}", if line.len() > 400 { format!("{}...", &line[..400]) } else { line.to_string() }));
                    }
                    match parse_job(&v, namespace.as_deref(), nonce_bits) {
                        Ok(j) => {
                            let _ = tx.send((src, Event::Job(j)));
                        }
                        Err(e) => log(l, &format!("job not understood ({e}): {line}")),
                    }
                }
                "mining.pause" => {
                    let p = &v["params"];
                    let why = p.get(0).and_then(|x| x.get("reason")).or_else(|| p.get("reason")).and_then(Value::as_str).unwrap_or("?").to_string();
                    let _ = tx.send((src, Event::Pause(why)));
                }
                "mining.set_target" | "mining.set_difficulty" => log(l, &format!("ignored: {line}")),
                "" => {
                    let id = v.get("id").and_then(Value::as_u64).unwrap_or(0);
                    let ok = v.get("result").map(|r| r.as_bool().unwrap_or(!r.is_null())).unwrap_or(false) && v.get("error").map_or(true, Value::is_null);
                    if let Some(job_id) = pending.remove(&id) {
                        if ok {
                            stats.accepted.fetch_add(1, Ordering::Relaxed);
                            log(l, &format!("share ACCEPTED (id {id}, job {job_id})"));
                        } else {
                            stats.rejected.fetch_add(1, Ordering::Relaxed);
                            log(l, &format!("share REJECTED (id {id}, job {job_id}): {line}"));
                        }
                    } else if id == 1 {
                        let r = &v["result"];
                        if !ok || !r.is_object() {
                            return Err(std::io::Error::other(format!("subscribe failed: {line}")));
                        }
                        let proto = r["protocol"].as_str().unwrap_or("?");
                        if proto != PROTOCOL {
                            log(l, &format!("WARNING: pool protocol {proto:?}, this miner speaks {PROTOCOL}"));
                        }
                        nonce_bits = r["nonce_bits"].as_u64().unwrap_or(64) as u32;
                        namespace = r["session_namespace"].as_str().and_then(hexd);
                        log(l, &format!("subscribed: {PROTOCOL}, nonce_bits {nonce_bits}, session namespace {}", r["session_namespace"].as_str().unwrap_or("-")));
                    } else if id == 2 {
                        if !ok {
                            return Err(std::io::Error::new(ErrorKind::PermissionDenied, format!("authorize failed: {line}")));
                        }
                        log(l, "authorized");
                    } else if id > 100 {
                        // hashrate report reply
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

fn parse_job(v: &Value, namespace: Option<&[u8]>, sub_bits: u32) -> Result<NJob, String> {
    let p = v.get("params").ok_or("no params")?;
    let p = if p.is_array() { p.get(0).ok_or("empty params")? } else { p };
    let job_id = match &p["job_id"] {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => return Err("no job_id".into()),
    };
    let f = hexd(p["pow_fields_hex"].as_str().ok_or("no pow_fields_hex")?).ok_or("pow_fields_hex not hex")?;
    if f.len() != 256 {
        return Err(format!("pow_fields_hex has {} bytes, expected 256", f.len()));
    }
    let nonce_field = p["nonce_field_index"].as_u64().unwrap_or(10) as usize;
    if nonce_field != 10 {
        return Err(format!("nonce_field_index {nonce_field} (the GPU kernel needs 10)"));
    }
    let bits = p["nonce_bits"].as_u64().map_or(sub_bits, |b| b as u32);
    if !(40..=128).contains(&bits) || bits % 8 != 0 {
        return Err(format!("nonce_bits {bits} (needs 40..128, whole bytes)"));
    }
    let own = (bits / 8) as usize;
    let prefix = match p["nonce_prefix_hex"].as_str() {
        Some(s) => hexd(s).ok_or("nonce_prefix_hex not hex")?,
        None => namespace.map(<[u8]>::to_vec).unwrap_or_default(),
    };
    if own + prefix.len() != 16 {
        return Err(format!("nonce_bits {bits} + nonce prefix of {} bytes != 16 bytes", prefix.len()));
    }
    let t = hexd(p["share_target_hex"].as_str().ok_or("no share_target_hex")?).ok_or("share_target_hex not hex")?;
    let share_target: [u8; 32] = t.try_into().map_err(|_| "share_target_hex is not 32 bytes")?;
    let block_target = p["block_target_hex"].as_str().and_then(hexd).and_then(|b| b.try_into().ok());
    let work_domain_id = match &p["work_domain_id"] {
        Value::String(s) => s.clone(),
        Value::Null => return Err("no work_domain_id".into()),
        x => x.to_string(),
    };
    let mut fields = [0u8; 256];
    fields.copy_from_slice(&f);
    Ok(NJob {
        job_id,
        fields,
        nonce_field,
        prefix,
        own_bytes: own,
        share_target,
        block_target,
        work_domain_id,
        height: p["height"].as_u64().unwrap_or(0),
        clean: p["clean"].as_bool().unwrap_or(true),
        expires: p["expires_in_seconds"].as_u64().map(|s| Instant::now() + Duration::from_secs(s)),
    })
}

/// The mining.submit line for nonce field bytes `nonce` (16 B LE).
pub fn submit_line(id: u64, j: &NJob, nonce: &[u8; 16]) -> String {
    json!({"id": id, "method": "mining.submit", "params": [j.job_id, hex(nonce), j.work_domain_id]}).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        let u = parse_url("stratum+ssl://eu2.innovlab.cc:19601").unwrap();
        assert_eq!((u.host.as_str(), u.port, u.tls, u.insecure), ("eu2.innovlab.cc", 19601, true, false));
        assert!(parse_url("eu2.innovlab.cc:19601").unwrap().tls);
        assert!(!parse_url("stratum+tcp://noid.suprnova.cc:3340").unwrap().tls);
        assert!(parse_url("stratum+tls://1.2.3.4:5?insecure").unwrap().insecure);
        assert!(parse_url("http://x:1").is_err());
        assert!(parse_url("x").is_err());
    }

    #[test]
    fn job_and_submit() {
        let fields = "00".repeat(256);
        let v: Value = serde_json::from_str(&format!(
            r#"{{"id":null,"method":"mining.notify","params":[{{"job_id":"46f6","pow_fields_hex":"{fields}","nonce_field_index":10,"nonce_prefix_hex":"24e67d0000000200","nonce_bits":64,"share_target_hex":"{}","block_target_hex":"{}","work_domain_id":"e929","expires_in_seconds":262,"clean":true,"height":239348}}]}}"#,
            "ff".repeat(32),
            "00".repeat(32)
        ))
        .unwrap();
        let j = parse_job(&v, None, 64).unwrap();
        assert_eq!((j.own_bytes, j.prefix.len(), j.height), (8, 8, 239348));
        let mut n = [0u8; 16];
        n[..4].copy_from_slice(&0x04808c74u32.to_le_bytes());
        n[8..].copy_from_slice(&j.prefix);
        let l = submit_line(3, &j, &n);
        assert!(l.contains(r#""params":["46f6","748c80040000000024e67d0000000200","e929"]"#), "{l}");
    }
}
