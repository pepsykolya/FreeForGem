// FFGN — see LICENSE
//! `--api [HOST:]PORT`: read-only HTTP status (default bind 127.0.0.1): `/summary` and `/gpus` (JSON), `/metrics`
//! (Prometheus text).  Refreshed every 10 s with the stats line.  No control endpoints.
use serde_json::json;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Default)]
pub struct Gpu {
    pub dev: i32,
    pub name: String,
    pub bus: String,
    /// attempts evaluated (the hashrate; 1.0.9: the engine's evaluated count)
    pub attempts: u64,
    /// 1.0.9: nonces scanned (the engine's cheap filter / digest table screen more nonces than they evaluate)
    pub screened: u64,
    pub alive: bool,
    pub bad: u64,
    pub power: Option<f64>,
    /// 1.0.3: (core lock, clock under load) when the core lock does not hold on this GPU
    pub lock_not_held: Option<(u32, u32)>,
    /// 1.0.4: attempts/s over the averaging window
    pub rate_avg: Option<f64>,
    /// 1.0.4: the OC profile's memory lock (MHz) that stayed: NEB runs several times slower
    pub mem_stuck: Option<u32>,
    /// 1.0.5: the KEP1 attempt kernel ("warp-mma" / "dp4a") and why (auto measurement or forced; 1.0.6: measured at every new model)
    pub kep1_kernel: Option<String>,
    /// 1.0.5: idle time of the GPU on the last model change (ms)
    pub model_gap_ms: Option<u64>,
}

#[derive(Clone, Default)]
pub struct Snapshot {
    pub rate: f64,
    /// 1.0.4: attempts/s over the last `avg_secs` (None in the first window)
    pub rate_avg: Option<f64>,
    pub avg_secs: u64,
    /// 1.0.4: the dataset of the current block: (slot, name: "Kepler" / "TESS")
    pub slot: Option<(u8, String)>,
    /// a job but no mining: the reason (shown as "status")
    pub not_mining: Option<String>,
    pub uptime: u64,
    pub solo: bool,
    pub source: String,
    pub height: Option<u32>,
    pub shares: u64,
    pub rejected: u64,
    pub blocks_sub: u64,
    pub blocks_acc: u64,
    pub bad: u64,
    pub gpus: Vec<Gpu>,
}

pub type Handle = Arc<Mutex<Snapshot>>;

pub fn publish(h: &Handle, s: Snapshot) {
    *h.lock().unwrap() = s;
}

fn summary(s: &Snapshot) -> String {
    json!({
        "miner": crate::PRODUCT, "version": env!("CARGO_PKG_VERSION"), "algo": super::ALGO_NAME, "uptime": s.uptime,
        "mode": if s.solo { "solo" } else { "pool" }, "source": s.source, "height": s.height,
        "status": s.not_mining.as_ref().map_or("mining".to_string(), |r| format!("NOT MINING: {r}")),
        "attempts_per_s": s.rate, "attempts_per_s_avg": s.rate_avg, "avg_seconds": s.avg_secs,
        "slot": s.slot.as_ref().map(|x| x.0), "dataset": s.slot.as_ref().map(|x| x.1.clone()), "shares": s.shares, "rejected": s.rejected,
        "blocks": {"submitted": s.blocks_sub, "accepted": s.blocks_acc}, "bad_results": s.bad,
        "power_w": s.gpus.iter().filter_map(|g| g.power).sum::<f64>(),
        "warnings": warnings(s),
    })
    .to_string()
}

/// the loud warnings of the GPUs: a core lock that does not hold (1.0.3), the OC profile's memory lock that stayed (1.0.4)
fn warnings(s: &Snapshot) -> Vec<String> {
    let mut v: Vec<String> = s.gpus.iter().filter_map(|g| g.lock_not_held.map(|(l, m)| crate::clocks::not_held_message(g.dev, l, m))).collect();
    v.extend(crate::neb::tune::warnings().into_iter().map(|w| format!("auto-uv: {w}")));
    v.extend(s.gpus.iter().filter_map(|g| g.mem_stuck.map(|m| format!("WARNING GPU{}: {}", g.dev, crate::clocks::mem_stuck_message(m)))));
    v
}

fn gpus(s: &Snapshot) -> String {
    json!(s.gpus.iter().map(|g| json!({"dev": g.dev, "name": g.name, "bus": g.bus, "attempts": g.attempts, "screened": g.screened, "alive": g.alive, "bad": g.bad, "power_w": g.power, "attempts_per_s_avg": g.rate_avg, "memory_locked_by_oc_mhz": g.mem_stuck,
        "kep1_kernel": g.kep1_kernel.as_ref().map(|k| k.split(' ').next().unwrap_or("").to_string()), "kep1_kernel_info": g.kep1_kernel, "model_change_idle_ms": g.model_gap_ms,
        "core_lock_holds": g.lock_not_held.is_none(), "core_lock_not_held": g.lock_not_held.map(|(l, m)| json!({"lock_mhz": l, "clock_mhz": m}))})).collect::<Vec<_>>()).to_string()
}

fn metrics(s: &Snapshot) -> String {
    let mut o = String::new();
    o += &format!("# TYPE ffgn_attempts_per_second gauge\nffgn_attempts_per_second {}\n", s.rate);
    if let Some(a) = s.rate_avg {
        o += &format!("# TYPE ffgn_attempts_per_second_avg gauge\nffgn_attempts_per_second_avg{{window=\"{}s\"}} {a}\n", s.avg_secs);
    }
    if let Some((n, name)) = &s.slot {
        o += &format!("# TYPE ffgn_dataset_slot gauge\nffgn_dataset_slot{{dataset=\"{name}\"}} {n}\n");
    }
    o += &format!("# TYPE ffgn_uptime_seconds counter\nffgn_uptime_seconds {}\n", s.uptime);
    o += &format!("# TYPE ffgn_shares_total counter\nffgn_shares_total {}\nffgn_rejected_total {}\n", s.shares, s.rejected);
    o += &format!("# TYPE ffgn_blocks_total counter\nffgn_blocks_total{{state=\"submitted\"}} {}\nffgn_blocks_total{{state=\"accepted\"}} {}\n", s.blocks_sub, s.blocks_acc);
    o += &format!("ffgn_bad_results_total {}\n", s.bad);
    for g in &s.gpus {
        o += &format!("ffgn_gpu_attempts_total{{gpu=\"{}\",bus=\"{}\"}} {}\n", g.dev, g.bus, g.attempts);
        o += &format!("ffgn_gpu_screened_total{{gpu=\"{}\",bus=\"{}\"}} {}\n", g.dev, g.bus, g.screened);
        o += &format!("ffgn_gpu_alive{{gpu=\"{}\"}} {}\n", g.dev, g.alive as u8);
        if let Some(p) = g.power {
            o += &format!("ffgn_gpu_power_watts{{gpu=\"{}\"}} {p}\n", g.dev);
        }
    }
    o
}

pub fn start(addr: &str) -> Handle {
    let h: Handle = Arc::new(Mutex::new(Snapshot::default()));
    let bind = if addr.contains(':') { addr.to_string() } else { format!("127.0.0.1:{addr}") };
    match TcpListener::bind(&bind) {
        Ok(l) => {
            println!("api: http://{bind}/summary /gpus /metrics (read-only)");
            let h2 = h.clone();
            std::thread::Builder::new()
                .name("api".into())
                .spawn(move || {
                    for s in l.incoming().flatten() {
                        let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
                        let _ = s.set_write_timeout(Some(Duration::from_secs(2)));
                        let mut s = s;
                        let mut b = [0u8; 2048];
                        let n = s.read(&mut b).unwrap_or(0);
                        let req = String::from_utf8_lossy(&b[..n]);
                        let path = req.split_whitespace().nth(1).unwrap_or("/");
                        let snap = h2.lock().unwrap().clone();
                        let (code, ct, body) = match path {
                            "/" | "/summary" => ("200 OK", "application/json", summary(&snap)),
                            "/gpus" => ("200 OK", "application/json", gpus(&snap)),
                            "/metrics" => ("200 OK", "text/plain; version=0.0.4", metrics(&snap)),
                            _ => ("404 Not Found", "text/plain", "not found\n".into()),
                        };
                        let _ = s.write_all(format!("HTTP/1.1 {code}\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes());
                    }
                })
                .ok();
        }
        Err(e) => eprintln!("api: cannot listen on {bind}: {e} (mining continues without the API)"),
    }
    h
}
