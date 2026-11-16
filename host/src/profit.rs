// FreeForGem — see LICENSE
//! `--auto-lock-goal profit` (1.7.8): the money side of the auto-lock choice.  Nothing here runs without that goal.
//!
//! Profit of a measured lock = income − electricity, US$ per day:
//!   income      = hash rate [H/s] × coins per (H/s) per day × coin price × (1 − 1 % miner fee) × (1 − pool fee)
//!   electricity = NVML watts of the GPU at that lock / 1000 × 24 h × the tariff averaged over the 24 hours (--power-price)
//!
//! Coins per (H/s) per day, from the network (the unit of the miner's own hash rate in every case):
//!   Pearl  86400 × block reward / work per block.  Work per block = 2^256 / network target (nbits of the job header, bytes
//!          72..76: the node's own CalcWork, in the unit of the pool's and the miner's TH/s).  Block reward from the block
//!          height (node: CalcBlockSubsidy = 2.1e9 PRL × E / ((h + E) × (h − 1 + E)), E = 4 years / 194 s = 650226).
//!          Without a height in the job (pools other than LuckyPool): LuckyPool's public api/stats (network.difficulty × 2^16 =
//!          the same work per block, checked 15.11.2026: 2.5781e17 × 65536 = 1.6896e22 against 2^256/target 1.6901e22).
//!   NOID   86400 × miner reward / work per block, work = 2^256 / block_target of the job (little-endian), miner reward of the
//!          v2 schedule (block 210537+: 16 NOID gross the first 1 051 200 blocks, then 11.3, 8, ...; 10 % development
//!          allocation the first three years -> 14.4 NOID to the miner).  Fees of the block's transactions are left out.
//!   PROX   86400 × slot reward / network attempts per slot (one winner per 10.24 s slot: k_net in the job says nothing about
//!          the competition).  From InnovLab's public stats API (network.attempts_per_slot, reward_motes; 1 PROX = 10^6 motes).
//!   CMFD   no --auto-lock (so no profit choice).
//!
//! Coin price: the median of several public tickers (GET only, User-Agent FreeForGem/<version>, nothing of the user is sent),
//! outliers more than OUTLIER_PCT from the median left out, tickers with less than THIN_USD of 24 h volume only used when there
//! is nothing else.  --coin-price X instead: no request to any exchange.
use serde_json::Value;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const UA: &str = concat!("FreeForGem/", env!("CARGO_PKG_VERSION"));
/// the developer fee of the miner (devfee.rs): 1 % of the time mines for the developer
pub const MINER_FEE_PCT: f64 = 1.0;
/// a ticker further than this from the median of all tickers is left out
pub const OUTLIER_PCT: f64 = 15.0;
/// a ticker with less 24 h volume (US$) is "thin": used only when no other ticker answers
pub const THIN_USD: f64 = 1000.0;
const TIMEOUT: Duration = Duration::from_secs(10);
const MAX_BODY: usize = 4 << 20;

// ---------------------------------------------------------------------------------------------------------------------
// coins
// ---------------------------------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Coin {
    Pearl,
    Noid,
    Prox,
    Cmfd,
}

impl Coin {
    /// the coin part of an autolock key / the coin name the coin's hook passes ("pearl", "noid-eff", "prox")
    pub fn of(name: &str) -> Option<Coin> {
        let n = name.to_ascii_lowercase();
        if n.starts_with("pearl") || n == "prl" {
            Some(Coin::Pearl)
        } else if n.starts_with("noid") {
            Some(Coin::Noid)
        } else if n.starts_with("prox") {
            Some(Coin::Prox)
        } else if n.starts_with("cmfd") {
            Some(Coin::Cmfd)
        } else {
            None
        }
    }
    pub fn sym(self) -> &'static str {
        match self {
            Coin::Pearl => "PRL",
            Coin::Noid => "NOID",
            Coin::Prox => "PROX",
            Coin::Cmfd => "CMFD",
        }
    }
}

/// H/s per unit of a stats line
pub fn unit_mult(unit: &str) -> f64 {
    match unit {
        "TH/s" => 1e12,
        "GH/s" => 1e9,
        "MH/s" => 1e6,
        "kH/s" | "KH/s" => 1e3,
        _ => 1.0,
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// the tariff: --power-price 0.12 | 0.12,0.053@23-07 (day price, then price@FROM-TO hours; averaged over the 24 hours)
// ---------------------------------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct Tariff {
    pub hours: [f64; 24],
    pub text: String,
}

impl Tariff {
    pub fn parse(s: &str) -> Result<Tariff, String> {
        let bad = |why: &str| format!("--power-price: '{s}' {why} (price per kWh: 0.12, or day,night@FROM-TO: 0.12,0.053@23-07)");
        let num = |x: &str| x.trim().parse::<f64>().ok().filter(|v| v.is_finite() && (0.0..=10.0).contains(v));
        let mut parts = s.split(',').map(str::trim).filter(|x| !x.is_empty());
        let base = parts.next().ok_or_else(|| bad("is empty"))?;
        if base.contains('@') {
            return Err(bad("starts with an hour range: the first value is the price of every other hour"));
        }
        let base = num(base).ok_or_else(|| bad("is not a price 0..10"))?;
        let mut hours = [base; 24];
        for p in parts {
            let (v, range) = p.split_once('@').ok_or_else(|| bad("has a second price without @FROM-TO"))?;
            let v = num(v).ok_or_else(|| bad("has a price that is not 0..10"))?;
            let (a, b) = range.split_once('-').ok_or_else(|| bad("has an hour range that is not FROM-TO"))?;
            let h = |x: &str| x.trim().trim_end_matches(":00").parse::<u32>().ok().filter(|h| *h <= 24);
            let (a, b) = (h(a).ok_or_else(|| bad("has an hour outside 0..24"))?, h(b).ok_or_else(|| bad("has an hour outside 0..24"))?);
            if a % 24 == b % 24 {
                return Err(bad("has an empty hour range"));
            }
            let mut x = a % 24;
            while x != b % 24 {
                hours[x as usize] = v;
                x = (x + 1) % 24;
            }
        }
        Ok(Tariff { hours, text: s.trim().to_string() })
    }

    /// $/kWh for a load that runs around the clock
    pub fn avg(&self) -> f64 {
        self.hours.iter().sum::<f64>() / 24.0
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// HTTP GET (http:// for the tests, https:// with the bundled roots); nothing but the request line, Host and User-Agent is sent
// ---------------------------------------------------------------------------------------------------------------------

trait Rw: Read + Write {}
impl<T: Read + Write> Rw for T {}

pub fn http_get(url: &str, timeout: Duration) -> Result<String, String> {
    let deadline = Instant::now() + timeout;
    let (tls, rest) = if let Some(r) = url.strip_prefix("https://") {
        (true, r)
    } else if let Some(r) = url.strip_prefix("http://") {
        (false, r)
    } else {
        return Err("not an http(s) URL".into());
    };
    let (hostport, path) = rest.split_once('/').map_or((rest, "/".to_string()), |(h, p)| (h, format!("/{p}")));
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (h, p.parse::<u16>().map_err(|_| "bad port")?),
        None => (hostport, if tls { 443 } else { 80 }),
    };
    let addrs: Vec<_> = (host, port).to_socket_addrs().map_err(|e| format!("DNS: {e}"))?.collect();
    let mut sock = None;
    let mut last = String::from("no address");
    for a in addrs {
        match TcpStream::connect_timeout(&a, Duration::from_secs(5)) {
            Ok(s) => {
                sock = Some(s);
                break;
            }
            Err(e) => last = format!("connect: {e}"),
        }
    }
    let sock = sock.ok_or(last)?;
    let left = deadline.saturating_duration_since(Instant::now()).max(Duration::from_millis(500));
    let _ = sock.set_read_timeout(Some(left));
    let _ = sock.set_write_timeout(Some(left));
    let mut s: Box<dyn Rw> = if tls {
        let roots = rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let cfg = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|e| e.to_string())?
            .with_root_certificates(roots)
            .with_no_client_auth();
        let name = rustls::pki_types::ServerName::try_from(host.to_string()).map_err(|e| e.to_string())?;
        let conn = rustls::ClientConnection::new(Arc::new(cfg), name).map_err(|e| e.to_string())?;
        Box::new(rustls::StreamOwned::new(conn, sock))
    } else {
        Box::new(sock)
    };
    let req = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: {UA}\r\nAccept: application/json\r\nConnection: close\r\n\r\n");
    s.write_all(req.as_bytes()).and_then(|_| s.flush()).map_err(|e| format!("send: {e}"))?;
    let mut r = Vec::new();
    let mut b = [0u8; 16384];
    loop {
        if Instant::now() > deadline {
            return Err("timeout".into());
        }
        match s.read(&mut b) {
            Ok(0) => break,
            Ok(n) => {
                r.extend_from_slice(&b[..n]);
                if r.len() > MAX_BODY {
                    return Err("answer larger than 4 MB".into());
                }
            }
            // a TLS peer that closes without close_notify, a timeout after the whole body: what was read is checked below
            Err(e) if !r.is_empty() && matches!(e.kind(), std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted) => break,
            Err(e) if r.is_empty() => return Err(format!("read: {e}")),
            Err(_) => break,
        }
    }
    parse_http(&r)
}

/// status line, headers, body (Content-Length or chunked): the body of a 200 answer
pub fn parse_http(r: &[u8]) -> Result<String, String> {
    let he = r.windows(4).position(|w| w == b"\r\n\r\n").ok_or("no HTTP answer")?;
    let head = String::from_utf8_lossy(&r[..he]).to_string();
    let body = &r[he + 4..];
    let status = head.split_whitespace().nth(1).unwrap_or("");
    if status != "200" {
        return Err(format!("HTTP {}", if status.is_empty() { "?" } else { status }));
    }
    let hv = |k: &str| head.lines().find_map(|l| l.split_once(':').filter(|(a, _)| a.trim().eq_ignore_ascii_case(k)).map(|(_, v)| v.trim().to_ascii_lowercase()));
    if hv("content-encoding").is_some_and(|e| e != "identity") {
        return Err("compressed answer".into());
    }
    let out = if hv("transfer-encoding").is_some_and(|t| t.contains("chunked")) {
        let mut out = Vec::new();
        let mut i = 0;
        loop {
            let le = body[i..].windows(2).position(|w| w == b"\r\n").ok_or("chunked answer cut short")? + i;
            let sz = usize::from_str_radix(String::from_utf8_lossy(&body[i..le]).split(';').next().unwrap_or("").trim(), 16).map_err(|_| "bad chunk size")?;
            if sz == 0 {
                break;
            }
            let st = le + 2;
            if st + sz > body.len() {
                return Err("chunked answer cut short".into());
            }
            out.extend_from_slice(&body[st..st + sz]);
            i = st + sz + 2;
            if i > body.len() {
                return Err("chunked answer cut short".into());
            }
        }
        out
    } else if let Some(n) = hv("content-length").and_then(|n| n.parse::<usize>().ok()) {
        if body.len() < n {
            return Err("answer cut short".into());
        }
        body[..n].to_vec()
    } else {
        body.to_vec()
    };
    Ok(String::from_utf8_lossy(&out).to_string())
}

// ---------------------------------------------------------------------------------------------------------------------
// price sources: one line per public ticker (easy to update; checked 15.11.2026)
// ---------------------------------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    /// SafeTrade (Peatio): ticker.last, ticker.volume (quote, USDT)
    Peatio,
    /// Gate: [0].last, [0].quote_volume (USDT)
    Gate,
    /// LBank: data[0].ticker.latest, .turnover (USDT)
    LBank,
    /// Coinbase Exchange: price, volume (base)
    Coinbase,
    /// Kraken: result.<pair>.c[0], v[1] (base, 24 h)
    Kraken,
    /// BigONE: data.close, data.volume (base)
    BigOne,
    /// CoinGecko simple/price: <id>.usd, <id>.usd_24h_vol (an aggregate of the exchanges)
    CoinGecko,
    /// CoinPaprika tickers: quotes.USD.price, .volume_24h (an aggregate)
    Paprika,
    /// LuckyPool api/stats: markets.price (SafeTrade's last), markets.exchanges[].quoteVolume
    LuckyPool,
}

pub struct Source {
    pub coin: Coin,
    pub name: &'static str,
    pub url: &'static str,
    pub kind: Kind,
    /// the key of the answer that holds this coin (CoinGecko id, Kraken pair); empty = not needed
    pub key: &'static str,
}

const fn src(coin: Coin, name: &'static str, url: &'static str, kind: Kind, key: &'static str) -> Source {
    Source { coin, name, url, kind, key }
}

/// Public tickers per coin, USD / USDT / USDC (taken as US$).  NOID: one exchange, the aggregators repeat it; PROX: one
/// exchange only (a warning when that is all); CMFD: no market known.
pub const SOURCES: &[Source] = &[
    src(Coin::Pearl, "SafeTrade PEARL/USDT", "https://safetrade.com/api/v2/peatio/public/markets/pearlusdt/tickers", Kind::Peatio, ""),
    src(Coin::Pearl, "Gate PEARL/USDT", "https://api.gateio.ws/api/v4/spot/tickers?currency_pair=PEARL_USDT", Kind::Gate, ""),
    src(Coin::Pearl, "LBank PEARL/USDT", "https://api.lbkex.com/v2/ticker/24hr.do?symbol=pearl_usdt", Kind::LBank, ""),
    src(Coin::Pearl, "BigONE PEARL/USDT", "https://big.one/api/v3/asset_pairs/PEARL-USDT/ticker", Kind::BigOne, ""),
    src(Coin::Pearl, "Coinbase PEARL/USD", "https://api.exchange.coinbase.com/products/PEARL-USD/ticker", Kind::Coinbase, ""),
    src(Coin::Pearl, "Kraken PEARL/USD", "https://api.kraken.com/0/public/Ticker?pair=PEARLUSD", Kind::Kraken, "PEARLUSD"),
    src(Coin::Pearl, "CoinGecko", "https://api.coingecko.com/api/v3/simple/price?ids=pearl-2&vs_currencies=usd&include_24hr_vol=true", Kind::CoinGecko, "pearl-2"),
    src(Coin::Pearl, "CoinPaprika", "https://api.coinpaprika.com/v1/tickers/prl-pearl-1", Kind::Paprika, ""),
    src(Coin::Pearl, "LuckyPool", "https://pearl.luckypool.io/api/stats", Kind::LuckyPool, ""),
    src(Coin::Prox, "SafeTrade PROX/USDT", "https://safetrade.com/api/v2/peatio/public/markets/proxusdt/tickers", Kind::Peatio, ""),
    src(Coin::Noid, "SafeTrade NOID/USDT", "https://safetrade.com/api/v2/peatio/public/markets/noidusdt/tickers", Kind::Peatio, ""),
    src(Coin::Noid, "CoinGecko", "https://api.coingecko.com/api/v3/simple/price?ids=parano1d&vs_currencies=usd&include_24hr_vol=true", Kind::CoinGecko, "parano1d"),
    src(Coin::Noid, "CoinPaprika", "https://api.coinpaprika.com/v1/tickers/noid-parano1d", Kind::Paprika, ""),
];

fn f(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
    .filter(|x| x.is_finite())
}

/// (price, 24 h volume in US$ if the ticker gives one) of one ticker's answer
pub fn parse_price(kind: Kind, key: &str, body: &str) -> Result<(f64, Option<f64>), String> {
    let v: Value = serde_json::from_str(body).map_err(|_| "not JSON".to_string())?;
    let (p, vol) = match kind {
        Kind::Peatio => (f(&v["ticker"]["last"]), f(&v["ticker"]["volume"])),
        Kind::Gate => (f(&v[0]["last"]), f(&v[0]["quote_volume"])),
        Kind::LBank => (f(&v["data"][0]["ticker"]["latest"]), f(&v["data"][0]["ticker"]["turnover"])),
        Kind::Coinbase => {
            let p = f(&v["price"]);
            (p, f(&v["volume"]).zip(p).map(|(a, b)| a * b))
        }
        Kind::Kraken => {
            let r = if key.is_empty() { v["result"].as_object().and_then(|o| o.values().next()).cloned().unwrap_or(Value::Null) } else { v["result"][key].clone() };
            let p = f(&r["c"][0]);
            (p, f(&r["v"][1]).zip(f(&r["p"][1]).or(p)).map(|(a, b)| a * b))
        }
        Kind::BigOne => {
            let p = f(&v["data"]["close"]);
            (p, f(&v["data"]["volume"]).zip(p).map(|(a, b)| a * b))
        }
        Kind::CoinGecko => (f(&v[key]["usd"]), f(&v[key]["usd_24h_vol"])),
        Kind::Paprika => (f(&v["quotes"]["USD"]["price"]), f(&v["quotes"]["USD"]["volume_24h"])),
        Kind::LuckyPool => {
            let m = &v["markets"];
            let vol = m["exchanges"].as_array().map(|a| a.iter().filter_map(|e| f(&e["quoteVolume"])).sum::<f64>()).filter(|x| *x > 0.0);
            (f(&m["price"]), vol)
        }
    };
    match p {
        Some(p) if p > 0.0 => Ok((p, vol.filter(|x| *x >= 0.0))),
        _ => Err("no price in the answer".into()),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Quote {
    pub name: String,
    pub price: f64,
    pub vol: Option<f64>,
    pub err: Option<String>,
    pub thin: bool,
    pub outlier: bool,
    pub used: bool,
}

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = v.len();
    if n == 0 {
        0.0
    } else if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

/// The price of a set of quotes: the thick tickers (the thin ones only when there is no thick one), the median of them, the
/// ones further than `dev_pct` from it left out, the median of the rest.  None = no ticker answered.
pub fn aggregate(qs: &mut [Quote], dev_pct: f64) -> Option<f64> {
    for q in qs.iter_mut() {
        q.thin = q.err.is_none() && q.vol.is_some_and(|v| v < THIN_USD);
        q.outlier = false;
        q.used = false;
    }
    let ok = |q: &Quote| q.err.is_none() && q.price > 0.0;
    let thick = qs.iter().any(|q| ok(q) && !q.thin);
    let pick = |q: &Quote| ok(q) && (!thick || !q.thin);
    let mut v: Vec<f64> = qs.iter().filter(|q| pick(q)).map(|q| q.price).collect();
    if v.is_empty() {
        return None;
    }
    let m = median(&mut v);
    for q in qs.iter_mut().filter(|q| pick(q)) {
        q.outlier = (q.price - m).abs() > m * dev_pct / 100.0;
        q.used = !q.outlier;
    }
    let mut v: Vec<f64> = qs.iter().filter(|q| q.used).map(|q| q.price).collect();
    Some(median(&mut v))
}

/// All tickers of `coin` at once (one thread each, TIMEOUT each).
pub fn fetch_quotes(srcs: &[&Source]) -> Vec<Quote> {
    let hs: Vec<_> = srcs
        .iter()
        .map(|s| {
            let (url, kind, key) = (s.url.to_string(), s.kind, s.key.to_string());
            std::thread::spawn(move || http_get(&url, TIMEOUT).and_then(|b| parse_price(kind, &key, &b)))
        })
        .collect();
    srcs.iter()
        .zip(hs)
        .map(|(s, h)| {
            let r = h.join().unwrap_or_else(|_| Err("thread failed".into()));
            match r {
                Ok((p, vol)) => Quote { name: s.name.into(), price: p, vol, err: None, thin: false, outlier: false, used: false },
                Err(e) => Quote { name: s.name.into(), price: 0.0, vol: None, err: Some(e), thin: false, outlier: false, used: false },
            }
        })
        .collect()
}

pub fn sources_of(coin: Coin) -> Vec<&'static Source> {
    SOURCES.iter().filter(|s| s.coin == coin).collect()
}

#[derive(Clone, Debug, PartialEq)]
pub struct Price {
    pub usd: f64,
    pub used: usize,
    pub lines: Vec<String>,
    pub warn: Option<String>,
}

/// The price of `coin` from its tickers, with one log line per ticker.  Err = none answered (or no market known).
pub fn price_from(coin: Coin, mut qs: Vec<Quote>) -> Result<Price, String> {
    if qs.is_empty() {
        return Err(format!("no public market of {} is known: profit needs --coin-price", coin.sym()));
    }
    let p = aggregate(&mut qs, OUTLIER_PCT);
    let mut lines = Vec::new();
    for q in &qs {
        let vol = q.vol.map_or(String::new(), |v| format!("  24 h volume {v:.0} $"));
        lines.push(match &q.err {
            Some(e) => format!("  {:<22} -          ({e})", q.name),
            None => format!(
                "  {:<22} {:<10.6}{vol}{}",
                q.name,
                q.price,
                if q.outlier {
                    format!("  OUTLIER (> {OUTLIER_PCT}% from the median): left out")
                } else if q.thin && !q.used {
                    "  THIN volume: left out".to_string()
                } else if q.thin {
                    "  THIN volume (nothing better)".to_string()
                } else {
                    String::new()
                }
            ),
        });
    }
    let Some(usd) = p else {
        return Err(format!("no price of {}: none of its {} ticker(s) answered (--coin-price X sets it by hand)", coin.sym(), qs.len()));
    };
    let used = qs.iter().filter(|q| q.used).count();
    let warn = if used == 1 {
        Some(format!("the price of {} rests on ONE ticker only ({}): --coin-price X if it looks wrong", coin.sym(), qs.iter().find(|q| q.used).map_or("", |q| &q.name)))
    } else if qs.iter().any(|q| q.used && q.thin) {
        Some(format!("the price of {} comes from thin tickers only", coin.sym()))
    } else {
        None
    };
    Ok(Price { usd, used, lines, warn })
}

// ---------------------------------------------------------------------------------------------------------------------
// the network: coins per (H/s) per day
// ---------------------------------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct Net {
    /// coins per (H/s of the miner's own unit) per day
    pub per_hs_day: f64,
    pub src: String,
    /// pool fee from the same answer (%), if it says
    pub pool_fee: Option<f64>,
}

/// Pearl block subsidy in PRL (node: blockchain/validate.go CalcBlockSubsidy, mainnet TargetTimePerBlock 194 s).
pub fn pearl_reward(height: u64) -> f64 {
    if height == 0 {
        return 0.0;
    }
    const GRAIN: u128 = 100_000_000;
    let e: u128 = (4 * 365 * 24 * 60 * 60) / 194;
    let h = height as u128;
    let sub = 2_100_000_000u128 * GRAIN * e / ((h + e) * (h - 1 + e));
    sub as f64 / GRAIN as f64
}

/// NOID reward to the miner in NOID (noid_chain consensus/emission.rs + development_allocation.rs, mainnet v2 at 210537,
/// 30 s blocks).  None before v2 (that schedule depends on the state size).
pub fn noid_miner_reward(height: u64) -> Option<f64> {
    const V2: u64 = 210_537;
    const REWARDS: [u64; 9] = [16_000_000, 11_300_000, 8_000_000, 5_650_000, 4_000_000, 2_830_000, 2_000_000, 1_410_000, 1_000_000];
    if height < V2 {
        return None;
    }
    let gross = REWARDS[((height - V2) / 1_051_200).min(8) as usize];
    // development allocation: 20 s blocks before v2 count against the three years, the rest at 30 s
    let legacy = V2 - 1;
    let end = legacy + (86_400 * 365 * 3 - legacy * 20) / 30;
    let miner = if height <= end { gross - 2 * (gross / 20) } else { gross };
    Some(miner as f64 / 1e6)
}

/// 2^256 / target, the target as a little-endian 256-bit number
pub fn work_le(t: &[u8; 32]) -> f64 {
    let x: f64 = t.iter().rev().fold(0.0, |a, &b| a * 256.0 + b as f64);
    if x > 0.0 { 2f64.powi(256) / x } else { 0.0 }
}

/// 2^256 / target of a compact nbits (Bitcoin form)
pub fn work_nbits(nbits: u32) -> f64 {
    let e = (nbits >> 24) as i32;
    let m = (nbits & 0x007f_ffff) as f64;
    if m == 0.0 {
        return 0.0;
    }
    2f64.powi(256) / (m * 2f64.powi(8 * (e - 3)))
}

/// From the miner's own job data (a line `auto-lock: network <coin> height H work W` of the round's / mining process).
pub fn net_from_work(coin: Coin, height: u64, work: f64) -> Option<Net> {
    if work <= 0.0 || !work.is_finite() {
        return None;
    }
    let reward = match coin {
        Coin::Pearl => Some(pearl_reward(height)).filter(|r| *r > 0.0)?,
        Coin::Noid => noid_miner_reward(height)?,
        _ => return None,
    };
    Some(Net { per_hs_day: 86400.0 * reward / work, src: format!("the pool's jobs (height {height}, work per block {work:.4e}, reward {reward:.4} {})", coin.sym()), pool_fee: None })
}

pub const LUCKYPOOL_STATS: &str = "https://pearl.luckypool.io/api/stats";
pub const INNOVLAB_PROX_STATS: &str = "https://prox.innovlab.cc/api/proxima/v1/stats";

/// LuckyPool api/stats: network.difficulty (× 2^16 = work per block) and network.reward (10^-8 PRL); config.fee (%)
pub fn pearl_net_api(body: &str) -> Option<Net> {
    let v: Value = serde_json::from_str(body).ok()?;
    let d = f(&v["network"]["difficulty"]).filter(|d| *d > 0.0)?;
    let units = f(&v["config"]["coinUnits"]).filter(|u| *u > 0.0).unwrap_or(1e8);
    let reward = f(&v["network"]["reward"]).map(|r| r / units).filter(|r| *r > 0.0).or_else(|| v["network"]["height"].as_u64().map(|h| pearl_reward(h + 1)))?;
    let work = d * 65536.0;
    Some(Net { per_hs_day: 86400.0 * reward / work, src: format!("LuckyPool api/stats (difficulty {d:.4e}, reward {reward:.4} PRL)"), pool_fee: f(&v["config"]["fee"]) })
}

/// InnovLab PROX stats: network.attempts_per_slot (rolling), reward_motes (10^-6 PROX); fee_bps
pub fn prox_net_api(body: &str) -> Option<Net> {
    let v: Value = serde_json::from_str(body).ok()?;
    let a = f(&v["network"]["attempts_per_slot"]).or_else(|| f(&v["network"]["rolling_attempts_per_slot"])).filter(|a| *a > 0.0)?;
    let reward = f(&v["reward_motes"]).map(|m| m / 1e6).filter(|r| *r > 0.0)?;
    let s = f(&v["network"]["slot_seconds"]).unwrap_or(10.24);
    Some(Net {
        per_hs_day: 86400.0 * reward / a,
        src: format!("InnovLab stats (network {:.2} TH/s = {a:.4e} attempts per {s} s slot, {reward} PROX per slot)", a / s / 1e12),
        pool_fee: f(&v["fee_bps"]).map(|b| b / 100.0),
    })
}

/// The public network API of a coin (Pearl: only when the pool's jobs give no height; PROX: always)
pub fn net_api(coin: Coin) -> Result<Net, String> {
    match coin {
        Coin::Pearl => http_get(LUCKYPOOL_STATS, TIMEOUT).and_then(|b| pearl_net_api(&b).ok_or_else(|| "no network data in LuckyPool api/stats".into())),
        Coin::Prox => http_get(INNOVLAB_PROX_STATS, TIMEOUT).and_then(|b| prox_net_api(&b).ok_or_else(|| "no network data in InnovLab stats".into())),
        _ => Err(format!("no public network API for {} (the pool's jobs give it while mining)", coin.sym())),
    }
}

/// The fee of a pool this miner knows (%, checked 15.11.2026); None = unknown (--pool-fee sets it)
pub fn known_pool_fee(coin: Coin, pool: &str) -> Option<(f64, &'static str)> {
    let p = pool.to_ascii_lowercase();
    match coin {
        Coin::Pearl if p.contains("luckypool") => Some((1.0, "LuckyPool")),
        Coin::Prox if p.contains("innovlab") || p.starts_with("innov") => Some((0.0, "InnovLab")),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// the money of one point
// ---------------------------------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct Econ {
    pub coin: Coin,
    pub price: f64,
    pub per_hs_day: f64,
    pub pool_fee: f64, // %
    pub kwh: f64,      // $/kWh averaged over the day
}

impl Econ {
    pub fn fee_mult(&self) -> f64 {
        (1.0 - MINER_FEE_PCT / 100.0) * (1.0 - self.pool_fee / 100.0)
    }
    /// coins per day at `rate` (unit of the stats line)
    pub fn coins(&self, rate: f64, unit: &str) -> f64 {
        rate * unit_mult(unit) * self.per_hs_day * self.fee_mult()
    }
    pub fn income(&self, rate: f64, unit: &str) -> f64 {
        self.coins(rate, unit) * self.price
    }
    pub fn elec(&self, w: f64) -> f64 {
        w / 1000.0 * 24.0 * self.kwh
    }
    pub fn profit(&self, rate: f64, unit: &str, w: f64) -> f64 {
        self.income(rate, unit) - self.elec(w)
    }
}

/// Switch from the current lock's profit to a better one only when it gains more than `pct` % (of |current|).
pub fn worth_switch(cur: f64, best: f64, pct: f64) -> bool {
    best - cur > cur.abs() * pct / 100.0 + 1e-9
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tariffs() {
        let t = Tariff::parse("0.12,0.053@23-07").unwrap();
        assert!((t.avg() - (16.0 * 0.12 + 8.0 * 0.053) / 24.0).abs() < 1e-12); // 0.0977 $/kWh
        assert!((t.avg() - 0.097667).abs() < 1e-5);
        assert_eq!(t.hours[23], 0.053);
        assert_eq!(t.hours[6], 0.053);
        assert_eq!(t.hours[7], 0.12);
        assert!((Tariff::parse("0.1").unwrap().avg() - 0.1).abs() < 1e-12);
        assert!((Tariff::parse("0.2,0.1@0-12").unwrap().avg() - 0.15).abs() < 1e-12);
        let three = Tariff::parse("0.2, 0.1@22-6, 0.05@2:00-4:00").unwrap(); // later ranges win
        assert_eq!((three.hours[1], three.hours[3], three.hours[5], three.hours[12]), (0.1, 0.05, 0.1, 0.2));
        for bad in ["", "x", "0.1@23-07", "0.12,0.05", "0.12,0.05@23-23", "0.12,0.05@25-7", "11", "-0.1"] {
            assert!(Tariff::parse(bad).is_err(), "{bad}");
        }
    }

    fn q(name: &str, p: f64, vol: Option<f64>) -> Quote {
        Quote { name: name.into(), price: p, vol, err: None, thin: false, outlier: false, used: false }
    }
    fn e(name: &str) -> Quote {
        Quote { name: name.into(), price: 0.0, vol: None, err: Some("timeout".into()), thin: false, outlier: false, used: false }
    }

    #[test]
    fn median_outliers_thin() {
        let mut v = vec![q("a", 1.50, Some(3e6)), q("b", 1.51, Some(9e6)), q("c", 1.49, None), q("d", 2.40, Some(1e5)), e("x")];
        let p = aggregate(&mut v, 15.0).unwrap();
        assert!((p - 1.50).abs() < 1e-9, "{p}"); // median of 1.49 1.50 1.51 (2.40 is an outlier)
        assert!(v[3].outlier && !v[3].used && v.iter().filter(|q| q.used).count() == 3);
        // a thin ticker far off does not move it, and is left out while thick ones answer
        let mut v = vec![q("a", 1.50, Some(3e6)), q("b", 1.52, Some(9e6)), q("p", 0.40, Some(0.35))];
        assert!((aggregate(&mut v, 15.0).unwrap() - 1.51).abs() < 1e-9);
        assert!(v[2].thin && !v[2].used && !v[2].outlier);
        // only thin ones: used (flagged by price_from)
        let mut v = vec![q("p", 0.40, Some(0.35))];
        assert_eq!(aggregate(&mut v, 15.0), Some(0.40));
        // nothing answered
        let mut v = vec![e("a"), e("b")];
        assert_eq!(aggregate(&mut v, 15.0), None);
        let pr = price_from(Coin::Prox, vec![q("SafeTrade", 0.034, Some(55000.0))]).unwrap();
        assert!(pr.warn.unwrap().contains("ONE ticker"));
        assert!(price_from(Coin::Cmfd, vec![]).unwrap_err().contains("--coin-price"));
        assert!(price_from(Coin::Noid, vec![e("a")]).unwrap_err().contains("none of its 1"));
        let pr = price_from(Coin::Pearl, vec![q("a", 1.5, Some(1e6)), q("b", 1.52, Some(1e6)), q("c", 1.0, Some(1e6))]).unwrap();
        assert_eq!(pr.used, 2);
        assert!(pr.lines[2].contains("OUTLIER"));
    }

    #[test]
    fn ticker_answers() {
        // samples of the real answers (15.11.2026), shortened
        let peatio = r#"{"at":"1794742359","ticker":{"buy":"0.0338","sell":"0.0331","last":"0.0336","volume":"55054.82103363","amount":"1636523.6931"}}"#;
        assert_eq!(parse_price(Kind::Peatio, "", peatio).unwrap(), (0.0336, Some(55054.82103363)));
        let gate = r#"[{"currency_pair":"PEARL_USDT","last":"1.516","base_volume":"1999572.8","quote_volume":"3004811.51018"}]"#;
        assert_eq!(parse_price(Kind::Gate, "", gate).unwrap(), (1.516, Some(3004811.51018)));
        let lbank = r#"{"msg":"Success","result":"true","data":[{"symbol":"pearl_usdt","ticker":{"vol":"653961.2","turnover":"982766.5233","latest":"1.5149"}}]}"#;
        assert_eq!(parse_price(Kind::LBank, "", lbank).unwrap(), (1.5149, Some(982766.5233)));
        let cb = r#"{"ask":"1.5155","bid":"1.5123","volume":"1000","price":"1.5"}"#;
        assert_eq!(parse_price(Kind::Coinbase, "", cb).unwrap(), (1.5, Some(1500.0)));
        let kr = r#"{"error":[],"result":{"PEARLUSD":{"c":["1.51660000","33.0"],"v":["85780.0","200000.0"],"p":["1.52","1.5"]}}}"#;
        assert_eq!(parse_price(Kind::Kraken, "PEARLUSD", kr).unwrap(), (1.5166, Some(300000.0)));
        let b1 = r#"{"code":0,"data":{"asset_pair_name":"PEARL-USDT","close":"1.5","volume":"100"}}"#;
        assert_eq!(parse_price(Kind::BigOne, "", b1).unwrap(), (1.5, Some(150.0)));
        let cg = r#"{"pearl-2":{"usd":1.51,"usd_24h_vol":21905666.67}}"#;
        assert_eq!(parse_price(Kind::CoinGecko, "pearl-2", cg).unwrap(), (1.51, Some(21905666.67)));
        assert!(parse_price(Kind::CoinGecko, "parano1d", cg).is_err());
        let pp = r#"{"id":"prl-pearl-1","quotes":{"USD":{"price":1.5007222106248643,"volume_24h":0.34516610844371887}}}"#;
        assert_eq!(parse_price(Kind::Paprika, "", pp).unwrap().1, Some(0.34516610844371887));
        let lp = r#"{"config":{"fee":1},"markets":{"btc":65386,"price":1.4972,"exchanges":[{"name":"SafeTrade","quoteVolume":1995345.13}]}}"#;
        assert_eq!(parse_price(Kind::LuckyPool, "", lp).unwrap(), (1.4972, Some(1995345.13)));
        // garbage, an error page, a zero price
        assert!(parse_price(Kind::Peatio, "", "<!DOCTYPE html><title>Attention Required! | Cloudflare</title>").is_err());
        assert!(parse_price(Kind::Gate, "", r#"{"label":"INVALID_CURRENCY","message":"Invalid currency PROX"}"#).is_err());
        assert!(parse_price(Kind::Peatio, "", r#"{"ticker":{"last":"0"}}"#).is_err());
        assert!(parse_price(Kind::Peatio, "", r#"{"ticker":{"last":"NaN"}}"#).is_err());
    }

    #[test]
    fn coin_formulas() {
        // Pearl: the node's subsidy (LuckyPool block 143550 paid 216714637403 grains)
        assert!((pearl_reward(143550) - 2167.14637403).abs() < 1e-6);
        // work per block from nbits of a LuckyPool job (15.11.2026, height 143562): 1.6901e22
        let w = work_nbits(0x1747873e);
        assert!((w / 1.69013662e22 - 1.0).abs() < 1e-6, "{w:e}");
        // = LuckyPool's network.difficulty × 2^16 within 0.1 %
        assert!((w / (257812789447537920.0 * 65536.0) - 1.0).abs() < 1e-3);
        let n = net_from_work(Coin::Pearl, 143562, w).unwrap();
        // a 3070 at 63.3 TH/s: 63.3e12 × 86400 × 2167.08 / 1.6901e22 = 0.701 PRL/day before fees
        let ec = Econ { coin: Coin::Pearl, price: 1.5, per_hs_day: n.per_hs_day, pool_fee: 1.0, kwh: 0.0977 };
        let c = ec.coins(63.325, "TH/s");
        assert!((c - 0.7013 * 0.99 * 0.99).abs() < 0.001, "{c}");
        assert!((ec.elec(87.2) - 87.2 * 24.0 / 1000.0 * 0.0977).abs() < 1e-12);
        // the API way gives the same within 0.1 %
        let api = pearl_net_api(r#"{"config":{"fee":1,"coinUnits":100000000},"network":{"difficulty":257812789447537920,"height":143561,"reward":216708631142}}"#).unwrap();
        assert!((api.per_hs_day / n.per_hs_day - 1.0).abs() < 1e-3);
        assert_eq!(api.pool_fee, Some(1.0));
        // PROX: 95 PROX per slot, 46 TH/s network (4.7104e14 attempts per 10.24 s slot): a 3070 at 2.641 GH/s
        let px = prox_net_api(r#"{"network":{"attempts_per_slot":"471040000000000","slot_seconds":10.24},"reward_motes":"95000000","fee_bps":0}"#).unwrap();
        let ep = Econ { coin: Coin::Prox, price: 0.034, per_hs_day: px.per_hs_day, pool_fee: 0.0, kwh: 0.0977 };
        let coins = ep.coins(2.641, "GH/s") / 0.99;
        assert!((coins - 2.641e9 / 46e12 * 86400.0 / 10.24 * 95.0).abs() < 1e-6, "{coins}"); // 46.0 PROX/day
        assert_eq!(px.pool_fee, Some(0.0));
        assert!(prox_net_api(r#"{"network":{}}"#).is_none());
        // NOID: v2 epoch 0 with the development allocation: 14.4 NOID; after the 3 years 16; later epochs
        assert_eq!(noid_miner_reward(239348), Some(14.4));
        assert_eq!(noid_miner_reward(100), None);
        assert!((noid_miner_reward(210537 + 1_051_200).unwrap() - 10.17).abs() < 1e-9);
        assert_eq!(noid_miner_reward(4_000_000), Some(5.65)); // epoch 3, past the 3 years (height 3 223 778)
        let mut t = [0u8; 32];
        t[30] = 1; // 2^240
        assert_eq!(work_le(&t), 65536.0);
        let nn = net_from_work(Coin::Noid, 239348, 1e15).unwrap();
        assert!((nn.per_hs_day - 86400.0 * 14.4 / 1e15).abs() < 1e-24);
        assert!(net_from_work(Coin::Prox, 1, 1.0).is_none());
        assert_eq!(unit_mult("MH/s"), 1e6);
        assert_eq!(Coin::of("noid-eff"), Some(Coin::Noid));
        assert_eq!(known_pool_fee(Coin::Pearl, "pearl-ru.luckypool.io:3360").unwrap().0, 1.0);
        assert!(known_pool_fee(Coin::Pearl, "prl-eu.kryptex.network:7048").is_none());
    }

    #[test]
    fn hysteresis() {
        assert!(!worth_switch(1.31, 1.34, 3.0)); // +2.3 %
        assert!(worth_switch(1.31, 1.52, 3.0)); // +16 %
        assert!(!worth_switch(1.0, 1.03, 3.0)); // exactly 3 %: stays
        assert!(worth_switch(-0.5, -0.4, 3.0)); // a loss that shrinks by 20 %
        assert!(!worth_switch(-0.5, -0.49, 3.0));
        assert!(worth_switch(0.0, 0.01, 3.0));
        assert!(!worth_switch(1.0, 0.9, 0.0));
    }

    /// A one-shot HTTP server on 127.0.0.1 for each answer (raw bytes), the URLs in order.
    fn mock(answers: Vec<Vec<u8>>) -> Vec<String> {
        answers
            .into_iter()
            .map(|a| {
                let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
                let port = l.local_addr().unwrap().port();
                std::thread::spawn(move || {
                    if let Ok((mut s, _)) = l.accept() {
                        let mut req = Vec::new();
                        let mut b = [0u8; 4096];
                        while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                            match s.read(&mut b) {
                                Ok(0) | Err(_) => break,
                                Ok(n) => req.extend_from_slice(&b[..n]),
                            }
                        }
                        let req = String::from_utf8_lossy(&req).to_string();
                        // the request carries nothing but the path, Host and the neutral User-Agent
                        // (anything else gets no answer: the test fails)
                        if !(req.starts_with("GET /t?x=1 HTTP/1.1\r\n") && req.contains(&format!("User-Agent: {UA}\r\n")) && !req.contains("Cookie")) {
                            return;
                        }
                        if a.is_empty() {
                            std::thread::sleep(Duration::from_secs(3)); // never answers
                        } else {
                            let _ = s.write_all(&a);
                        }
                    }
                });
                format!("http://127.0.0.1:{port}/t?x=1")
            })
            .collect()
    }

    #[test]
    fn http_sources_with_a_mock() {
        let body = r#"{"ticker":{"last":"0.0336","volume":"55054.8"}}"#;
        let ok = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()).into_bytes();
        let chunked = format!("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{}\r\n{:x}\r\n{}\r\n0\r\n\r\n", 10, &body[..10], body.len() - 10, &body[10..]).into_bytes();
        let cf = b"HTTP/1.1 403 Forbidden\r\nContent-Length: 5\r\n\r\nnope!".to_vec();
        let garbage = b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\n<html>...".to_vec();
        let ob = r#"{"ticker":{"last":"0.09","volume":"99999"}}"#;
        let off = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{ob}", ob.len()).into_bytes();
        let urls = mock(vec![ok, chunked, cf, garbage, vec![], off]);
        let t = Duration::from_secs(1);
        assert_eq!(parse_price(Kind::Peatio, "", &http_get(&urls[0], t).unwrap()).unwrap().0, 0.0336);
        assert_eq!(http_get(&urls[1], t).unwrap(), body);
        assert_eq!(http_get(&urls[2], t).unwrap_err(), "HTTP 403");
        assert!(parse_price(Kind::Peatio, "", &http_get(&urls[3], t).unwrap()).is_err());
        let t0 = Instant::now();
        assert!(http_get(&urls[4], t).is_err()); // a server that never answers: the timeout, not a hang
        assert!(t0.elapsed() < Duration::from_secs(3));
        // a set: two good (one chunked), a 403, garbage, no answer, an outlier -> the median of the good ones
        let mut qs: Vec<Quote> = Vec::new();
        let urls2 = mock(vec![
            format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}", body.len()).into_bytes(),
            format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}", body.len(), body.replace("0.0336", "0.0340")).into_bytes(),
            b"HTTP/1.1 403 Forbidden\r\n\r\n".to_vec(),
        ]);
        for (i, u) in urls2.iter().chain(std::iter::once(&urls[5])).enumerate() {
            qs.push(match http_get(u, t).and_then(|b| parse_price(Kind::Peatio, "", &b)) {
                Ok((p, v)) => q(&format!("s{i}"), p, v),
                Err(er) => Quote { err: Some(er), ..e(&format!("s{i}")) },
            });
        }
        let pr = price_from(Coin::Prox, qs).unwrap();
        assert!((pr.usd - 0.0338).abs() < 1e-9, "{}", pr.usd);
        assert_eq!(pr.used, 2);
        assert!(pr.lines.iter().any(|l| l.contains("HTTP 403")) && pr.lines.iter().any(|l| l.contains("OUTLIER")));
        assert!(http_get("ftp://x/y", t).is_err());
    }

    /// The real tickers and network APIs (network access; cargo test live_sources -- --ignored --nocapture)
    #[test]
    #[ignore]
    fn live_sources() {
        for c in [Coin::Pearl, Coin::Prox, Coin::Noid, Coin::Cmfd] {
            match price_from(c, fetch_quotes(&sources_of(c))) {
                Ok(p) => {
                    println!("{} {:.5} $ ({} used){}", c.sym(), p.usd, p.used, p.warn.map_or(String::new(), |w| format!(" WARNING {w}")));
                    for l in p.lines {
                        println!("{l}");
                    }
                }
                Err(e) => println!("{}: {e}", c.sym()),
            }
            println!("  network: {:?}", net_api(c).map(|n| format!("{:.4e} per H/s per day, {}", n.per_hs_day, n.src)));
        }
    }
}
