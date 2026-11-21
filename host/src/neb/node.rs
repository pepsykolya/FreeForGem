// FFGN — see LICENSE
//! Solo: the work service of a NEB node (spec/node.md section 7.1).  Binary frames over TCP; the node sends a Model and a
//! Job when the miner connects, on every new best block, every 30 s without a job, and after a rejected Submit.  The miner
//! mines the reference header (all-zero extranonce, `time_start`, nonce rolled), reports nonces under the share target with
//! Share frames (no answer) and solutions with Submit (answered by Result).  The block pays the node's `--miner-lock`.
//!
//! The service listens on 127.0.0.1 without authentication: a rig without its own node reaches one through `ffgn --gate`
//! (gate.rs, neb-pool/1 with a token) or an SSH tunnel.  `--node` takes a list (failover / failback as for pools).
use super::proto::{self, Job};
use super::refc::Model;
use super::{Ev, Find, NJob, Origin};
use crate::stratum::{Src, Stats};
use std::collections::VecDeque;
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// no frame for this long (the node sends a job at least every 30 s once the network runs): the node is asked whether it lives
const SILENCE: Duration = Duration::from_secs(95);
/// 1.0.4: a node that accepts connections but sends nothing keeps the link (before the start of the network a node is silent:
/// 1.0.3 dropped the gate's link every 95 s, the rigs went to their backup pool); after a job, this much silence reconnects
/// (a new connection gets a fresh Model and Job)
const SILENCE_WORK: Duration = Duration::from_secs(300);

pub struct NodeLogin {
    pub addrs: Vec<String>,
    pub addr: String,
    pub link: Arc<crate::failover::Link>,
    pub stop: Arc<AtomicBool>,
    pub enabled: Arc<AtomicBool>,
    /// Hello name: "ffgn/<ver>" for a node; the login ADDRESS.worker for a pool that speaks the work protocol (Aria)
    pub name: String,
    pub src: Src,
    /// log prefix of a pool session ("user", "fee", ...)
    pub label: String,
    /// a pool (Aria): every Share is a pool share, counted in `Stats::submitted`; its answers (if any) counted
    pub pool: bool,
    /// set once the session has a job (the fee login's readiness)
    pub ready: Arc<AtomicBool>,
    /// numbers the connections of this login (F-23: two logins never share a number); a find carries it
    pub conn_seq: Arc<AtomicU64>,
    /// solutions not delivered yet (the connection died before / while sending): sent first on the next connection (F-08)
    pub held: Arc<Mutex<Vec<Find>>>,
}

/// Counters of the node session (besides `Stats`: submitted/accepted/rejected are blocks).
#[derive(Default)]
pub struct NodeStats {
    pub shares_sent: AtomicU64,
    pub jobs: AtomicU64,
    pub models: AtomicU64,
    pub conn: AtomicU64,
}

pub fn spawn(l: NodeLogin, tx: Sender<(Src, Ev)>, rx: Receiver<Find>, stats: Arc<Stats>, ns: Arc<NodeStats>) {
    std::thread::Builder::new()
        .name("neb-node".into())
        .spawn(move || {
            let mut l = l;
            let lp = crate::failover::Loop {
                name: "node",
                link: l.link.clone(),
                follow: None,
                stop: l.stop.clone(),
                enabled: l.enabled.clone(),
                fee: false,
                label: Box::new({
                    let a = l.addrs.clone();
                    move |e| a[e.min(a.len() - 1)].clone()
                }),
            };
            crate::failover::run(
                lp,
                &mut l,
                |l, e| l.addr = l.addrs[e].clone(),
                |l| session(l, &tx, &rx, &stats, &ns),
                |l| {
                    stats.connected.store(0, Ordering::Relaxed);
                    l.ready.store(false, Ordering::Relaxed);
                    let _ = tx.send((l.src, Ev::Down));
                    keep_blocks(l, &rx);
                },
                |l| keep_blocks(l, &rx),
            );
        })
        .expect("spawn node thread");
}

/// The queue of a dead connection: shares are dropped, solutions kept for the next connection (F-08: a node keeps its 64
/// newest jobs for every miner, so the job id of a solution stays valid across a reconnect).
pub fn keep_blocks(l: &NodeLogin, rx: &Receiver<Find>) {
    while let Ok(f) = rx.try_recv() {
        if f.block {
            l.held.lock().unwrap().push(f);
        }
    }
    let mut h = l.held.lock().unwrap();
    let n = h.len();
    if n > 16 {
        h.drain(..n - 16);
    }
}

/// One check of a node entry (failback prober): connect, a Model and a Job within the timeout.
pub fn probe(addr: &str, timeout: Duration) -> Result<(), String> {
    let mut s = connect(addr).map_err(|e| e.to_string())?;
    s.set_read_timeout(Some(Duration::from_millis(200))).map_err(|e| e.to_string())?;
    let _ = s.write_all(&proto::hello("ffgn-probe"));
    let t = Instant::now();
    let mut buf = Vec::new();
    let mut tmp = [0u8; 65536];
    let (mut model, mut job) = (false, false);
    while t.elapsed() < timeout {
        match s.read(&mut tmp) {
            Ok(0) => return Err("the node closed the connection (no --miner-lock?)".into()),
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => continue,
            Err(e) => return Err(e.to_string()),
        }
        while let Some((t, r, c)) = proto::parse_frame(&buf)? {
            model |= t == proto::MSG_MODEL;
            job |= t == proto::MSG_JOB && Job::decode(&buf[r]).is_ok();
            buf.drain(..c);
        }
        if model && job {
            return Ok(());
        }
    }
    Err(format!("no job within {} s", timeout.as_secs()))
}

pub fn connect(addr: &str) -> std::io::Result<TcpStream> {
    let mut last = None;
    for a in addr.to_socket_addrs()? {
        match TcpStream::connect_timeout(&a, Duration::from_secs(10)) {
            Ok(s) => {
                s.set_nodelay(true)?;
                let ka = socket2::TcpKeepalive::new().with_time(Duration::from_secs(30)).with_interval(Duration::from_secs(10));
                let _ = socket2::SockRef::from(&s).set_tcp_keepalive(&ka);
                return Ok(s);
            }
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| std::io::Error::new(ErrorKind::NotFound, format!("{addr} does not resolve"))))
}

pub fn session(l: &NodeLogin, tx: &Sender<(Src, Ev)>, rx: &Receiver<Find>, stats: &Stats, ns: &NodeStats) -> std::io::Result<()> {
    let tag = if l.pool { format!("pool:{}", l.label) } else { "node".to_string() };
    eprintln!("[{tag}] connecting to {} (NEB work protocol{})", l.addr, if l.pool { format!(", login {}", l.name) } else { String::new() });
    let mut s = connect(&l.addr)?;
    s.set_read_timeout(Some(Duration::from_millis(20)))?;
    s.set_write_timeout(Some(Duration::from_secs(10)))?;
    s.write_all(&proto::hello(&l.name))?;
    let conn = l.conn_seq.fetch_add(1, Ordering::Relaxed) + 1;
    ns.conn.store(conn, Ordering::Relaxed);
    stats.connected.store(1, Ordering::Relaxed);
    let mut buf: Vec<u8> = Vec::new();
    let mut tmp = vec![0u8; 1 << 16];
    let mut model: Option<(u8, Arc<Model>)> = None;
    let mut last_rx = Instant::now();
    let mut last_probe = Instant::now();
    let mut said_alive = false;
    let mut got_job = false;
    // Submits waiting for their Result (the node answers in order): (height, job id)
    let mut pending: VecDeque<(u32, u64, Instant)> = VecDeque::new();
    let mut drain = crate::failover::Drain::new();
    let held: Vec<Find> = std::mem::take(&mut *l.held.lock().unwrap());
    let mut queue: VecDeque<Find> = held.into_iter().collect();
    loop {
        if l.stop.load(Ordering::Relaxed) || !l.enabled.load(Ordering::Relaxed) {
            l.held.lock().unwrap().extend(queue.into_iter().filter(|f| f.block));
            return Ok(());
        }
        while let Ok(f) = rx.try_recv() {
            queue.push_back(f);
        }
        while let Some(f) = queue.pop_front() {
            let Origin::Node { id } = f.job.origin else { continue };
            let old = f.job.conn != conn;
            if drain.frozen() || (old && (l.pool || !f.block)) {
                // a share of an earlier connection is dropped; a node keeps the job of a solution, a pool may not
                if f.block && !l.pool {
                    l.held.lock().unwrap().push(f);
                }
                continue;
            }
            let mut out = Vec::new();
            if f.share && !old {
                out.extend_from_slice(&proto::share(id, f.nonce));
            }
            if f.block {
                out.extend_from_slice(&proto::submit(id, &f.extranonce, f.time, f.nonce));
            }
            if let Err(e) = s.write_all(&out) {
                if f.block {
                    l.held.lock().unwrap().push(f); // sent again on the next connection
                }
                l.held.lock().unwrap().extend(queue.into_iter().filter(|f| f.block));
                return Err(e);
            }
            if f.share && !old {
                ns.shares_sent.fetch_add(1, Ordering::Relaxed);
                if l.pool {
                    // F-12: a work-protocol pool (Aria) does not answer shares: a share sent counts as accepted (an answer
                    // that refuses it, if a pool sends one, takes it back)
                    stats.submitted.fetch_add(1, Ordering::Relaxed);
                    stats.accepted.fetch_add(1, Ordering::Relaxed);
                }
            }
            if f.block {
                stats.submitted.fetch_add(1, Ordering::Relaxed);
                pending.push_back((f.job.job.height, id, Instant::now()));
                println!("[{tag}] BLOCK submitted: height {} job {id} nonce {}{}", f.job.job.height, f.nonce, if old { " (found before the reconnect)" } else { "" });
            }
        }
        if drain.check(&l.link, pending.is_empty()) {
            return Ok(());
        }
        match s.read(&mut tmp) {
            Ok(0) => {
                return Err(std::io::Error::new(
                    ErrorKind::ConnectionAborted,
                    if got_job { "the node closed the connection" } else { "the node refused the miner (no job): start neb-node with --miner-lock" },
                ));
            }
            Ok(n) => {
                buf.extend_from_slice(&tmp[..n]);
                last_rx = Instant::now();
                said_alive = false;
            }
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted) => {}
            Err(e) => return Err(e),
        }
        if last_rx.elapsed() > SILENCE {
            let quiet = last_rx.elapsed().as_secs();
            if l.pool {
                return Err(std::io::Error::new(ErrorKind::TimedOut, format!("no frame from the node for {} s", SILENCE.as_secs())));
            }
            if got_job && last_rx.elapsed() > SILENCE_WORK {
                return Err(std::io::Error::new(ErrorKind::TimedOut, format!("no frame from the node for {quiet} s although it accepts connections: reconnecting for a fresh job")));
            }
            // 1.0.4: is the node alive?  A new TCP connection to it (closed at once); refused / timed out = a dead node
            if last_probe.elapsed() > SILENCE {
                last_probe = Instant::now();
                match connect(&l.addr) {
                    Ok(p) => {
                        drop(p);
                        if !said_alive {
                            said_alive = true;
                            println!(
                                "[{tag}] no frame from the node for {quiet} s, but it accepts connections: the link is kept{}",
                                if got_job { " (no new block yet)" } else { " (no job yet: the network has not started?)" }
                            );
                        }
                    }
                    Err(e) => {
                        return Err(std::io::Error::new(ErrorKind::TimedOut, format!("no frame from the node for {quiet} s and a new connection to it fails ({e})")));
                    }
                }
            }
        }
        loop {
            let (t, r, c) = match proto::parse_frame(&buf) {
                Ok(Some(x)) => x,
                Ok(None) => break,
                Err(e) => return Err(std::io::Error::new(ErrorKind::InvalidData, format!("bad frame from the node: {e}"))),
            };
            let p = &buf[r];
            match t {
                proto::MSG_MODEL => {
                    let (slot, hash, packed) = proto::split_model(p).map_err(|e| std::io::Error::new(ErrorKind::InvalidData, e))?;
                    let reuse = model.as_ref().filter(|(s0, m)| *s0 == slot && m.hash == hash).cloned();
                    model = Some(match reuse {
                        Some(m) => m,
                        None => {
                            let m = Model::from_packed(packed, &hash).map_err(|e| std::io::Error::new(ErrorKind::InvalidData, format!("model frame: {e}")))?;
                            ns.models.fetch_add(1, Ordering::Relaxed);
                            (slot, Arc::new(m))
                        }
                    });
                }
                proto::MSG_JOB => {
                    let job = Job::decode(p).map_err(|e| std::io::Error::new(ErrorKind::InvalidData, format!("job frame: {e}")))?;
                    let Some((slot, m)) = model.clone() else {
                        return Err(std::io::Error::new(ErrorKind::InvalidData, "a job without a model frame"));
                    };
                    got_job = true;
                    ns.jobs.fetch_add(1, Ordering::Relaxed);
                    l.ready.store(true, Ordering::Relaxed);
                    let en = vec![0u8; job.extranonce_size as usize];
                    let nj = NJob {
                        src: l.src,
                        origin: Origin::Node { id: job.id },
                        slot,
                        model: m,
                        pool_target: None,
                        en1: en,
                        en2_size: 0,
                        nonce_range: None,
                        conn,
                        received: Instant::now(),
                        ref_next: AtomicU64::new(0),
                        ref_base: super::random_base(),
                        job,
                    };
                    let _ = tx.send((l.src, Ev::Job(Arc::new(nj))));
                }
                proto::MSG_RESULT => {
                    let (ok, code) = proto::decode_result(p).map_err(|e| std::io::Error::new(ErrorKind::InvalidData, e))?;
                    if pending.is_empty() && l.pool {
                        // a pool's answer to a share (counted as accepted when it was sent)
                        if !ok {
                            stats.accepted.fetch_sub(1, Ordering::Relaxed);
                            stats.rejected.fetch_add(1, Ordering::Relaxed);
                            println!("[{tag}] share REJECTED: code {code} {}", proto::result_text(code));
                        }
                        buf.drain(..c);
                        continue;
                    }
                    let (h, id, at) = pending.pop_front().unwrap_or((0, 0, Instant::now()));
                    if ok {
                        stats.accepted.fetch_add(1, Ordering::Relaxed);
                        println!("[{tag}] BLOCK ACCEPTED by the node: height {h} (job {id}, {} ms)", at.elapsed().as_millis());
                    } else {
                        stats.rejected.fetch_add(1, Ordering::Relaxed);
                        if code == 1 || code == 3 {
                            stats.stale_rej.fetch_add(1, Ordering::Relaxed);
                        }
                        println!("[{tag}] block REJECTED: height {h} (job {id}): code {code} {}", proto::result_text(code));
                    }
                }
                _ => {}
            }
            buf.drain(..c);
        }
    }
}
