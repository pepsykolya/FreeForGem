// FFGN — see LICENSE
//! Pool client of `neb-pool/1` (spec/pool.md of NEB 1.0.0): JSON lines over TCP (or TLS: `stratum+ssl://`), stratum style.
//!   -> mining.subscribe [agent, "neb-pool/1"]   <- {session, extranonce1, extranonce2_size, chain_id, dataset_hash}
//!   -> mining.authorize [login, password]       (the user's login; the dev-fee login in the SAME session, spec section 4)
//!   <- mining.set_target [target] / mining.set_model [slot, weights_hash, weights] / mining.notify [job]
//!   -> mining.submit [login, job_id, extranonce2, time, nonce]
//! The same client talks to `ffgn --gate` (gate.rs), which adds `ffgn_nonce: [base, log2 span]` to the subscribe result: the
//! nonce range of this session on the node's reference header (extranonce2_size 0).
//! `--pool A,B` failover / failback through failover.rs, as every FreeForGem coin.
use super::proto::{self, Job, hex, unhex, unhex32};
use super::refc::Model;
use super::{Ev, Find, NJob, Origin};
use crate::net::{Conn, PoolUrl, connect};
use crate::stratum::{Src, Stats};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const PROTOCOL: &str = "neb-pool/1";
/// submits per second at most (F-14)
const SUBMIT_RATE: u32 = 25;
const SILENCE_LIMIT: Duration = Duration::from_secs(300);
/// stale-share fix: the pool said "job not found" for its newest job and sent no new one this long: a new connection
const GONE_RECONNECT: Duration = Duration::from_secs(if cfg!(test) { 2 } else { 20 });

/// FFGN 1.0.13 (Rig05, 5x CMP 50HX, 21.11.2026): 25 % stale shares ("job not found") from 12:37 on, the same binary 0-3 %
/// before; the pool's answers came 2-34 s after the submit, several at once (a TCP retransmission backoff: the rig's r8169 NIC
/// logged "NETDEV WATCHDOG: transmit queue 0 timed out", 63 % of the pool connection's bytes retransmitted, 30 % loss of
/// 1200-byte pings).  The miner cannot repair that, so it says so loudly: answers this late, this many within the window, one
/// WARNING line per WARN_EVERY with the median delay and the stale count.
const LATE_ANSWER: Duration = Duration::from_secs(2);
const LATE_N: usize = 3;
const LATE_WINDOW: Duration = Duration::from_secs(300);
const LATE_WARN_EVERY: Duration = Duration::from_secs(600);

/// Late pool answers (see LATE_ANSWER).
#[derive(Default)]
pub struct LateAnswers {
    late: std::collections::VecDeque<(Instant, Duration)>,
    warned: Option<Instant>,
}

impl LateAnswers {
    /// An answer `delay` after its submit, at `now`; Some(line) = warn now.
    pub fn note(&mut self, now: Instant, delay: Duration, stale: u64, sent: u64) -> Option<String> {
        while self.late.front().is_some_and(|x| now.saturating_duration_since(x.0) > LATE_WINDOW) {
            self.late.pop_front();
        }
        if delay < LATE_ANSWER {
            return None;
        }
        self.late.push_back((now, delay));
        if self.late.len() < LATE_N || self.warned.is_some_and(|w| now.saturating_duration_since(w) < LATE_WARN_EVERY) {
            return None;
        }
        self.warned = Some(now);
        let mut d: Vec<f64> = self.late.iter().map(|x| x.1.as_secs_f64()).collect();
        d.sort_by(|a, b| a.partial_cmp(b).unwrap());
        Some(format!(
            "WARNING: the pool answers late: {} answers {:.0}-{:.0} s (median {:.1} s) after their submit within {} min; stale so far {stale} of {sent} sent. \
             The shares reach the pool late and come back as stale (\"job not found\"). A ping can look fine: this is lost packets on \
             this rig's network (NIC / cable / switch; Linux: dmesg | grep -i 'transmit queue' / ss -ti), not the miner's queue",
            d.len(),
            d[0],
            d[d.len() - 1],
            d[d.len() / 2],
            LATE_WINDOW.as_secs() / 60
        ))
    }
}

/// The protocol of a pool entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// not known yet: JSON first (neb-pool/1 or Suprnova stratum); no answer to mining.subscribe = the work protocol
    Auto,
    /// JSON lines: neb-pool/1 or the Suprnova dialect (told apart by the subscribe result)
    Json,
    /// the NEB work protocol (binary frames of spec/node.md 7.1) with the login as the Hello name: Aria
    Work,
    /// an Auto entry that gave no JSON answer: the next connection tries the work protocol; it becomes Work only when a job
    /// arrives that way, else Auto again (F-04: nothing is decided for good without a positive sign)
    TryWork,
}

/// `work://HOST:PORT` (or `neb+work://`, `aria://`) = the NEB work protocol; `stratum+tcp://` / `stratum+ssl://` = JSON
/// stratum; a bare HOST:PORT = Auto, except known hosts (ariabrain.com = work protocol).
pub fn kind_of(s: &str) -> Kind {
    let t = s.trim().to_ascii_lowercase();
    if t.starts_with("work://") || t.starts_with("neb+work://") || t.starts_with("aria://") || t.contains("ariabrain.com") {
        Kind::Work
    } else if t.contains("://") {
        Kind::Json
    } else {
        Kind::Auto
    }
}

/// `stratum+tcp://` / `tcp://` / bare HOST:PORT = plain TCP (the NEB reference pool is plain); `stratum+ssl://` = TLS.
pub fn parse_url(s: &str) -> Result<PoolUrl, String> {
    let s = &{
        let t = s.trim();
        let lo = t.to_ascii_lowercase();
        ["work://", "neb+work://", "aria://"].iter().find(|p| lo.starts_with(*p)).map_or(t.to_string(), |p| t[p.len()..].to_string())
    };
    let t = s.trim();
    let plain = !t.contains("://") || t.starts_with("stratum+tcp://") || t.starts_with("tcp://") || t.starts_with("stratum://") || t.starts_with("neb://");
    let fixed = if t.starts_with("neb://") { t.replacen("neb://", "stratum+tcp://", 1) } else if !t.contains("://") { format!("stratum+tcp://{t}") } else { t.to_string() };
    let mut u = crate::net::parse_url(&fixed)?;
    if plain {
        u.tls = false;
    }
    Ok(u)
}

pub struct Login {
    pub name: &'static str,
    /// index of `url` in `urls`
    pub entry: usize,
    pub url: PoolUrl,
    pub urls: Vec<PoolUrl>,
    pub link: Arc<crate::failover::Link>,
    pub follow: Option<Arc<crate::failover::Link>>,
    pub src: Src,
    /// the user's login (address.worker), or the dev-fee login of a fee-only session
    pub login: String,
    pub password: String,
    /// dev-fee login authorized in the same session (pool mode); None for a gate / fee-only session
    pub fee_login: Option<String>,
    pub enabled: Arc<AtomicBool>,
    pub stop: Arc<AtomicBool>,
    pub dataset_hash: [u8; 32],
    /// the fee login is authorized on the current connection
    pub fee_ok: Arc<AtomicBool>,
    pub conn_seq: Arc<AtomicU64>,
    /// a token for `ffgn --gate` (sent as the password when set)
    pub fee_stats: Arc<Stats>,
    /// protocol of each entry (Auto entries learn theirs on the first connection)
    pub kinds: Arc<Mutex<Vec<Kind>>>,
    /// work-protocol session counters
    pub ns: Arc<super::node::NodeStats>,
    /// the session has a usable job (fee-only sessions: the fee slot may start)
    pub ready: Arc<AtomicBool>,
    /// the pool is an ffgn gate (solo through someone's node): the dev fee goes to the public fee pools
    pub gate: Arc<AtomicBool>,
}

pub fn spawn(l: Login, tx: Sender<(Src, Ev)>, rx: Receiver<Find>, stats: Arc<Stats>) {
    std::thread::Builder::new()
        .name(format!("neb-pool-{}", l.name))
        .spawn(move || {
            let mut l = l;
            let src = l.src;
            let lp = crate::failover::Loop {
                name: l.name,
                link: l.link.clone(),
                follow: l.follow.clone(),
                stop: l.stop.clone(),
                enabled: l.enabled.clone(),
                fee: src == Src::Fee,
                label: Box::new({
                    let u = l.urls.clone();
                    move |e| {
                        let x = &u[e.min(u.len() - 1)];
                        format!("{}:{}", x.host, x.port)
                    }
                }),
            };
            crate::failover::run(
                lp,
                &mut l,
                |l, e| {
                    l.url = l.urls[e].clone();
                    l.entry = e;
                },
                |l| session(l, &tx, &rx, &stats),
                |l| {
                    stats.connected.store(0, Ordering::Relaxed);
                    l.fee_ok.store(false, Ordering::Relaxed);
                    let _ = tx.send((l.src, Ev::Down));
                    while rx.try_recv().is_ok() {}
                },
                |_| while rx.try_recv().is_ok() {},
            );
        })
        .expect("spawn NEB pool thread");
}

/// failback / --pool-check: connect + subscribe + authorize + a usable job, nothing submitted
/// 1.0.3: the password of `mining.authorize`.  `--password` belongs to the first pool of the list (on HiveOS the Pass of the flight
/// sheet, for an ffgn gate its token) or to any ffgn gate (it says so in its subscribe answer, before the login); a backup pool
/// gets "x" (before 1.0.3 the gate token went to Suprnova / Aria while the gate was down).
pub fn auth_password(password: &str, entry: usize, is_gate: bool, supr: bool) -> String {
    if (entry == 0 || is_gate) && !password.is_empty() {
        return password.to_string();
    }
    if entry == 0 && !supr {
        return String::new();
    }
    "x".into()
}

#[cfg(test)]
mod stale_tests {
    use super::super::{DatasetFile, Opener, Shared, Work, refc, start_verifiers};
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;

    const FF: &str = "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";

    fn job_line(id: &str, height: u32, wh: &str) -> String {
        let t = super::super::now_unix();
        let z = "00".repeat(32);
        format!(
            "{}\n",
            json!({"method":"mining.notify","params":[{"job_id":id,"slot":0,"clean":true,"version":1,"parent":format!("{height:064x}"),"time_start":t,"time_window":600,"bits":0,"target":z,"share_target":FF,"coinbase_prefix":"0102","coinbase_suffix":"0304","branch":[],"tx_count":1,"shares_root":z,"height":height,"expiry":t+600,"weights_hash":wh}]})
        )
    }

    /// Rig05 (5 GPUs, an Athlon X4 840): a backlog of hits behind a slow CPU check when the pool's clean job arrives. The
    /// pool session reads the new job at once (the checks do not hold it up), not one share of the old job is sent after
    /// it, the old job's backlog is dropped and counted ("dropped stale"), never rejected; every share sent passed the check.
    #[test]
    fn a_clean_job_behind_a_slow_cpu_check_is_read_at_once_and_no_old_share_follows_it() {
        let (set, m) = refc::synthetic(refc::Arch::Kep1).unwrap();
        let wh = hex(&m.hash);
        let packed = hex(&m.packed);
        let file = Arc::new(DatasetFile { hash: [0; 32], bytes: Arc::new(Vec::new()), network: "test", sets: vec![Arc::new(set)] });
        // the mock pool: every submit recorded (job id, when), answered "true"; job B when the test says so
        let lst = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = lst.local_addr().unwrap().port();
        let (go_b, go_rx) = mpsc::channel::<()>();
        let (b_sent_tx, b_sent) = mpsc::channel::<Instant>();
        let submits: Arc<Mutex<Vec<(String, Instant)>>> = Arc::new(Mutex::new(Vec::new()));
        let subs2 = submits.clone();
        let pool = std::thread::spawn(move || {
            let (s, _) = lst.accept().unwrap();
            let w = Arc::new(Mutex::new(s.try_clone().unwrap()));
            let put = |w: &Mutex<std::net::TcpStream>, v: Value| w.lock().unwrap().write_all(format!("{v}
").as_bytes()).unwrap();
            let wb = w.clone();
            let wh2 = wh.clone();
            std::thread::spawn(move || {
                if go_rx.recv().is_ok() {
                    wb.lock().unwrap().write_all(job_line("B", 101, &wh2).as_bytes()).unwrap();
                    let _ = b_sent_tx.send(Instant::now());
                }
            });
            for line in BufReader::new(s).lines() {
                let Ok(line) = line else { break };
                let v: Value = serde_json::from_str(line.trim()).unwrap();
                let id = v["id"].as_u64().unwrap_or(0);
                match v["method"].as_str().unwrap_or("") {
                    "mining.subscribe" => put(&w, json!({"id":id,"result":{"session":"t","extranonce1":"01020304","extranonce2_size":4},"error":null})),
                    "mining.authorize" => {
                        put(&w, json!({"id":id,"result":true,"error":null}));
                        put(&w, json!({"method":"mining.set_target","params":[FF]}));
                        put(&w, json!({"method":"mining.set_model","params":[0, wh, packed]}));
                        w.lock().unwrap().write_all(job_line("A", 100, &wh).as_bytes()).unwrap();
                    }
                    "mining.submit" => {
                        subs2.lock().unwrap().push((v["params"][1].as_str().unwrap().to_string(), Instant::now()));
                        put(&w, json!({"id":id,"result":true,"error":null}));
                    }
                    _ => {}
                }
            }
        });
        let url = parse_url(&format!("127.0.0.1:{port}")).unwrap();
        let l = Login {
            name: "test",
            entry: 0,
            url: url.clone(),
            urls: vec![url],
            link: crate::failover::Link::new(1),
            follow: None,
            src: Src::User,
            login: "addr.rig05".into(),
            password: String::new(),
            fee_login: None,
            enabled: Arc::new(AtomicBool::new(true)),
            stop: Arc::new(AtomicBool::new(false)),
            dataset_hash: [0; 32],
            fee_ok: Arc::new(AtomicBool::new(false)),
            conn_seq: Arc::new(AtomicU64::new(0)),
            fee_stats: Arc::new(Stats::default()),
            kinds: Arc::new(Mutex::new(vec![Kind::Json])),
            ns: Arc::new(super::super::node::NodeStats::default()),
            ready: Arc::new(AtomicBool::new(false)),
            gate: Arc::new(AtomicBool::new(false)),
        };
        let stats = Arc::new(Stats::default());
        let (ev_tx, ev_rx) = mpsc::channel::<(Src, Ev)>();
        let (share_tx, share_rx) = mpsc::channel::<Find>();
        let lstop = l.stop.clone();
        let st2 = stats.clone();
        let sess = std::thread::spawn(move || session(&l, &ev_tx, &share_rx, &st2));
        // the miner's side: a slow CPU check (30 ms a hit, 2 threads), the checked shares straight to the session
        let (find_tx, find_rx) = mpsc::channel::<Find>();
        let open: Opener = Arc::new(|_, _, _| anyhow::bail!("no device in this test"));
        let sh = Arc::new(Shared::new(Arc::new(AtomicBool::new(false)), find_tx, file, Arc::new(Vec::new()), false, open, Duration::ZERO, Duration::from_millis(30)));
        let vs = start_verifiers(&sh, 2);
        let router = std::thread::spawn(move || {
            while let Ok(f) = find_rx.recv() {
                let _ = share_tx.send(f);
            }
        });
        let job_a = match ev_rx.recv_timeout(Duration::from_secs(10)).expect("job A") {
            (_, Ev::Job(j)) => j,
            _ => panic!("not a job"),
        };
        assert_eq!(job_a.job.height, 100);
        let gs = super::super::tests::test_gpu(0, 0);
        let w = Arc::new(Work::new(job_a.clone(), false, 1));
        // the GPUs found 40 hits of job A at once: 40 x 30 ms / 2 threads = 600 ms of CPU checks
        let hits: Vec<(u32, [u8; 32])> = (0..40u32).map(|n| (n, [0u8; 32])).collect();
        sh.enqueue(&gs, &w, 0, &hits, Instant::now());
        std::thread::sleep(Duration::from_millis(150));
        go_b.send(()).unwrap();
        let b_at = b_sent.recv_timeout(Duration::from_secs(5)).unwrap();
        let job_b = match ev_rx.recv_timeout(Duration::from_secs(5)).expect("job B") {
            (_, Ev::Job(j)) => j,
            _ => panic!("not a job"),
        };
        let read_ms = b_at.elapsed().as_millis();
        let lag = job_b.received.saturating_duration_since(b_at);
        assert_eq!(job_b.job.height, 101);
        assert!(lag < Duration::from_millis(60), "the clean job was read {} ms after the pool sent it (a check backlog in front of it)", lag.as_millis());
        assert!(job_a.voided() && !job_b.voided(), "the clean job voided job A at once");
        // the backlog: what is left of job A is dropped before (or after) its check, never sent
        let t = Instant::now();
        while (sh.checking.load(Ordering::SeqCst) > 0 || !sh.checkq.lock().unwrap().is_empty()) && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(5));
        }
        std::thread::sleep(Duration::from_millis(200));
        lstop.store(true, Ordering::SeqCst);
        sh.stop.store(true, Ordering::SeqCst);
        for h in vs {
            h.join().unwrap();
        }
        let _ = sess.join();
        pool.join().unwrap();
        let subs = submits.lock().unwrap().clone();
        let sent_a = subs.iter().filter(|s| s.0 == "A").count() as u64;
        let late_a: Vec<u128> = subs.iter().filter(|s| s.0 == "A" && s.1 > b_at + Duration::from_millis(30)).map(|s| s.1.duration_since(b_at).as_millis()).collect();
        assert!(late_a.is_empty(), "job A shares reached the pool after its clean job B: {late_a:?} ms after B");
        assert!(sent_a > 0, "the checks before job B went out");
        let dropped = sh.dropped[0].load(Ordering::Relaxed) + stats.stale_drop.load(Ordering::Relaxed);
        assert!(dropped >= 20, "most of the 600 ms backlog was dropped, not sent: dropped {dropped}, sent {sent_a}");
        assert_eq!(sent_a + dropped, 40, "every hit is sent once or dropped once (sent {sent_a}, dropped {dropped})");
        assert_eq!(stats.rejected.load(Ordering::Relaxed), 0, "no stale reject");
        assert_eq!(stats.submitted.load(Ordering::Relaxed), sent_a);
        assert_eq!(gs.bad.load(Ordering::Relaxed), 0);
        assert!(sh.shares.load(Ordering::Relaxed) >= sent_a, "every share sent passed the CPU check");
        drop(router);
        println!("job B read {read_ms} ms after the pool sent it ({} ms socket -> session); job A: {sent_a} sent before B, {dropped} dropped", lag.as_millis());
    }

    /// Rig01 (8x RTX 3070): rejects in bursts on a job the pool had dropped before its new job reached us. The first
    /// "job not found" voids the job: its next shares are dropped (not sent, not rejected); no new job within
    /// GONE_RECONNECT -> a new connection.
    #[test]
    fn job_not_found_voids_the_job_and_no_new_job_reconnects() {
        let (_set, m) = refc::synthetic(refc::Arch::Kep1).unwrap();
        let (wh, packed) = (hex(&m.hash), hex(&m.packed));
        let lst = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = lst.local_addr().unwrap().port();
        let submits = Arc::new(AtomicU64::new(0));
        let sub2 = submits.clone();
        let pool = std::thread::spawn(move || {
            let (s, _) = lst.accept().unwrap();
            let mut w = s.try_clone().unwrap();
            for line in BufReader::new(s).lines() {
                let Ok(line) = line else { break };
                let v: Value = serde_json::from_str(line.trim()).unwrap();
                let id = v["id"].as_u64().unwrap_or(0);
                let out = match v["method"].as_str().unwrap_or("") {
                    "mining.subscribe" => format!("{}
", json!({"id":id,"result":{"session":"t","extranonce1":"01020304","extranonce2_size":4},"error":null})),
                    "mining.authorize" => format!(
                        "{}
{}
{}
{}",
                        json!({"id":id,"result":true,"error":null}),
                        json!({"method":"mining.set_target","params":[FF]}),
                        json!({"method":"mining.set_model","params":[0, wh, packed]}),
                        job_line("A", 100, &wh)
                    ),
                    "mining.submit" => {
                        sub2.fetch_add(1, Ordering::SeqCst);
                        format!("{}
", json!({"id":id,"result":null,"error":[21,"job not found"]}))
                    }
                    _ => String::new(),
                };
                if w.write_all(out.as_bytes()).is_err() {
                    break;
                }
            }
        });
        let url = parse_url(&format!("127.0.0.1:{port}")).unwrap();
        let l = Login {
            name: "test",
            entry: 0,
            url: url.clone(),
            urls: vec![url],
            link: crate::failover::Link::new(1),
            follow: None,
            src: Src::User,
            login: "addr.rig01".into(),
            password: String::new(),
            fee_login: None,
            enabled: Arc::new(AtomicBool::new(true)),
            stop: Arc::new(AtomicBool::new(false)),
            dataset_hash: [0; 32],
            fee_ok: Arc::new(AtomicBool::new(false)),
            conn_seq: Arc::new(AtomicU64::new(0)),
            fee_stats: Arc::new(Stats::default()),
            kinds: Arc::new(Mutex::new(vec![Kind::Json])),
            ns: Arc::new(super::super::node::NodeStats::default()),
            ready: Arc::new(AtomicBool::new(false)),
            gate: Arc::new(AtomicBool::new(false)),
        };
        let stats = Arc::new(Stats::default());
        let (ev_tx, ev_rx) = mpsc::channel::<(Src, Ev)>();
        let (share_tx, share_rx) = mpsc::channel::<Find>();
        let st2 = stats.clone();
        let t0 = Instant::now();
        let sess = std::thread::spawn(move || session(&l, &ev_tx, &share_rx, &st2));
        let a = match ev_rx.recv_timeout(Duration::from_secs(10)).expect("job A") {
            (_, Ev::Job(j)) => j,
            _ => panic!("not a job"),
        };
        let find = |n: u32| Find { job: a.clone(), extranonce: vec![1, 2, 3, 4, 0, 0, 0, 1], time: a.job.time_start, nonce: n, ticket: [0; 32], share: true, block: false, fee: false, found: Instant::now(), checked: Instant::now() };
        share_tx.send(find(1)).unwrap();
        let t = Instant::now();
        while !a.voided() && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(a.voided(), "the pool's \"job not found\" voids the job");
        for n in 2..5 {
            share_tx.send(find(n)).unwrap();
        }
        let r = sess.join().unwrap();
        assert!(r.as_ref().is_err_and(|e| e.to_string().contains("sent no new one")), "{r:?}");
        assert!(t0.elapsed() < Duration::from_secs(8));
        pool.join().unwrap();
        assert_eq!(submits.load(Ordering::SeqCst), 1, "only the first share of the dropped job went out");
        assert_eq!(stats.rejected.load(Ordering::Relaxed), 1);
        assert_eq!(stats.stale_drop.load(Ordering::Relaxed), 3, "the next 3 were dropped (dropped stale), not rejected");
    }

    /// A clean job voids every other job of the connection once; the end of the connection voids the rest.
    #[test]
    fn a_clean_job_voids_the_other_jobs_once() {
        let mk = |id: &str| {
            let mut j = Arc::try_unwrap(super::super::tests::njob(4, None)).ok().unwrap();
            j.origin = Origin::Pool { job_id: id.into() };
            Arc::new(j)
        };
        let (a, b, c) = (mk("a"), mk("b"), mk("c"));
        let mut is = Issued(vec![a.clone(), b.clone()]);
        assert_eq!(is.void_except("b"), 1);
        assert!(a.voided() && !b.voided());
        assert_eq!(is.void_except("b"), 0, "voided once");
        is.0.push(c.clone());
        drop(is);
        assert!(b.voided() && c.voided(), "the connection's end voids its jobs");
    }

    /// 1.0.14 (Shahtyor 21.11 22:25: 13 shares of an older block's job sent 2 s after the new block, all "job not found"):
    /// Suprnova sends a new block as clean=false; a job of a higher height voids the lower heights, the same height stays.
    #[test]
    fn a_new_block_without_the_clean_flag_voids_older_heights_only() {
        let mk = |id: &str, h: u32| {
            let mut j = Arc::try_unwrap(super::super::tests::njob(4, None)).ok().unwrap();
            j.origin = Origin::Pool { job_id: id.into() };
            j.job.height = h;
            Arc::new(j)
        };
        let (a, b, c) = (mk("a", 4779), mk("b", 4780), mk("c", 4780));
        let mut is = Issued(vec![a.clone(), b.clone(), c.clone()]);
        assert_eq!(is.void_below(4780), 1);
        assert!(a.voided() && !b.voided() && !c.voided(), "same height stays valid");
        assert_eq!(is.void_below(4780), 0, "voided once");
        assert_eq!(is.void_below(4779), 0, "an older job arriving late voids nothing");
        assert_eq!(is.void_below(4781), 2);
        std::mem::forget(is);
    }
}

#[cfg(test)]
mod late_tests {
    use super::*;

    /// The Rig05 pattern: answers 2-34 s late in bursts: one WARNING, then quiet for 10 min; answers on time never warn.
    #[test]
    fn late_pool_answers_warn_once_per_ten_minutes() {
        let mut l = LateAnswers::default();
        let t = Instant::now();
        for i in 0..50 {
            assert!(l.note(t + Duration::from_secs(i), Duration::from_millis(60), 0, i).is_none());
        }
        assert!(l.note(t + Duration::from_secs(60), Duration::from_secs(34), 0, 51).is_none());
        assert!(l.note(t + Duration::from_secs(61), Duration::from_secs(20), 0, 52).is_none());
        let w = l.note(t + Duration::from_secs(62), Duration::from_secs(3), 5, 53).expect("the third late answer warns");
        assert!(w.contains("median 20.0 s") && w.contains("stale so far 5 of 53") && w.contains("transmit queue"), "{w}");
        for i in 0..20 {
            assert!(l.note(t + Duration::from_secs(70 + i), Duration::from_secs(5), 5, 60).is_none(), "once per 10 min");
        }
        assert!(l.note(t + Duration::from_secs(700), Duration::from_secs(5), 9, 90).is_none(), "the old ones fell out of the 5 min window");
        assert!(l.note(t + Duration::from_secs(701), Duration::from_secs(5), 9, 91).is_none());
        assert!(l.note(t + Duration::from_secs(702), Duration::from_secs(5), 9, 92).is_some());
    }
}

#[cfg(test)]
mod pw_tests {
    use super::auth_password;
    #[test]
    fn the_password_goes_to_the_first_pool_or_a_gate_only() {
        // HiveOS: GATE first with the token, then Suprnova and Aria
        assert_eq!(auth_password("TOKEN", 0, true, false), "TOKEN");
        assert_eq!(auth_password("TOKEN", 1, false, true), "x");
        assert_eq!(auth_password("TOKEN", 2, false, false), "x");
        // a gate further down the list still gets it (it identifies itself before the login)
        assert_eq!(auth_password("TOKEN", 2, true, false), "TOKEN");
        // a pool password for the first pool; "x" / empty as before
        assert_eq!(auth_password("d=1024", 0, false, true), "d=1024");
        assert_eq!(auth_password("", 0, false, true), "x");
        assert_eq!(auth_password("", 0, false, false), "");
    }
}

pub fn probe(l: &Login, entry: usize, timeout: Duration) -> Result<(), String> {
    probe_job(l, entry, timeout, |_| Ok(()))
}

/// `probe` whose first job must also pass `accept` (1.0.3 --pool-check: a job of this dataset file's network); its Err is the verdict.
pub fn probe_job(l: &Login, entry: usize, timeout: Duration, accept: impl Fn(&NJob) -> Result<(), String>) -> Result<(), String> {
    let p = Login {
        name: "probe",
        url: l.urls[entry].clone(),
        urls: l.urls.clone(),
        link: crate::failover::Link::new(1),
        follow: None,
        src: Src::User,
        login: l.login.clone(),
        password: l.password.clone(),
        fee_login: None,
        enabled: Arc::new(AtomicBool::new(true)),
        stop: Arc::new(AtomicBool::new(false)),
        dataset_hash: l.dataset_hash,
        fee_ok: Arc::new(AtomicBool::new(false)),
        conn_seq: Arc::new(AtomicU64::new(1 << 40)),
        fee_stats: Arc::new(Stats::default()),
        kinds: l.kinds.clone(),
        ns: Arc::new(super::node::NodeStats::default()),
        ready: Arc::new(AtomicBool::new(false)),
        gate: Arc::new(AtomicBool::new(false)),
        entry,
    };
    let wrong: std::cell::RefCell<Option<String>> = std::cell::RefCell::new(None);
    let r = crate::failover::probe_session::<Ev, Find>(
        timeout,
        move |en, tx, rx| {
            let p = Login { enabled: en, ..p };
            session(&p, &tx, &rx, &Stats::default())
        },
        |e| match e {
            Ev::Job(j) => {
                if let Err(m) = accept(j) {
                    wrong.borrow_mut().get_or_insert(m);
                }
                true
            }
            _ => false,
        },
    );
    match wrong.into_inner() {
        Some(m) => Err(m),
        None => r,
    }
}

fn err(s: impl Into<String>) -> std::io::Error {
    std::io::Error::other(s.into())
}

fn short(line: &str) -> String {
    if line.len() > 300 { format!("{}...", &line[..300]) } else { line.to_string() }
}

/// a job of `mining.notify` (fields of spec/pool.md section 4)
fn parse_job(j: &Value, en_size: usize) -> Result<(String, Job, u8, [u8; 32]), String> {
    let s = |k: &str| j.get(k).and_then(Value::as_str).ok_or_else(|| format!("{k} missing"));
    let u = |k: &str| j.get(k).and_then(Value::as_u64).ok_or_else(|| format!("{k} missing")).and_then(|v| u32::try_from(v).map_err(|_| format!("{k} too large")));
    let b32 = |k: &str| s(k).and_then(|v| unhex32(v).ok_or_else(|| format!("{k} is not 32 bytes of hex")));
    let by = |k: &str| s(k).and_then(|v| unhex(v).ok_or_else(|| format!("{k} is not hex")));
    let branch: Vec<[u8; 32]> = j
        .get("branch")
        .and_then(Value::as_array)
        .ok_or("branch missing")?
        .iter()
        .map(|x| x.as_str().and_then(unhex32).ok_or_else(|| "branch entry is not 32 bytes".to_string()))
        .collect::<Result<_, _>>()?;
    if branch.len() > 4096 {
        return Err("branch too long".into());
    }
    let job_id = s("job_id")?.to_string();
    let slot = u("slot")?;
    let job = Job {
        id: 0,
        clean: j.get("clean").and_then(Value::as_bool).unwrap_or(false),
        version: u("version")?,
        parent: b32("parent")?,
        time_start: u("time_start")?,
        time_window: u("time_window")?,
        bits: u("bits")?,
        target: b32("target")?,
        share_target: b32("share_target")?,
        coinbase_prefix: by("coinbase_prefix")?,
        coinbase_suffix: by("coinbase_suffix")?,
        extranonce_size: en_size.min(255) as u8,
        branch,
        tx_count: u("tx_count")?,
        shares_root: b32("shares_root")?,
        height: u("height")?,
        expiry: u("expiry")?,
    };
    Ok((job_id, job, u8::try_from(slot).map_err(|_| "slot too large")?, b32("weights_hash")?))
}

/// stale-share fix: the jobs a pool connection gave out (the newest 16).
struct Issued(Vec<Arc<NJob>>);

impl Issued {
    /// a clean job: every other job of the connection is void; how many were voided now
    fn void_except(&mut self, job_id: &str) -> usize {
        let mut n = 0;
        for j in &self.0 {
            let same = matches!(&j.origin, Origin::Pool { job_id: x } if x == job_id);
            if !same && !j.obsolete.swap(true, Ordering::SeqCst) {
                n += 1;
            }
        }
        n
    }
}

impl Issued {
    /// 1.0.14: a job of a NEWER block height voids the jobs of older heights even when the pool marks it `clean=false`
    /// (Suprnova sends every new block that way): their shares only come back as "job not found". Jobs of the same height
    /// stay valid. How many were voided now.
    fn void_below(&mut self, height: u32) -> usize {
        let mut n = 0;
        for j in &self.0 {
            if j.job.height < height && !j.obsolete.swap(true, Ordering::SeqCst) {
                n += 1;
            }
        }
        n
    }
}

impl Drop for Issued {
    /// the connection ended: its jobs are void (a new connection has a new extranonce1 and new job ids)
    fn drop(&mut self) {
        if self.void_except("") > 0 {
            super::jobs_voided();
        }
    }
}

fn session(l: &Login, tx: &Sender<(Src, Ev)>, rx: &Receiver<Find>, stats: &Stats) -> std::io::Result<()> {
    let kind = l.kinds.lock().unwrap().get(l.entry).copied().unwrap_or(Kind::Auto);
    if kind == Kind::Work || kind == Kind::TryWork {
        let nl = super::node::NodeLogin {
            addrs: vec![],
            addr: format!("{}:{}", l.url.host, l.url.port),
            link: l.link.clone(),
            stop: l.stop.clone(),
            enabled: l.enabled.clone(),
            name: l.login.clone(),
            src: l.src,
            pool: true,
            label: l.name.to_string(),
            ready: l.ready.clone(),
            conn_seq: l.conn_seq.clone(),
            held: Arc::new(Mutex::new(Vec::new())),
        };
        // the fee login of a work-protocol pool is a connection of its own (fee session), not a second login here
        l.fee_ok.store(l.fee_login.is_none() && l.src == Src::Fee, Ordering::Relaxed);
        let r = super::node::session(&nl, tx, rx, stats, &l.ns);
        if kind == Kind::TryWork {
            let got = l.ready.load(Ordering::Relaxed);
            if let Some(k) = l.kinds.lock().unwrap().get_mut(l.entry) {
                *k = if got { Kind::Work } else { Kind::Auto };
            }
        }
        l.ready.store(false, Ordering::Relaxed);
        return r;
    }
    eprintln!("[pool:{}] connecting to {} as {}", l.name, l.url, l.login);
    let mut c: Conn = connect(&l.url)?;
    let conn = l.conn_seq.fetch_add(1, Ordering::Relaxed) + 1;
    stats.connected.store(1, Ordering::Relaxed);
    let agent = format!("{}/{}", crate::PRODUCT.to_ascii_lowercase(), env!("CARGO_PKG_VERSION"));
    c.send(format!("{}\n", json!({"id":1,"method":"mining.subscribe","params":[agent, PROTOCOL]})).as_bytes())?;
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = vec![0u8; 1 << 16];
    let mut next_id = 10u64;
    let mut en1: Option<Vec<u8>> = None;
    let mut en2_size = 0usize;
    let mut range: Option<(u32, u64)> = None;
    let mut target: Option<[u8; 32]> = None;
    let mut models: HashMap<[u8; 32], (u8, Arc<Model>)> = HashMap::new();
    let mut model_order: Vec<[u8; 32]> = Vec::new();
    // id -> (fee login?, job id, sent)
    let mut pending: HashMap<u64, (bool, String, Instant)> = HashMap::new();
    let mut auth_ids: HashMap<u64, bool> = HashMap::new(); // id -> fee login
    let mut user_ok = false;
    let mut valid: Vec<String> = Vec::new();
    let mut sil = crate::silence::Silence::new(SILENCE_LIMIT);
    let mut drain = crate::failover::Drain::new();
    let fee_stats = &l.fee_stats;
    let mut raw_shown = 0;
    let mut supr = false;
    let mut is_gate = false;
    let mut rate_win = (Instant::now(), 0u32);
    let started = Instant::now();
    let mut answered = false;
    let mut late = LateAnswers::default();
    // stale-share fix: the jobs this connection gave out; a clean job voids the others (NJob::obsolete), so do the end of
    // this connection
    let mut issued = Issued(Vec::new());
    // stale-share fix: since when the pool's "job not found" voided the newest job (no new job since)
    let mut gone_since: Option<Instant> = None;
    loop {
        if gone_since.is_some_and(|t| t.elapsed() > GONE_RECONNECT) {
            return Err(err(format!("the pool dropped its newest job (\"job not found\") and sent no new one in {} s: reconnecting", GONE_RECONNECT.as_secs())));
        }
        if kind == Kind::Auto && !answered && started.elapsed() > Duration::from_secs(6) {
            l.kinds.lock().unwrap()[l.entry] = Kind::TryWork;
            return Err(err("no answer to mining.subscribe in 6 s: the next connection tries the NEB work protocol (as Aria)"));
        }
        if l.stop.load(Ordering::Relaxed) || !l.enabled.load(Ordering::Relaxed) {
            return Ok(());
        }
        if let Some(idle) = sil.expired() {
            return Err(crate::silence::Silence::error(idle));
        }
        if pending.len() > 256 {
            pending.retain(|_, (_, _, t)| t.elapsed() < Duration::from_secs(120));
        }
        if drain.check(&l.link, pending.is_empty()) {
            return Ok(());
        }
        match c.recv(&mut chunk)? {
            Some(0) => {
                if kind == Kind::Auto && !answered {
                    l.kinds.lock().unwrap()[l.entry] = Kind::TryWork;
                    return Err(err("the pool closed the connection without a JSON answer: the next connection tries the NEB work protocol"));
                }
                return Err(err("the pool closed the connection"));
            }
            Some(n) => {
                buf.extend_from_slice(&chunk[..n]);
                sil.rx();
                // stale-share fix: a full chunk = more is waiting (a model + the job): read it before anything is sent
                let mut more = n == chunk.len();
                while more && buf.len() < 262_144 * 2 {
                    match c.recv(&mut chunk)? {
                        Some(0) => return Err(err("the pool closed the connection")),
                        Some(k) => {
                            buf.extend_from_slice(&chunk[..k]);
                            more = k == chunk.len();
                        }
                        None => more = false,
                    }
                }
            }
            None => {}
        }
        if buf.len() > 262_144 * 2 && !buf.contains(&b'\n') {
            return Err(err("a line from the pool is longer than 256 KiB"));
        }
        while let Some(p) = buf.iter().position(|&b| b == b'\n') {
            let raw: Vec<u8> = buf.drain(..=p).collect();
            let line = String::from_utf8_lossy(&raw);
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(line) else {
                if kind == Kind::Auto && !answered {
                    l.kinds.lock().unwrap()[l.entry] = Kind::TryWork;
                    return Err(err("the pool does not speak JSON: the next connection tries the NEB work protocol"));
                }
                eprintln!("[pool:{}] unparsable: {}", l.name, short(line));
                continue;
            };
            if !answered {
                answered = true;
                if kind == Kind::Auto {
                    l.kinds.lock().unwrap()[l.entry] = Kind::Json;
                }
            }
            let method = v.get("method").and_then(Value::as_str).unwrap_or("");
            let params = v.get("params").cloned().unwrap_or(Value::Null);
            match method {
                "mining.set_nonce_range" => {
                    // ffgn gate: this session's range of the reference header's nonces [base, log2 span]
                    range = params.as_array().and_then(|a| Some((a.first()?.as_u64()? as u32, 1u64 << a.get(1)?.as_u64()?.min(32))));
                    if let Some((b, sp)) = range {
                        eprintln!("[pool:{}] gate nonce range {b:#010x} + {sp}", l.name);
                    }
                }
                "mining.set_difficulty" => match params.get(0).and_then(Value::as_f64) {
                    Some(d) if d > 0.0 => target = Some(super::supr::target_of_difficulty(d)),
                    _ => eprintln!("[pool:{}] set_difficulty not understood: {}", l.name, short(line)),
                },
                "mining.set_target" => {
                    target = params.get(0).and_then(Value::as_str).and_then(unhex32);
                    if target.is_none() {
                        eprintln!("[pool:{}] set_target not understood: {}", l.name, short(line));
                    }
                }
                "mining.set_model" => {
                    match super::supr::parse_model(&params) {
                        Ok((s, h, w)) => match Model::from_packed(&w, &h) {
                            Ok(m) => {
                                models.insert(h, (s as u8, Arc::new(m)));
                                model_order.retain(|x| x != &h);
                                model_order.push(h);
                                while model_order.len() > 8 {
                                    let o = model_order.remove(0);
                                    models.remove(&o);
                                }
                            }
                            Err(e) => eprintln!("[pool:{}] model refused: {e}", l.name),
                        },
                        Err(e) => eprintln!("[pool:{}] set_model not understood: {e}", l.name),
                    }
                }
                "mining.notify" => {
                    if raw_shown < 1 {
                        raw_shown += 1;
                        eprintln!("[pool:{}] first job: {}", l.name, short(line));
                    }
                    let Some(e1) = en1.clone() else {
                        eprintln!("[pool:{}] job before the subscription result: ignored", l.name);
                        continue;
                    };
                    let parsed = if supr || params.get(0).is_some_and(Value::is_string) {
                        super::supr::parse_notify(&params, e1.len() + en2_size, 0).map(|sj| {
                            let mut job = sj.job;
                            job.target = super::supr::compact_target(job.bits);
                            // share target = block target x 32 (spec/shares.md 11.2)
                            job.share_target = shl5(&job.target);
                            (sj.job_id, job, sj.slot, sj.weights_hash)
                        })
                    } else {
                        parse_job(params.get(0).unwrap_or(&Value::Null), e1.len() + en2_size)
                    };
                    match parsed {
                        Ok((job_id, job, slot, wh)) => {
                            if job.clean {
                                valid.clear();
                                if issued.void_except(&job_id) > 0 {
                                    super::jobs_voided();
                                }
                            } else if issued.void_below(job.height) > 0 {
                                // 1.0.14: a new block without the clean flag (Suprnova): the older heights are void now
                                super::jobs_voided();
                            }
                            valid.retain(|x| x != &job_id);
                            valid.push(job_id.clone());
                            if valid.len() > 16 {
                                valid.remove(0);
                            }
                            let Some((mslot, m)) = models.get(&wh).cloned() else {
                                eprintln!("[pool:{}] job {job_id} names a model the pool did not send: not mined", l.name);
                                continue;
                            };
                            if mslot != slot {
                                eprintln!("[pool:{}] job {job_id}: slot {slot} but its model came for slot {mslot}: not mined", l.name);
                                continue;
                            }
                            let Some(t) = target else {
                                eprintln!("[pool:{}] job {job_id} before any set_target: not mined", l.name);
                                continue;
                            };
                            // the pool target is never below the share target (spec section 7.3): a lower one would lose chain shares
                            let pt = if proto::admits(&t, &job.share_target) { t } else { job.share_target };
                            let nj = NJob {
                                src: l.src,
                                origin: Origin::Pool { job_id },
                                slot,
                                model: m,
                                pool_target: Some(pt),
                                en1: e1.clone(),
                                en2_size,
                                nonce_range: range,
                                conn,
                                received: Instant::now(),
                        ref_next: AtomicU64::new(0),
                        ref_base: super::random_base(),
                        obsolete: AtomicBool::new(false),
                                job,
                            };
                            l.ready.store(true, Ordering::Relaxed);
                            gone_since = None;
                            let nj = Arc::new(nj);
                            issued.0.retain(|j| !j.voided());
                            issued.0.push(nj.clone());
                            if issued.0.len() > 16 {
                                issued.0.remove(0);
                            }
                            let _ = tx.send((l.src, Ev::Job(nj)));
                        }
                        Err(e) => eprintln!("[pool:{}] job not understood ({e}): {}", l.name, short(line)),
                    }
                }
                "" => {
                    let id = v.get("id").and_then(Value::as_u64).unwrap_or(0);
                    let ok = v.get("result").is_some_and(|r| r.as_bool().unwrap_or(!r.is_null())) && v.get("error").map_or(true, Value::is_null);
                    let etext = v.get("error").filter(|e| !e.is_null()).map(|e| e.to_string()).unwrap_or_default();
                    if id == 1 {
                        let mut r = v.get("result").cloned().unwrap_or(Value::Null);
                        if !ok {
                            return Err(err(format!("subscribe refused: {etext}")));
                        }
                        if let Some(a) = r.as_array().cloned() {
                            // classic stratum result (Suprnova): [[subscriptions], extranonce1, extranonce2_size]
                            supr = true;
                            r = json!({"extranonce1": a.get(1).cloned().unwrap_or(Value::Null), "extranonce2_size": a.get(2).cloned().unwrap_or(Value::Null), "session": "stratum"});
                        }
                        let e1 = r.get("extranonce1").and_then(Value::as_str).and_then(unhex).ok_or_else(|| err("subscribe: no extranonce1"))?;
                        en2_size = r.get("extranonce2_size").and_then(Value::as_u64).ok_or_else(|| err("subscribe: no extranonce2_size"))? as usize;
                        if e1.len() + en2_size == 0 || e1.len() + en2_size > 32 {
                            return Err(err(format!("subscribe: extranonce {} + {en2_size} bytes is out of range", e1.len())));
                        }
                        if let Some(dh) = r.get("dataset_hash").and_then(Value::as_str).and_then(unhex32) {
                            if dh != l.dataset_hash {
                                // F-19: a pool of another network is no use: with one pool the miner stops, with a list it fails over
                                let msg = format!(
                                    "the pool works on another dataset file (BLAKE3 {}…, ours {}…): wrong network for this pool / dataset",
                                    &hex(&dh)[..16],
                                    &hex(&l.dataset_hash)[..16]
                                );
                                if l.urls.len() == 1 && l.src == Src::User {
                                    super::fatal(&msg);
                                }
                                return Err(err(msg));
                            }
                        }
                        // an ffgn gate (the nonce range comes after the login: mining.set_nonce_range)
                        is_gate = r.get("ffgn_gate").is_some();
                        en1 = Some(e1);
                        l.gate.store(is_gate, Ordering::Relaxed);
                        eprintln!(
                            "[pool:{}] subscribed: session {} extranonce2 {} bytes{}",
                            l.name,
                            r.get("session").and_then(Value::as_str).unwrap_or("?"),
                            en2_size,
                            range.map_or(String::new(), |(b, s)| format!(", gate nonce range {b:#010x} + {s}"))
                        );
                        c.send(format!("{}\n", json!({"id":2,"method":"mining.authorize","params":[l.login, auth_password(&l.password, l.entry, is_gate, supr)]})).as_bytes())?;
                        auth_ids.insert(2, false);
                        if let Some(f) = l.fee_login.as_ref().filter(|_| !is_gate) {
                            c.send(format!("{}\n", json!({"id":3,"method":"mining.authorize","params":[f, "x"]})).as_bytes())?;
                            auth_ids.insert(3, true);
                        }
                    } else if let Some(fee) = auth_ids.remove(&id) {
                        if fee {
                            l.fee_ok.store(ok, Ordering::Relaxed);
                            if !ok {
                                eprintln!("[pool:{}] the dev-fee login was refused ({etext}): the fee slots are skipped on this pool", l.name);
                            }
                        } else if ok {
                            user_ok = true;
                            if l.fee_login.is_none() && l.src == Src::Fee {
                                l.fee_ok.store(true, Ordering::Relaxed);
                            }
                            eprintln!("[pool:{}] authorized {}", l.name, l.login);
                        } else {
                            return Err(err(format!("login {} refused: {etext}", l.login)));
                        }
                    } else if let Some((fee, job_id, sent)) = pending.remove(&id) {
                        let st = if fee { &**fee_stats } else { stats };
                        let who = if fee { " (dev fee)" } else { "" };
                        if let Some(w) = late.note(Instant::now(), sent.elapsed(), st.stale_rej.load(Ordering::Relaxed), st.submitted.load(Ordering::Relaxed)) {
                            eprintln!("[pool:{}] {w}", l.name);
                        }
                        if ok {
                            st.accepted.fetch_add(1, Ordering::Relaxed);
                            println!("[pool:{}] share ACCEPTED{who} (job {job_id}, {} ms)", l.name, sent.elapsed().as_millis());
                        } else {
                            st.rejected.fetch_add(1, Ordering::Relaxed);
                            if etext.contains("21") || etext.to_lowercase().contains("expir") || etext.to_lowercase().contains("not found") {
                                st.stale_rej.fetch_add(1, Ordering::Relaxed);
                                // stale-share fix (Rig01: rejects in bursts before the pool's new job reached us): the pool has
                                // dropped this job; it is not mined any more, its queued hits are dropped, until the next job
                                let gone = etext.starts_with("[21,") || etext.to_lowercase().contains("job not found");
                                if let Some(j) = issued.0.iter().filter(|_| gone).find(|j| matches!(&j.origin, Origin::Pool { job_id: x } if *x == job_id)) {
                                    if !j.obsolete.swap(true, Ordering::SeqCst) {
                                        let newest = issued.0.last().is_some_and(|n| Arc::ptr_eq(n, j));
                                        println!("[pool:{}] job {job_id} is gone at the pool (\"job not found\"): not mined any more{}", l.name, if newest { ", waiting for the pool's next job" } else { "" });
                                        if newest {
                                            gone_since = Some(Instant::now());
                                        }
                                        super::jobs_voided();
                                    }
                                }
                            }
                            println!("[pool:{}] share REJECTED{who} (job {job_id}): {etext}", l.name);
                        }
                        sil.answered();
                    }
                }
                _ => eprintln!("[pool:{}] ignored: {}", l.name, short(line)),
            }
        }
        // stale-share fix (Rig05): the queued shares go out AFTER what the pool sent was read: a clean job read in this
        // pass voids the old job's shares here (dropped, "dropped stale"), never sent behind the new job
        while let Ok(f) = rx.try_recv() {
            let Origin::Pool { job_id } = &f.job.origin else { continue };
            if f.job.conn != conn || drain.frozen() || !valid.contains(job_id) || f.job.voided() {
                let st = if f.fee { &**fee_stats } else { stats };
                st.stale_drop.fetch_add(1, Ordering::Relaxed);
                if super::debug_timing() {
                    println!("[timing] share job {job_id} dropped before the socket: its job was voided (found {} ms ago)", f.found.elapsed().as_millis());
                }
                continue;
            }
            // F-14: at most SUBMIT_RATE submits per second (a pool target far too easy), the rest dropped and counted
            if rate_win.0.elapsed() >= Duration::from_secs(1) {
                rate_win = (Instant::now(), 0);
            }
            rate_win.1 += 1;
            if rate_win.1 > SUBMIT_RATE && !f.block {
                stats.skipped.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            let fee = f.fee;
            let login = if fee { l.fee_login.clone().unwrap_or_else(|| l.login.clone()) } else { l.login.clone() };
            next_id += 1;
            let en2 = hex(&f.extranonce[f.job.en1.len()..]);
            let line = if supr {
                json!({"id": next_id, "method": "mining.submit", "params": [login, job_id, en2, format!("{:08x}", f.time), format!("{:08x}", f.nonce)]})
            } else {
                json!({"id": next_id, "method": "mining.submit", "params": [login, job_id, en2, f.time, f.nonce]})
            };
            pending.insert(next_id, (fee, job_id.clone(), Instant::now()));
            let st = if fee { &**fee_stats } else { stats };
            st.submitted.fetch_add(1, Ordering::Relaxed);
            if f.block {
                println!("[pool:{}] BLOCK candidate (ticket under the network target) submitted: height {} nonce {}", l.name, f.job.job.height, f.nonce);
            }
            c.send(format!("{line}\n").as_bytes())?;
            if super::debug_timing() {
                let (a, b) = (f.checked.saturating_duration_since(f.found), f.checked.elapsed());
                println!("[timing] share job {job_id}: scan -> CPU check {} ms, check -> socket {} ms, job age {} ms", a.as_millis(), b.as_millis(), f.job.received.elapsed().as_millis());
            }
        }
        let _ = user_ok;
    }
}

/// The fee login's stats live in `Login::fee_stats`; a mutex-free snapshot helper for the stats line.
#[allow(dead_code)]
pub fn snapshot(s: &Stats) -> (u64, u64, u64) {
    (s.submitted.load(Ordering::Relaxed), s.accepted.load(Ordering::Relaxed), s.rejected.load(Ordering::Relaxed))
}

/// v * 32 (saturating at 2^256 - 1), little-endian
pub fn shl5(v: &[u8; 32]) -> [u8; 32] {
    if v[31] >> 3 != 0 {
        return [0xff; 32];
    }
    let mut o = [0u8; 32];
    for i in (0..32).rev() {
        o[i] = (v[i] << 5) | if i > 0 { v[i - 1] >> 3 } else { 0 };
    }
    o
}
