// FFGT — see LICENSE
//! Local read-only HTTP API (`--api-port N`, default 4068 on 127.0.0.1; `--api-bind ADDR:PORT` for another address):
//!   GET /summary  {"miner","version","uptime","algo","target","hashrate_ghs","power_w","efficiency_ghj","user":{...},"fee":{...},
//!                  "candidates","bad","devfee","solo","height"}
//!   GET /gpus     [{"dev","name","bus","hashrate_ghs","power_w","efficiency_ghj","alive","bad"}]
//!   GET /metrics  Prometheus text (ffgt_hashrate_ghs{gpu=...}, ffgt_power_watts, ffgt_shares_total{src,kind})
//! Plain std TCP, one thread, no request body; nothing secret is served (no node token, no pool password).
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Mutex;

#[derive(Clone, Default)]
pub struct GpuLine {
    pub dev: i32,
    pub name: String,
    pub bus: String,
    pub ghs: f64,
    pub w: Option<f64>,
    pub alive: bool,
    pub bad: u64,
}

#[derive(Clone, Default)]
pub struct Snapshot {
    pub uptime: u64,
    pub total_ghs: f64,
    pub total_w: Option<f64>,
    pub gpus: Vec<GpuLine>,
    pub user: (u64, u64, u64),
    pub fee: (u64, u64, u64),
    pub candidates: u64,
    pub bad: u64,
    pub devfee: bool,
    pub solo: bool,
    pub height: u64,
}

static SNAP: Mutex<Option<Snapshot>> = Mutex::new(None);
static TARGET: Mutex<String> = Mutex::new(String::new());

pub fn publish_static<T>(_gpus: &[T], target: &str) {
    *TARGET.lock().unwrap() = target.to_string();
}

pub fn publish(s: Snapshot) {
    *SNAP.lock().unwrap() = Some(s);
}

fn eff(ghs: f64, w: Option<f64>) -> Value {
    match w {
        Some(w) if w > 0.0 => json!((ghs / w * 1e4).round() / 1e4),
        _ => Value::Null,
    }
}

fn r3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

pub fn summary() -> Value {
    let s = SNAP.lock().unwrap().clone().unwrap_or_default();
    json!({
        "miner": crate::PRODUCT, "version": env!("CARGO_PKG_VERSION"), "uptime": s.uptime, "algo": super::ALGO_NAME,
        "target": *TARGET.lock().unwrap(), "hashrate_ghs": r3(s.total_ghs), "power_w": s.total_w.map(|w| w.round()),
        "efficiency_ghj": eff(s.total_ghs, s.total_w),
        "user": {"submitted": s.user.0, "accepted": s.user.1, "rejected": s.user.2},
        "fee": {"submitted": s.fee.0, "accepted": s.fee.1, "rejected": s.fee.2},
        "candidates": s.candidates, "bad": s.bad, "devfee": s.devfee, "solo": s.solo, "height": s.height,
    })
}

pub fn gpus() -> Value {
    let s = SNAP.lock().unwrap().clone().unwrap_or_default();
    Value::Array(
        s.gpus
            .iter()
            .map(|g| json!({"dev": g.dev, "name": g.name, "bus": g.bus, "hashrate_ghs": r3(g.ghs), "power_w": g.w.map(|w| w.round()), "efficiency_ghj": eff(g.ghs, g.w), "alive": g.alive, "bad": g.bad}))
            .collect(),
    )
}

pub fn metrics() -> String {
    let s = SNAP.lock().unwrap().clone().unwrap_or_default();
    let mut o = String::new();
    o.push_str("# TYPE ffgt_hashrate_ghs gauge\n");
    for g in &s.gpus {
        o.push_str(&format!("ffgt_hashrate_ghs{{gpu=\"{}\",bus=\"{}\"}} {:.4}\n", g.dev, g.bus, g.ghs));
    }
    o.push_str(&format!("ffgt_hashrate_ghs{{gpu=\"total\"}} {:.4}\n", s.total_ghs));
    o.push_str("# TYPE ffgt_power_watts gauge\n");
    for g in &s.gpus {
        if let Some(w) = g.w {
            o.push_str(&format!("ffgt_power_watts{{gpu=\"{}\"}} {:.1}\n", g.dev, w));
        }
    }
    o.push_str("# TYPE ffgt_shares_total counter\n");
    for (src, t) in [("user", s.user), ("fee", s.fee)] {
        o.push_str(&format!("ffgt_shares_total{{src=\"{src}\",kind=\"submitted\"}} {}\nffgt_shares_total{{src=\"{src}\",kind=\"accepted\"}} {}\nffgt_shares_total{{src=\"{src}\",kind=\"rejected\"}} {}\n", t.0, t.1, t.2));
    }
    o.push_str(&format!("# TYPE ffgt_uptime_seconds counter\nffgt_uptime_seconds {}\n", s.uptime));
    o
}

/// Starts the API server thread; an address in use is reported and ignored (mining goes on).
pub fn start(bind: &str) {
    let l = match TcpListener::bind(bind) {
        Ok(l) => l,
        Err(e) => {
            println!("api: cannot listen on {bind}: {e} (API off)");
            return;
        }
    };
    println!("api: http://{bind}/summary /gpus /metrics");
    std::thread::Builder::new()
        .name("ffgt-api".into())
        .spawn(move || {
            for c in l.incoming().flatten() {
                let mut c = c;
                let _ = c.set_read_timeout(Some(std::time::Duration::from_secs(2)));
                let mut b = [0u8; 2048];
                let n = c.read(&mut b).unwrap_or(0);
                let req = String::from_utf8_lossy(&b[..n]);
                let path = req.split_whitespace().nth(1).unwrap_or("/");
                let (code, ct, body) = match path.split('?').next().unwrap_or("/") {
                    "/" | "/summary" => ("200 OK", "application/json", summary().to_string()),
                    "/gpus" => ("200 OK", "application/json", gpus().to_string()),
                    "/metrics" => ("200 OK", "text/plain; version=0.0.4", metrics()),
                    _ => ("404 Not Found", "text/plain", "not found\n".into()),
                };
                let _ = c.write_all(format!("HTTP/1.1 {code}\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes());
            }
        })
        .ok();
}
