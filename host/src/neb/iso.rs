// FFGN — see LICENSE
//! 1.0.11 (ISO): one engine process per GPU, so a CUDA fault on one GPU costs only that GPU a few seconds.
//!
//! Why (CRASH-12gpu, 12x RTX 2060 SUPER, 21.11.2026): with every GPU in ONE process, the sticky CUDA error of one card
//! (Xid 31 / 43, "an illegal memory access") ends CUDA for every GPU of the process: all 12 workers stopped, the miner
//! restarted, 60-80 s lost on every card, 3 times in 35 min for one marginal card.  A CUDA context with a sticky error
//! cannot be reopened inside the process (the retries fail with `cudaStreamCreate failed`): only the process exit frees it.
//!
//! Design: the miner process keeps everything it had (pool and fee connections, jobs, the CPU check of every hit, the
//! audits, stats / API / HiveOS, auto-uv, auto-lock, the fault records); only the engine calls of a GPU (the `Scanner`
//! trait of gpu.rs) run in a child process of that GPU: `ffgn --neb-engine DEV`, the same binary.  The worker thread of the
//! GPU stays in the miner and talks to its child over a loopback TCP socket (127.0.0.1, a random port, a random token: only
//! the child the miner started is accepted; works the same on Windows and Linux, and the engine's own output to stdout
//! cannot mix with the frames).  When the child dies (a CUDA fault, a crash of the engine) or stops answering (a hung GPU:
//! it is killed), the worker's call fails, the worker drops the child and starts a new one: only this GPU restarts.
//!
//! Frames: u32 length (LE, op byte included), u8 op, body.  Requests: OPEN, SET_MODEL, SCAN, EVAL, SELFTEST, BENCH,
//! INJECT, and CANCEL (answered by the child's reader thread at once: nebgpu_cancel, no reply).  Replies: OK (the device's
//! status, then the call's result) or ERR (the engine's error text).  The status (evaluated attempts, KEP1 kernel, the
//! path of each slot) comes with every reply, so the worker's `evaluated()` / `kep1_kernel()` / `path()` cost no round trip.
use crate::neb::gpu::{self, Backend, Canceller, Path, ScanOut, Scanner};
use crate::neb::refc::{self, DatasetFile, Model};
use anyhow::{Result, anyhow, bail};
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak, mpsc};
use std::time::{Duration, Instant};

/// The hidden command line flag of a child: `ffgn --neb-engine DEV`.
pub const ENGINE_FLAG: &str = "--neb-engine";
const ENV_PORT: &str = "FFGN_ENGINE_PORT";
const ENV_TOKEN: &str = "FFGN_ENGINE_TOKEN";

const OP_OPEN: u8 = 1;
const OP_SET_MODEL: u8 = 2;
const OP_SCAN: u8 = 3;
const OP_EVAL: u8 = 4;
const OP_SELFTEST: u8 = 5;
const OP_BENCH: u8 = 6;
const OP_CANCEL: u8 = 7;
const OP_INJECT: u8 = 8;
const OP_HELLO: u8 = 0x7f;
const R_OK: u8 = 0x80;
const R_ERR: u8 = 0x81;
const MAX_FRAME: usize = 1 << 30;

/// How long a call may take before the child is taken for hung and killed.  A scan call is sized to 0.1-0.5 s (mod.rs
/// call_size); a model upload builds the digest table (~1 s on a 2060 SUPER) after the engine's full-path trial.
const T_OPEN: Duration = Duration::from_secs(300);
const T_MODEL: Duration = Duration::from_secs(180);
const T_SCAN: Duration = Duration::from_secs(30);
const T_EVAL: Duration = Duration::from_secs(60);
const T_CONNECT: Duration = Duration::from_secs(60);

static ON: AtomicBool = AtomicBool::new(false);

/// The miner mines with one engine process per GPU (set once by `mine`).
pub fn enabled() -> bool {
    ON.load(Ordering::Relaxed)
}

pub fn set_enabled(on: bool) {
    ON.store(on, Ordering::Relaxed);
}

/// `--single-process` or FFGN_SINGLE_PROCESS=1: every GPU in the miner process (as before 1.0.11).
pub fn wanted(single_process_flag: bool, cuda: bool) -> bool {
    // 1.8.0: the per-coin capability of the registry (coins.rs, owner Q5: on for NEB)
    cuda && crate::coins::isolated(crate::coins::Coin::Neb, single_process_flag)
}

// ---------------------------------------------------------------------------------------------------------------------
// frames
// ---------------------------------------------------------------------------------------------------------------------

fn write_frame(w: &mut impl Write, op: u8, body: &[u8]) -> std::io::Result<()> {
    let mut b = Vec::with_capacity(5 + body.len());
    b.extend_from_slice(&((body.len() + 1) as u32).to_le_bytes());
    b.push(op);
    b.extend_from_slice(body);
    w.write_all(&b)?;
    w.flush()
}

fn read_frame(r: &mut impl Read) -> std::io::Result<(u8, Vec<u8>)> {
    let mut l = [0u8; 4];
    r.read_exact(&mut l)?;
    let n = u32::from_le_bytes(l) as usize;
    if n == 0 || n > MAX_FRAME {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("bad frame length {n}")));
    }
    let mut op = [0u8; 1];
    r.read_exact(&mut op)?;
    let mut b = vec![0u8; n - 1];
    r.read_exact(&mut b)?;
    Ok((op[0], b))
}

#[derive(Default)]
struct Enc(Vec<u8>);

impl Enc {
    fn u8(&mut self, v: u8) -> &mut Self {
        self.0.push(v);
        self
    }
    fn u32(&mut self, v: u32) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn i32(&mut self, v: i32) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn u64(&mut self, v: u64) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn f64(&mut self, v: f64) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn raw(&mut self, v: &[u8]) -> &mut Self {
        self.0.extend_from_slice(v);
        self
    }
    fn str(&mut self, v: &str) -> &mut Self {
        self.u32(v.len() as u32).raw(v.as_bytes())
    }
}

struct Dec<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Dec<'a> {
    fn new(b: &'a [u8]) -> Dec<'a> {
        Dec { b, at: 0 }
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.b.len() - self.at < n {
            bail!("engine process: short frame");
        }
        let s = &self.b[self.at..self.at + n];
        self.at += n;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn f64(&mut self) -> Result<f64> {
        Ok(f64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn arr<const N: usize>(&mut self) -> Result<[u8; N]> {
        Ok(self.take(N)?.try_into().unwrap())
    }
    fn str(&mut self) -> Result<String> {
        let n = self.u32()? as usize;
        Ok(String::from_utf8_lossy(self.take(n)?).into_owned())
    }
    fn rest(&mut self) -> &'a [u8] {
        let s = &self.b[self.at..];
        self.at = self.b.len();
        s
    }
}

/// What the worker reads from its device between calls (`Scanner::evaluated`, `kep1_kernel`, `path`).
#[derive(Clone, Debug, Default, PartialEq)]
struct Status {
    evaluated: Option<u64>,
    kep1: Option<String>,
    paths: Vec<Option<(Path, String)>>,
}

fn put_status(e: &mut Enc, s: &dyn Scanner, nsets: usize) {
    match s.evaluated() {
        Some(v) => e.u8(1).u64(v),
        None => e.u8(0),
    };
    match s.kep1_kernel() {
        Some(k) => e.u8(1).str(&k),
        None => e.u8(0),
    };
    e.u32(nsets as u32);
    for ds in 0..nsets {
        match s.path(ds) {
            Some((p, t)) => e.u8(match p {
                Path::Plain => 0,
                Path::Cheap => 1,
                Path::Table => 2,
            })
            .str(&t),
            None => e.u8(255),
        };
    }
}

fn get_status(d: &mut Dec) -> Result<Status> {
    let mut s = Status::default();
    if d.u8()? == 1 {
        s.evaluated = Some(d.u64()?);
    }
    if d.u8()? == 1 {
        s.kep1 = Some(d.str()?);
    }
    let n = d.u32()? as usize;
    for _ in 0..n.min(64) {
        let p = match d.u8()? {
            0 => Some(Path::Plain),
            1 => Some(Path::Cheap),
            2 => Some(Path::Table),
            _ => None,
        };
        s.paths.push(match p {
            Some(p) => Some((p, d.str()?)),
            None => None,
        });
    }
    Ok(s)
}

// ---------------------------------------------------------------------------------------------------------------------
// the child: `ffgn --neb-engine DEV`
// ---------------------------------------------------------------------------------------------------------------------

/// The main of an engine process: connects back to the miner, then serves its calls until the miner closes the socket.
pub fn child_main(dev: i32) -> i32 {
    // Ctrl+C in the console / terminal reaches every process of the group: the miner shuts down and closes the socket; the
    // child ending first would look like a crash of its GPU
    let _ = ctrlc::set_handler(|| {});
    let (Some(port), Ok(token)) = (std::env::var(ENV_PORT).ok().and_then(|p| p.parse::<u16>().ok()), std::env::var(ENV_TOKEN)) else {
        eprintln!("{ENGINE_FLAG} is started by the miner itself (one engine process per GPU), not by hand");
        return 2;
    };
    let mut s = match TcpStream::connect(("127.0.0.1", port)) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("GPU{dev} engine process: cannot reach the miner on 127.0.0.1:{port}: {e}");
            return 3;
        }
    };
    let _ = s.set_nodelay(true);
    if write_frame(&mut s, OP_HELLO, token.as_bytes()).is_err() {
        return 3;
    }
    serve(s, true, None)
}

enum After {
    Reply(Vec<u8>),
    /// the reply, then this thread stops answering (FFG_TEST_FAULT_KIND=hang)
    Hang(Vec<u8>),
    /// no reply: the process ends (FFG_TEST_FAULT_KIND=exit)
    Exit,
}

/// Serves the calls of one device on `stream`.  `process`: this is a child process (it exits when the miner is gone, and an
/// `exit` test fault ends the process); else a thread of the tests (the socket is closed instead).
fn serve(stream: TcpStream, process: bool, local: Option<Arc<DatasetFile>>) -> i32 {
    let canc: Arc<Mutex<Option<Canceller>>> = Arc::new(Mutex::new(None));
    let (tx, rx) = mpsc::channel::<(u8, Vec<u8>)>();
    let Ok(mut rd) = stream.try_clone() else { return 3 };
    {
        let canc = canc.clone();
        std::thread::spawn(move || {
            loop {
                match read_frame(&mut rd) {
                    Ok((OP_CANCEL, _)) => {
                        let c = canc.lock().unwrap().clone();
                        if let Some(c) = c {
                            c();
                        }
                    }
                    Ok(f) => {
                        if tx.send(f).is_err() {
                            break;
                        }
                    }
                    Err(_) => {
                        // the miner closed the socket (it stops, restarts this GPU, or died): nothing left to serve
                        if process {
                            std::process::exit(0);
                        }
                        break;
                    }
                }
            }
        });
    }
    let mut w = stream;
    let mut sc: Option<Box<dyn Scanner>> = None;
    let mut nsets = 0usize;
    let mut dev = -1;
    for (op, body) in rx {
        let r = handle(op, &body, &mut sc, &mut nsets, &mut dev, &canc, local.as_ref());
        let reply = |w: &mut TcpStream, sc: &Option<Box<dyn Scanner>>, payload: Vec<u8>| {
            let mut e = Enc::default();
            if let Some(s) = sc {
                put_status(&mut e, s.as_ref(), nsets);
            } else {
                put_status_none(&mut e);
            }
            e.raw(&payload);
            write_frame(w, R_OK, &e.0)
        };
        let ok = match r {
            Ok(After::Reply(p)) => reply(&mut w, &sc, p).is_ok(),
            Ok(After::Hang(p)) => {
                let _ = reply(&mut w, &sc, p);
                loop {
                    std::thread::sleep(Duration::from_secs(3600));
                }
            }
            Ok(After::Exit) => {
                if process {
                    std::process::exit(3);
                }
                let _ = w.shutdown(Shutdown::Both);
                return 3;
            }
            Err(e) => write_frame(&mut w, R_ERR, format!("{e:#}").as_bytes()).is_ok(),
        };
        if !ok {
            break;
        }
    }
    // the scanner (the engine context) is closed here, before the process ends; no cancel may reach it after that
    *canc.lock().unwrap() = None;
    drop(sc);
    0
}

fn put_status_none(e: &mut Enc) {
    e.u8(0).u8(0).u32(0);
}

fn handle(op: u8, body: &[u8], sc: &mut Option<Box<dyn Scanner>>, nsets: &mut usize, dev: &mut i32, canc: &Mutex<Option<Canceller>>, local: Option<&Arc<DatasetFile>>) -> Result<After> {
    let mut d = Dec::new(body);
    if op == OP_OPEN {
        let backend = if d.u8()? == 1 { Backend::Cpu } else { Backend::Cuda };
        let threads = d.u32()? as usize;
        *dev = d.i32()?;
        let raw = d.rest();
        gpu::load(Some(backend), threads)?;
        let file = match local {
            Some(f) => f.clone(),
            None => {
                // the CUDA engine takes the file as it is; the parsed sets are for the CPU reference backend
                let bytes = Arc::new(raw.to_vec());
                let sets = if raw.is_empty() { Vec::new() } else { refc::parse_sets(bytes.clone()).map_err(anyhow::Error::msg)? };
                Arc::new(DatasetFile { hash: *blake3::hash(raw).as_bytes(), bytes, network: "miner", sets })
            }
        };
        *nsets = file.sets.len();
        let s = gpu::open(*dev, &file, raw)?;
        *canc.lock().unwrap() = s.canceller();
        let mut e = Enc::default();
        e.str(&s.kernel());
        *sc = Some(s);
        return Ok(After::Reply(e.0));
    }
    if op == OP_INJECT {
        let kind = d.u8()?;
        return Ok(match kind {
            1 => {
                let t = cuda_fault(*dev).map_err(anyhow::Error::msg)?;
                let mut e = Enc::default();
                e.str(&t);
                After::Reply(e.0)
            }
            2 => After::Exit,
            _ => After::Hang(Vec::new()),
        });
    }
    let Some(s) = sc.as_mut() else { bail!("engine process: the device is not open") };
    let mut e = Enc::default();
    match op {
        OP_SET_MODEL => {
            let ds = d.u32()? as usize;
            let hash: [u8; 32] = d.arr()?;
            let m = Model::from_packed(d.rest(), &hash).map_err(anyhow::Error::msg)?;
            s.set_model(ds, &m)?;
        }
        OP_SCAN => {
            let ds = d.u32()? as usize;
            let header: [u8; 80] = d.arr()?;
            let (start, count) = (d.u32()?, d.u32()?);
            let target: [u8; 32] = d.arr()?;
            let bound: [u8; 32] = d.arr()?;
            let o = s.scan(ds, &header, start, count, &target, &bound)?;
            e.u8(o.compute_error as u8);
            match o.cancelled_done {
                Some(v) => e.u8(1).u32(v),
                None => e.u8(0),
            };
            e.u32(o.hits.len() as u32);
            for (n, t) in &o.hits {
                e.u32(*n).raw(t);
            }
        }
        OP_EVAL => {
            let ds = d.u32()? as usize;
            let n = d.u32()? as usize;
            let keys: Vec<[u8; 32]> = (0..n).map(|_| d.arr::<32>()).collect::<Result<_>>()?;
            match s.eval_keys(ds, &keys)? {
                Some(t) => {
                    e.u8(1);
                    for x in &t {
                        e.raw(x);
                    }
                }
                None => {
                    e.u8(0);
                }
            }
        }
        OP_SELFTEST => {
            let r = s.selftest()?;
            e.str(&r);
        }
        OP_BENCH => {
            let (ds, arch, ms) = (d.i32()?, d.u32()?, d.u32()?);
            let r = s.bench(ds, arch, ms)?;
            e.f64(r);
        }
        _ => bail!("engine process: unknown call {op}"),
    }
    Ok(After::Reply(e.0))
}

// ---------------------------------------------------------------------------------------------------------------------
// the miner's side: a `Scanner` whose engine runs in a child process
// ---------------------------------------------------------------------------------------------------------------------

type ChildRef = Arc<Mutex<std::process::Child>>;

/// The engine processes alive (for `kill`: a GPU that hangs in an auto-uv step).
static CHILDREN: Mutex<Vec<(i32, Weak<Mutex<std::process::Child>>)>> = Mutex::new(Vec::new());

/// Stops the engine process(es) of `dev` (a hung GPU): its worker sees the call fail and starts a new one.  The number stopped.
pub fn kill(dev: i32) -> usize {
    let mut g = CHILDREN.lock().unwrap();
    g.retain(|(_, w)| w.strong_count() > 0);
    let mut n = 0;
    for (d, w) in g.iter() {
        if *d == dev {
            if let Some(c) = w.upgrade() {
                if c.lock().unwrap().kill().is_ok() {
                    n += 1;
                }
            }
        }
    }
    n
}

pub struct Remote {
    dev: i32,
    w: Arc<Mutex<TcpStream>>,
    r: TcpStream,
    child: Option<ChildRef>,
    thread: Option<std::thread::JoinHandle<i32>>,
    kernel: String,
    st: Status,
    dead: Option<String>,
}

impl Remote {
    /// One call: the request, then its reply within `timeout`.  A child that ends or does not answer is dead for good (the
    /// worker drops it and starts a new one).
    fn call(&mut self, op: u8, body: &[u8], timeout: Duration, what: &str) -> Result<Vec<u8>> {
        if let Some(d) = &self.dead {
            bail!("{d}");
        }
        let sent = {
            let mut w = self.w.lock().unwrap_or_else(|e| e.into_inner());
            write_frame(&mut *w, op, body)
        };
        let got = sent.and_then(|_| {
            self.r.set_read_timeout(Some(timeout))?;
            read_frame(&mut self.r)
        });
        match got {
            Ok((R_OK, b)) => {
                let mut d = Dec::new(&b);
                self.st = get_status(&mut d)?;
                Ok(d.rest().to_vec())
            }
            Ok((R_ERR, b)) => Err(anyhow!("{}", String::from_utf8_lossy(&b))),
            Ok((x, _)) => {
                self.stop_child();
                self.dead = Some(format!("GPU{}: the engine process sent an unknown reply {x} to {what}: it was stopped", self.dev));
                bail!("{}", self.dead.clone().unwrap());
            }
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {
                self.stop_child();
                self.dead = Some(format!("GPU{}: the engine process did not answer {what} within {} s (a hung GPU?): it was stopped", self.dev, timeout.as_secs()));
                bail!("{}", self.dead.clone().unwrap());
            }
            Err(e) => {
                let how = self.exit_status().unwrap_or_else(|| format!("connection: {e}"));
                self.dead = Some(format!("GPU{}: the engine process ended during {what} ({how}): a crash of this GPU's engine (CUDA fault, driver)", self.dev));
                bail!("{}", self.dead.clone().unwrap());
            }
        }
    }

    /// How the child ended (it is given a moment to be reaped).
    fn exit_status(&mut self) -> Option<String> {
        let c = self.child.as_ref()?;
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(2) {
            if let Ok(Some(st)) = c.lock().unwrap().try_wait() {
                return Some(st.to_string());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        None
    }

    fn stop_child(&mut self) {
        let _ = self.r.shutdown(Shutdown::Both);
        if let Some(c) = &self.child {
            let mut c = c.lock().unwrap();
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

impl Drop for Remote {
    fn drop(&mut self) {
        // closing the socket ends the child (it closes its engine context and exits); a child that does not end is killed
        let _ = self.r.shutdown(Shutdown::Both);
        if let Some(c) = &self.child {
            let t = Instant::now();
            loop {
                let mut g = c.lock().unwrap();
                if !matches!(g.try_wait(), Ok(None)) {
                    break;
                }
                if t.elapsed() > Duration::from_secs(5) {
                    let _ = g.kill();
                    let _ = g.wait();
                    break;
                }
                drop(g);
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        if let Some(h) = self.thread.take() {
            let _ = h.join();
        }
    }
}

impl Scanner for Remote {
    fn kernel(&self) -> String {
        self.kernel.clone()
    }
    fn canceller(&self) -> Option<Canceller> {
        let w = self.w.clone();
        Some(Arc::new(move || {
            if let Ok(mut s) = w.lock() {
                let _ = write_frame(&mut *s, OP_CANCEL, &[]);
            }
        }))
    }
    fn evaluated(&self) -> Option<u64> {
        self.st.evaluated
    }
    fn path(&self, ds: usize) -> Option<(Path, String)> {
        self.st.paths.get(ds).cloned().flatten()
    }
    fn kep1_kernel(&self) -> Option<String> {
        self.st.kep1.clone()
    }
    fn set_model(&mut self, ds: usize, m: &Model) -> Result<()> {
        let mut e = Enc::default();
        e.u32(ds as u32).raw(&m.hash).raw(&m.packed);
        self.call(OP_SET_MODEL, &e.0, T_MODEL, "a model upload")?;
        Ok(())
    }
    fn scan(&mut self, ds: usize, header: &[u8; 80], start: u32, count: u32, target: &[u8; 32], bound: &[u8; 32]) -> Result<ScanOut> {
        let mut e = Enc::default();
        e.u32(ds as u32).raw(header).u32(start).u32(count).raw(target).raw(bound);
        let p = self.call(OP_SCAN, &e.0, T_SCAN, "a scan")?;
        let mut d = Dec::new(&p);
        let compute_error = d.u8()? != 0;
        let cancelled_done = if d.u8()? == 1 { Some(d.u32()?) } else { None };
        let n = d.u32()? as usize;
        let mut hits = Vec::with_capacity(n.min(1 << 16));
        for _ in 0..n {
            hits.push((d.u32()?, d.arr::<32>()?));
        }
        Ok(ScanOut { hits, compute_error, cancelled_done })
    }
    fn eval_keys(&mut self, ds: usize, keys: &[[u8; 32]]) -> Result<Option<Vec<[u8; 32]>>> {
        let mut e = Enc::default();
        e.u32(ds as u32).u32(keys.len() as u32);
        for k in keys {
            e.raw(k);
        }
        let p = self.call(OP_EVAL, &e.0, T_EVAL, "an audit evaluation")?;
        let mut d = Dec::new(&p);
        if d.u8()? == 0 {
            return Ok(None);
        }
        Ok(Some((0..keys.len()).map(|_| d.arr::<32>()).collect::<Result<_>>()?))
    }
    fn selftest(&mut self) -> Result<String> {
        let p = self.call(OP_SELFTEST, &[], T_OPEN, "the self-test")?;
        Dec::new(&p).str()
    }
    fn bench(&mut self, ds: i32, arch: u32, ms: u32) -> Result<f64> {
        let mut e = Enc::default();
        e.i32(ds).u32(arch).u32(ms);
        let p = self.call(OP_BENCH, &e.0, Duration::from_millis(ms as u64) + T_EVAL, "a bench")?;
        Dec::new(&p).f64()
    }
    fn inject_fault(&mut self, kind: &str) -> Result<bool> {
        let k = match kind {
            "kernel" => 1u8,
            "exit" => 2,
            "hang" => 3,
            _ => return Ok(false),
        };
        let p = self.call(OP_INJECT, &[k], T_EVAL, "a test fault")?;
        if k == 1 {
            eprintln!("GPU{}: {}", self.dev, Dec::new(&p).str().unwrap_or_default());
        }
        Ok(true)
    }
}

fn token() -> String {
    let a: [u8; 16] = rand::random();
    crate::neb::proto::hex(&a)
}

/// Waits for the child that knows `token` (another local process that connects first is dropped).
fn accept(l: &TcpListener, token: &str, alive: &mut dyn FnMut() -> Result<()>) -> Result<TcpStream> {
    l.set_nonblocking(true)?;
    let t = Instant::now();
    while t.elapsed() < T_CONNECT {
        match l.accept() {
            Ok((s, _)) => {
                s.set_nonblocking(false)?;
                s.set_read_timeout(Some(Duration::from_secs(10)))?;
                let mut s2 = s;
                match read_frame(&mut s2) {
                    Ok((OP_HELLO, b)) if b == token.as_bytes() => {
                        s2.set_read_timeout(None)?;
                        let _ = s2.set_nodelay(true);
                        return Ok(s2);
                    }
                    _ => continue,
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                alive()?;
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) => return Err(e.into()),
        }
    }
    bail!("the engine process did not connect within {} s", T_CONNECT.as_secs())
}

fn attach(dev: i32, s: TcpStream, child: Option<ChildRef>, thread: Option<std::thread::JoinHandle<i32>>, raw: &[u8], backend: Backend, threads: usize) -> Result<Box<dyn Scanner>> {
    let w = Arc::new(Mutex::new(s.try_clone()?));
    let mut rm = Remote { dev, w, r: s, child, thread, kernel: String::new(), st: Status::default(), dead: None };
    let mut e = Enc::default();
    e.u8(if backend == Backend::Cpu { 1 } else { 0 }).u32(threads as u32).i32(dev).raw(raw);
    let p = rm.call(OP_OPEN, &e.0, T_OPEN, "the device open")?;
    rm.kernel = Dec::new(&p).str()?;
    Ok(Box::new(rm))
}

/// Opens a device in an engine process of its own (the counterpart of `gpu::open`).
pub fn open(dev: i32, _file: &Arc<DatasetFile>, raw: &[u8]) -> Result<Box<dyn Scanner>> {
    let l = TcpListener::bind(("127.0.0.1", 0))?;
    let port = l.local_addr()?.port();
    let tok = token();
    let exe = std::env::current_exe()?;
    // stdout / stderr are the miner's: the engine's own lines go to the same log
    let child = std::process::Command::new(&exe)
        .arg(ENGINE_FLAG)
        .arg(dev.to_string())
        .env(ENV_PORT, port.to_string())
        .env(ENV_TOKEN, &tok)
        .stdin(std::process::Stdio::null())
        .spawn()
        .map_err(|e| anyhow!("GPU{dev}: cannot start its engine process {}: {e}", exe.display()))?;
    let child: ChildRef = Arc::new(Mutex::new(child));
    {
        let mut g = CHILDREN.lock().unwrap();
        g.retain(|(_, w)| w.strong_count() > 0);
        g.push((dev, Arc::downgrade(&child)));
    }
    let c2 = child.clone();
    let s = accept(&l, &tok, &mut || match c2.lock().unwrap().try_wait() {
        Ok(Some(st)) => bail!("GPU{dev}: its engine process ended at the start ({st})"),
        _ => Ok(()),
    });
    let s = match s {
        Ok(s) => s,
        Err(e) => {
            let mut c = child.lock().unwrap();
            let _ = c.kill();
            let _ = c.wait();
            return Err(e);
        }
    };
    attach(dev, s, Some(child), None, raw, gpu::backend(), gpu::device_count().max(1))
}

/// Tests: the engine "process" is a thread of this process on a real loopback socket (the same frames and code paths).
#[cfg(test)]
pub fn open_thread(dev: i32, file: &Arc<DatasetFile>) -> Result<Box<dyn Scanner>> {
    let l = TcpListener::bind(("127.0.0.1", 0))?;
    let port = l.local_addr()?.port();
    let tok = token();
    let t2 = tok.clone();
    let f2 = file.clone();
    let h = std::thread::spawn(move || {
        let Ok(mut s) = TcpStream::connect(("127.0.0.1", port)) else { return 3 };
        if write_frame(&mut s, OP_HELLO, t2.as_bytes()).is_err() {
            return 3;
        }
        serve(s, false, Some(f2))
    });
    let s = accept(&l, &tok, &mut || Ok(()))?;
    attach(dev, s, None, Some(h), &[], Backend::Cpu, 1)
}

// ---------------------------------------------------------------------------------------------------------------------
// a real CUDA fault for the tests (FFG_TEST_FAULT_KIND=kernel)
// ---------------------------------------------------------------------------------------------------------------------

/// A kernel that stores to a device address nothing is mapped at: the MMU fault (Xid 31) leaves the sticky
/// "an illegal memory access was encountered" in the context, as the faults of a marginal card do.
const FAULT_PTX: &[u8] = b".version 6.0\n.target sm_50\n.address_size 64\n.visible .entry ffgn_fault()\n{\n .reg .b64 %rd<2>;\n .reg .b32 %r<2>;\n mov.u64 %rd1, 64;\n mov.u32 %r1, 1;\n st.global.u32 [%rd1], %r1;\n ret;\n}\n\0";

/// Runs FAULT_PTX in the primary context of CUDA device `dev` (the one the engine's runtime API uses) of this process.
pub fn cuda_fault(dev: i32) -> Result<String, String> {
    use std::ffi::c_void;
    let lib = crate::gpu::dl::Lib::open(std::path::Path::new(if cfg!(windows) { "nvcuda.dll" } else { "libcuda.so.1" }))?;
    let sym = |n: &str| lib.sym(n).ok_or_else(|| format!("the CUDA driver has no {n}"));
    unsafe {
        let init: unsafe extern "system" fn(u32) -> i32 = std::mem::transmute(sym("cuInit")?);
        let get: unsafe extern "system" fn(*mut i32, i32) -> i32 = std::mem::transmute(sym("cuDeviceGet")?);
        let retain: unsafe extern "system" fn(*mut *mut c_void, i32) -> i32 = std::mem::transmute(sym("cuDevicePrimaryCtxRetain")?);
        let push: unsafe extern "system" fn(*mut c_void) -> i32 = std::mem::transmute(sym("cuCtxPushCurrent_v2")?);
        let load: unsafe extern "system" fn(*mut *mut c_void, *const c_void) -> i32 = std::mem::transmute(sym("cuModuleLoadData")?);
        let func: unsafe extern "system" fn(*mut *mut c_void, *mut c_void, *const std::ffi::c_char) -> i32 = std::mem::transmute(sym("cuModuleGetFunction")?);
        #[allow(clippy::type_complexity)]
        let launch: unsafe extern "system" fn(*mut c_void, u32, u32, u32, u32, u32, u32, u32, *mut c_void, *mut *mut c_void, *mut *mut c_void) -> i32 = std::mem::transmute(sym("cuLaunchKernel")?);
        let sync: unsafe extern "system" fn() -> i32 = std::mem::transmute(sym("cuCtxSynchronize")?);
        let ck = |r: i32, w: &str| if r == 0 { Ok(()) } else { Err(format!("test fault: {w} = CUDA error {r}")) };
        ck(init(0), "cuInit")?;
        let mut d = 0i32;
        ck(get(&mut d, dev), "cuDeviceGet")?;
        let mut ctx = std::ptr::null_mut();
        ck(retain(&mut ctx, d), "cuDevicePrimaryCtxRetain")?;
        ck(push(ctx), "cuCtxPushCurrent")?;
        let mut m = std::ptr::null_mut();
        ck(load(&mut m, FAULT_PTX.as_ptr() as *const c_void), "cuModuleLoadData")?;
        let mut f = std::ptr::null_mut();
        ck(func(&mut f, m, c"ffgn_fault".as_ptr()), "cuModuleGetFunction")?;
        let l = launch(f, 1, 1, 1, 1, 1, 1, 0, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut());
        let s = sync();
        // the library stays loaded (the context is the engine's)
        std::mem::forget(lib);
        Ok(format!("TEST FAULT (FFG_TEST_FAULT_KIND=kernel): a kernel stored to an unmapped address on CUDA device {dev}: launch {l}, synchronize {s} (700 = an illegal memory access)"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_and_status_round_trip() {
        let mut b = Vec::new();
        write_frame(&mut b, OP_SCAN, b"abc").unwrap();
        let (op, body) = read_frame(&mut &b[..]).unwrap();
        assert_eq!((op, body.as_slice()), (OP_SCAN, &b"abc"[..]));
        // a bad length is refused, not allocated
        let bad = [0xffu8, 0xff, 0xff, 0x7f, 1];
        assert!(read_frame(&mut &bad[..]).is_err());
        let st = Status { evaluated: Some(77), kep1: Some("warp-mma".into()), paths: vec![Some((Path::Table, "digest table L<=2".into())), None, Some((Path::Cheap, "cheap k=6".into()))] };
        let mut e = Enc::default();
        e.u8(if st.evaluated.is_some() { 1 } else { 0 }).u64(77).u8(1).str("warp-mma").u32(3).u8(2).str("digest table L<=2").u8(255).u8(1).str("cheap k=6");
        assert_eq!(get_status(&mut Dec::new(&e.0)).unwrap(), st);
    }

    /// the cost of one call through the engine process' socket (cargo test --release -- --ignored round_trip --nocapture)
    #[test]
    #[ignore]
    fn round_trip_cost() {
        gpu::load(Some(Backend::Cpu), 1).unwrap();
        let (file, m) = cpu_file();
        let mut rem = open_thread(0, &file).unwrap();
        rem.set_model(0, &m).unwrap();
        let t = Instant::now();
        for _ in 0..5000 {
            rem.scan(0, &[1; 80], 0, 0, &[0; 32], &[0; 32]).unwrap();
        }
        println!("one scan call through the socket: {:.1} us", t.elapsed().as_secs_f64() / 5000.0 * 1e6);
    }

    #[test]
    fn wanted_only_for_cuda_and_not_single_process() {
        assert!(!wanted(true, true));
        assert!(!wanted(false, false));
        if std::env::var("FFGN_SINGLE_PROCESS").is_err() {
            assert!(wanted(false, true));
        }
    }

    fn cpu_file() -> (Arc<DatasetFile>, refc::Model) {
        let (set, m) = refc::synthetic(refc::Arch::Kep1).unwrap();
        (Arc::new(DatasetFile { hash: [0; 32], bytes: Arc::new(Vec::new()), network: "test", sets: vec![Arc::new(set)] }), m)
    }

    #[test]
    fn a_remote_device_gives_the_same_hits_as_the_local_one_and_a_dead_one_fails_cleanly() {
        gpu::load(Some(Backend::Cpu), 1).unwrap();
        let (file, m) = cpu_file();
        let mut local = gpu::open(0, &file, &[]).unwrap();
        let mut rem = open_thread(0, &file).unwrap();
        assert_eq!(rem.kernel(), "C reference");
        local.set_model(0, &m).unwrap();
        rem.set_model(0, &m).unwrap();
        let h = [9u8; 80];
        let mut bound = [0xffu8; 32];
        bound[31] = 0x20;
        let a = local.scan(0, &h, 500, 24, &[0; 32], &bound).unwrap();
        let b = rem.scan(0, &h, 500, 24, &[0; 32], &bound).unwrap();
        assert!(!a.hits.is_empty());
        assert_eq!(a.hits, b.hits);
        assert_eq!(rem.eval_keys(0, &[[1; 32]]).unwrap(), None);
        assert!(rem.canceller().is_some());
        rem.canceller().unwrap()(); // a cancel between calls is harmless
        assert_eq!(rem.scan(0, &h, 500, 24, &[0; 32], &bound).unwrap().hits, a.hits);
        // the engine "process" dies: the call fails with a crash text (faults::sticky), every later call too
        assert!(rem.inject_fault("exit").is_err());
        let e = format!("{:#}", rem.scan(0, &h, 500, 4, &[0; 32], &bound).err().unwrap());
        assert!(e.contains("engine process ended"), "{e}");
        assert!(crate::faults::sticky(&e), "{e}");
        // an engine error is passed on as it is and the device stays usable
        let mut rem2 = open_thread(0, &file).unwrap();
        let e = format!("{:#}", rem2.scan(0, &h, 0, 4, &[0; 32], &bound).err().unwrap());
        assert!(e.contains("no model for dataset 0"), "{e}");
        rem2.set_model(0, &m).unwrap();
        assert_eq!(rem2.scan(0, &h, 500, 24, &[0; 32], &bound).unwrap().hits, a.hits);
    }
}
