// FreeForGemPear — see LICENSE
//! Idle mode (Windows, 1.7.5, `--idle`): mine only while nobody uses the PC.  A watcher thread polls every 250 ms:
//!   keyboard / mouse (GetLastInputInfo), gamepad (XInput packet number), a fullscreen game / presentation / "do not
//!   disturb" (SHQueryUserNotificationState), a program from `--idle-pause-for` (process list, every 2 s).
//!   other programs at work (every second): their share of the CPU (all processes but the miner and its children: undervolt
//!   checks) above `--idle-cpu` % for 3 s, or a GPU engine of another program (3D / compute / copy above
//!   `--idle-gpu` %, video decode at all: a film or a stream) - Windows performance counters (PDH, "GPU Engine").
//! Any of them = the user is active: at once `--active pause` (no mining) or `light:PCT` (PCT % of the time).  Idle again only
//! after `--idle-minutes` with none of them; a locked screen / screensaver (QUNS_NOT_PRESENT) counts as idle at once.
//! The miner reads `mode()`; what a pause does to clocks, undervolt, checks and the fee is in main.rs / uv.rs / clocks.rs.
use anyhow::{Result, bail, ensure};

use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq)]
pub struct Opts {
    pub on: bool,
    pub minutes: f64,       // --idle-minutes: no input this long = idle
    pub active_pct: u8,     // --active: 0 = pause, 1..99 = light at this % of the time
    pub pause_for: Vec<String>, // --idle-pause-for: lower-case exe names that make the user "active" while they run
    pub cpu_pct: f64,           // --idle-cpu: other programs above this % of the whole CPU = in use (0 = off)
    pub gpu_pct: f64,           // --idle-gpu: another program's GPU engine above this % = in use (0 = off)
    pub ignore: Vec<String>,    // --idle-ignore: more programs whose CPU / GPU load is not "the PC in use" (besides MINERS)
}

/// Other miners are not someone using the PC (PC_White 1.7.5 test: xmrig on the CPU kept the GPU paused for good): their CPU
/// and GPU load is not counted.  A miner hidden under a system name (PC_BOX: tcpsvcs.exe) still counts.
pub const MINERS: &[&str] = &[
    "xmrig.exe", "xmrig-notls.exe", "xmrig-cuda.exe", "srbminer-multi.exe", "srbminer.exe", "t-rex.exe", "lolminer.exe",
    "nbminer.exe", "teamredminer.exe", "rigel.exe", "onezerominer.exe", "bzminer.exe", "nanominer.exe", "wildrig.exe",
    "gminer.exe", "excavator.exe", "xmr-stak.exe", "cpuminer.exe", "cpuminer-opt.exe", "ffgs.exe", "freeforgempear.exe",
    "freeforgem.exe", "ffg-cpu.exe", "kryptex.exe",
];

impl Default for Opts {
    fn default() -> Opts {
        Opts { on: false, minutes: 5.0, active_pct: 0, pause_for: Vec::new(), cpu_pct: 25.0, gpu_pct: 10.0, ignore: Vec::new() }
    }
}

/// `--active` value: `pause`, `light` (30 %), `light:PCT` (1-99).
pub fn parse_active(v: &str) -> Result<u8> {
    let v = v.trim().to_ascii_lowercase();
    if v == "pause" {
        return Ok(0);
    }
    let pct = match v.strip_prefix("light") {
        Some("") => 30,
        Some(r) => match r.strip_prefix(':').or_else(|| r.strip_prefix('=')).map(|p| p.trim().trim_end_matches('%').parse::<u8>()) {
            Some(Ok(p)) => p,
            _ => bail!("--active: '{v}' is not pause, light or light:PCT"),
        },
        None => bail!("--active: '{v}' is not pause, light or light:PCT"),
    };
    ensure!((1..=99).contains(&pct), "--active light:{pct}: the load must be 1-99 % (0 = pause)");
    Ok(pct)
}

/// `--idle-cpu` / `--idle-gpu`: a share in % (0 = this signal off).
pub fn parse_pct(v: &str, flag: &str) -> Result<f64> {
    let p: f64 = v.trim().trim_end_matches('%').parse().map_err(|_| anyhow::anyhow!("{flag}: '{v}' is not a percentage"))?;
    ensure!((0.0..=100.0).contains(&p), "{flag}: {p} is not 0-100 %");
    Ok(p)
}

/// `--idle-pause-for`: comma list of program names (`cs2.exe`, `blender`): `.exe` added when missing, lower case.
pub fn parse_list(v: &str) -> Vec<String> {
    v.split(',')
        .map(|s| s.trim().trim_matches('"').to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .map(|s| if s.ends_with(".exe") { s } else { format!("{s}.exe") })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    Full,     // idle (or idle mode off): mine at full load
    Pause,    // the user is active: no mining
    Light(u8), // the user is active: mine PCT % of the time
}

// 0 = full, 1 = user active
static STATE: AtomicU8 = AtomicU8::new(0);
static ACTIVE_PCT: AtomicU8 = AtomicU8::new(0);

/// What the miner should do now.  Always `Full` without `--idle`.
pub fn mode() -> Mode {
    if STATE.load(Ordering::Relaxed) == 0 {
        Mode::Full
    } else {
        match ACTIVE_PCT.load(Ordering::Relaxed) {
            0 => Mode::Pause,
            p => Mode::Light(p),
        }
    }
}

/// Starts the watcher.  Windows only; elsewhere (HiveOS, Linux) `--idle` is ignored with a note.  Returns an error when it
/// cannot work (a session without a desktop: a service / a task "run whether the user is logged on or not").
pub fn start(o: &Opts) -> Result<()> {
    if !o.on {
        return Ok(());
    }
    #[cfg(not(windows))]
    {
        println!("idle: --idle is for Windows PCs; ignored here (mining at full load)");
        Ok(())
    }
    #[cfg(windows)]
    {
        ensure!(
            sys::session_id() != Some(0),
            "--idle: the miner runs without a desktop (Windows session 0: a service, or a scheduled task \"run whether the user is logged on or not\"), where keyboard and mouse cannot be seen. Start it in the user's session (start.bat, or a task \"at log on\" for that user)"
        );
        ACTIVE_PCT.store(o.active_pct, Ordering::Relaxed);
        println!(
            "idle: mining only while the PC is idle ({} min without keyboard, mouse or gamepad, no fullscreen game{}{}{}); while it is used: {}",
            fmt_min(o.minutes),
            if o.cpu_pct > 0.0 { format!(", other programs under {:.0} % CPU", o.cpu_pct) } else { String::new() },
            if o.gpu_pct > 0.0 { format!(", no other program on the GPU (over {:.0} %, or playing video)", o.gpu_pct) } else { String::new() },
            if o.pause_for.is_empty() { String::new() } else { format!(", none of {} running", o.pause_for.join(", ")) },
            if o.active_pct == 0 { "mining paused".to_string() } else { format!("light load, {} % of the time", o.active_pct) }
        );
        let o = o.clone();
        std::thread::Builder::new().name("idle".into()).spawn(move || watch(o))?;
        Ok(())
    }
}

fn fmt_min(m: f64) -> String {
    if m.fract() == 0.0 { format!("{m:.0}") } else { format!("{m:.1}") }
}

/// The decision, separate from the Windows calls (tests): `busy` = a signal right now, `quiet` = time since the last input or
/// busy signal, `away` = locked screen / screensaver.  Returns the new state (true = the PC is in use).
pub(crate) fn decide(active: bool, busy: bool, quiet: Duration, away: bool, idle_after: Duration) -> bool {
    if away {
        return false;
    }
    busy || (quiet < idle_after && (active || quiet < Duration::from_secs(1)))
}

#[cfg(windows)]
fn watch(o: Opts) {
    let idle_after = Duration::from_secs_f64(o.minutes * 60.0);
    let mut pad = sys::Pad::load();
    let mut ignore: Vec<String> = MINERS.iter().map(|m| m.to_string()).collect();
    ignore.extend(o.ignore.iter().cloned());
    let mut load = sys::Load::new(o.gpu_pct > 0.0, ignore);
    // test hook: FFG_IDLE_TEST_FILE=path ignores the PC; "in use" while that file exists
    let test_file = std::env::var_os("FFG_IDLE_TEST_FILE").map(std::path::PathBuf::from);
    let mut slow_at = Instant::now() - Duration::from_secs(10);
    let debug = std::env::var_os("FFG_IDLE_DEBUG").is_some();
    let (mut proc_busy, mut load_busy): (Option<String>, Option<String>) = (None, None);
    // the last moment a busy signal (fullscreen, a program at work, the gamepad) was seen: idle counts from the later of it and input
    let mut busy_seen: Option<Instant> = None;
    let mut active = false;
    let mut since = Instant::now();
    loop {
        std::thread::sleep(Duration::from_millis(250));
        let (busy, since_input, away): (Option<String>, Duration, bool) = if let Some(f) = &test_file {
            (f.exists().then(|| "test file".to_string()), Duration::MAX, false)
        } else {
            if slow_at.elapsed() >= Duration::from_secs(1) {
                slow_at = Instant::now();
                if !o.pause_for.is_empty() {
                    proc_busy = sys::running(&o.pause_for);
                }
                load_busy = load.sample(o.cpu_pct, o.gpu_pct);
                if debug {
                    println!("idle: debug: input {:.1} s ago, other programs: {}", sys::since_input().unwrap_or_default().as_secs_f64(), load_busy.as_deref().unwrap_or("quiet"));
                }
            }
            let quns = sys::notification_state();
            let busy = match quns {
                Some(sys::QUNS_BUSY) | Some(sys::QUNS_RUNNING_D3D_FULL_SCREEN) => Some("a fullscreen program".to_string()),
                Some(sys::QUNS_PRESENTATION_MODE) => Some("presentation mode".to_string()),
                _ if proc_busy.is_some() => proc_busy.as_ref().map(|p| format!("{p} running")),
                _ if load_busy.is_some() => load_busy.clone(),
                _ if pad.as_mut().is_some_and(|p| p.moved()) => Some("gamepad".to_string()),
                _ => None,
            };
            (busy, sys::since_input().unwrap_or(Duration::ZERO), quns == Some(sys::QUNS_NOT_PRESENT))
        };
        if busy.is_some() {
            busy_seen = Some(Instant::now());
        }
        let quiet = since_input.min(busy_seen.map_or(Duration::MAX, |t| t.elapsed()));
        let now_active = decide(active, busy.is_some(), quiet, away, idle_after);
        if now_active != active {
            let held = since.elapsed();
            since = Instant::now();
            active = now_active;
            STATE.store(active as u8, Ordering::Relaxed);
            if active {
                let what = busy.unwrap_or_else(|| "keyboard / mouse".into());
                let todo = match ACTIVE_PCT.load(Ordering::Relaxed) {
                    0 => "mining paused".to_string(),
                    p => format!("light load ({p} %)"),
                };
                println!("idle: the PC is in use ({what}): {todo} (mined {} at full load)", fmt_dur(held));
            } else {
                println!(
                    "idle: {}: full load (was in use {})",
                    if away { "screen locked".to_string() } else { format!("idle for {} min", fmt_min(o.minutes)) },
                    fmt_dur(held)
                );
            }
        }
    }
}

fn fmt_dur(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 { format!("{}h{:02}m", s / 3600, s % 3600 / 60) } else if s >= 60 { format!("{}m{:02}s", s / 60, s % 60) } else { format!("{s}s") }
}

#[cfg(windows)]
mod sys {
    use std::ffi::c_void;
    use std::time::Duration;

    pub const QUNS_NOT_PRESENT: i32 = 1;
    pub const QUNS_BUSY: i32 = 2;
    pub const QUNS_RUNNING_D3D_FULL_SCREEN: i32 = 3;
    pub const QUNS_PRESENTATION_MODE: i32 = 4;

    #[repr(C)]
    struct LastInput {
        size: u32,
        time: u32,
    }
    #[repr(C)]
    struct ProcEntry {
        size: u32,
        usage: u32,
        pid: u32,
        heap: usize,
        module: u32,
        threads: u32,
        parent: u32,
        prio: i32,
        flags: u32,
        exe: [u16; 260],
    }
    #[repr(C)]
    #[derive(Default)]
    struct XState {
        packet: u32,
        buttons: u16,
        lt: u8,
        rt: u8,
        lx: i16,
        ly: i16,
        rx: i16,
        ry: i16,
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetLastInputInfo(p: *mut LastInput) -> i32;
    }
    #[link(name = "shell32")]
    unsafe extern "system" {
        fn SHQueryUserNotificationState(p: *mut i32) -> i32;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetTickCount() -> u32;
        fn GetCurrentProcessId() -> u32;
        fn ProcessIdToSessionId(pid: u32, sid: *mut u32) -> i32;
        fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> *mut c_void;
        fn Process32FirstW(h: *mut c_void, e: *mut ProcEntry) -> i32;
        fn Process32NextW(h: *mut c_void, e: *mut ProcEntry) -> i32;
        fn CloseHandle(h: *mut c_void) -> i32;
        fn LoadLibraryA(name: *const std::ffi::c_char) -> *mut c_void;
        fn GetProcAddress(h: *mut c_void, name: *const std::ffi::c_char) -> *mut c_void;
    }

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct FileTime {
        lo: u32,
        hi: u32,
    }
    impl FileTime {
        fn ticks(self) -> u64 {
            (self.hi as u64) << 32 | self.lo as u64
        }
    }
    #[repr(C)]
    struct PdhItem {
        name: *mut u16,
        status: u32,
        _pad: u32,
        value: f64,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetSystemTimes(idle: *mut FileTime, kernel: *mut FileTime, user: *mut FileTime) -> i32;
        fn GetProcessTimes(h: *mut c_void, c: *mut FileTime, e: *mut FileTime, k: *mut FileTime, u: *mut FileTime) -> i32;
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
    }
    #[link(name = "pdh")]
    unsafe extern "system" {
        fn PdhOpenQueryW(src: *const u16, user: usize, q: *mut isize) -> u32;
        fn PdhAddEnglishCounterW(q: isize, path: *const u16, user: usize, c: *mut isize) -> u32;
        fn PdhCollectQueryData(q: isize) -> u32;
        fn PdhGetFormattedCounterArrayW(c: isize, fmt: u32, size: *mut u32, count: *mut u32, items: *mut PdhItem) -> u32;
    }

    /// (pid, parent pid, lower-case exe name) of every process
    fn processes() -> Vec<(u32, u32, String)> {
        let mut v = Vec::new();
        let h = unsafe { CreateToolhelp32Snapshot(2, 0) };
        if h.is_null() || h as isize == -1 {
            return v;
        }
        let mut e: ProcEntry = unsafe { std::mem::zeroed() };
        e.size = std::mem::size_of::<ProcEntry>() as u32;
        let mut ok = unsafe { Process32FirstW(h, &mut e) } != 0;
        while ok {
            let n = e.exe.iter().position(|&c| c == 0).unwrap_or(e.exe.len());
            v.push((e.pid, e.parent, String::from_utf16_lossy(&e.exe[..n]).to_ascii_lowercase()));
            ok = unsafe { Process32NextW(h, &mut e) } != 0;
        }
        unsafe { CloseHandle(h) };
        v
    }

    /// CPU time (100 ns ticks) of a process; None when it cannot be opened
    fn cpu_ticks(pid: u32) -> Option<u64> {
        const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
        let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if h.is_null() {
            return None;
        }
        let (mut c, mut e, mut k, mut u) = (FileTime::default(), FileTime::default(), FileTime::default(), FileTime::default());
        let ok = unsafe { GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u) } != 0;
        unsafe { CloseHandle(h) };
        ok.then(|| k.ticks() + u.ticks())
    }

    /// The load of the other programs: CPU share (system busy time minus the miner's and its children's) and GPU engines.
    pub struct Load {
        last: Option<(u64, u64, u64)>, // system busy, system total, miner tree CPU ticks
        ours: std::collections::HashMap<u32, u64>,
        cpu_hot: u32,                // seconds in a row above the CPU threshold
        gpu: Option<(isize, isize)>, // PDH query, counter
        ignore: Vec<String>,         // lower-case exe names whose load does not count (other miners)
    }

    impl Load {
        pub fn new(gpu: bool, ignore: Vec<String>) -> Load {
            let gpu = if gpu {
                let (mut q, mut c) = (0isize, 0isize);
                let path: Vec<u16> = "\\GPU Engine(*)\\Utilization Percentage\0".encode_utf16().collect();
                let ok = unsafe { PdhOpenQueryW(std::ptr::null(), 0, &mut q) } == 0
                    && unsafe { PdhAddEnglishCounterW(q, path.as_ptr(), 0, &mut c) } == 0
                    && unsafe { PdhCollectQueryData(q) } == 0;
                if !ok {
                    println!("idle: the GPU performance counters are not available: other programs' GPU use is not seen");
                }
                ok.then_some((q, c))
            } else {
                None
            };
            Load { last: None, ours: Default::default(), cpu_hot: 0, gpu, ignore }
        }

        /// Called every second: why the PC counts as in use (None = the other programs are quiet).
        pub fn sample(&mut self, cpu_pct: f64, gpu_pct: f64) -> Option<String> {
            let procs = processes();
            let me = unsafe { GetCurrentProcessId() };
            // the miner and its descendants (undervolt check processes)
            // the miner, the programs whose load does not count (other miners), and their descendants
            let mut tree = vec![me];
            tree.extend(procs.iter().filter(|x| self.ignore.iter().any(|n| *n == x.2)).map(|x| x.0));
            let mut i = 0;
            while i < tree.len() {
                let p = tree[i];
                let kids: Vec<u32> = procs.iter().filter(|x| x.1 == p && x.0 != p && !tree.contains(&x.0)).map(|x| x.0).collect();
                tree.extend(kids);
                i += 1;
            }
            let mut why = None;
            if cpu_pct > 0.0 {
                // ended children keep their last ticks in `ours` (their time stays counted as the miner's)
                for &p in &tree {
                    if let Some(t) = cpu_ticks(p) {
                        self.ours.insert(p, t);
                    }
                }
                let mine: u64 = self.ours.values().sum();
                let (mut idle, mut k, mut u) = (FileTime::default(), FileTime::default(), FileTime::default());
                if unsafe { GetSystemTimes(&mut idle, &mut k, &mut u) } != 0 {
                    let total = k.ticks() + u.ticks(); // kernel time includes idle
                    let busy = total.saturating_sub(idle.ticks());
                    if let Some((b0, t0, m0)) = self.last {
                        let dt = total.saturating_sub(t0).max(1) as f64;
                        let other = busy.saturating_sub(b0).saturating_sub(mine.saturating_sub(m0)) as f64 / dt * 100.0;
                        // 3 s in a row: a short spike (a page load, an update check) does not stop mining
                        self.cpu_hot = if other >= cpu_pct { self.cpu_hot + 1 } else { 0 };
                        if self.cpu_hot >= 3 {
                            why = Some(format!("other programs use {other:.0} % CPU"));
                        }
                    }
                    self.last = Some((busy, total, mine));
                }
            }
            if let (Some((q, c)), None) = (self.gpu, &why) {
                if unsafe { PdhCollectQueryData(q) } == 0 {
                    const FMT: u32 = 0x200 | 0x8000; // PDH_FMT_DOUBLE | PDH_FMT_NOCAP100
                    let (mut size, mut n) = (0u32, 0u32);
                    unsafe { PdhGetFormattedCounterArrayW(c, FMT, &mut size, &mut n, std::ptr::null_mut()) };
                    let mut buf = vec![0u64; (size as usize).div_ceil(8) + 1];
                    if size > 0 && unsafe { PdhGetFormattedCounterArrayW(c, FMT, &mut size, &mut n, buf.as_mut_ptr().cast()) } == 0 {
                        let items = unsafe { std::slice::from_raw_parts(buf.as_ptr().cast::<PdhItem>(), n as usize) };
                        let mut best = 0.0;
                        for it in items {
                            if it.status != 0 || it.name.is_null() {
                                continue;
                            }
                            let len = (0..).take_while(|&k| unsafe { *it.name.add(k) } != 0).count();
                            let name = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(it.name, len) });
                            // "pid_1234_luid_0x..._phys_0_eng_0_engtype_3D"
                            let Some(pid) = name.strip_prefix("pid_").and_then(|r| r.split('_').next()).and_then(|p| p.parse::<u32>().ok()) else {
                                continue;
                            };
                            if tree.contains(&pid) {
                                continue;
                            }
                            let exe = procs.iter().find(|x| x.0 == pid).map_or("", |x| x.2.as_str());
                            if exe == "dwm.exe" {
                                continue; // the desktop compositor draws whatever moves on the screen
                            }
                            let video = name.contains("engtype_VideoDecode");
                            let (limit, what) = if video { (1.0, "video decode") } else { (gpu_pct, "GPU") };
                            if it.value >= limit && it.value > best {
                                best = it.value;
                                let who = if exe.is_empty() { format!("pid {pid}") } else { exe.to_string() };
                                why = Some(format!("{who} uses {:.0} % {what}", it.value));
                            }
                        }
                    }
                }
            }
            why
        }
    }

    pub fn session_id() -> Option<u32> {
        let mut s = 0u32;
        (unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut s) } != 0).then_some(s)
    }

    /// Time since the last keyboard / mouse input of this session.
    pub fn since_input() -> Option<Duration> {
        let mut li = LastInput { size: std::mem::size_of::<LastInput>() as u32, time: 0 };
        if unsafe { GetLastInputInfo(&mut li) } == 0 {
            return None;
        }
        Some(Duration::from_millis(unsafe { GetTickCount() }.wrapping_sub(li.time) as u64))
    }

    pub fn notification_state() -> Option<i32> {
        let mut s = 0i32;
        (unsafe { SHQueryUserNotificationState(&mut s) } == 0).then_some(s)
    }

    /// The first program of `names` (lower-case exe names) that runs now.
    pub fn running(names: &[String]) -> Option<String> {
        const TH32CS_SNAPPROCESS: u32 = 2;
        let h = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if h.is_null() || h as isize == -1 {
            return None;
        }
        let mut e: ProcEntry = unsafe { std::mem::zeroed() };
        e.size = std::mem::size_of::<ProcEntry>() as u32;
        let mut found = None;
        let mut ok = unsafe { Process32FirstW(h, &mut e) } != 0;
        while ok {
            let n = e.exe.iter().position(|&c| c == 0).unwrap_or(e.exe.len());
            let exe = String::from_utf16_lossy(&e.exe[..n]).to_ascii_lowercase();
            if names.iter().any(|x| *x == exe) {
                found = Some(exe);
                break;
            }
            ok = unsafe { Process32NextW(h, &mut e) } != 0;
        }
        unsafe { CloseHandle(h) };
        found
    }

    type XGet = unsafe extern "system" fn(u32, *mut XState) -> u32;

    /// XInput gamepads (up to 4): a changed packet number = the user touched a pad.
    pub struct Pad {
        get: XGet,
        last: [Option<u32>; 4],
    }

    impl Pad {
        pub fn load() -> Option<Pad> {
            for dll in [&b"xinput1_4.dll\0"[..], b"xinput9_1_0.dll\0"] {
                let m = unsafe { LoadLibraryA(dll.as_ptr().cast()) };
                if m.is_null() {
                    continue;
                }
                let f = unsafe { GetProcAddress(m, b"XInputGetState\0".as_ptr().cast()) };
                if !f.is_null() {
                    return Some(Pad { get: unsafe { std::mem::transmute::<*mut c_void, XGet>(f) }, last: [None; 4] });
                }
            }
            None
        }

        pub fn moved(&mut self) -> bool {
            let mut moved = false;
            for i in 0..4 {
                let mut s = XState::default();
                let now = (unsafe { (self.get)(i as u32, &mut s) } == 0).then_some(s.packet);
                if let (Some(a), Some(b)) = (self.last[i], now) {
                    moved |= a != b;
                }
                self.last[i] = now;
            }
            moved
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_values() {
        assert_eq!(parse_active("pause").unwrap(), 0);
        assert_eq!(parse_active("light").unwrap(), 30);
        assert_eq!(parse_active("Light:45").unwrap(), 45);
        assert_eq!(parse_active("light:20%").unwrap(), 20);
        assert!(parse_active("light:0").is_err());
        assert!(parse_active("light:100").is_err());
        assert!(parse_active("half").is_err());
        assert_eq!(parse_list(" CS2.exe, blender ,,"), vec!["cs2.exe".to_string(), "blender.exe".to_string()]);
    }

    #[test]
    fn decisions() {
        let five = Duration::from_secs(300);
        let s = Duration::from_secs;
        // fresh input: in use at once; stays in use until 5 min without input
        assert!(decide(false, false, s(0), false, five));
        assert!(decide(true, false, s(200), false, five));
        assert!(!decide(true, false, s(301), false, five));
        // idle, the last input long ago (a timer that has not run out yet): stays idle
        assert!(!decide(false, false, s(100), false, five));
        // a fullscreen game / a busy program with no input: in use
        assert!(decide(false, true, s(900), false, five));
        // locked screen: idle at once
        assert!(!decide(true, false, s(0), true, five));
        assert_eq!(parse_pct("25%", "--idle-cpu").unwrap(), 25.0);
        assert!(parse_pct("120", "--idle-cpu").is_err());
    }
}
