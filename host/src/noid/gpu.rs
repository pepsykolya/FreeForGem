// FreeForGemPear — see LICENSE
//! Safe wrappers around the C ABI of the NOID search library (gpu/noidgpu.h): `libnoidgpu.so` / `noidgpu.dll`, loaded at run
//! time from the binary's folder (then the system search path).  Closed-source kernels like the Pearl library; separate from it.
use crate::gpu::dl;
use std::time::{Duration, Instant};
use anyhow::{Result, bail};
use std::ffi::CStr;
use std::os::raw::c_char;

pub const MODE_EFF: i32 = 0;
pub const MODE_SPEED: i32 = 1;

pub fn lib_file() -> &'static str {
    if cfg!(windows) { "noidgpu.dll" } else { "libnoidgpu.so" }
}

macro_rules! nd_api {
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

nd_api! {
    nd_api_version: fn() -> i32;
    nd_last_error: fn() -> *const c_char;
    nd_driver_version: fn() -> i32;
    nd_device_count: fn() -> i32;
    nd_device_name: fn(device: i32, out: *mut c_char, len: i32) -> i32;
    nd_device_pci_bus_id: fn(device: i32, out: *mut c_char, len: i32) -> i32;
    nd_device_uuid: fn(device: i32, out: *mut c_char, len: i32) -> i32;
    nd_sm_count: fn(device: i32) -> i32;
    nd_compute_capability: fn(device: i32) -> i32;
    nd_kernel_label_for: fn(device: i32, mode: i32) -> *const c_char;
    nd_init: fn(device: i32, mode: i32) -> i32;
    nd_kernel_label: fn() -> *const c_char;
    nd_grid_nonces: fn() -> i32;
    nd_max_hits: fn() -> i32;
    nd_set_job: fn(fields: *const u8, target: *const u8) -> i32;
    nd_launch: fn(slot: i32, base: u32, iters: i32) -> i32;
    nd_wait: fn(slot: i32, hits: *mut u32, max_hits: i32) -> i32;
    nd_digest: fn(start: u32, n: u32, out: *mut u8) -> i32;
    nd_release: fn() -> ();
}

static API: std::sync::OnceLock<Api> = std::sync::OnceLock::new();

/// Loads the library once (Err: why it cannot be used).
pub fn load() -> Result<()> {
    if API.get().is_some() {
        return Ok(());
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
                    if unsafe { (a.nd_api_version)() } != 1 {
                        bail!("{} has API version {}, this miner needs 1 (mismatched files?)", c.display(), unsafe { (a.nd_api_version)() });
                    }
                    let _ = API.set(a);
                    return Ok(());
                }
                Err(e) => errs.push(e),
            },
            Err(e) => errs.push(format!("{}: {e}", c.display())),
        }
    }
    bail!("the NOID GPU library cannot be loaded ({})", errs.join("; "))
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
    cstr(unsafe { nd_last_error() })
}

fn text(f: unsafe extern "C" fn(i32, *mut c_char, i32) -> i32, dev: i32, what: &str) -> Result<String> {
    let mut buf = [0 as c_char; 128];
    if unsafe { f(dev, buf.as_mut_ptr(), buf.len() as i32) } != 0 {
        bail!("{what}({dev}): {}", last_error());
    }
    Ok(cstr(buf.as_ptr()))
}

/// CUDA driver version (13000 = R580 / CUDA 13.0); None without a driver.
pub fn driver_version() -> Option<i32> {
    let v = unsafe { nd_driver_version() };
    (v > 0).then_some(v)
}
pub fn device_count() -> usize {
    unsafe { nd_device_count() }.max(0) as usize
}
pub fn device_name(dev: i32) -> String {
    text(api().nd_device_name, dev, "nd_device_name").unwrap_or_else(|_| "?".into())
}
pub fn device_pci_bus_id(dev: i32) -> Result<String> {
    text(api().nd_device_pci_bus_id, dev, "nd_device_pci_bus_id")
}
#[allow(dead_code)]
pub fn device_uuid(dev: i32) -> Result<String> {
    text(api().nd_device_uuid, dev, "nd_device_uuid")
}
#[allow(dead_code)]
pub fn sm_count(dev: i32) -> usize {
    unsafe { nd_sm_count(dev) }.max(0) as usize
}
/// 10 * major + minor
pub fn compute_capability(dev: i32) -> i32 {
    unsafe { nd_compute_capability(dev) }
}
/// "eff:n2gc/sm86"; empty when the device is not supported
pub fn kernel_label_for(dev: i32, mode: i32) -> String {
    cstr(unsafe { nd_kernel_label_for(dev, mode) })
}

/// Binds the calling thread to `dev` and loads the kernel of `mode`.
pub fn init(dev: i32, mode: i32) -> Result<()> {
    if unsafe { nd_init(dev, mode) } != 0 {
        bail!("GPU{dev}: {}", last_error());
    }
    Ok(())
}
pub fn kernel_label() -> String {
    cstr(unsafe { nd_kernel_label() })
}
pub fn grid_nonces() -> u64 {
    unsafe { nd_grid_nonces() }.max(0) as u64
}
pub fn max_hits() -> usize {
    unsafe { nd_max_hits() }.max(0) as usize
}
pub fn set_job(fields: &[u8; 256], target: &[u8; 32]) -> Result<()> {
    if unsafe { nd_set_job(fields.as_ptr(), target.as_ptr()) } != 0 {
        bail!("nd_set_job: {}", last_error());
    }
    Ok(())
}
// nd_wait spins on the CPU until the launch is done (one core at 100 % per GPU thread: a runnable thread all the time, which
// the load average counts -- HiveOS reboots a rig at a high load average, Rig01 1.7.2 beta2).  The calling thread sleeps until
// shortly before the launch should be done (from the measured time per nonce of this GPU), then waits the rest: a few % CPU.
struct Pace {
    launched: [Option<(Instant, u64)>; 2], // per slot: when, nonces
    last_done: Option<Instant>,
    per_nonce: f64, // seconds per nonce on this GPU (0 = not measured yet)
}
thread_local! {
    static PACE: std::cell::RefCell<Pace> = const { std::cell::RefCell::new(Pace { launched: [None, None], last_done: None, per_nonce: 0.0 }) };
}

pub fn launch(slot: i32, base: u32, iters: u32) -> Result<()> {
    if unsafe { nd_launch(slot, base, iters as i32) } != 0 {
        bail!("nd_launch: {}", last_error());
    }
    let n = grid_nonces() * iters.max(1) as u64;
    PACE.with(|p| p.borrow_mut().launched[(slot & 1) as usize] = Some((Instant::now(), n)));
    Ok(())
}
/// Returns (candidates found, candidates stored in `buf`).  Sleeps (no CPU) through most of the launch first.
pub fn wait(slot: i32, buf: &mut [u32]) -> Result<(usize, usize)> {
    let s = (slot & 1) as usize;
    let mine = PACE.with(|p| {
        let p = p.borrow();
        p.launched[s].map(|(t, n)| {
            // it runs after the launch before it (two in flight): from the later of its start and the last completion
            let begin = p.last_done.map_or(t, |d| d.max(t));
            (begin, n, p.per_nonce)
        })
    });
    if let Some((begin, n, per)) = mine {
        if per > 0.0 {
            let due = begin + Duration::from_secs_f64(n as f64 * per * 0.85);
            let now = Instant::now();
            if due > now + Duration::from_millis(2) {
                std::thread::sleep(due - now);
            }
        }
    }
    let r = unsafe { nd_wait(slot, buf.as_mut_ptr(), buf.len() as i32) };
    let now = Instant::now();
    PACE.with(|p| {
        let mut p = p.borrow_mut();
        if let Some((t, n)) = p.launched[s].take() {
            let begin = p.last_done.map_or(t, |d| d.max(t));
            let took = now.duration_since(begin).as_secs_f64();
            if n > 0 && took > 0.0 {
                let x = took / n as f64;
                // a sleep that overshot makes `took` too long: only shorter times pull the estimate down quickly
                p.per_nonce = if p.per_nonce == 0.0 { x } else if x < p.per_nonce { 0.5 * p.per_nonce + 0.5 * x } else { 0.9 * p.per_nonce + 0.1 * x };
            }
        }
        p.last_done = Some(now);
    });
    if r < 0 {
        bail!("nd_wait: {}", last_error());
    }
    let r = r as usize;
    Ok((r, r.min(buf.len()).min(max_hits())))
}
pub fn digest(start: u32, n: u32) -> Result<Vec<[u8; 32]>> {
    let mut out = vec![[0u8; 32]; n as usize];
    if unsafe { nd_digest(start, n, out.as_mut_ptr() as *mut u8) } != 0 {
        bail!("nd_digest: {}", last_error());
    }
    Ok(out)
}
pub fn release() {
    PACE.with(|p| *p.borrow_mut() = Pace { launched: [None, None], last_done: None, per_nonce: 0.0 });
    if API.get().is_some() {
        unsafe { nd_release() };
    }
}
