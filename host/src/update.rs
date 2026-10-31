// FreeForGemPear — see LICENSE
//! New-version notice (1.7.5, as in ffgs 0.5.4): while mining, 30-90 s after the start and then about every 12 h, one plain
//! GET of github.com/pepsykolya/FreeForGem/releases/latest (User-Agent freeforgem/<version>, nothing else is sent), in its
//! own thread: no effect on mining when GitHub is unreachable (nothing is logged then).  A newer release is reported in the
//! log and written to `update-available` next to the miner (HiveOS h-stats.sh shows it after the version and TH/W); nothing
//! is downloaded or installed.  --no-update-check turns it off, --check-update checks once and exits.
use anyhow::{Result, anyhow, bail};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;

const REPO: &str = "pepsykolya/FreeForGem";
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// this version (test hook: FFG_UPDATE_TEST_VERSION pretends an older one)
fn current() -> String {
    std::env::var("FFG_UPDATE_TEST_VERSION").unwrap_or_else(|_| VERSION.to_string())
}

/// "v1.7.5" / "1.7.5" / "1.7.2beta" -> [1, 7, 5]; empty when not a version
pub(crate) fn parse_ver(s: &str) -> Vec<u32> {
    let s = s.trim().trim_start_matches(['v', 'V']);
    let mut v = Vec::new();
    for part in s.split('.') {
        let d: String = part.chars().take_while(|c| c.is_ascii_digit()).collect();
        if d.is_empty() {
            break;
        }
        v.push(d.parse().unwrap_or(0));
        if d.len() != part.len() {
            break; // "2beta": the number ends the version
        }
    }
    v
}

/// a newer than b (missing parts are 0: 1.8 == 1.8.0)
pub(crate) fn newer(a: &[u32], b: &[u32]) -> bool {
    let n = a.len().max(b.len());
    let pad = |x: &[u32]| (0..n).map(|i| x.get(i).copied().unwrap_or(0)).collect::<Vec<_>>();
    pad(a) > pad(b)
}

fn file() -> std::path::PathBuf {
    crate::cmfd::exe_dir().join("update-available")
}

/// The tag of the latest release, from the redirect of github.com/<repo>/releases/latest (not the REST API: its 60 requests
/// an hour per IP were used up at our farm, where many rigs and tools share one address).
fn latest() -> Result<String> {
    let host = "github.com";
    let roots = rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let cfg = rustls::ClientConfig::builder_with_provider(provider).with_safe_default_protocol_versions()?.with_root_certificates(roots).with_no_client_auth();
    let conn = rustls::ClientConnection::new(Arc::new(cfg), host.try_into()?)?;
    let addr = std::net::ToSocketAddrs::to_socket_addrs(&(host, 443))?.next().ok_or_else(|| anyhow!("{host}: no address"))?;
    let sock = TcpStream::connect_timeout(&addr, Duration::from_secs(10))?;
    sock.set_read_timeout(Some(Duration::from_secs(15)))?;
    let mut tls = rustls::StreamOwned::new(conn, sock);
    write!(tls, "GET /{REPO}/releases/latest HTTP/1.1\r\nHost: {host}\r\nUser-Agent: freeforgem/{VERSION}\r\nConnection: close\r\n\r\n")?;
    // the headers are enough: the page redirects to .../releases/tag/<latest tag>
    let mut r = Vec::new();
    let mut b = [0u8; 4096];
    while !r.windows(4).any(|w| w == b"\r\n\r\n") && r.len() < 65536 {
        match tls.read(&mut b) {
            Ok(0) | Err(_) => break,
            Ok(n) => r.extend_from_slice(&b[..n]),
        }
    }
    let r = String::from_utf8_lossy(&r);
    match r.split_whitespace().nth(1).unwrap_or("") {
        "301" | "302" | "303" | "307" | "308" => {}
        "429" => bail!("GitHub rate limit for this IP address (HTTP 429), try later"),
        "" => bail!("no HTTP answer"),
        s => bail!("HTTP {s} (no release yet?)"),
    }
    let loc = r
        .lines()
        .find_map(|l| l.split_once(':').filter(|(k, _)| k.trim().eq_ignore_ascii_case("location")).map(|(_, v)| v.trim().to_string()))
        .ok_or_else(|| anyhow!("no redirect in the answer"))?;
    let tag: String = loc.rsplit_once("/releases/tag/").map_or("", |x| x.1).chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')).collect();
    if parse_ver(&tag).is_empty() {
        bail!("no version in the answer");
    }
    Ok(tag)
}

/// One check: Some(version) = a newer release; the `update-available` file follows the answer.
fn check() -> Result<Option<String>> {
    let tag = latest()?;
    let v = tag.trim_start_matches(['v', 'V']).to_string();
    if newer(&parse_ver(&tag), &parse_ver(&current())) {
        let _ = std::fs::write(file(), &v);
        Ok(Some(v))
    } else {
        let _ = std::fs::remove_file(file());
        Ok(None)
    }
}

/// While mining: 30-90 s after the start, then every 11-13 h (rigs started together behind one IP do not ask at once;
/// GitHub allows 60 requests an hour without a login).
pub fn start() {
    // a file left by an older version (replaced by this one) must not announce what is installed now
    if let Ok(v) = std::fs::read_to_string(file()) {
        if !newer(&parse_ver(&v), &parse_ver(&current())) {
            let _ = std::fs::remove_file(file());
        }
    }
    let _ = std::thread::Builder::new().name("update".into()).spawn(|| {
        std::thread::sleep(Duration::from_secs(30 + rand::random::<u64>() % 61));
        loop {
            if let Ok(Some(v)) = check() {
                println!(
                    "FreeForGem {v} is available: https://github.com/{REPO}/releases/tag/v{v} (this is {}; nothing is updated automatically, --no-update-check hides this)",
                    current()
                );
            }
            std::thread::sleep(Duration::from_secs(12 * 3600 - 3600 + rand::random::<u64>() % 7201));
        }
    });
}

/// --check-update: once, now (exit code 0 = up to date, 10 = a newer release, 1 = the check failed)
pub fn check_now() -> i32 {
    match check() {
        Ok(None) => {
            println!("FreeForGem {} is up to date", current());
            0
        }
        Ok(Some(v)) => {
            println!("FreeForGem {v} is available: https://github.com/{REPO}/releases/tag/v{v} (this is {})", current());
            10
        }
        Err(e) => {
            println!("update check failed: {e:#}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_ver("v1.7.5"), vec![1, 7, 5]);
        assert_eq!(parse_ver("1.7.2beta"), vec![1, 7, 2]);
        assert_eq!(parse_ver("beta"), Vec::<u32>::new());
        assert!(newer(&parse_ver("v1.7.6"), &parse_ver("1.7.5")));
        assert!(newer(&parse_ver("1.8"), &parse_ver("1.7.9")));
        assert!(!newer(&parse_ver("1.7.5"), &parse_ver("1.7.5")));
        assert!(!newer(&parse_ver("1.7"), &parse_ver("1.7.0")));
        assert!(!newer(&parse_ver("1.7.4"), &parse_ver("1.7.5")));
    }
}
