// FreeForGemPear — see LICENSE
//! Safe wrappers around the C ABI of libpearlgpu (see gpu/pearlgpu.h).
use anyhow::{Result, bail};
use std::ffi::CStr;

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct Hit {
    pub row: u32,
    pub col: u32,
    pub jackpot: [u32; 8], // BLAKE3(transcript, key = a_noise_seed), LE words
}

unsafe extern "C" {
    fn pg_device_count() -> i32;
    fn pg_sm_count(device: i32) -> i32;
    fn pg_kdim() -> i32;
    fn pg_device_name(device: i32, out: *mut std::os::raw::c_char, len: i32) -> i32;
    fn pg_init(device: i32) -> i32;
    fn pg_set_job(br_t: *const i8, q1: *const i32, q2: *const i32, n: i32, k: i32) -> i32;
    fn pg_attempt(seed: *const u8, p1: *const i32, p2: *const i32, m: i32, target: *const u32, hits: *mut Hit, max_hits: i32) -> i32;
    fn pg_launch(slot: i32, seed: *const u8, p1: *const i32, p2: *const i32, m: i32, target: *const u32) -> i32;
    fn pg_wait(slot: i32, hits: *mut Hit, max_hits: i32) -> i32;
    fn pg_debug_al(seed: *const u8, m: i32, row0: i32, rows: i32, out: *mut i8) -> i32;
    fn pg_last_error() -> *const std::os::raw::c_char;
    fn pg_device_pci_bus_id(device: i32, out: *mut std::os::raw::c_char, len: i32) -> i32;
    fn pg_device_uuid(device: i32, out: *mut std::os::raw::c_char, len: i32) -> i32;
    fn pg_kernel_label() -> *const std::os::raw::c_char;
    fn pg_row_align() -> i32;
    fn pg_check_run(seed: *const u8, m: i32, reps: i32, digest: *mut u64, diff_words: *mut u64) -> i32;
}

fn last_error() -> String {
    unsafe {
        let p = pg_last_error();
        if p.is_null() { "?".into() } else { CStr::from_ptr(p).to_string_lossy().into_owned() }
    }
}

/// Number of CUDA devices visible to the process (honours CUDA_VISIBLE_DEVICES).
/// k the loaded libpearlgpu was built for.
pub fn kdim() -> usize {
    unsafe { pg_kdim().max(0) as usize }
}

pub fn device_count() -> usize {
    unsafe { pg_device_count().max(0) as usize }
}

/// Number of SMs of CUDA device `device` (0 on error).
pub fn sm_count(device: i32) -> usize {
    unsafe { pg_sm_count(device).max(0) as usize }
}

pub fn device_name(device: i32) -> String {
    let mut buf = [0 as std::os::raw::c_char; 128];
    if unsafe { pg_device_name(device, buf.as_mut_ptr(), buf.len() as i32) } != 0 {
        return "?".into();
    }
    unsafe { CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().into_owned()
}

/// Binds the calling thread to `device` (the library state is per thread: one worker thread per GPU).
pub fn init(device: i32) -> Result<()> {
    if unsafe { pg_init(device) } != 0 {
        bail!("pg_init({device}): {}", last_error());
    }
    Ok(())
}

pub fn set_job(br_t: &[i8], q1: &[i32], q2: &[i32], n: usize, k: usize) -> Result<()> {
    assert_eq!(br_t.len(), n * 128);
    assert!(q1.len() == k && q2.len() == k);
    if unsafe { pg_set_job(br_t.as_ptr(), q1.as_ptr(), q2.as_ptr(), n as i32, k as i32) } != 0 {
        bail!("pg_set_job: {}", last_error());
    }
    Ok(())
}

/// Runs one attempt; returns (total hits reported, hits stored in `buf`).
pub fn attempt(seed: &[u8; 32], p1: &[i32], p2: &[i32], m: usize, target: &[u32; 8], buf: &mut [Hit]) -> Result<(usize, usize)> {
    let r = unsafe { pg_attempt(seed.as_ptr(), p1.as_ptr(), p2.as_ptr(), m as i32, target.as_ptr(), buf.as_mut_ptr(), buf.len() as i32) };
    if r < 0 {
        bail!("pg_attempt: {}", last_error());
    }
    let r = r as usize;
    Ok((r, r.min(buf.len()).min(64))) // the library copies at most 64 hits
}

/// Enqueues one attempt in `slot` (0 or 1) and returns immediately; collect it with `wait(slot)`.
pub fn launch(slot: i32, seed: &[u8; 32], p1: &[i32], p2: &[i32], m: usize, target: &[u32; 8]) -> Result<()> {
    if unsafe { pg_launch(slot, seed.as_ptr(), p1.as_ptr(), p2.as_ptr(), m as i32, target.as_ptr()) } != 0 {
        bail!("pg_launch: {}", last_error());
    }
    Ok(())
}

/// Waits for the attempt in `slot`; returns (total hits reported, hits stored in `buf`, at most 64).
pub fn wait(slot: i32, buf: &mut [Hit]) -> Result<(usize, usize)> {
    let r = unsafe { pg_wait(slot, buf.as_mut_ptr(), buf.len() as i32) };
    if r < 0 {
        bail!("pg_wait: {}", last_error());
    }
    let r = r as usize;
    Ok((r, r.min(buf.len()).min(64)))
}

pub fn debug_al(seed: &[u8; 32], m: usize, row0: usize, rows: usize) -> Result<Vec<i8>> {
    let mut out = vec![0i8; rows * 128];
    if unsafe { pg_debug_al(seed.as_ptr(), m as i32, row0 as i32, rows as i32, out.as_mut_ptr()) } != 0 {
        bail!("pg_debug_al: {}", last_error());
    }
    Ok(out)
}

/// PCI bus id ("0000:0D:00.0") of CUDA device `device` (ordinal as seen by the process).
pub fn device_pci_bus_id(device: i32) -> Result<String> {
    let mut buf = [0 as std::os::raw::c_char; 64];
    if unsafe { pg_device_pci_bus_id(device, buf.as_mut_ptr(), buf.len() as i32) } != 0 {
        bail!("pg_device_pci_bus_id({device}): {}", last_error());
    }
    Ok(unsafe { CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().into_owned())
}

/// UUID ("GPU-xxxxxxxx-...") of CUDA device `device`.
pub fn device_uuid(device: i32) -> Result<String> {
    let mut buf = [0 as std::os::raw::c_char; 64];
    if unsafe { pg_device_uuid(device, buf.as_mut_ptr(), buf.len() as i32) } != 0 {
        bail!("pg_device_uuid({device}): {}", last_error());
    }
    Ok(unsafe { CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().into_owned())
}

/// Row granularity of the kernel the library selected for the calling thread (128 for gen13, 256 for the Ada kernel t256).  Only
/// valid after `init` on this thread; an `m` that is not a multiple of it silently runs on gen13 (slower on Ada).
pub fn row_align() -> usize {
    unsafe { pg_row_align().max(128) as usize }
}

/// Identifier of the embedded search kernel build.
pub fn kernel_label() -> String {
    unsafe {
        let p = pg_kernel_label();
        if p.is_null() { "?".into() } else { CStr::from_ptr(p).to_string_lossy().into_owned() }
    }
}

/// `reps` back-to-back attempts of one seed in check mode (see pearlgpu.h): returns the 256-bit digest of the first run's tile
/// transcripts (deterministic for a given job and seed) and the number of 32-bit words in which the other runs differ from it.
pub fn check_run(seed: &[u8; 32], m: usize, reps: u32) -> Result<([u64; 4], u64)> {
    let (mut d, mut diff) = ([0u64; 4], 0u64);
    if unsafe { pg_check_run(seed.as_ptr(), m as i32, reps as i32, d.as_mut_ptr(), &mut diff) } != 0 {
        bail!("pg_check_run: {}", last_error());
    }
    Ok((d, diff))
}
