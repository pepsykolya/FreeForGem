// FreeForGemPear — see LICENSE
//! Pool transport shared by the NEB clients (neb/pool.rs): a pool URL (`stratum+ssl://` / `stratum+tcp://`, `?insecure`) and one
//! connection over plain TCP or TLS (rustls with the Mozilla roots of webpki-roots), with TCP keepalive and a short read tick.
//! (FreeForGem 1.7.8: the transport of the NOID pool client, noid/pool.rs.)
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

// the session loop sends the queued shares once per read timeout: short, so a share waits at most this long (stale risk)
// (FFGN 1.0.13 stale-share fix: 5 ms, was 20; the loop reads what the pool sent before it sends)
const READ_TICK_MS: u64 = 5;

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
/// HOST:PORT = TLS (neb/pool.rs turns a bare HOST:PORT into stratum+tcp:// before it calls this).  `?insecure` turns off the certificate check (self-signed pool certificates).
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

pub(crate) enum Conn {
    Plain(TcpStream),
    Tls(Box<rustls::ClientConnection>, TcpStream),
}

impl Conn {
    /// Plain bytes from the pool; Ok(None) = nothing within the socket's read timeout, Ok(Some(0)) = closed.
    pub(crate) fn recv(&mut self, b: &mut [u8]) -> std::io::Result<Option<usize>> {
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

    pub(crate) fn send(&mut self, b: &[u8]) -> std::io::Result<()> {
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

pub(crate) fn connect(u: &PoolUrl) -> std::io::Result<Conn> {
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
        sock.set_read_timeout(Some(Duration::from_millis(READ_TICK_MS)))?;
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
    sock.set_read_timeout(Some(Duration::from_millis(READ_TICK_MS)))?;
    Ok(Conn::Tls(Box::new(c), sock))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        let u = parse_url("stratum+ssl://pool.example:19601").unwrap();
        assert_eq!((u.host.as_str(), u.port, u.tls, u.insecure), ("pool.example", 19601, true, false));
        assert!(parse_url("pool.example:19601").unwrap().tls);
        assert!(!parse_url("stratum+tcp://neb.suprnova.cc:7804").unwrap().tls);
        assert!(parse_url("stratum+tls://1.2.3.4:5?insecure").unwrap().insecure);
        assert!(parse_url("http://x:1").is_err());
        assert!(parse_url("x").is_err());
    }
}
