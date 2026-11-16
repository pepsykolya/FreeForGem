// FreeForGem — see LICENSE
//! Safe wrappers around the C ABI of the PROX search library (gpu/proxgpu.h): `libproxgpu.so` / `proxgpu.dll`, loaded at run
//! time from the binary's folder (then the system search path).  Closed-source kernel (ffgprox kernel A), separate from the
//! Pearl and NOID libraries.
use crate::gpu::dl;
use anyhow::{Result, bail};
use std::ffi::CStr;
use std::os::raw::c_char;

pub fn lib_file() -> &'static str {
    if cfg!(windows) { "proxgpu.dll" } else { "libproxgpu.so" }
}

/// prox_job_t of proxgpu.h (376 bytes): the job template the library builds once per job (midstate + tail).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ProxJob {
    pub midstate: [u64; 8],
    pub tail: [u64; 32],
    pub tail_blocks: u32,
    pub tail_len: u32,
    pub total_len: u64,
    pub nonce_offset: u32,
    pub k: u32,
    pub beat: [u8; 26],
    pub has_beat: u8,
    pub _pad: [u8; 5],
}
impl Default for ProxJob {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

macro_rules! px_api {
    ($($f:ident: fn($($an:ident: $at:ty),*) -> $r:ty;)*) => {
        struct Api {
            _lib: dl::Lib,
            $($f: unsafe extern "C" fn($($at),*) -> $r,)*
        }
        impl Api {
            fn load(lib: dl::Lib) -> Result<Api, String> {
                Ok(Api {
                    $($f: {
                        let p = lib.sym(stringify!($f)).ok_or_else(|| format!("{} has no {}", lib_file(), stringify!($f)))?;
                        unsafe { std::mem::transmute::<*mut std::ffi::c_void, unsafe extern "C" fn($($at),*) -> $r>(p) }
                    },)*
                    _lib: lib,
                })
            }
        }
        $(#[allow(dead_code)] unsafe fn $f($($an: $at),*) -> $r { unsafe { (api().$f)($($an),*) } })*
    };
}

px_api! {
    px_api_version: fn() -> i32;
    px_last_error: fn() -> *const c_char;
    px_job_size: fn() -> i32;
    px_driver_version: fn() -> i32;
    px_device_count: fn() -> i32;
    px_device_name: fn(dev: i32, out: *mut c_char, len: i32) -> i32;
    px_device_pci_bus_id: fn(dev: i32, out: *mut c_char, len: i32) -> i32;
    px_compute_capability: fn(dev: i32) -> i32;
    px_sm_count: fn(dev: i32) -> i32;
    prox_job_build: fn(job: *mut ProxJob, msg: *const u8, len: usize, k: u32, beat26: *const u8) -> i32;
    prox_search: fn(dev: i32, job: *const ProxJob, start: u64, count: u64, out: *mut u64, max: u32) -> i32;
    prox_counter_to_nonce: fn(job: *const ProxJob, c: u64) -> u64;
    prox_variant_name: fn(job: *const ProxJob) -> *const c_char;
}

static API: std::sync::OnceLock<Api> = std::sync::OnceLock::new();

/// Loads the library once (Err: why it cannot be used).  CUDA ordinals follow the PCI bus order (as nvidia-smi / HiveOS).
pub fn load() -> Result<()> {
    if API.get().is_some() {
        return Ok(());
    }
    if std::env::var_os("CUDA_DEVICE_ORDER").is_none() {
        unsafe { std::env::set_var("CUDA_DEVICE_ORDER", "PCI_BUS_ID") };
    }
    let file = lib_file();
    let mut errs = Vec::new();
    let mut cands = Vec::new();
    if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())) {
        cands.push(dir.join(file));
    }
    cands.push(std::path::PathBuf::from(file));
    for c in cands {
        match dl::Lib::open(&c) {
            Ok(l) => match Api::load(l) {
                Ok(a) => {
                    let (v, sz) = unsafe { ((a.px_api_version)(), (a.px_job_size)()) };
                    if v != 1 || sz as usize != std::mem::size_of::<ProxJob>() {
                        bail!("{} has API version {v} / job size {sz}, this miner needs 1 / {} (mismatched files?)", c.display(), std::mem::size_of::<ProxJob>());
                    }
                    let _ = API.set(a);
                    return Ok(());
                }
                Err(e) => errs.push(e),
            },
            Err(e) => errs.push(format!("{}: {e}", c.display())),
        }
    }
    bail!("the PROX GPU library cannot be loaded ({})", errs.join("; "))
}

fn api() -> &'static Api {
    if let Err(e) = load() {
        panic!("{e:#}");
    }
    API.get().unwrap()
}

fn cstr(p: *const c_char) -> String {
    if p.is_null() { String::new() } else { unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned() }
}

fn last_error() -> String {
    cstr(unsafe { px_last_error() })
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
    let v = unsafe { px_driver_version() };
    (v > 0).then_some(v)
}
pub fn device_count() -> usize {
    unsafe { px_device_count() }.max(0) as usize
}
pub fn device_name(dev: i32) -> String {
    text(api().px_device_name, dev, "device name").unwrap_or_else(|_| "?".into())
}
pub fn device_pci_bus_id(dev: i32) -> Result<String> {
    text(api().px_device_pci_bus_id, dev, "PCI bus id")
}
pub fn compute_capability(dev: i32) -> i32 {
    unsafe { px_compute_capability(dev) }
}
pub fn sm_count(dev: i32) -> i32 {
    unsafe { px_sm_count(dev) }
}

/// Job template of `msg` (the message without the nonce), `k` required zero bits, optional solo beat.
pub fn job_build(msg: &[u8], k: u32, beat: Option<&[u8; 26]>) -> Result<ProxJob> {
    let mut j = ProxJob::default();
    let b = beat.map_or(std::ptr::null(), |b| b.as_ptr());
    if unsafe { prox_job_build(&mut j, msg.as_ptr(), msg.len(), k, b) } != 0 {
        bail!("PROX job of {} bytes is too long for the kernel (max 4096)", msg.len());
    }
    Ok(j)
}

/// Searches the counters start .. start+count-1 on `dev`; candidates (real nonces) are appended to `out`.
pub fn search(dev: i32, job: &ProxJob, start: u64, count: u64, buf: &mut [u64], out: &mut Vec<u64>) -> Result<()> {
    // keep the expected number of candidates per call far below the buffer (k is at least 1)
    let kf = job.k.clamp(1, 32);
    let per = ((buf.len() as u64 / 8) << kf).max(1 << 20);
    let mut done = 0u64;
    while done < count {
        let n = per.min(count - done);
        let r = unsafe { prox_search(dev, job, start.wrapping_add(done), n, buf.as_mut_ptr(), buf.len() as u32) };
        if r < 0 {
            bail!("GPU{dev}: PROX search failed ({r}): CUDA error (driver fault / Xid?)");
        }
        if r as usize >= buf.len() {
            bail!("GPU{dev}: PROX result buffer full (k={} too low for this call size)", job.k);
        }
        out.extend_from_slice(&buf[..r as usize]);
        done += n;
    }
    Ok(())
}

pub fn counter_to_nonce(job: &ProxJob, c: u64) -> u64 {
    unsafe { prox_counter_to_nonce(job, c) }
}

pub fn variant_name(job: &ProxJob) -> String {
    cstr(unsafe { prox_variant_name(job) })
}
