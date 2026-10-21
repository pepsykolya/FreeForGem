// FreeForGemPear — see LICENSE
//! Windows scheduling knobs for the GPU-feeding path (experiment; all off unless the env variable is set):
//!   FFGP_TIMER1=1      timeBeginPeriod(1) for the process (1 ms timer resolution for waits)
//!   FFGP_NOTHROTTLE=1  opt the process out of power throttling (EcoQoS / execution-speed throttling, timer coalescing)
//!   FFGP_PROC_PRIO=1   process priority class ABOVE_NORMAL (2: HIGH, 0: leave Normal); default 1 with the AMD backend - also lifts the GPU runtime's own threads
//!                      above Normal-class CPU load (e.g. a CPU miner on the same box)
//!   FFGP_FEED_PRIO=1   GPU-feeding threads at THREAD_PRIORITY_ABOVE_NORMAL (2: HIGHEST)
//! The two-slot pipeline keeps only one attempt (~9 ms on a 6800 XT) queued ahead, so a feeding thread that wakes up
//! late leaves the GPU idle.
#[cfg(windows)]
mod sys {
    use std::ffi::c_void;
    #[link(name = "winmm")]
    unsafe extern "system" {
        pub fn timeBeginPeriod(u_period: u32) -> u32;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        pub fn GetCurrentThread() -> *mut c_void;
        pub fn SetThreadPriority(h: *mut c_void, priority: i32) -> i32;
        pub fn GetCurrentProcess() -> *mut c_void;
        pub fn SetProcessInformation(h: *mut c_void, class: i32, info: *const c_void, size: u32) -> i32;
        pub fn SetPriorityClass(h: *mut c_void, class: u32) -> i32;
        pub fn SetConsoleCtrlHandler(handler: Option<unsafe extern "system" fn(u32) -> i32>, add: i32) -> i32;
    }
    #[repr(C)]
    pub struct PowerThrottlingState {
        pub version: u32,
        pub control_mask: u32,
        pub state_mask: u32,
    }
}

fn env_level(name: &str) -> u32 {
    std::env::var(name).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(0)
}

/// Process-wide knobs; call once at start-up (Pearl mining).  `amd`: the AMD (OpenCL) backend is in use.
pub fn process(amd: bool) {
    #[cfg(windows)]
    unsafe {
        // AMD backend default: Above Normal, so the OpenCL runtime's submission threads and the feeding thread are not
        // starved by Normal-class CPU load; measured (6800 XT + xmrig on all cores): average 32.6 -> 33.6 TH/s, no dips
        let pp = match std::env::var("FFGP_PROC_PRIO") { Ok(_) => env_level("FFGP_PROC_PRIO"), Err(_) => amd as u32 };
        if pp > 0 {
            let class = if pp >= 2 { 0x80 } else { 0x8000 }; // HIGH / ABOVE_NORMAL_PRIORITY_CLASS
            let r = sys::SetPriorityClass(sys::GetCurrentProcess(), class);
            println!("wintune: priority class {class:#x} -> {}", if r != 0 { "ok" } else { "failed" });
        }
        if env_level("FFGP_TIMER1") > 0 {
            let r = sys::timeBeginPeriod(1);
            println!("wintune: timeBeginPeriod(1) -> {r}");
        }
        if env_level("FFGP_NOTHROTTLE") > 0 {
            // ProcessPowerThrottling = 4; control EXECUTION_SPEED (1) | IGNORE_TIMER_RESOLUTION (4), state 0 = off
            let st = sys::PowerThrottlingState { version: 1, control_mask: 0x1 | 0x4, state_mask: 0 };
            let r = sys::SetProcessInformation(
                sys::GetCurrentProcess(),
                4,
                &st as *const _ as *const std::ffi::c_void,
                std::mem::size_of::<sys::PowerThrottlingState>() as u32,
            );
            println!("wintune: power throttling off -> {}", if r != 0 { "ok" } else { "failed" });
        }
    }
}

/// Knobs for the calling GPU-feeding thread.
pub fn feed_thread() {
    #[cfg(windows)]
    unsafe {
        let l = env_level("FFGP_FEED_PRIO");
        if l > 0 {
            let p = if l >= 2 { 2 } else { 1 }; // THREAD_PRIORITY_HIGHEST / ABOVE_NORMAL
            let r = sys::SetThreadPriority(sys::GetCurrentThread(), p);
            println!("wintune: feed thread priority {p} -> {}", if r != 0 { "ok" } else { "failed" });
        }
    }
}

/// Windows: runs `f` (put the owner's GPU settings back) when the console window is closed or the user logs off / the PC
/// shuts down.  For these events Windows ends the process as soon as the console handlers return, so the normal shutdown
/// (the stop flag set by the Ctrl+C handler) never runs; `f` runs synchronously inside the handler instead (Windows waits
/// up to 5 s).  Ctrl+C / Ctrl+Break are left to the ctrlc handler.  No-op elsewhere.
pub fn on_console_close(f: fn()) {
    #[cfg(windows)]
    {
        static ON_CLOSE: std::sync::OnceLock<fn()> = std::sync::OnceLock::new();
        unsafe extern "system" fn handler(kind: u32) -> i32 {
            // CTRL_CLOSE_EVENT 2, CTRL_LOGOFF_EVENT 5, CTRL_SHUTDOWN_EVENT 6
            if matches!(kind, 2 | 5 | 6) {
                if let Some(f) = ON_CLOSE.get() {
                    f();
                }
            }
            0 // FALSE: the next handler (ctrlc: sets the stop flag) runs too
        }
        if ON_CLOSE.set(f).is_ok() {
            // registered after ctrlc's handler, so it is called first (Windows calls the handlers last-registered first)
            unsafe { sys::SetConsoleCtrlHandler(Some(handler), 1) };
        }
    }
    #[cfg(not(windows))]
    let _ = f;
}
