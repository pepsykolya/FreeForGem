// FFGN — see LICENSE
//! Devices that evaluate NEB attempts.
//!   * `nebgpu`: the closed-source FFGN engine (`libnebgpu.so` / `nebgpu.dll`, C API v1 of `nebgpu.h`), loaded at run time
//!     from the binary's folder, then the system search path.  One context per CUDA device.
//!   * `cpu`: the C reference on CPU threads (`--backend cpu --cpu-threads N`): tests and machines without a GPU.
//! Every hit of any device is evaluated again with the C reference (refc.rs) before it is sent (mod.rs).
use crate::gpu::dl;
use crate::neb::refc::{self, Dataset, DatasetFile, Model};
use anyhow::{Result, bail};
use std::ffi::{CStr, c_char, c_void};
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Backend {
    Cuda,
    Cpu,
}

fn lib_file() -> &'static str {
    if cfg!(windows) { "nebgpu.dll" } else { "libnebgpu.so" }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct DeviceInfo {
    pub index: i32,
    pub name: [c_char; 128],
    pub pci_bus_id: [c_char; 32],
    pub cc_major: i32,
    pub cc_minor: i32,
    pub sm_count: i32,
    pub clock_mhz: i32,
    pub mem_total: u64,
    pub mem_free: u64,
    pub supported: i32,
    pub tensor_int8: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Hit {
    pub nonce: u32,
    pub flags: u32,
    pub ticket: [u8; 32],
}

pub const E_CANCELLED: i32 = -7;
pub const E_COMPUTE: i32 = -10;
pub const OPT_CANARY_EVERY: i32 = 1;

macro_rules! api {
    ($($f:ident: fn($($an:ident: $at:ty),*) -> $r:ty;)* @optional: $($o:ident: fn($($bn:ident: $bt:ty),*) -> $or:ty;)*) => {
        struct Api {
            _lib: dl::Lib,
            $($f: unsafe extern "C" fn($($at),*) -> $r,)*
            // 1.0.9: entry points of newer engines; None with an older library (the miner works without them)
            $($o: Option<unsafe extern "C" fn($($bt),*) -> $or>,)*
        }
        impl Api {
            fn load(lib: dl::Lib) -> Result<Api, String> {
                Ok(Api {
                    $($f: {
                        let p = lib.sym(stringify!($f)).ok_or_else(|| format!("{} has no {}", lib_file(), stringify!($f)))?;
                        unsafe { std::mem::transmute::<*mut c_void, unsafe extern "C" fn($($at),*) -> $r>(p) }
                    },)*
                    $($o: lib.sym(stringify!($o)).map(|p| unsafe { std::mem::transmute::<*mut c_void, unsafe extern "C" fn($($bt),*) -> $or>(p) }),)*
                    _lib: lib,
                })
            }
        }
    };
}

api! {
    nebgpu_api_version: fn() -> i32;
    nebgpu_build_info: fn() -> *const c_char;
    nebgpu_device_count: fn() -> i32;
    nebgpu_device_info_get: fn(dev: i32, out: *mut DeviceInfo) -> i32;
    nebgpu_open: fn(dev: i32, out: *mut *mut c_void) -> i32;
    nebgpu_close: fn(ctx: *mut c_void) -> ();
    nebgpu_last_error: fn(ctx: *const c_void) -> *const c_char;
    nebgpu_load_datasets_file: fn(ctx: *mut c_void, file: *const u8, len: usize) -> i32;
    nebgpu_set_model_packed: fn(ctx: *mut c_void, ds: i32, packed: *const u8, len: usize) -> i32;
    nebgpu_scan: fn(ctx: *mut c_void, ds: i32, header: *const u8, start: u32, count: u32, target: *const u8, share: *const u8, hits: *mut Hit, max: u32, nhits: *mut u32, done: *mut u32) -> i32;
    nebgpu_cancel: fn(ctx: *mut c_void) -> ();
    nebgpu_eval_keys: fn(ctx: *mut c_void, ds: i32, keys: *const u8, n: u32, digests: *mut u8, tickets: *mut u8, idx: *mut u32) -> i32;
    nebgpu_selftest: fn(ctx: *mut c_void, report: *mut c_char, len: usize) -> i32;
    nebgpu_set_option: fn(ctx: *mut c_void, opt: i32, v: i64) -> i32;
    nebgpu_bench: fn(ctx: *mut c_void, ds: i32, arch: u32, ms: u32, rate: *mut f64) -> i32;
    nebgpu_canary: fn(ctx: *mut c_void, ds: i32, n: u32, bad: *mut u32) -> i32;
    nebgpu_kep1_path: fn(ctx: *const c_void, text: *mut c_char, len: usize) -> i32;
    @optional:
    // nebgpu 0.6.0 (FFGN 1.0.9): evaluated attempts (the cheap filter and the digest table skip nonces: the screened count
    // is not the hashrate) and the path of a slot (call sizing)
    nebgpu_eval_stats: fn(ctx: *const c_void, screened: *mut u64, evaluated: *mut u64, table: *mut u64) -> i32;
    nebgpu_slot_path: fn(ctx: *const c_void, ds: i32, text: *mut c_char, len: usize) -> i32;
}

static API: std::sync::OnceLock<Api> = std::sync::OnceLock::new();
static BACKEND: std::sync::OnceLock<Backend> = std::sync::OnceLock::new();
static CPU_THREADS: std::sync::OnceLock<usize> = std::sync::OnceLock::new();

fn cstr(p: *const c_char) -> String {
    if p.is_null() { String::new() } else { unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned() }
}

fn open_lib() -> Result<Api> {
    let file = lib_file();
    let mut cands = Vec::new();
    if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())) {
        cands.push(dir.join(file));
    }
    cands.push(std::path::PathBuf::from(file));
    let mut errs = Vec::new();
    for c in cands {
        match dl::Lib::open(&c) {
            Ok(l) => match Api::load(l) {
                Ok(a) => {
                    let v = unsafe { (a.nebgpu_api_version)() };
                    if v != 1 {
                        bail!("{} has API version {v}, this miner needs 1 (mismatched files?)", c.display());
                    }
                    return Ok(a);
                }
                Err(e) => errs.push(e),
            },
            Err(e) => errs.push(format!("{}: {e}", c.display())),
        }
    }
    bail!("{}", errs.join("; "))
}

/// Selects the backend: `cpu` explicitly, else the GPU library (CUDA ordinals in PCI bus order, as nvidia-smi / HiveOS).
pub fn load(want: Option<Backend>, cpu_threads: usize) -> Result<()> {
    if BACKEND.get().is_some() {
        return Ok(());
    }
    if want == Some(Backend::Cpu) {
        let _ = CPU_THREADS.set(cpu_threads.max(1));
        let _ = BACKEND.set(Backend::Cpu);
        return Ok(());
    }
    if std::env::var_os("CUDA_DEVICE_ORDER").is_none() {
        unsafe { std::env::set_var("CUDA_DEVICE_ORDER", "PCI_BUS_ID") };
    }
    let a = open_lib().map_err(|e| anyhow::anyhow!("the NEB GPU library cannot be used ({e:#}); --backend cpu mines with the CPU reference"))?;
    let _ = API.set(a);
    let _ = BACKEND.set(Backend::Cuda);
    Ok(())
}

fn api() -> &'static Api {
    API.get().expect("neb::gpu::load first")
}

pub fn backend() -> Backend {
    *BACKEND.get().expect("neb::gpu::load first")
}

pub fn build_info() -> String {
    match backend() {
        Backend::Cpu => "C reference (CPU)".into(),
        Backend::Cuda => cstr(unsafe { (api().nebgpu_build_info)() }),
    }
}

pub fn device_count() -> usize {
    match backend() {
        Backend::Cpu => *CPU_THREADS.get().unwrap_or(&1),
        Backend::Cuda => unsafe { (api().nebgpu_device_count)() }.max(0) as usize,
    }
}

pub fn info(dev: i32) -> Result<DeviceInfo> {
    let mut d: DeviceInfo = unsafe { std::mem::zeroed() };
    if backend() == Backend::Cpu {
        d.index = dev;
        d.supported = 1;
        let n = b"CPU reference thread";
        for (i, &c) in n.iter().enumerate() {
            d.name[i] = c as c_char;
        }
        return Ok(d);
    }
    let r = unsafe { (api().nebgpu_device_info_get)(dev, &mut d) };
    if r != 0 {
        bail!("device info of GPU{dev}: {}", cstr(unsafe { (api().nebgpu_last_error)(std::ptr::null()) }));
    }
    Ok(d)
}

pub fn device_name(dev: i32) -> String {
    info(dev).map(|i| cstr(i.name.as_ptr())).unwrap_or_else(|_| "?".into())
}

pub fn device_pci_bus_id(dev: i32) -> Result<String> {
    if backend() == Backend::Cpu {
        return Ok(format!("cpu:{dev}"));
    }
    Ok(cstr(info(dev)?.pci_bus_id.as_ptr()))
}

/// What a worker needs from a device.
pub trait Scanner: Send {
    fn kernel(&self) -> String;
    /// Model for dataset index `ds` (the slot of the job).
    fn set_model(&mut self, ds: usize, m: &Model) -> Result<()>;
    /// Hits (nonce, device ticket) in start..start+count with ticket <= `bound`; complete (a cut report is re-scanned in halves).
    fn scan(&mut self, ds: usize, header: &[u8; 80], start: u32, count: u32, target: &[u8; 32], bound: &[u8; 32]) -> Result<ScanOut>;
    /// Exact device tickets of explicit keys (audits); None when the device cannot (CPU: same code as the check).
    fn eval_keys(&mut self, ds: usize, keys: &[[u8; 32]]) -> Result<Option<Vec<[u8; 32]>>>;
    /// Built-in self-test of the device; Ok(report).
    fn selftest(&mut self) -> Result<String>;
    /// attempts per second on dataset `ds` (or a synthetic model of `arch` when ds < 0) for ~ms
    fn bench(&mut self, ds: i32, arch: u32, ms: u32) -> Result<f64>;
    /// 1.0.5: the KEP1 attempt kernel of the engine ("warp-mma (auto: ...)", "dp4a (...)"); None before the first KEP1 model
    fn kep1_kernel(&self) -> Option<String> {
        None
    }
    /// 1.0.11: asks a running scan of this device to return early, from another thread (None = cannot): the engine's
    /// nebgpu_cancel in this process, or a cancel frame to the device's engine process (iso.rs)
    fn canceller(&self) -> Option<Canceller> {
        None
    }
    /// 1.0.11 (tests only, FFG_TEST_FAULT_KIND): make this device fail for real: `kernel` = a CUDA kernel that writes to an
    /// invalid address in the device's context (a sticky "illegal memory access", Xid 31), `exit` = the engine process dies,
    /// `hang` = the engine process stops answering. Ok(false): not supported here (the caller simulates the fault instead).
    fn inject_fault(&mut self, _kind: &str) -> Result<bool> {
        Ok(false)
    }
    /// 1.0.9: attempts EVALUATED by the device since it was opened (engine 0.6.0 `nebgpu_eval_stats`): its cheap filter and
    /// digest table screen nonces and evaluate only some, each with the same win probability, so this is the hashrate.
    /// None: every scanned nonce is evaluated (older engine, CPU).
    fn evaluated(&self) -> Option<u64> {
        None
    }
    /// 1.0.9: the path the engine uses for dataset `ds` (engine 0.6.0 `nebgpu_slot_path`): Plain / Cheap / Table and its text
    fn path(&self, _ds: usize) -> Option<(Path, String)> {
        None
    }
}

/// 1.0.9: how the engine mines a dataset slot (nebgpu_slot_path)
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Path {
    /// every nonce evaluated
    Plain,
    /// the cheap filter: nonces screened (~0.25-0.4 G/s), a share of them evaluated
    Cheap,
    /// the digest table: nonces screened at ~4-5 G/s, ~1-4 % of them evaluated from the table
    Table,
}

/// 1.0.11: what `Scanner::canceller` gives: callable from any thread, any number of times.
pub type Canceller = Arc<dyn Fn() + Send + Sync>;

/// Asks a running scan of the engine context `h` to return early (nebgpu_cancel: any thread).
fn cancel(h: usize) {
    if h != 0 && backend() == Backend::Cuda {
        unsafe { (api().nebgpu_cancel)(h as *mut c_void) };
    }
}

pub struct ScanOut {
    pub hits: Vec<(u32, [u8; 32])>,
    /// the device reported a compute error (canary mismatch: unstable clocks); hits discarded
    pub compute_error: bool,
    pub cancelled_done: Option<u32>,
}

pub struct NebGpu {
    dev: i32,
    ctx: *mut c_void,
    hits: Vec<Hit>,
    name: String,
}

// SAFETY: a context is used by one thread at a time (&mut self); nebgpu.h allows it to move between threads
unsafe impl Send for NebGpu {}

impl Drop for NebGpu {
    fn drop(&mut self) {
        unsafe { (api().nebgpu_close)(self.ctx) };
    }
}

impl NebGpu {
    fn err(&self) -> String {
        cstr(unsafe { (api().nebgpu_last_error)(self.ctx) })
    }
    fn check(&self, r: i32, what: &str) -> Result<()> {
        if r != 0 {
            bail!("GPU{} {what}: {} (code {r})", self.dev, self.err());
        }
        Ok(())
    }
}

/// A CUDA context without datasets (the tuner's check: the engine's synthetic models).
pub fn open_bare(dev: i32) -> Result<NebGpu> {
    let mut ctx = std::ptr::null_mut();
    let r = unsafe { (api().nebgpu_open)(dev, &mut ctx) };
    if r != 0 || ctx.is_null() {
        bail!("GPU{dev}: cannot open the device: {} (code {r})", cstr(unsafe { (api().nebgpu_last_error)(std::ptr::null()) }));
    }
    Ok(NebGpu { dev, ctx, hits: Vec::new(), name: build_info() })
}

impl NebGpu {
    /// attempts/s on the engine's synthetic model of `arch` (1 KEP1, 2 RET2), canary included
    pub fn bench_arch(&mut self, arch: u32, ms: u32) -> Result<f64> {
        self.bench(-1, arch, ms)
    }
    /// explicit canary of `n` keys on a synthetic model; the number of wrong tickets
    pub fn canary_synthetic(&mut self, n: u32) -> Result<u32> {
        let mut bad = 0u32;
        let r = unsafe { (api().nebgpu_canary)(self.ctx, -1, n, &mut bad) };
        if r == E_COMPUTE {
            return Ok(bad.max(1));
        }
        self.check(r, "canary")?;
        Ok(bad)
    }
}

/// Opens a device: CUDA context with the whole dataset file, or a CPU reference thread.
pub fn open(dev: i32, file: &Arc<DatasetFile>, raw: &[u8]) -> Result<Box<dyn Scanner>> {
    if backend() == Backend::Cpu {
        return Ok(Box::new(Cpu { file: file.clone(), models: vec![None; file.sets.len()], sc: refc::Scratch::default() }));
    }
    let mut ctx = std::ptr::null_mut();
    let r = unsafe { (api().nebgpu_open)(dev, &mut ctx) };
    if r != 0 || ctx.is_null() {
        bail!("GPU{dev}: cannot open the device: {} (code {r})", cstr(unsafe { (api().nebgpu_last_error)(std::ptr::null()) }));
    }
    let g = NebGpu { dev, ctx, hits: vec![Hit { nonce: 0, flags: 0, ticket: [0; 32] }; 1024], name: build_info() };
    if !raw.is_empty() {
        g.check(unsafe { (api().nebgpu_load_datasets_file)(ctx, raw.as_ptr(), raw.len()) }, "dataset upload")?;
    }
    Ok(Box::new(g))
}

impl Scanner for NebGpu {
    fn kernel(&self) -> String {
        self.name.clone()
    }
    fn canceller(&self) -> Option<Canceller> {
        // F-17: the worker clears its copy (under the lock the main loop cancels with) before this context is closed
        let h = self.ctx as usize;
        Some(Arc::new(move || cancel(h)))
    }
    fn inject_fault(&mut self, kind: &str) -> Result<bool> {
        if kind != "kernel" {
            return Ok(false);
        }
        // in this process: the fault ends CUDA for every GPU of the process (the problem --single-process keeps)
        let t = crate::neb::iso::cuda_fault(self.dev).map_err(anyhow::Error::msg)?;
        eprintln!("GPU{}: {t}", self.dev);
        Ok(true)
    }
    fn evaluated(&self) -> Option<u64> {
        let f = api().nebgpu_eval_stats?;
        let mut ev = 0u64;
        let r = unsafe { f(self.ctx, std::ptr::null_mut(), &mut ev, std::ptr::null_mut()) };
        (r == 0).then_some(ev)
    }
    fn path(&self, ds: usize) -> Option<(Path, String)> {
        let f = api().nebgpu_slot_path?;
        let mut buf = vec![0 as c_char; 512];
        let r = unsafe { f(self.ctx, ds as i32, buf.as_mut_ptr(), buf.len()) };
        let p = match r {
            0 => Path::Plain,
            1 => Path::Cheap,
            2 => Path::Table,
            _ => return None,
        };
        Some((p, cstr(buf.as_ptr())))
    }
    fn kep1_kernel(&self) -> Option<String> {
        // nebgpu 0.5.2: two KEP1 slots with the per-watt sort verdicts need ~600 bytes (the 200 of 1.0.8 cut the tail)
        let mut buf = vec![0 as c_char; 1024];
        let p = unsafe { (api().nebgpu_kep1_path)(self.ctx, buf.as_mut_ptr(), buf.len()) };
        (p > 0).then(|| cstr(buf.as_ptr()))
    }
    fn set_model(&mut self, ds: usize, m: &Model) -> Result<()> {
        let r = unsafe { (api().nebgpu_set_model_packed)(self.ctx, ds as i32, m.packed.as_ptr(), m.packed.len()) };
        self.check(r, "model upload")
    }
    fn scan(&mut self, ds: usize, header: &[u8; 80], start: u32, count: u32, target: &[u8; 32], bound: &[u8; 32]) -> Result<ScanOut> {
        let mut out = ScanOut { hits: Vec::new(), compute_error: false, cancelled_done: None };
        let mut todo = vec![(start, count)];
        // F-18: nonces evaluated so far (sub-ranges done in full plus the part of a cancelled call); after a cancel only the
        // evaluated part of a cut report is scanned again (its hits beyond the buffer), the untried rest is dropped
        let mut evaluated = 0u64;
        let mut cancelled = false;
        while let Some((s, c)) = todo.pop() {
            let (mut n, mut done) = (0u32, 0u32);
            let r = unsafe { (api().nebgpu_scan)(self.ctx, ds as i32, header.as_ptr(), s, c, target.as_ptr(), bound.as_ptr(), self.hits.as_mut_ptr(), self.hits.len() as u32, &mut n, &mut done) };
            if r == E_COMPUTE {
                out.compute_error = true;
                out.hits.clear();
                return Ok(out);
            }
            if r == E_CANCELLED {
                if !cancelled {
                    evaluated += done as u64;
                }
                if (n as usize) <= self.hits.len() {
                    out.hits.extend(self.hits[..n as usize].iter().map(|h| (h.nonce, h.ticket)));
                } else if !cancelled && done > 1 {
                    // a cut report of the evaluated part: scan that part again in halves (no other range after a cancel)
                    todo.clear();
                    todo.push((s.wrapping_add(done / 2), done - done / 2));
                    todo.push((s, done / 2));
                } else {
                    out.hits.extend(self.hits.iter().map(|h| (h.nonce, h.ticket)));
                }
                if !cancelled && (n as usize) <= self.hits.len() {
                    todo.clear(); // the untried rest of the range: the work is being replaced
                }
                cancelled = true;
                continue;
            }
            self.check(r, "scan")?;
            if n as usize > self.hits.len() {
                if c <= 1 {
                    bail!("GPU{}: {n} hits reported for one nonce", self.dev);
                }
                todo.push((s.wrapping_add(c / 2), c - c / 2));
                todo.push((s, c / 2));
                continue;
            }
            if !cancelled {
                evaluated += c as u64;
            }
            out.hits.extend(self.hits[..n as usize].iter().map(|h| (h.nonce, h.ticket)));
        }
        if cancelled {
            out.cancelled_done = Some(evaluated.min(u32::MAX as u64) as u32);
        }
        Ok(out)
    }
    fn eval_keys(&mut self, ds: usize, keys: &[[u8; 32]]) -> Result<Option<Vec<[u8; 32]>>> {
        let mut t = vec![[0u8; 32]; keys.len()];
        let r = unsafe { (api().nebgpu_eval_keys)(self.ctx, ds as i32, keys.as_ptr() as *const u8, keys.len() as u32, std::ptr::null_mut(), t.as_mut_ptr() as *mut u8, std::ptr::null_mut()) };
        self.check(r, "audit evaluation")?;
        Ok(Some(t))
    }
    fn selftest(&mut self) -> Result<String> {
        let mut buf = vec![0 as c_char; 512];
        let r = unsafe { (api().nebgpu_selftest)(self.ctx, buf.as_mut_ptr(), buf.len()) };
        let rep = cstr(buf.as_ptr());
        if r != 0 {
            bail!("GPU{} self-test FAILED: {rep} {}", self.dev, self.err());
        }
        Ok(rep)
    }
    fn bench(&mut self, ds: i32, arch: u32, ms: u32) -> Result<f64> {
        let mut rate = 0.0;
        let r = unsafe { (api().nebgpu_bench)(self.ctx, ds, arch, ms, &mut rate) };
        if r == E_COMPUTE {
            bail!("GPU{}: wrong results at these clocks (compute check failed)", self.dev);
        }
        self.check(r, "bench")?;
        Ok(rate)
    }
}


#[allow(dead_code)]
pub fn set_option_canary(s: &mut NebGpu, every: i64) {
    unsafe { (api().nebgpu_set_option)(s.ctx, OPT_CANARY_EVERY, every) };
}

struct Cpu {
    file: Arc<DatasetFile>,
    models: Vec<Option<Arc<Model>>>,
    sc: refc::Scratch,
}

impl Cpu {
    fn ds(&self, ds: usize) -> Result<&Arc<Dataset>> {
        self.file.sets.get(ds).ok_or_else(|| anyhow::anyhow!("dataset index {ds} not in the file"))
    }
}

impl Scanner for Cpu {
    fn kernel(&self) -> String {
        "C reference".into()
    }
    fn set_model(&mut self, ds: usize, m: &Model) -> Result<()> {
        let d = self.ds(ds)?.clone();
        anyhow::ensure!(d.arch == m.arch, "dataset {ds} is {} but the model is {}", d.arch.name(), m.arch.name());
        // a copy is cheap next to the evaluation; the caller keeps its Arc
        self.models[ds] = Some(Arc::new(Model::from_packed(&m.packed, &m.hash).map_err(anyhow::Error::msg)?));
        Ok(())
    }
    fn scan(&mut self, ds: usize, header: &[u8; 80], start: u32, count: u32, _target: &[u8; 32], bound: &[u8; 32]) -> Result<ScanOut> {
        let d = self.ds(ds)?.clone();
        let m = self.models[ds].clone().ok_or_else(|| anyhow::anyhow!("no model for dataset {ds}"))?;
        let mut hits = Vec::new();
        for k in 0..count {
            let n = start.wrapping_add(k);
            let t = refc::ticket_of(&mut self.sc, &m, &d, header, n).map_err(anyhow::Error::msg)?;
            if crate::neb::proto::admits(bound, &t) {
                hits.push((n, t));
            }
        }
        Ok(ScanOut { hits, compute_error: false, cancelled_done: None })
    }
    fn eval_keys(&mut self, _ds: usize, _keys: &[[u8; 32]]) -> Result<Option<Vec<[u8; 32]>>> {
        Ok(None)
    }
    fn selftest(&mut self) -> Result<String> {
        refc::self_check().map_err(anyhow::Error::msg)?;
        Ok("C reference = NEB vectors (KEP1, RET2)".into())
    }
    fn bench(&mut self, _ds: i32, arch: u32, ms: u32) -> Result<f64> {
        let a = if arch == 2 { refc::Arch::Ret2 } else { refc::Arch::Kep1 };
        let (d, m) = refc::synthetic(a).map_err(anyhow::Error::msg)?;
        let t = std::time::Instant::now();
        let mut n = 0u32;
        let h = [7u8; 80];
        while t.elapsed().as_millis() < ms as u128 {
            refc::ticket_of(&mut self.sc, &m, &d, &h, n).map_err(anyhow::Error::msg)?;
            n += 1;
        }
        Ok(n as f64 / t.elapsed().as_secs_f64())
    }
}
