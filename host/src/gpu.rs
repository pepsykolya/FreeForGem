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
    fn pg_device_name(device: i32, out: *mut std::os::raw::c_char, len: i32) -> i32;
    fn pg_init(device: i32) -> i32;
    fn pg_set_job(br_t: *const i8, q1: *const i32, q2: *const i32, n: i32, k: i32) -> i32;
    fn pg_attempt(seed: *const u8, p1: *const i32, p2: *const i32, m: i32, target: *const u32, hits: *mut Hit, max_hits: i32) -> i32;
    fn pg_launch(slot: i32, seed: *const u8, p1: *const i32, p2: *const i32, m: i32, target: *const u32) -> i32;
    fn pg_wait(slot: i32, hits: *mut Hit, max_hits: i32) -> i32;
    fn pg_debug_al(seed: *const u8, m: i32, row0: i32, rows: i32, out: *mut i8) -> i32;
    fn pg_last_error() -> *const std::os::raw::c_char;
}

fn last_error() -> String {
    unsafe {
        let p = pg_last_error();
        if p.is_null() { "?".into() } else { CStr::from_ptr(p).to_string_lossy().into_owned() }
    }
}

/// Number of CUDA devices visible to the process (honours CUDA_VISIBLE_DEVICES).
pub fn device_count() -> usize {
    unsafe { pg_device_count().max(0) as usize }
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
