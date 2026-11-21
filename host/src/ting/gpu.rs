// FFGT — see LICENSE
//! Safe wrappers around the C ABI of the Ting search library (gpu/tinggpu.h), loaded at run time from the binary's folder (then
//! the system search path): `libtinggpu.so` / `tinggpu.dll` (CUDA, NVIDIA sm_61+) or `libtinggpu_cl.so` / `tinggpu_cl.dll`
//! (OpenCL: AMD RDNA and others).  Closed-source kernel; one backend per process (`--backend cuda|opencl`, default: CUDA when
//! it sees a GPU, else OpenCL).
use crate::gpu::dl;
use anyhow::{Result, bail};
use std::ffi::CStr;
use std::os::raw::c_char;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Backend {
    Cuda,
    OpenCl,
}

fn lib_file(b: Backend) -> &'static str {
    match (b, cfg!(windows)) {
        (Backend::Cuda, true) => "tinggpu.dll",
        (Backend::Cuda, false) => "libtinggpu.so",
        (Backend::OpenCl, true) => "tinggpu_cl.dll",
        (Backend::OpenCl, false) => "libtinggpu_cl.so",
    }
}

/// tg_job_t of tinggpu.h (128 bytes).
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct TgJob {
    pub midstate: [u32; 8],
    pub tail: [u32; 16],
    pub target: [u32; 8],
}

macro_rules! tg_api {
    ($($f:ident: fn($($an:ident: $at:ty),*) -> $r:ty;)*) => {
        struct Api {
            _lib: dl::Lib,
            backend: Backend,
            group: Option<Group>,
            $($f: unsafe extern "C" fn($($at),*) -> $r,)*
        }
        impl Api {
            fn load(lib: dl::Lib, backend: Backend) -> Result<Api, String> {
                Ok(Api {
                    $($f: {
                        let p = lib.sym(stringify!($f)).ok_or_else(|| format!("{} has no {}", lib_file(backend), stringify!($f)))?;
                        unsafe { std::mem::transmute::<*mut std::ffi::c_void, unsafe extern "C" fn($($at),*) -> $r>(p) }
                    },)*
                    group: Group::load(&lib),
                    _lib: lib,
                    backend,
                })
            }
        }
    };
}

tg_api! {
    tg_api_version: fn() -> i32;
    tg_last_error: fn() -> *const c_char;
    tg_job_size: fn() -> i32;
    tg_driver_version: fn() -> i32;
    tg_device_count: fn() -> i32;
    tg_device_name: fn(dev: i32, out: *mut c_char, len: i32) -> i32;
    tg_device_pci_bus_id: fn(dev: i32, out: *mut c_char, len: i32) -> i32;
    tg_compute_capability: fn(dev: i32) -> i32;
    tg_sm_count: fn(dev: i32) -> i32;
    tg_job_build: fn(job: *mut TgJob, header: *const u8, target: *const u8) -> i32;
    tg_search: fn(dev: i32, job: *const TgJob, base: *const u8, g_start: u32, count: u32, out: *mut u32, max: u32) -> i32;
    tg_bench: fn(dev: i32, secs: f64) -> f64;
    tg_kernel_name: fn(dev: i32) -> *const c_char;
    tg_search_w: fn(dev: i32, job: *const TgJob, base: *const u8, word: i32, g_start: u32, count: u32, out: *mut u32, max: u32) -> i32;
}

type SearchGroupFn = unsafe extern "C" fn(dev: i32, jobs: *const TgJob, n: i32, base: *const u8, word: i32, g_start: u32, count: u32, out: *mut u32, max: u32) -> i32;
type GroupOkFn = unsafe extern "C" fn(dev: i32, n: i32) -> i32;

/// API 3 (0.2.2): template grouping (tg_search_group); absent in an older library = groups of 1.
struct Group {
    ok: GroupOkFn,
    search: SearchGroupFn,
}
impl Group {
    fn load(lib: &dl::Lib) -> Option<Group> {
        let ok = lib.sym("tg_group_ok")?;
        let search = lib.sym("tg_search_group")?;
        Some(unsafe { Group { ok: std::mem::transmute::<*mut std::ffi::c_void, GroupOkFn>(ok), search: std::mem::transmute::<*mut std::ffi::c_void, SearchGroupFn>(search) } })
    }
}

static API: std::sync::OnceLock<Api> = std::sync::OnceLock::new();

fn open(b: Backend) -> Result<Api> {
    let file = lib_file(b);
    let mut errs = Vec::new();
    let mut cands = Vec::new();
    if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())) {
        cands.push(dir.join(file));
    }
    cands.push(std::path::PathBuf::from(file));
    for c in cands {
        match dl::Lib::open(&c) {
            Ok(l) => match Api::load(l, b) {
                Ok(a) => {
                    let (v, sz) = unsafe { ((a.tg_api_version)(), (a.tg_job_size)()) };
                    if v < 2 || sz as usize != std::mem::size_of::<TgJob>() {
                        bail!("{} has API version {v} / job size {sz}, this miner needs 2 / {} (mismatched files?)", c.display(), std::mem::size_of::<TgJob>());
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

/// Loads the library once.  `want`: the --backend choice (None = CUDA if it sees a GPU, else OpenCL).  CUDA ordinals follow
/// the PCI bus order (as nvidia-smi / HiveOS).
pub fn load(want: Option<Backend>) -> Result<()> {
    if API.get().is_some() {
        return Ok(());
    }
    if std::env::var_os("CUDA_DEVICE_ORDER").is_none() {
        unsafe { std::env::set_var("CUDA_DEVICE_ORDER", "PCI_BUS_ID") };
    }
    let order: Vec<Backend> = match want {
        Some(b) => vec![b],
        None => vec![Backend::Cuda, Backend::OpenCl],
    };
    let mut errs = Vec::new();
    for b in order {
        match open(b) {
            Ok(a) => {
                let n = unsafe { (a.tg_device_count)() };
                if n > 0 || want.is_some() {
                    let _ = API.set(a);
                    return Ok(());
                }
                errs.push(format!("{}: no GPU", lib_file(b)));
            }
            Err(e) => errs.push(format!("{e:#}")),
        }
    }
    bail!("the Ting GPU library cannot be used ({})", errs.join("; "))
}

/// The library is loaded (the tests' thread engines run without it).
pub fn loaded() -> bool {
    API.get().is_some()
}

fn api() -> &'static Api {
    API.get().expect("ting::gpu::load first")
}

pub fn backend() -> Backend {
    api().backend
}

fn cstr(p: *const c_char) -> String {
    if p.is_null() { String::new() } else { unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned() }
}

pub fn last_error() -> String {
    cstr(unsafe { (api().tg_last_error)() })
}

fn text(f: unsafe extern "C" fn(i32, *mut c_char, i32) -> i32, dev: i32, what: &str) -> Result<String> {
    let mut buf = [0 as c_char; 128];
    if unsafe { f(dev, buf.as_mut_ptr(), buf.len() as i32) } != 0 {
        bail!("{what}({dev}): {}", last_error());
    }
    Ok(cstr(buf.as_ptr()))
}

#[allow(dead_code)]
pub fn driver_version() -> Option<i32> {
    let v = unsafe { (api().tg_driver_version)() };
    (v > 0).then_some(v)
}
pub fn device_count() -> usize {
    unsafe { (api().tg_device_count)() }.max(0) as usize
}
pub fn device_name(dev: i32) -> String {
    text(api().tg_device_name, dev, "device name").unwrap_or_else(|_| "?".into())
}
pub fn device_pci_bus_id(dev: i32) -> Result<String> {
    text(api().tg_device_pci_bus_id, dev, "PCI bus id")
}
pub fn compute_capability(dev: i32) -> i32 {
    unsafe { (api().tg_compute_capability)(dev) }
}
pub fn sm_count(dev: i32) -> i32 {
    unsafe { (api().tg_sm_count)(dev) }
}
pub fn kernel_name(dev: i32) -> String {
    cstr(unsafe { (api().tg_kernel_name)(dev) })
}

/// The library's job of a header and a big-endian target (the host checks it against its own build, pow::Work).
pub fn job_build(header: &[u8; 294], target: &[u8; 32]) -> TgJob {
    let mut j = TgJob::default();
    unsafe { (api().tg_job_build)(&mut j, header.as_ptr(), target.as_ptr()) };
    j
}

/// Candidates (g values: first BlockId word <= target word 0) among g_start .. g_start+count-1 for the nonces `base` with
/// bytes 10..13 = g big-endian; appended to `out`.  Err on a GPU fault or when the library's buffer overflowed.
pub fn search(dev: i32, job: &TgJob, base: &[u8; 16], g_start: u32, count: u32, buf: &mut [u32], out: &mut Vec<u32>) -> Result<()> {
    search_w(dev, job, base, 11, g_start, count, buf, out)
}

/// The same with g in message word `word` of the last block: 11 = nonce bytes 10..13 (kernel t1, solo), 10 = bytes 6..9
/// (kernel t1p: a pool's extranonce1 in bytes 12..15 stays fixed).
#[allow(clippy::too_many_arguments)]
pub fn search_w(dev: i32, job: &TgJob, base: &[u8; 16], word: i32, g_start: u32, count: u32, buf: &mut [u32], out: &mut Vec<u32>) -> Result<()> {
    let r = unsafe { (api().tg_search_w)(dev, job, base.as_ptr(), word, g_start, count, buf.as_mut_ptr(), buf.len() as u32) };
    if r < 0 {
        bail!("GPU{dev}: Ting search failed: {} (driver fault / Xid?)", last_error());
    }
    let n = r as usize;
    if n > buf.len() {
        bail!("GPU{dev}: Ting result buffer full ({n} candidates in one call: target too easy for this call size)");
    }
    out.extend_from_slice(&buf[..n]);
    Ok(())
}

pub fn bench(dev: i32, secs: f64) -> Result<f64> {
    let r = unsafe { (api().tg_bench)(dev, secs) };
    if r < 0.0 {
        bail!("GPU{dev}: bench failed: {}", last_error());
    }
    Ok(r)
}

/// The largest group of jobs (one shared last block / target) the library searches at once on `dev`, at most `want`
/// (1 = no grouping: an OpenCL or pre-0.2.2 library).
pub fn group_size(dev: i32, want: usize) -> usize {
    let Some(g) = api().group.as_ref() else { return 1 };
    (1..=want.min(8)).rev().find(|&n| unsafe { (g.ok)(dev, n as i32) } == 1).unwrap_or(1)
}

/// Group search: g in `word` for every job of `jobs` (they share tail and target; at most 8, a size group_size allows).
/// Candidates as (job index, g) appended to `out`.  `buf` holds 2 words per candidate.  One job = search_w.
#[allow(clippy::too_many_arguments)]
pub fn search_group(dev: i32, jobs: &[TgJob], base: &[u8; 16], word: i32, g_start: u32, count: u32, buf: &mut [u32], out: &mut Vec<(usize, u32)>) -> Result<()> {
    if jobs.len() == 1 || api().group.is_none() {
        let mut v = Vec::new();
        for (i, j) in jobs.iter().enumerate() {
            v.clear();
            search_w(dev, j, base, word, g_start, count, buf, &mut v)?;
            out.extend(v.iter().map(|&g| (i, g)));
        }
        return Ok(());
    }
    let g = api().group.as_ref().unwrap();
    let max = (buf.len() / 2) as u32;
    let r = unsafe { (g.search)(dev, jobs.as_ptr(), jobs.len() as i32, base.as_ptr(), word, g_start, count, buf.as_mut_ptr(), max) };
    if r < 0 {
        bail!("GPU{dev}: Ting group search ({} jobs) failed: {} (driver fault / Xid?)", jobs.len(), last_error());
    }
    let n = r as usize;
    if n > max as usize {
        bail!("GPU{dev}: Ting result buffer full ({n} candidates in one call: target too easy for this call size)");
    }
    out.extend((0..n).map(|k| (buf[2 * k + 1] as usize, buf[2 * k])));
    Ok(())
}
