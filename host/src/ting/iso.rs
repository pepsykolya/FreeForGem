// FreeForGem — see LICENSE
//! 1.8.0 (owner Q5, 21.11.2026): one Ting engine process per GPU, so a CUDA fault on one GPU costs only that GPU a few seconds.
//!
//! Why: the sticky CUDA error of one card (Xid 31 / 43, "an illegal memory access") ends CUDA for every GPU of a process, and a
//! context with a sticky error cannot be reopened inside it (CRASH-12gpu, ISO.md of FFGN): with every GPU in one process all
//! workers stop and the miner restarts.  The same pattern as NEB's `neb/iso.rs`: the miner keeps everything (pool / node / gate
//! connections, the dev fee, jobs, the CPU check of every candidate, stats / API / HiveOS, auto-uv, auto-lock, fault records);
//! only the search calls of a GPU run in a child process of that GPU: `freeforgem --ting-engine DEV --backend B`, the same
//! binary, over a loopback TCP socket (127.0.0.1, random port, random 128-bit token; the engine's own output to stdout cannot
//! mix with the frames).  A child that dies or does not answer is dropped by the GPU's worker, which starts a new one.
//!
//! Frames: u32 length (LE, op byte included), u8 op, body.  Requests OPEN, SEARCH, INJECT; replies OK / ERR (the engine's text).
//! Ting has no cancel: a search call is sized to ~0.25 s (mod.rs CALL_SECS).
use super::gpu::{self, Backend, TgJob};
use anyhow::{Result, anyhow, bail};
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

/// The hidden command line flag of a child: `freeforgem --ting-engine DEV`.
pub const ENGINE_FLAG: &str = "--ting-engine";
const ENV_PORT: &str = "FFG_TING_ENGINE_PORT";
const ENV_TOKEN: &str = "FFG_TING_ENGINE_TOKEN";

const OP_OPEN: u8 = 1;
const OP_SEARCH: u8 = 2;
const OP_INJECT: u8 = 3;
const OP_HELLO: u8 = 0x7f;
const R_OK: u8 = 0x80;
const R_ERR: u8 = 0x81;
const MAX_FRAME: usize = 1 << 20;
/// candidates per call the engine returns at most (the worker's buffer: 4096 pairs)
pub const MAX_PAIRS: usize = 4096;

const T_OPEN: Duration = Duration::from_secs(120);
/// a search call takes ~0.25 s; 30 s without an answer = a hung GPU (the in-process stall watch: 60 s)
const T_SEARCH: Duration = Duration::from_secs(30);
const T_CONNECT: Duration = Duration::from_secs(60);

/// The engine of one GPU as the worker sees it: in this process (`Local`) or in a child (`Remote`).
pub trait Engine: Send {
    /// The largest group of jobs this device searches at once (1 = no group kernels).
    fn group_max(&self) -> usize;
    /// gpu::search_group: candidates (job index, g) appended to `out`.
    fn search_group(&mut self, jobs: &[TgJob], base: &[u8; 16], word: i32, g_start: u32, count: u32, out: &mut Vec<(usize, u32)>) -> Result<()>;
    /// Test hook (FFG_TEST_FAULT_KIND): `kernel` = a real CUDA fault in the engine's context, `exit` = the engine process dies,
    /// `hang` = it stops answering.  Ok(false): not available here.
    fn inject_fault(&mut self, kind: &str) -> Result<bool>;
}

/// Every GPU in the miner process (`--single-process`, OpenCL, the self-test and the bench).
pub struct Local {
    dev: i32,
    gmax: usize,
    buf: Vec<u32>,
}

impl Local {
    pub fn new(dev: i32) -> Local {
        Local { dev, gmax: super::group_max(dev), buf: vec![0u32; 2 * MAX_PAIRS] }
    }
}

impl Engine for Local {
    fn group_max(&self) -> usize {
        self.gmax
    }
    fn search_group(&mut self, jobs: &[TgJob], base: &[u8; 16], word: i32, g_start: u32, count: u32, out: &mut Vec<(usize, u32)>) -> Result<()> {
        gpu::search_group(self.dev, jobs, base, word, g_start, count, &mut self.buf, out)
    }
    fn inject_fault(&mut self, kind: &str) -> Result<bool> {
        if kind == "kernel" && gpu::backend() == Backend::Cuda {
            // the control of the isolation test: the same fault in the miner's own context reaches every GPU of the process
            eprintln!("GPU{}: {}", self.dev, crate::neb::iso::cuda_fault(self.dev).map_err(anyhow::Error::msg)?);
            return Ok(true);
        }
        Ok(false)
    }
}

/// One engine process per GPU for this run: CUDA only (OpenCL stays in the process), the coin's default unless --single-process
/// or FFG_SINGLE_PROCESS=1 (coins::isolated).
pub fn wanted(single_process_flag: bool, cuda: bool) -> bool {
    cuda && crate::coins::isolated(crate::coins::Coin::Ting, single_process_flag)
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
            bail!("Ting engine process: short frame");
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
    fn str(&mut self) -> Result<String> {
        let n = self.u32()? as usize;
        Ok(String::from_utf8_lossy(self.take(n)?).into_owned())
    }
}

fn put_job(e: &mut Vec<u8>, j: &TgJob) {
    for w in j.midstate.iter().chain(j.tail.iter()).chain(j.target.iter()) {
        e.extend_from_slice(&w.to_le_bytes());
    }
}

fn get_job(d: &mut Dec) -> Result<TgJob> {
    let mut j = TgJob::default();
    for w in j.midstate.iter_mut().chain(j.tail.iter_mut()).chain(j.target.iter_mut()) {
        *w = d.u32()?;
    }
    Ok(j)
}

fn search_body(jobs: &[TgJob], base: &[u8; 16], word: i32, g_start: u32, count: u32) -> Vec<u8> {
    let mut e = Vec::with_capacity(32 + 128 * jobs.len());
    e.extend_from_slice(&word.to_le_bytes());
    e.extend_from_slice(&g_start.to_le_bytes());
    e.extend_from_slice(&count.to_le_bytes());
    e.extend_from_slice(base);
    e.extend_from_slice(&(jobs.len() as u32).to_le_bytes());
    for j in jobs {
        put_job(&mut e, j);
    }
    e
}

// ---------------------------------------------------------------------------------------------------------------------
// the child: `freeforgem --ting-engine DEV --backend cuda`
// ---------------------------------------------------------------------------------------------------------------------

/// The device a child serves: the real library, or (tests) a CPU reference that checks the first `TEST_NONCES` nonces of each call.
enum Dev {
    Gpu(Local),
    #[cfg(test)]
    Cpu,
}

#[cfg(test)]
const TEST_NONCES: u32 = 512;

/// The main of an engine process: connects back to the miner, then serves its calls until the miner closes the socket.
pub fn child_main(dev: i32, backend: Option<Backend>) -> i32 {
    // Ctrl+C reaches every process of the console group: the miner shuts down and closes the socket; the child ending first
    // would look like a crash of its GPU
    let _ = ctrlc::set_handler(|| {});
    let (Some(port), Ok(token)) = (std::env::var(ENV_PORT).ok().and_then(|p| p.parse::<u16>().ok()), std::env::var(ENV_TOKEN)) else {
        eprintln!("{ENGINE_FLAG} is started by the miner itself (one engine process per GPU), not by hand");
        return 2;
    };
    let mut s = match TcpStream::connect(("127.0.0.1", port)) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("GPU{dev} Ting engine process: cannot reach the miner on 127.0.0.1:{port}: {e}");
            return 3;
        }
    };
    let _ = s.set_nodelay(true);
    if write_frame(&mut s, OP_HELLO, token.as_bytes()).is_err() {
        return 3;
    }
    serve(s, true, backend, false)
}

/// Serves the calls of one device.  `process`: a child process (it exits when the miner is gone); else a thread of the tests.
fn serve(mut s: TcpStream, process: bool, backend: Option<Backend>, cpu: bool) -> i32 {
    let mut dev: Option<Dev> = None;
    loop {
        let (op, body) = match read_frame(&mut s) {
            Ok(f) => f,
            Err(_) => {
                // the miner closed the socket (it stops, restarts this GPU, or died): nothing left to serve
                if process {
                    std::process::exit(0);
                }
                return 0;
            }
        };
        let r: Result<Vec<u8>> = (|| {
            let mut d = Dec::new(&body);
            match op {
                OP_OPEN => {
                    let n = d.i32()?;
                    let mut e = Vec::new();
                    if cpu {
                        #[cfg(test)]
                        {
                            dev = Some(Dev::Cpu);
                            e.extend_from_slice(&8u32.to_le_bytes());
                            let k = "cpu reference";
                            e.extend_from_slice(&(k.len() as u32).to_le_bytes());
                            e.extend_from_slice(k.as_bytes());
                            return Ok(e);
                        }
                    }
                    gpu::load(backend)?;
                    let l = Local::new(n);
                    let k = gpu::kernel_name(n);
                    e.extend_from_slice(&(l.group_max() as u32).to_le_bytes());
                    e.extend_from_slice(&(k.len() as u32).to_le_bytes());
                    e.extend_from_slice(k.as_bytes());
                    dev = Some(Dev::Gpu(l));
                    Ok(e)
                }
                OP_SEARCH => {
                    let (word, g0, count) = (d.i32()?, d.u32()?, d.u32()?);
                    let base: [u8; 16] = d.take(16)?.try_into().unwrap();
                    let n = d.u32()? as usize;
                    if n == 0 || n > 8 {
                        bail!("Ting engine process: a group of {n} jobs");
                    }
                    let jobs: Vec<TgJob> = (0..n).map(|_| get_job(&mut d)).collect::<Result<_>>()?;
                    let mut out = Vec::new();
                    match dev.as_mut() {
                        Some(Dev::Gpu(l)) => l.search_group(&jobs, &base, word, g0, count, &mut out)?,
                        #[cfg(test)]
                        Some(Dev::Cpu) => {
                            for g in g0..g0.saturating_add(count.min(TEST_NONCES)) {
                                let nonce = super::pow::nonce_of_w(&base, word, g);
                                for (k, j) in jobs.iter().enumerate() {
                                    let id = super::pow::job_hash(j, &nonce);
                                    if u32::from_be_bytes(id[..4].try_into().unwrap()) <= j.target[0] {
                                        out.push((k, g));
                                    }
                                }
                            }
                        }
                        None => bail!("Ting engine process: the device is not open"),
                    }
                    let mut e = Vec::with_capacity(4 + 8 * out.len());
                    e.extend_from_slice(&(out.len() as u32).to_le_bytes());
                    for (k, g) in out {
                        e.extend_from_slice(&(k as u32).to_le_bytes());
                        e.extend_from_slice(&g.to_le_bytes());
                    }
                    Ok(e)
                }
                OP_INJECT => match d.u8()? {
                    1 => {
                        let n = match dev.as_ref() {
                            Some(Dev::Gpu(l)) => l.dev,
                            _ => bail!("no CUDA device in this engine"),
                        };
                        let t = crate::neb::iso::cuda_fault(n).map_err(anyhow::Error::msg)?;
                        let mut e = Vec::new();
                        e.extend_from_slice(&(t.len() as u32).to_le_bytes());
                        e.extend_from_slice(t.as_bytes());
                        Ok(e)
                    }
                    2 => {
                        if process {
                            std::process::exit(3);
                        }
                        let _ = s.shutdown(Shutdown::Both);
                        Err(anyhow!("exit"))
                    }
                    _ => {
                        let _ = write_frame(&mut s, R_OK, &[]);
                        loop {
                            std::thread::sleep(Duration::from_secs(3600));
                        }
                    }
                },
                x => bail!("Ting engine process: unknown call {x}"),
            }
        })();
        let ok = match r {
            Ok(p) => write_frame(&mut s, R_OK, &p).is_ok(),
            Err(e) if op == OP_INJECT && format!("{e}") == "exit" => return 3,
            Err(e) => write_frame(&mut s, R_ERR, format!("{e:#}").as_bytes()).is_ok(),
        };
        if !ok {
            return 0;
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// the miner's side
// ---------------------------------------------------------------------------------------------------------------------

type ChildRef = Arc<Mutex<std::process::Child>>;

/// The engine processes alive (for `kill`: a GPU that hangs).
static CHILDREN: Mutex<Vec<(i32, Weak<Mutex<std::process::Child>>)>> = Mutex::new(Vec::new());

/// Stops the engine process(es) of `dev`: its worker sees the call fail and starts a new one.  The number stopped.
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

/// PIDs of the engine processes alive (the log line at the start, the RAM check of a rig).
pub fn pids() -> Vec<(i32, u32)> {
    let mut g = CHILDREN.lock().unwrap();
    g.retain(|(_, w)| w.strong_count() > 0);
    g.iter().filter_map(|(d, w)| w.upgrade().map(|c| (*d, c.lock().unwrap().id()))).collect()
}

pub struct Remote {
    dev: i32,
    s: TcpStream,
    child: Option<ChildRef>,
    thread: Option<std::thread::JoinHandle<i32>>,
    gmax: usize,
    pub kernel: String,
    dead: Option<String>,
}

impl Remote {
    fn call(&mut self, op: u8, body: &[u8], timeout: Duration, what: &str) -> Result<Vec<u8>> {
        if let Some(d) = &self.dead {
            bail!("{d}");
        }
        let got = write_frame(&mut self.s, op, body).and_then(|_| {
            self.s.set_read_timeout(Some(timeout))?;
            read_frame(&mut self.s)
        });
        match got {
            Ok((R_OK, b)) => Ok(b),
            Ok((R_ERR, b)) => Err(anyhow!("{}", String::from_utf8_lossy(&b))),
            Ok((x, _)) => {
                self.stop_child();
                self.dead = Some(format!("GPU{}: the Ting engine process sent an unknown reply {x} to {what}: it was stopped", self.dev));
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
        let _ = self.s.shutdown(Shutdown::Both);
        if let Some(c) = &self.child {
            let mut c = c.lock().unwrap();
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

impl Drop for Remote {
    fn drop(&mut self) {
        // closing the socket ends the child (it frees its CUDA context and exits); a child that does not end is killed
        let _ = self.s.shutdown(Shutdown::Both);
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

impl Engine for Remote {
    fn group_max(&self) -> usize {
        self.gmax
    }
    fn search_group(&mut self, jobs: &[TgJob], base: &[u8; 16], word: i32, g_start: u32, count: u32, out: &mut Vec<(usize, u32)>) -> Result<()> {
        let p = self.call(OP_SEARCH, &search_body(jobs, base, word, g_start, count), T_SEARCH, "a search")?;
        let mut d = Dec::new(&p);
        let n = d.u32()? as usize;
        for _ in 0..n {
            let k = d.u32()? as usize;
            out.push((k, d.u32()?));
        }
        Ok(())
    }
    fn inject_fault(&mut self, kind: &str) -> Result<bool> {
        let k = match kind {
            "kernel" => 1u8,
            "exit" => 2,
            "hang" => 3,
            _ => return Ok(false),
        };
        let p = self.call(OP_INJECT, &[k], Duration::from_secs(60), "a test fault")?;
        if k == 1 {
            eprintln!("GPU{}: {}", self.dev, Dec::new(&p).str().unwrap_or_default());
        }
        Ok(true)
    }
}

fn token() -> String {
    let a: [u8; 16] = rand::random();
    super::pow::hex(&a)
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
    bail!("the Ting engine process did not connect within {} s", T_CONNECT.as_secs())
}

fn attach(dev: i32, s: TcpStream, child: Option<ChildRef>, thread: Option<std::thread::JoinHandle<i32>>) -> Result<Remote> {
    let mut rm = Remote { dev, s, child, thread, gmax: 1, kernel: String::new(), dead: None };
    let p = rm.call(OP_OPEN, &dev.to_le_bytes(), T_OPEN, "the device open")?;
    let mut d = Dec::new(&p);
    rm.gmax = (d.u32()? as usize).clamp(1, 8);
    rm.kernel = d.str()?;
    Ok(rm)
}

/// Opens device `dev` in an engine process of its own.
pub fn open(dev: i32) -> Result<Box<dyn Engine>> {
    let l = TcpListener::bind(("127.0.0.1", 0))?;
    let port = l.local_addr()?.port();
    let tok = token();
    let exe = std::env::current_exe()?;
    let mut cmd = std::process::Command::new(&exe);
    cmd.arg(ENGINE_FLAG).arg(dev.to_string()).arg("--backend").arg(if gpu::backend() == Backend::OpenCl { "opencl" } else { "cuda" });
    if !super::grouping() {
        cmd.env("FFGT_NO_GROUP", "1");
    }
    // stdout / stderr are the miner's: the engine's own lines go to the same log
    let child = cmd
        .env(ENV_PORT, port.to_string())
        .env(ENV_TOKEN, &tok)
        .stdin(std::process::Stdio::null())
        .spawn()
        .map_err(|e| anyhow!("GPU{dev}: cannot start its Ting engine process {}: {e}", exe.display()))?;
    let child: ChildRef = Arc::new(Mutex::new(child));
    {
        let mut g = CHILDREN.lock().unwrap();
        g.retain(|(_, w)| w.strong_count() > 0);
        g.push((dev, Arc::downgrade(&child)));
    }
    let c2 = child.clone();
    let s = accept(&l, &tok, &mut || match c2.lock().unwrap().try_wait() {
        Ok(Some(st)) => bail!("GPU{dev}: its Ting engine process ended at the start ({st})"),
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
    Ok(Box::new(attach(dev, s, Some(child), None)?))
}

/// Tests: the engine "process" is a thread on a real loopback socket (the same frames and code paths), CPU reference device.
#[cfg(test)]
pub fn open_thread(dev: i32) -> Result<Box<dyn Engine>> {
    let l = TcpListener::bind(("127.0.0.1", 0))?;
    let port = l.local_addr()?.port();
    let tok = token();
    let t2 = tok.clone();
    let h = std::thread::spawn(move || {
        let Ok(mut s) = TcpStream::connect(("127.0.0.1", port)) else { return 3 };
        if write_frame(&mut s, OP_HELLO, t2.as_bytes()).is_err() {
            return 3;
        }
        serve(s, false, None, true)
    });
    let s = accept(&l, &tok, &mut || Ok(()))?;
    Ok(Box::new(attach(dev, s, None, Some(h))?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ting::pow;

    #[test]
    fn frames_and_jobs_round_trip() {
        let mut b = Vec::new();
        write_frame(&mut b, OP_SEARCH, b"abc").unwrap();
        let (op, body) = read_frame(&mut &b[..]).unwrap();
        assert_eq!((op, body.as_slice()), (OP_SEARCH, &b"abc"[..]));
        let bad = [0xffu8, 0xff, 0xff, 0x7f, 1];
        assert!(read_frame(&mut &bad[..]).is_err(), "a bad length is refused, not allocated");
        let j = pow::job_words(&[0x33; pow::HEADER_LEN], &[0x12; 32]);
        let mut e = Vec::new();
        put_job(&mut e, &j);
        assert_eq!(e.len(), 128);
        assert_eq!(get_job(&mut Dec::new(&e)).unwrap(), j);
    }

    #[test]
    fn wanted_only_for_cuda_and_not_single_process() {
        assert!(!wanted(true, true));
        assert!(!wanted(false, false));
        if std::env::var("FFG_SINGLE_PROCESS").is_err() && std::env::var("FFGN_SINGLE_PROCESS").is_err() {
            assert!(wanted(false, true));
        }
    }

    #[test]
    fn a_remote_engine_gives_the_cpu_candidates_and_a_dead_one_fails_cleanly() {
        let h = [0x5au8; pow::HEADER_LEN];
        let mut t = [0xffu8; 32];
        t[..4].copy_from_slice(&0x0fff_ffffu32.to_be_bytes()); // 1 in 16
        let jobs = vec![pow::job_words(&h, &t), pow::job_words(&[0x6bu8; pow::HEADER_LEN], &t)];
        let base = [7u8; 16];
        let mut rem = open_thread(0).unwrap();
        assert_eq!(rem.group_max(), 8);
        let mut got = Vec::new();
        rem.search_group(&jobs, &base, 11, 1000, 400, &mut got).unwrap();
        let mut want = Vec::new();
        for g in 1000..1400u32 {
            for (k, j) in jobs.iter().enumerate() {
                let id = pow::job_hash(j, &pow::nonce_of_w(&base, 11, g));
                if id[..4] <= t[..4] {
                    want.push((k, g));
                }
            }
        }
        assert!(!want.is_empty());
        assert_eq!(got, want);
        // the engine "process" dies: the call fails with a crash text (faults::sticky), every later call too
        assert!(rem.inject_fault("exit").is_err());
        let e = format!("{:#}", rem.search_group(&jobs, &base, 11, 0, 4, &mut got).err().unwrap());
        assert!(e.contains("engine process ended"), "{e}");
        assert!(crate::faults::sticky(&e), "{e}");
    }
}
