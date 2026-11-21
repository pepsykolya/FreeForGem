// FreeForGemPear — see LICENSE
//! NVML loaded at run time (libnvidia-ml.so.1 / nvml.dll): no link-time dependency, the miner starts without it.
//! Only the calls the opt-in undervolt tuner (uv.rs) needs.
use anyhow::{Result, anyhow, bail};
use std::ffi::{CStr, CString, c_char, c_int, c_uint, c_void};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[cfg(unix)]
mod sys {
    use std::ffi::{c_char, c_int, c_void};
    #[link(name = "dl")]
    unsafe extern "C" {
        pub fn dlopen(file: *const c_char, flags: c_int) -> *mut c_void;
        pub fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    }
    pub unsafe fn open(name: *const c_char) -> *mut c_void {
        unsafe { dlopen(name, 2) } // RTLD_NOW
    }
    pub unsafe fn sym(h: *mut c_void, name: *const c_char) -> *mut c_void {
        unsafe { dlsym(h, name) }
    }
}

#[cfg(windows)]
mod sys {
    use std::ffi::{c_char, c_void};
    unsafe extern "system" {
        fn LoadLibraryA(name: *const c_char) -> *mut c_void;
        fn GetProcAddress(h: *mut c_void, name: *const c_char) -> *mut c_void;
    }
    pub unsafe fn open(name: *const c_char) -> *mut c_void {
        unsafe { LoadLibraryA(name) }
    }
    pub unsafe fn sym(h: *mut c_void, name: *const c_char) -> *mut c_void {
        unsafe { GetProcAddress(h, name) }
    }
}

/// Opaque nvmlDevice_t.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Dev(pub *mut c_void);
unsafe impl Send for Dev {}
unsafe impl Sync for Dev {}

#[repr(C)]
struct PciInfo {
    bus_id_legacy: [c_char; 16],
    domain: c_uint,
    bus: c_uint,
    device: c_uint,
    pci_device_id: c_uint,
    pci_subsystem_id: c_uint,
    bus_id: [c_char; 32],
}

#[repr(C)]
struct ClockOffset {
    version: c_uint,
    clock_type: c_uint, // NVML_CLOCK_GRAPHICS = 0
    pstate: c_uint,     // NVML_PSTATE_0 = 0
    offset: c_int,
    min: c_int,
    max: c_int,
}
const CLOCK_OFFSET_V1: c_uint = (std::mem::size_of::<ClockOffset>() as c_uint) | (1 << 24);

#[repr(C)]
struct EventData {
    device: *mut c_void,
    event_type: u64,
    event_data: u64,
    gpu_instance: c_uint,
    compute_instance: c_uint,
}

const XID_CRITICAL: u64 = 0x8;

/// Which API drives the graphics-clock V/F offset on this driver.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OffMode {
    Gpc,   // nvmlDeviceGet/SetGpcClkVfOffset
    Clock, // nvmlDeviceGet/SetClockOffsets (newer drivers)
}

/// The Xid event set of one `Nvml`: the set, drained events not yet taken (device handle, Xid), the handles registered in it.
struct Ev {
    set: *mut c_void,
    buf: Vec<(usize, u64)>,
    regd: Vec<usize>,
}
impl Default for Ev {
    fn default() -> Ev {
        Ev { set: std::ptr::null_mut(), buf: Vec::new(), regd: Vec::new() }
    }
}

type FnI = unsafe extern "C" fn() -> c_int;
type FnDevU = unsafe extern "C" fn(*mut c_void, *mut c_uint) -> c_int;

pub struct Nvml {
    shutdown: Option<FnI>,
    error_string: Option<unsafe extern "C" fn(c_int) -> *const c_char>,
    driver_version: unsafe extern "C" fn(*mut c_char, c_uint) -> c_int,
    count: unsafe extern "C" fn(*mut c_uint) -> c_int,
    by_bus: unsafe extern "C" fn(*const c_char, *mut *mut c_void) -> c_int,
    get_uuid: unsafe extern "C" fn(*mut c_void, *mut c_char, c_uint) -> c_int,
    get_name: unsafe extern "C" fn(*mut c_void, *mut c_char, c_uint) -> c_int,
    get_cc: Option<unsafe extern "C" fn(*mut c_void, *mut c_int, *mut c_int) -> c_int>,
    get_index: Option<FnDevU>,
    get_pci: unsafe extern "C" fn(*mut c_void, *mut PciInfo) -> c_int,
    get_power: FnDevU,
    get_pl: Option<FnDevU>,
    pm_get: Option<FnDevU>,
    pm_set: Option<unsafe extern "C" fn(*mut c_void, c_uint) -> c_int>,
    pm_minmax: Option<unsafe extern "C" fn(*mut c_void, *mut c_uint, *mut c_uint) -> c_int>,
    throttle: Option<unsafe extern "C" fn(*mut c_void, *mut u64) -> c_int>,
    get_clock: unsafe extern "C" fn(*mut c_void, c_uint, *mut c_uint) -> c_int,
    get_max_clock: Option<unsafe extern "C" fn(*mut c_void, c_uint, *mut c_uint) -> c_int>,
    get_temp: unsafe extern "C" fn(*mut c_void, c_uint, *mut c_uint) -> c_int,
    get_util: Option<unsafe extern "C" fn(*mut c_void, *mut [c_uint; 2]) -> c_int>,
    gpc_get: Option<unsafe extern "C" fn(*mut c_void, *mut c_int) -> c_int>,
    gpc_set: Option<unsafe extern "C" fn(*mut c_void, c_int) -> c_int>,
    gpc_minmax: Option<unsafe extern "C" fn(*mut c_void, *mut c_int, *mut c_int) -> c_int>,
    co_get: Option<unsafe extern "C" fn(*mut c_void, *mut ClockOffset) -> c_int>,
    co_set: Option<unsafe extern "C" fn(*mut c_void, *mut ClockOffset) -> c_int>,
    lgc_set: Option<unsafe extern "C" fn(*mut c_void, c_uint, c_uint) -> c_int>,
    lgc_reset: Option<unsafe extern "C" fn(*mut c_void) -> c_int>,
    lmc_set: Option<unsafe extern "C" fn(*mut c_void, c_uint, c_uint) -> c_int>,
    lmc_reset: Option<unsafe extern "C" fn(*mut c_void) -> c_int>,
    sup_mem: Option<unsafe extern "C" fn(*mut c_void, *mut c_uint, *mut c_uint) -> c_int>,
    sup_gr: Option<unsafe extern "C" fn(*mut c_void, c_uint, *mut c_uint, *mut c_uint) -> c_int>,
    ev_create: Option<unsafe extern "C" fn(*mut *mut c_void) -> c_int>,
    ev_register: Option<unsafe extern "C" fn(*mut c_void, u64, *mut c_void) -> c_int>,
    ev_wait: Option<unsafe extern "C" fn(*mut c_void, *mut EventData, c_uint) -> c_int>,
    events: Mutex<Ev>,
}
unsafe impl Send for Nvml {}
unsafe impl Sync for Nvml {}

impl Nvml {
    /// Loads the NVML library and initialises it.
    pub fn load() -> Result<Nvml> {
        let names: &[&str] = if cfg!(windows) {
            &["nvml.dll", "C:\\Program Files\\NVIDIA Corporation\\NVSMI\\nvml.dll"]
        } else {
            &["libnvidia-ml.so.1", "libnvidia-ml.so"]
        };
        let mut h = std::ptr::null_mut();
        for n in names {
            let c = CString::new(*n)?;
            h = unsafe { sys::open(c.as_ptr()) };
            if !h.is_null() {
                break;
            }
        }
        if h.is_null() {
            bail!("{} not found (NVIDIA driver without NVML?)", names[0]);
        }
        let sym = |n: &str| -> *mut c_void {
            let c = CString::new(n).unwrap();
            unsafe { sys::sym(h, c.as_ptr()) }
        };
        macro_rules! req {
            ($n:expr) => {{
                let p = sym($n);
                if p.is_null() {
                    bail!("NVML symbol {} missing (driver too old)", $n);
                }
                unsafe { std::mem::transmute(p) }
            }};
        }
        macro_rules! opt {
            ($n:expr) => {{
                let p = sym($n);
                if p.is_null() { None } else { Some(unsafe { std::mem::transmute(p) }) }
            }};
        }
        let init: FnI = req!("nvmlInit_v2");
        let n = Nvml {
            shutdown: opt!("nvmlShutdown"),
            error_string: opt!("nvmlErrorString"),
            driver_version: req!("nvmlSystemGetDriverVersion"),
            count: req!("nvmlDeviceGetCount_v2"),
            by_bus: req!("nvmlDeviceGetHandleByPciBusId_v2"),
            get_uuid: req!("nvmlDeviceGetUUID"),
            get_name: req!("nvmlDeviceGetName"),
            get_cc: opt!("nvmlDeviceGetCudaComputeCapability"),
            get_index: opt!("nvmlDeviceGetIndex"),
            get_pci: req!("nvmlDeviceGetPciInfo_v3"),
            get_power: req!("nvmlDeviceGetPowerUsage"),
            get_pl: opt!("nvmlDeviceGetEnforcedPowerLimit"),
            pm_get: opt!("nvmlDeviceGetPowerManagementLimit"),
            pm_set: opt!("nvmlDeviceSetPowerManagementLimit"),
            pm_minmax: opt!("nvmlDeviceGetPowerManagementLimitConstraints"),
            // the newer name first (driver 535+), the older one is the same call
            throttle: {
                let p = sym("nvmlDeviceGetCurrentClocksEventReasons");
                let p = if p.is_null() { sym("nvmlDeviceGetCurrentClocksThrottleReasons") } else { p };
                if p.is_null() { None } else { Some(unsafe { std::mem::transmute(p) }) }
            },
            get_clock: req!("nvmlDeviceGetClockInfo"),
            get_max_clock: opt!("nvmlDeviceGetMaxClockInfo"),
            get_temp: req!("nvmlDeviceGetTemperature"),
            get_util: opt!("nvmlDeviceGetUtilizationRates"),
            gpc_get: opt!("nvmlDeviceGetGpcClkVfOffset"),
            gpc_set: opt!("nvmlDeviceSetGpcClkVfOffset"),
            gpc_minmax: opt!("nvmlDeviceGetGpcClkMinMaxVfOffset"),
            co_get: opt!("nvmlDeviceGetClockOffsets"),
            co_set: opt!("nvmlDeviceSetClockOffsets"),
            lgc_set: opt!("nvmlDeviceSetGpuLockedClocks"),
            lgc_reset: opt!("nvmlDeviceResetGpuLockedClocks"),
            lmc_set: opt!("nvmlDeviceSetMemoryLockedClocks"),
            lmc_reset: opt!("nvmlDeviceResetMemoryLockedClocks"),
            sup_mem: opt!("nvmlDeviceGetSupportedMemoryClocks"),
            sup_gr: opt!("nvmlDeviceGetSupportedGraphicsClocks"),
            ev_create: opt!("nvmlEventSetCreate"),
            ev_register: opt!("nvmlDeviceRegisterEvents"),
            ev_wait: opt!("nvmlEventSetWait_v2"),
            events: Mutex::new(Ev::default()),
        };
        let r = unsafe { init() };
        if r != 0 {
            bail!("nvmlInit failed: {}", n.err(r));
        }
        Ok(n)
    }

    pub fn err(&self, r: c_int) -> String {
        if let Some(f) = self.error_string {
            let p = unsafe { f(r) };
            if !p.is_null() {
                return format!("{} (code {r})", unsafe { CStr::from_ptr(p) }.to_string_lossy());
            }
        }
        format!("NVML error {r}")
    }

    fn ck(&self, r: c_int, what: &str) -> Result<()> {
        if r == 0 { Ok(()) } else { Err(anyhow!("{what}: {}", self.err(r))) }
    }

    /// 1.0.12: the handle to call NVML with for `d` now, holding the GPU's call lock (one NVML call per GPU at a time); None in
    /// the GPU's quiet period after a fault (`gate`). Every per-GPU method below goes through here or `on_r`.
    fn on<R>(&self, d: Dev, f: impl FnOnce(*mut c_void) -> R) -> Option<R> {
        match self.gate(d, false) {
            Gate::Go(h, lock) => {
                let _g = lock.as_ref().map(|l| l.lock().unwrap_or_else(|e| e.into_inner()));
                Some(f(h))
            }
            Gate::Busy | Gate::Gone => None,
        }
    }

    fn on_r<R>(&self, d: Dev, f: impl FnOnce(*mut c_void) -> Result<R>) -> Result<R> {
        match self.gate(d, true) {
            Gate::Go(h, lock) => {
                let _g = lock.as_ref().map(|l| l.lock().unwrap_or_else(|e| e.into_inner()));
                f(h)
            }
            Gate::Busy => Err(anyhow!("{BUSY}")),
            Gate::Gone => Err(anyhow!("{GONE}")),
        }
    }

    /// 1.0.12: may `d` be called now, and with which handle?  Busy in the GPU's quiet period; when it ends, a fresh handle by the
    /// PCI bus id (never the one from before the fault) - Gone when the bus id gives none.  A handle this program did not get from
    /// `by_bus` is called as it is.
    /// FreeForGem 1.8.0 (every coin shares this file): a GPU whose coin reports its mining (`bind_dev` + `gpu_mining`: NEB) has
    /// FFGN's quiet period; a GPU of any other coin (Pearl, NOID, CMFD, PROX, Ting: no mining signal) has its time part only
    /// (QUIET_MIN), and there a call that returns a Result (`wait`: an offset / clock / power-limit set, the reads of the tuners)
    /// waits for the end of it instead of failing: those tuners set the offset back right after an Xid and take an error there
    /// as a failed card, so the set is late by at most QUIET_MIN, never lost.
    fn gate(&self, d: Dev, wait: bool) -> Gate {
        let me = d.0 as usize;
        let mut waited = false;
        let mut msg: Option<String> = None;
        loop {
            let mut reg = quiet::reg();
            let Some(i) = reg.iter().position(|s| s.handles.contains(&me)) else { return Gate::Go(d.0, None) };
            if let Some(t) = reg[i].fault {
                let el = t.elapsed();
                let exiting = quiet::EXITING.load(Ordering::Relaxed);
                let mined = reg[i].mined.is_some_and(|m| m.elapsed() >= quiet::QUIET_MINED);
                let mines = reg[i].mines;
                let done = el >= quiet::QUIET_MAX || (el >= quiet::QUIET_MIN && (mined || exiting || !mines));
                if !done {
                    if (exiting || (wait && !mines)) && !waited {
                        // the miner exits (the owner's offsets / clocks / power limit go back): wait for the time part of the quiet
                        // period only (the GPU does not mine any more)
                        drop(reg);
                        std::thread::sleep(quiet::QUIET_MIN.saturating_sub(el).min(quiet::QUIET_MIN));
                        waited = true;
                        continue;
                    }
                    return Gate::Busy;
                }
                let s = &mut reg[i];
                s.fault = None;
                s.mined = None;
                s.stale = true;
                quiet::QUIET_N.fetch_sub(1, Ordering::Relaxed);
                msg = Some(format!(
                    "nvml: GPU bus {}: NVML calls for this GPU resume {:.0} s after its fault{} (a fresh NVML handle by its bus id)",
                    crate::uv::short_bus(&s.shown),
                    el.as_secs_f64(),
                    if mined { ", its new engine context mines" } else if exiting { " (the miner exits)" } else if !mines { "" } else { " (its engine has not mined since: the longest quiet period)" }
                ));
            }
            if reg[i].stale {
                let (bus, lock) = (reg[i].shown.clone(), reg[i].lock.clone());
                drop(reg);
                if let Some(m) = msg.take() {
                    println!("{m}"); // not under the lock of the GPU table (the log pipe may be slow)
                }
                let fresh = {
                    let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
                    self.raw_by_bus(&bus)
                };
                let mut reg = quiet::reg();
                let s = &mut reg[i];
                if s.fault.is_some() {
                    return Gate::Busy; // a new fault meanwhile
                }
                let Ok(h) = fresh else { return Gate::Gone };
                let h = h as usize;
                let first = s.stale;
                s.stale = false;
                s.cur = h;
                s.epoch += 1;
                s.resumed = Some(Instant::now());
                if !s.handles.contains(&h) {
                    s.handles.push(h);
                }
                drop(reg);
                if first {
                    quiet::run_hooks(&bus);
                }
                return Gate::Go(h as *mut c_void, Some(lock));
            }
            return Gate::Go(reg[i].cur as *mut c_void, Some(reg[i].lock.clone()));
        }
    }

    /// 1.0.13 (recover.rs): the GPU's quiet period ends now, with a fresh handle (see `quiet::resume_with`).
    pub fn resume_after_fault(&self, bus: &str) -> Result<()> {
        quiet::resume_with(bus, |b| self.raw_by_bus(b).map(|h| h as usize))
    }

    fn raw_by_bus(&self, bus: &str) -> Result<*mut c_void> {
        let c = CString::new(bus)?;
        let mut h = std::ptr::null_mut();
        self.ck(unsafe { (self.by_bus)(c.as_ptr(), &mut h) }, &format!("nvmlDeviceGetHandleByPciBusId({bus})"))?;
        Ok(h)
    }

    pub fn driver_version(&self) -> String {
        let mut b = [0 as c_char; 96];
        if unsafe { (self.driver_version)(b.as_mut_ptr(), b.len() as c_uint) } != 0 {
            return "?".into();
        }
        unsafe { CStr::from_ptr(b.as_ptr()) }.to_string_lossy().into_owned()
    }

    pub fn device_count(&self) -> Result<u32> {
        let mut n = 0;
        self.ck(unsafe { (self.count)(&mut n) }, "nvmlDeviceGetCount")?;
        Ok(n)
    }

    /// Handle by PCI bus id ("0000:0D:00.0"), the only way this program ever selects a GPU.
    /// 1.0.12: none in the GPU's quiet period after a fault (an NVML call for that GPU); the handle is remembered for the GPU.
    pub fn by_bus(&self, bus: &str) -> Result<Dev> {
        let lock = quiet::slot_for_call(bus)?;
        let h = {
            let _g = lock.as_ref().map(|l| l.lock().unwrap_or_else(|e| e.into_inner()));
            self.raw_by_bus(bus)?
        };
        quiet::note_handle(bus, h as usize);
        Ok(Dev(h))
    }

    pub fn uuid(&self, d: Dev) -> Result<String> {
        self.on_r(d, |h| {
            let mut b = [0 as c_char; 96];
            self.ck(unsafe { (self.get_uuid)(h, b.as_mut_ptr(), b.len() as c_uint) }, "nvmlDeviceGetUUID")?;
            Ok(unsafe { CStr::from_ptr(b.as_ptr()) }.to_string_lossy().into_owned())
        })
    }

    pub fn name(&self, d: Dev) -> Result<String> {
        self.on_r(d, |h| {
            let mut b = [0 as c_char; 96];
            self.ck(unsafe { (self.get_name)(h, b.as_mut_ptr(), b.len() as c_uint) }, "nvmlDeviceGetName")?;
            Ok(unsafe { CStr::from_ptr(b.as_ptr()) }.to_string_lossy().into_owned())
        })
    }

    /// CUDA compute capability (major, minor), e.g. (8, 6) for RTX 30, (8, 9) for RTX 40, (12, 0) for RTX 50.
    pub fn compute_capability(&self, d: Dev) -> Option<(i32, i32)> {
        let f = self.get_cc?;
        self.on(d, |h| {
            let (mut ma, mut mi) = (0, 0);
            (unsafe { f(h, &mut ma, &mut mi) } == 0).then_some((ma, mi))
        })?
    }

    pub fn index(&self, d: Dev) -> Option<u32> {
        let f = self.get_index?;
        self.on(d, |h| {
            let mut i = 0;
            (unsafe { f(h, &mut i) } == 0).then_some(i)
        })?
    }

    /// (domain, bus, device) of the handle.
    pub fn pci(&self, d: Dev) -> Result<(u32, u32, u32)> {
        self.on_r(d, |h| {
            let mut p: PciInfo = unsafe { std::mem::zeroed() };
            self.ck(unsafe { (self.get_pci)(h, &mut p) }, "nvmlDeviceGetPciInfo")?;
            Ok((p.domain, p.bus, p.device))
        })
    }

    /// PCI device id of the GPU (e.g. 0x1e09 = TU102 of the CMP 50HX); NVML reports (device << 16) | vendor.
    pub fn pci_device_id(&self, d: Dev) -> Option<u16> {
        self.on(d, |h| {
            let mut p: PciInfo = unsafe { std::mem::zeroed() };
            (unsafe { (self.get_pci)(h, &mut p) } == 0).then_some((p.pci_device_id >> 16) as u16)
        })?
    }

    pub fn power_w(&self, d: Dev) -> Option<f64> {
        self.on(d, |h| {
            let mut mw = 0;
            (unsafe { (self.get_power)(h, &mut mw) } == 0).then(|| mw as f64 / 1000.0)
        })?
    }

    /// 1.0.12 (lostgpu.rs): the driver lost this GPU: its handle answers NVML_ERROR_GPU_IS_LOST (15), or its bus id no longer gives
    /// a handle (nvidia-smi: "Unable to determine the device handle ... Unknown Error").  In the GPU's quiet period after a fault:
    /// not lost (no NVML call, no verdict); after it, no fresh handle by the bus id = lost.
    pub fn gpu_lost(&self, d: Dev, bus: &str) -> bool {
        const NVML_ERROR_GPU_IS_LOST: c_int = 15;
        match self.gate(d, false) {
            Gate::Busy => false,
            Gate::Gone => true,
            Gate::Go(h, lock) => {
                let _g = lock.as_ref().map(|l| l.lock().unwrap_or_else(|e| e.into_inner()));
                let mut mw = 0;
                (unsafe { (self.get_power)(h, &mut mw) }) == NVML_ERROR_GPU_IS_LOST || self.raw_by_bus(bus).is_err()
            }
        }
    }

    /// Enforced power limit in watts (what the card is held to now).
    pub fn power_limit_w(&self, d: Dev) -> Option<f64> {
        let f = self.get_pl?;
        self.on(d, |h| {
            let mut mw = 0;
            (unsafe { f(h, &mut mw) } == 0).then(|| mw as f64 / 1000.0)
        })?
    }

    /// FFGN 1.0.12 (`--power-limit`): the power management limit in mW (the one `nvidia-smi -pl` sets; the enforced limit
    /// above may be lower when another limit applies).
    pub fn pm_limit_mw(&self, d: Dev) -> Option<u32> {
        let f = self.pm_get?;
        self.on(d, |h| {
            let mut mw = 0;
            (unsafe { f(h, &mut mw) } == 0 && mw > 0).then_some(mw)
        })?
    }

    /// (min, max) power management limit the card accepts, in mW.
    pub fn pm_constraints_mw(&self, d: Dev) -> Option<(u32, u32)> {
        let f = self.pm_minmax?;
        self.on(d, |h| {
            let (mut lo, mut hi) = (0, 0);
            (unsafe { f(h, &mut lo, &mut hi) } == 0 && hi > 0 && lo <= hi).then_some((lo, hi))
        })?
    }

    /// Set the power management limit (mW); needs root/Administrator, like the clock locks.
    pub fn set_pm_limit_mw(&self, d: Dev, mw: u32) -> Result<()> {
        let f = self.pm_set.ok_or_else(|| anyhow!("nvmlDeviceSetPowerManagementLimit not in this driver"))?;
        self.on_r(d, |h| self.ck(unsafe { f(h, mw) }, "nvmlDeviceSetPowerManagementLimit"))
    }

    /// The reasons the clocks are held down now (nvmlClocksThrottleReason* / nvmlClocksEventReason* bit mask).
    pub fn throttle_reasons(&self, d: Dev) -> Option<u64> {
        let f = self.throttle?;
        self.on(d, |h| {
            let mut r = 0u64;
            (unsafe { f(h, &mut r) } == 0).then_some(r)
        })?
    }

    /// Why the clocks are held down right now (NVML clocks event reasons bitmask): bit 0x4 = software power cap, 0x1 = idle.
    /// (FFG auto-lock; since the FFGN 1.0.12 port the same query as `throttle_reasons`, through the GPU's NVML gate)
    pub fn clock_reasons(&self, d: Dev) -> Option<u64> {
        self.throttle_reasons(d)
    }

    pub fn clock_mhz(&self, d: Dev) -> Option<u32> {
        self.on(d, |h| {
            let mut c = 0;
            (unsafe { (self.get_clock)(h, 0, &mut c) } == 0).then_some(c) // NVML_CLOCK_GRAPHICS
        })?
    }

    /// Current memory clock (NVML_CLOCK_MEM).
    pub fn mem_clock_mhz(&self, d: Dev) -> Option<u32> {
        self.on(d, |h| {
            let mut c = 0;
            (unsafe { (self.get_clock)(h, 2, &mut c) } == 0).then_some(c)
        })?
    }

    /// GPU utilization in percent (nvmlDeviceGetUtilizationRates: the share of the last sample period with a kernel running).
    pub fn util_gpu(&self, d: Dev) -> Option<u32> {
        let f = self.get_util?;
        self.on(d, |h| {
            let mut u = [0u32; 2]; // nvmlUtilization_t { gpu, memory }
            (unsafe { f(h, &mut u) } == 0).then_some(u[0])
        })?
    }

    /// Highest memory clock of the board (nvmlDeviceGetMaxClockInfo, NVML_CLOCK_MEM).
    /// Maximum graphics clock (nvmlDeviceGetMaxClockInfo, NVML_CLOCK_GRAPHICS): the top of --auto-lock's default range.
    pub fn max_core_clock_mhz(&self, d: Dev) -> Option<u32> {
        let f = self.get_max_clock?;
        self.on(d, |h| {
            let mut c = 0;
            (unsafe { f(h, 0, &mut c) } == 0 && c > 0).then_some(c)
        })?
    }

    pub fn max_mem_clock_mhz(&self, d: Dev) -> Option<u32> {
        let f = self.get_max_clock?;
        self.on(d, |h| {
            let mut c = 0;
            (unsafe { f(h, 2, &mut c) } == 0).then_some(c)
        })?
    }

    /// PROX (1.7.8): the lowest memory clock at which the card still offers its full graphics clock range (BLAKE2b does not touch
    /// the video memory; memory clocks that cap the core, e.g. 405 MHz = P8 on GeForce, are skipped).  Err: why it cannot tell.
    pub fn low_mem_clock(&self, d: Dev) -> Result<u32> {
        let (Some(sm), Some(sg)) = (self.sup_mem, self.sup_gr) else { bail!("NVML has no supported-clocks query") };
        self.on_r(d, |h| {
            let mut n: c_uint = 64;
            let mut mc = [0 as c_uint; 64];
            let r = unsafe { sm(h, &mut n, mc.as_mut_ptr()) };
            if r != 0 || n == 0 {
                bail!("supported memory clocks: {}", self.err(if r != 0 { r } else { 1 }));
            }
            let mut v = Vec::new();
            for &m in &mc[..(n as usize).min(64)] {
                let mut k: c_uint = 128;
                let mut g = [0 as c_uint; 128];
                if unsafe { sg(h, m, &mut k, g.as_mut_ptr()) } != 0 || k == 0 {
                    continue;
                }
                v.push((m, *g[..(k as usize).min(128)].iter().max().unwrap()));
            }
            let best = v.iter().map(|x| x.1).max().unwrap_or(0);
            v.iter().filter(|x| x.1 >= best && best > 0).map(|x| x.0).min().ok_or_else(|| anyhow::anyhow!("no usable memory clock found"))
        })
    }

    pub fn temp_c(&self, d: Dev) -> Option<u32> {
        self.on(d, |h| {
            let mut t = 0;
            (unsafe { (self.get_temp)(h, 0, &mut t) } == 0).then_some(t) // NVML_TEMPERATURE_GPU
        })?
    }

    /// Finds the working V/F offset API of this driver; returns (mode, current offset, min, max).
    pub fn probe_offset(&self, d: Dev) -> Result<(OffMode, i32, i32, i32)> {
        self.on_r(d, |h| {
            let mut why = Vec::new();
            if let (Some(g), Some(_)) = (self.gpc_get, self.gpc_set) {
                let mut cur = 0;
                let r = unsafe { g(h, &mut cur) };
                if r == 0 {
                    let (mut lo, mut hi) = (-1000, 1000);
                    if let Some(mm) = self.gpc_minmax {
                        if unsafe { mm(h, &mut lo, &mut hi) } != 0 {
                            (lo, hi) = (-1000, 1000);
                        }
                    }
                    return Ok((OffMode::Gpc, cur, lo, hi));
                }
                why.push(format!("GetGpcClkVfOffset: {}", self.err(r)));
            }
            if let (Some(g), Some(_)) = (self.co_get, self.co_set) {
                let mut o = ClockOffset { version: CLOCK_OFFSET_V1, clock_type: 0, pstate: 0, offset: 0, min: 0, max: 0 };
                let r = unsafe { g(h, &mut o) };
                if r == 0 {
                    return Ok((OffMode::Clock, o.offset, o.min, o.max));
                }
                why.push(format!("GetClockOffsets: {}", self.err(r)));
            }
            if why.is_empty() {
                bail!("this NVML has no V/F offset API");
            }
            bail!("V/F offset not available on this GPU/driver ({})", why.join("; "))
        })
    }

    pub fn get_offset(&self, d: Dev, m: OffMode) -> Result<i32> {
        self.on_r(d, |h| match m {
            OffMode::Gpc => {
                let mut v = 0;
                self.ck(unsafe { self.gpc_get.unwrap()(h, &mut v) }, "nvmlDeviceGetGpcClkVfOffset")?;
                Ok(v)
            }
            OffMode::Clock => {
                let mut o = ClockOffset { version: CLOCK_OFFSET_V1, clock_type: 0, pstate: 0, offset: 0, min: 0, max: 0 };
                self.ck(unsafe { self.co_get.unwrap()(h, &mut o) }, "nvmlDeviceGetClockOffsets")?;
                Ok(o.offset)
            }
        })
    }

    pub fn set_offset(&self, d: Dev, m: OffMode, v: i32) -> Result<()> {
        self.on_r(d, |h| match m {
            OffMode::Gpc => self.ck(unsafe { self.gpc_set.unwrap()(h, v) }, "nvmlDeviceSetGpcClkVfOffset"),
            OffMode::Clock => {
                let mut o = ClockOffset { version: CLOCK_OFFSET_V1, clock_type: 0, pstate: 0, offset: v, min: 0, max: 0 };
                self.ck(unsafe { self.co_set.unwrap()(h, &mut o) }, "nvmlDeviceSetClockOffsets")
            }
        })
    }

    /// Lock the core (graphics) clock to a fixed MHz (nvmlDeviceSetGpuLockedClocks, like `nvidia-smi -lgc N,N`).
    pub fn set_core_locked(&self, d: Dev, mhz: u32) -> Result<()> {
        let f = self.lgc_set.ok_or_else(|| anyhow!("nvmlDeviceSetGpuLockedClocks not in this driver"))?;
        self.on_r(d, |h| self.ck(unsafe { f(h, mhz, mhz) }, "nvmlDeviceSetGpuLockedClocks"))
    }
    pub fn reset_core_locked(&self, d: Dev) -> Result<()> {
        let f = self.lgc_reset.ok_or_else(|| anyhow!("nvmlDeviceResetGpuLockedClocks not in this driver"))?;
        self.on_r(d, |h| self.ck(unsafe { f(h) }, "nvmlDeviceResetGpuLockedClocks"))
    }
    /// Lock the memory clock to a fixed MHz (nvmlDeviceSetMemoryLockedClocks, like `nvidia-smi -lmc N,N`).
    pub fn set_mem_locked(&self, d: Dev, mhz: u32) -> Result<()> {
        let f = self.lmc_set.ok_or_else(|| anyhow!("nvmlDeviceSetMemoryLockedClocks not in this driver"))?;
        self.on_r(d, |h| self.ck(unsafe { f(h, mhz, mhz) }, "nvmlDeviceSetMemoryLockedClocks"))
    }
    pub fn reset_mem_locked(&self, d: Dev) -> Result<()> {
        let f = self.lmc_reset.ok_or_else(|| anyhow!("nvmlDeviceResetMemoryLockedClocks not in this driver"))?;
        self.on_r(d, |h| self.ck(unsafe { f(h) }, "nvmlDeviceResetMemoryLockedClocks"))
    }
    /// Memory-clock V/F offset in MHz (nvmlDeviceSetClockOffsets with NVML_CLOCK_MEM; needs driver 555+).
    pub fn set_mem_offset(&self, d: Dev, v: i32) -> Result<()> {
        let f = self.co_set.ok_or_else(|| anyhow!("nvmlDeviceSetClockOffsets not in this driver (memory offset needs driver 555+)"))?;
        self.on_r(d, |h| {
            let mut o = ClockOffset { version: CLOCK_OFFSET_V1, clock_type: 2, pstate: 0, offset: v, min: 0, max: 0 }; // NVML_CLOCK_MEM = 2
            self.ck(unsafe { f(h, &mut o) }, "nvmlDeviceSetClockOffsets(MEM)")
        })
    }

    /// Subscribes to critical Xid events of `d` (best effort).
    pub fn watch_xid(&self, d: Dev) -> bool {
        let (Some(create), Some(reg)) = (self.ev_create, self.ev_register) else { return false };
        let mut g = self.events.lock().unwrap_or_else(|e| e.into_inner());
        if g.set.is_null() {
            let mut set = std::ptr::null_mut();
            if unsafe { create(&mut set) } != 0 {
                return false;
            }
            g.set = set;
        }
        let set = g.set;
        let ok = self.on(d, |h| (unsafe { reg(h, XID_CRITICAL, set) } == 0).then_some(h as usize)).flatten();
        if let Some(h) = ok {
            g.regd.push(h);
        }
        ok.is_some()
    }

    /// Critical Xid errors seen on `d` since the last call.
    /// 1.0.12: draining the event set (a call on the set, with no wait: not a call on a GPU) goes on in a GPU's quiet period, so
    /// the Xid of the fault (79 = fallen off the bus: lostgpu.rs, the rig caution of auto-uv) still reaches the crash record; an Xid
    /// of a GPU starts its quiet period.  After a fresh handle the GPU is registered again with it.
    pub fn take_xids(&self, d: Dev) -> Vec<u64> {
        let Some(wait) = self.ev_wait else { return Vec::new() };
        let mut g = self.events.lock().unwrap_or_else(|e| e.into_inner());
        if g.set.is_null() {
            return Vec::new();
        }
        let mut fresh = Vec::new();
        loop {
            let mut e = EventData { device: std::ptr::null_mut(), event_type: 0, event_data: 0, gpu_instance: 0, compute_instance: 0 };
            if unsafe { wait(g.set, &mut e, 0) } != 0 {
                break;
            }
            if e.event_type & XID_CRITICAL != 0 {
                g.buf.push((e.device as usize, e.event_data));
                fresh.push((e.device as usize, e.event_data));
            }
        }
        let same = quiet::handles_of(d.0 as usize);
        let mut mine = Vec::new();
        g.buf.retain(|&(dev, x)| {
            if same.contains(&dev) {
                mine.push(x);
                false
            } else {
                true
            }
        });
        let regd = g.regd.clone();
        let set = g.set;
        drop(g);
        for (h, x) in fresh {
            quiet::fault_handle(h, &format!("Xid {x} reported by the driver"));
        }
        // a fresh handle after a quiet period: registered for the Xid events again (the old one may be gone with the reset)
        if let Some(reg) = self.ev_register {
            if same.iter().any(|h| regd.contains(h)) {
                if let Some(Some(h)) = self.on(d, |h| (!regd.contains(&(h as usize))).then_some(h)) {
                    let ok = self.on(d, |h2| unsafe { reg(h2, XID_CRITICAL, set) } == 0).unwrap_or(false);
                    if ok {
                        self.events.lock().unwrap_or_else(|e| e.into_inner()).regd.push(h as usize);
                    }
                }
            }
        }
        // 1.7.6: an Xid that the kernel log pins on another process (another miner on the same rig) is not this miner's
        if mine.is_empty() {
            return mine;
        }
        // 1.0.12: the PCI address from the bus id this program selected the GPU by (no NVML call in the quiet period)
        let pci = quiet::pci_of_handle(d.0 as usize).or_else(|| self.pci(d).ok());
        mine.retain(|&x| match pci.and_then(|p| crate::faults::foreign_xid(p, x)) {
            Some((pid, name)) => {
                println!("faults: Xid {x} on GPU bus {:02x} was caused by another process (pid {pid}, {name}), not by this miner: not counted", pci.map_or(0, |p| p.1));
                false
            }
            None => true,
        });
        mine
    }
}

enum Gate {
    Go(*mut c_void, Option<Arc<Mutex<()>>>),
    Busy,
    Gone,
}

/// The error text of a call refused in a GPU's quiet period (`is_busy`).
pub const BUSY: &str = "NVML quiet period after a fault of this GPU (no NVML call for it now)";
const GONE: &str = "no NVML handle by the bus id of this GPU after its fault (lost by the driver?)";

/// 1.0.12: was this call refused because the GPU is in its quiet period after a fault?
pub fn is_busy(e: &anyhow::Error) -> bool {
    format!("{e:#}").contains(BUSY)
}

/// FFGN 1.0.12 (PC_AORUS, RTX 3080 Ti, Windows, nvml.dll 8.17.15.9159): an auto-uv step faulted the card (the driver's Xid records
/// 15:40:22-:28, "cudaEventSynchronize: unknown error" in the GPU's engine process); the MINER process then crashed inside
/// nvml.dll (0xc0000005) on an NVML call for that GPU while the driver reset it.  A GPU fault must never kill the miner, so:
///  * a fault of a GPU (an error of its worker / engine process: any CUDA error, the engine process ended or hung; an Xid of it
///    drained from an event set) starts its QUIET PERIOD: no NVML call for that GPU from any thread (power sampler, the power
///    limit keeper and its throttle-reason query, auto-uv / auto-lock reads and offset sets, the clock keeper, the temperature
///    limit, --table-mclock, the lost-GPU check, the API / stats reads) until at least QUIET_MIN has passed AND the GPU's new
///    engine context has mined QUIET_MINED (`gpu_mining`, from its worker after every scan); a new fault starts it again.  The
///    calls return "unknown" (None / an Err with BUSY), which every caller already takes as a failed read.  A GPU whose engine
///    never mines again gets its calls back after QUIET_MAX (the lost-GPU check needs them); an exit waits for QUIET_MIN only;
///  * then the handle is taken fresh by the PCI bus id (never the one from before the fault; every handle the callers hold of
///    that GPU maps to it) and the resume hooks run (`on_resume`: this run's clock locks, then its offset, set again);
///  * every NVML call of a GPU holds that GPU's call lock: one call per GPU at a time (several threads during a reset).
/// Cheap on the hot path: `gpu_mining` is one atomic load while no GPU is quiet; a call takes one short lock and a short scan.
pub mod quiet {
    use super::*;

    pub const QUIET_MIN: Duration = Duration::from_secs(10);
    pub const QUIET_MINED: Duration = Duration::from_secs(3);
    pub const QUIET_MAX: Duration = Duration::from_secs(120);
    /// the hooks of a resume may still be setting the GPU's clocks / offset this long after it (`settling`)
    const SETTLE: Duration = Duration::from_secs(5);

    pub(super) static QUIET_N: AtomicUsize = AtomicUsize::new(0);
    pub(super) static EXITING: AtomicBool = AtomicBool::new(false);

    pub(super) struct Slot {
        key: String,
        pub(super) shown: String,
        devs: Vec<i32>,
        pub(super) handles: Vec<usize>,
        pub(super) cur: usize,
        pub(super) fault: Option<Instant>,
        pub(super) mined: Option<Instant>,
        pub(super) stale: bool,
        pub(super) epoch: u64,
        pub(super) resumed: Option<Instant>,
        faults: u64,
        pub(super) lock: Arc<Mutex<()>>,
        /// FreeForGem: the coin of this GPU reports its mining (`bind_dev`, then `gpu_mining` after every scan: NEB)
        pub(super) mines: bool,
    }

    static REG: Mutex<Vec<Slot>> = Mutex::new(Vec::new());
    static HOOKS: Mutex<Vec<fn(&str)>> = Mutex::new(Vec::new());

    pub(super) fn reg() -> std::sync::MutexGuard<'static, Vec<Slot>> {
        REG.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// "0000:01:00.0", "00000000:01:00.0", "01:00.0" -> "0000:01:00.0"; None = not a PCI bus id (a CPU device of the tests).
    pub fn norm_bus(s: &str) -> Option<String> {
        let s = s.trim().to_ascii_lowercase();
        let p: Vec<&str> = s.split(':').collect();
        let (dom, bus, df) = match p.len() {
            3 => (p[0], p[1], p[2]),
            2 => ("0", p[0], p[1]),
            _ => return None,
        };
        let (dv, f) = df.split_once('.').unwrap_or((df, "0"));
        let h = |x: &str| u32::from_str_radix(x, 16).ok();
        Some(format!("{:04x}:{:02x}:{:02x}.{:x}", h(dom)?, h(bus)?, h(dv)?, h(f)?))
    }

    fn pci_of(key: &str) -> Option<(u32, u32, u32)> {
        let p: Vec<&str> = key.split([':', '.']).collect();
        let h = |x: &str| u32::from_str_radix(x, 16).ok();
        Some((h(p.first()?)?, h(p.get(1)?)?, h(p.get(2)?)?))
    }

    pub fn same_bus(a: &str, b: &str) -> bool {
        match (norm_bus(a), norm_bus(b)) {
            (Some(x), Some(y)) => x == y,
            _ => a.eq_ignore_ascii_case(b),
        }
    }

    fn find_or_add(reg: &mut Vec<Slot>, key: &str, shown: &str) -> usize {
        if let Some(i) = reg.iter().position(|s| s.key == key) {
            return i;
        }
        reg.push(Slot {
            key: key.to_string(),
            shown: shown.to_string(),
            devs: Vec::new(),
            handles: Vec::new(),
            cur: 0,
            fault: None,
            mined: None,
            stale: false,
            epoch: 0,
            resumed: None,
            faults: 0,
            lock: Arc::new(Mutex::new(())),
            mines: false,
        });
        reg.len() - 1
    }

    /// `by_bus`: the GPU's call lock (None: not a PCI bus id); Err in its quiet period.
    /// FreeForGem: a GPU whose coin does not report its mining waits for the time part of its quiet period (as `gate`).
    pub(super) fn slot_for_call(bus: &str) -> Result<Option<Arc<Mutex<()>>>> {
        let Some(k) = norm_bus(bus) else { return Ok(None) };
        let mut waited = false;
        loop {
            let mut reg = reg();
            let i = find_or_add(&mut reg, &k, bus);
            if let Some(t) = reg[i].fault {
                let el = t.elapsed();
                if !reg[i].mines && !waited && el < QUIET_MIN {
                    drop(reg);
                    std::thread::sleep(QUIET_MIN.saturating_sub(el));
                    waited = true;
                    continue;
                }
                if reg[i].mines || el < QUIET_MIN {
                    bail!("{BUSY}");
                }
                // the time part is over: the quiet period ends here (this call takes a fresh handle by the bus id anyway; the
                // resume hooks run at the next call through `gate`)
                let s = &mut reg[i];
                s.fault = None;
                s.mined = None;
                s.stale = true;
                QUIET_N.fetch_sub(1, Ordering::Relaxed);
            }
            return Ok(Some(reg[i].lock.clone()));
        }
    }

    pub(super) fn note_handle(bus: &str, h: usize) {
        let Some(k) = norm_bus(bus) else { return };
        let mut reg = reg();
        let i = find_or_add(&mut reg, &k, bus);
        let s = &mut reg[i];
        if !s.handles.contains(&h) {
            s.handles.push(h);
        }
        if s.cur == 0 {
            s.cur = h;
        }
    }

    /// Every handle of the GPU of handle `h` (the handle itself when it is not this program's).
    pub(super) fn handles_of(h: usize) -> Vec<usize> {
        reg().iter().find(|s| s.handles.contains(&h)).map_or_else(|| vec![h], |s| s.handles.clone())
    }

    pub(super) fn pci_of_handle(h: usize) -> Option<(u32, u32, u32)> {
        reg().iter().find(|s| s.handles.contains(&h)).and_then(|s| pci_of(&s.key))
    }

    /// The miner's GPU `dev` (CUDA ordinal) is the one on `bus` (its engine process reports faults by the ordinal: `fault_dev`).
    pub fn bind_dev(dev: i32, bus: &str) {
        let Some(k) = norm_bus(bus) else { return };
        let mut reg = reg();
        for s in reg.iter_mut() {
            s.devs.retain(|&d| d != dev);
        }
        let i = find_or_add(&mut reg, &k, bus);
        reg[i].devs.push(dev);
        reg[i].mines = true; // FreeForGem: this coin's worker reports its scans (gpu_mining): FFGN's full quiet period
    }

    /// Returns the line to log (outside the lock of the table) when the quiet period begins.
    fn start(reg: &mut [Slot], i: usize, why: &str) -> Option<String> {
        let s = &mut reg[i];
        let first = s.fault.is_none();
        s.fault = Some(Instant::now());
        s.mined = None;
        s.faults += 1;
        if !first {
            return None;
        }
        QUIET_N.fetch_add(1, Ordering::Relaxed);
        let why: String = why.chars().take(160).collect();
        Some(format!(
            "nvml: GPU bus {}: a fault of this GPU ({why}): no NVML calls for it for at least {} s and until its new engine context has mined {} s (then a fresh NVML handle)",
            crate::uv::short_bus(&s.shown),
            QUIET_MIN.as_secs(),
            QUIET_MINED.as_secs()
        ))
    }

    fn say(m: Option<String>) {
        if let Some(m) = m {
            println!("{m}");
        }
    }

    /// A fault of the GPU on `bus` (its worker's / engine process' error, an Xid): its quiet period starts (again).
    pub fn gpu_fault(bus: &str, why: &str) {
        let Some(k) = norm_bus(bus) else { return };
        let mut reg = reg();
        let i = find_or_add(&mut reg, &k, bus);
        let m = start(&mut reg, i, why);
        drop(reg);
        say(m);
    }

    /// A fault of the miner's GPU `dev` (iso.rs: its engine process' call failed, before that process is dropped).
    pub fn fault_dev(dev: i32, why: &str) {
        let mut reg = reg();
        let m = reg.iter().position(|s| s.devs.contains(&dev)).and_then(|i| start(&mut reg, i, why));
        drop(reg);
        say(m);
    }

    pub(super) fn fault_handle(h: usize, why: &str) {
        let mut reg = reg();
        let m = reg.iter().position(|s| s.handles.contains(&h)).and_then(|i| start(&mut reg, i, why));
        drop(reg);
        say(m);
    }

    /// The worker of the GPU on `bus` finished a scan: the first one after a fault starts the "mined" part of the quiet period.
    /// One atomic load while no GPU is quiet (the hot path).
    pub fn gpu_mining(bus: &str) {
        if QUIET_N.load(Ordering::Relaxed) == 0 {
            return;
        }
        let Some(k) = norm_bus(bus) else { return };
        let mut reg = reg();
        if let Some(s) = reg.iter_mut().find(|s| s.key == k && s.fault.is_some()) {
            s.mined.get_or_insert_with(Instant::now);
        }
    }

    /// Is the GPU on `bus` in its quiet period now?
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_quiet(bus: &str) -> bool {
        let Some(k) = norm_bus(bus) else { return false };
        reg().iter().any(|s| s.key == k && s.fault.is_some())
    }

    /// In its quiet period, waiting for its fresh handle, or its resume hooks may still run: readings of its state (the offset,
    /// the clocks) are no verdict now.
    pub fn settling(bus: &str) -> bool {
        let Some(k) = norm_bus(bus) else { return false };
        reg().iter().any(|s| s.key == k && (s.fault.is_some() || s.stale || s.resumed.is_some_and(|t| t.elapsed() < SETTLE)))
    }

    /// Faults of the GPU on `bus` so far (auto-uv: a window with a fault in it gives no verdict).
    pub fn faults(bus: &str) -> u64 {
        let Some(k) = norm_bus(bus) else { return 0 };
        reg().iter().find(|s| s.key == k).map_or(0, |s| s.faults)
    }

    /// 1.0.13 (recover.rs): time since the GPU's fault while its NVML calls are paused (None: not paused).
    pub fn since_fault(bus: &str) -> Option<Duration> {
        let k = norm_bus(bus)?;
        reg().iter().find(|s| s.key == k).and_then(|s| s.fault).map(|t| t.elapsed())
    }

    /// 1.0.13 (recover.rs): the GPU's quiet period ends NOW (the driver answered a probe made in a process of its own, before
    /// the GPU's new engine starts), with a fresh handle by its bus id (`lookup`); no resume hooks: the recovery sets the clocks,
    /// then the safe offset itself, in that order, before the engine starts.  Err: a new fault meanwhile (BUSY) / no handle (GONE).
    pub fn resume_with(bus: &str, lookup: impl FnOnce(&str) -> Result<usize>) -> Result<()> {
        let Some(k) = norm_bus(bus) else { return Ok(()) };
        let (lock, shown, el) = {
            let mut reg = reg();
            let i = find_or_add(&mut reg, &k, bus);
            let s = &mut reg[i];
            let el = s.fault.map(|t| t.elapsed());
            if s.fault.take().is_some() {
                QUIET_N.fetch_sub(1, Ordering::Relaxed);
            }
            s.mined = None;
            s.stale = true;
            (s.lock.clone(), s.shown.clone(), el)
        };
        let fresh = {
            let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
            lookup(&shown)
        };
        let mut reg = reg();
        let i = find_or_add(&mut reg, &k, bus);
        let s = &mut reg[i];
        if s.fault.is_some() {
            bail!("{BUSY}");
        }
        let h = fresh.map_err(|e| anyhow!("{GONE}: {e:#}"))?;
        s.stale = false;
        s.cur = h;
        s.epoch += 1;
        s.resumed = Some(Instant::now());
        if !s.handles.contains(&h) {
            s.handles.push(h);
        }
        let line = format!(
            "nvml: GPU bus {}: NVML calls for this GPU resume {} after its fault: the driver answered a probe process; a fresh NVML handle by its bus id, then its clocks and safe offset BEFORE its new engine starts",
            crate::uv::short_bus(&s.shown),
            el.map_or("now".into(), |e| format!("{:.0} s", e.as_secs_f64()))
        );
        drop(reg);
        println!("{line}");
        Ok(())
    }

    /// The miner exits: the quiet period of a GPU is its time part only (its engine does not mine any more).
    pub fn set_exiting() {
        EXITING.store(true, Ordering::Relaxed);
    }

    /// Runs `f(bus)` (on a thread of its own) when a GPU's NVML calls resume after its quiet period, with the fresh handle.
    pub fn on_resume(f: fn(&str)) {
        let mut g = HOOKS.lock().unwrap_or_else(|e| e.into_inner());
        if !g.iter().any(|x| *x as usize == f as usize) {
            g.push(f);
        }
    }

    pub(super) fn run_hooks(bus: &str) {
        let hs = HOOKS.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if hs.is_empty() {
            return;
        }
        let bus = bus.to_string();
        // on a thread of its own: the call that ended the quiet period may hold the lock of its module (the offset, the power
        // limit), which a hook takes again
        let _ = std::thread::Builder::new().name("nvml-resume".into()).spawn(move || {
            for h in hs {
                h(&bus);
            }
        });
    }

    /// Tests: the quiet period of `bus` began (and its mining) `secs` earlier.
    #[cfg(test)]
    pub fn age(bus: &str, secs: f64) {
        let k = norm_bus(bus).unwrap();
        let mut reg = reg();
        let s = reg.iter_mut().find(|s| s.key == k).unwrap();
        let d = Duration::from_secs_f64(secs);
        s.fault = s.fault.map(|t| t.checked_sub(d).unwrap_or(t));
        s.mined = s.mined.map(|t| t.checked_sub(d).unwrap_or(t));
        s.resumed = s.resumed.map(|t| t.checked_sub(d).unwrap_or(t));
    }
}

pub use quiet::{bind_dev, fault_dev, gpu_fault, gpu_mining};
impl Drop for Nvml {
    fn drop(&mut self) {
        if let Some(f) = self.shutdown {
            unsafe { f() };
        }
    }
}

/// Tests: an `Nvml` on mock functions (no driver): handles by bus id (a fresh one on every lookup), every per-GPU call recorded.
#[cfg(test)]
pub mod mock {
    use super::*;
    use std::sync::atomic::AtomicI64;

    /// (handle, call) of every mock call
    pub static CALLS: Mutex<Vec<(usize, &'static str)>> = Mutex::new(Vec::new());
    /// lookups per bus key (the handle's low bits)
    static LOOKUPS: Mutex<Vec<(String, usize)>> = Mutex::new(Vec::new());
    /// bus keys whose lookup fails (a GPU the driver lost)
    pub static LOST: Mutex<Vec<String>> = Mutex::new(Vec::new());
    /// calls in flight / most at once of the power reading (the per-GPU call lock)
    pub static IN_FLIGHT: AtomicI64 = AtomicI64::new(0);
    pub static MAX_IN_FLIGHT: AtomicI64 = AtomicI64::new(0);
    /// the power reading takes this long (ms)
    pub static POWER_MS: AtomicI64 = AtomicI64::new(0);

    fn note(h: *mut c_void, what: &'static str) {
        CALLS.lock().unwrap_or_else(|e| e.into_inner()).push((h as usize, what));
    }

    /// Calls made with handle `h` (by name).
    pub fn calls(h: usize) -> Vec<&'static str> {
        CALLS.lock().unwrap_or_else(|e| e.into_inner()).iter().filter(|x| x.0 == h).map(|x| x.1).collect()
    }

    /// Calls made with any handle the mock gave out for `bus`.
    pub fn calls_for_bus(bus: &str) -> usize {
        let base = base_of(&quiet::norm_bus(bus).unwrap());
        CALLS.lock().unwrap_or_else(|e| e.into_inner()).iter().filter(|x| x.0 >> 16 == base >> 16).count()
    }

    fn base_of(key: &str) -> usize {
        let b = usize::from_str_radix(&key[5..7], 16).unwrap();
        (b + 1) << 16
    }

    pub fn lookups(bus: &str) -> usize {
        let k = quiet::norm_bus(bus).unwrap();
        LOOKUPS.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|x| x.0 == k).map_or(0, |x| x.1)
    }

    unsafe extern "C" fn by_bus(b: *const c_char, h: *mut *mut c_void) -> c_int {
        let s = unsafe { CStr::from_ptr(b) }.to_string_lossy().into_owned();
        let Some(k) = quiet::norm_bus(&s) else { return 2 };
        if LOST.lock().unwrap_or_else(|e| e.into_inner()).contains(&k) {
            return 999;
        }
        let mut g = LOOKUPS.lock().unwrap_or_else(|e| e.into_inner());
        let n = match g.iter_mut().find(|x| x.0 == k) {
            Some(x) => {
                x.1 += 1;
                x.1
            }
            None => {
                g.push((k.clone(), 1));
                1
            }
        };
        unsafe { *h = (base_of(&k) | n << 4) as *mut c_void };
        0
    }
    unsafe extern "C" fn power(h: *mut c_void, mw: *mut c_uint) -> c_int {
        // the serialization test's GPU (bus a7) only: other tests' GPUs may be called at the same time
        let watched = h as usize >> 16 == 0xa8;
        if watched {
            let n = IN_FLIGHT.fetch_add(1, Ordering::SeqCst) + 1;
            MAX_IN_FLIGHT.fetch_max(n, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(POWER_MS.load(Ordering::Relaxed).max(0) as u64));
        }
        note(h, "power");
        unsafe { *mw = 200_000 };
        if watched {
            IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
        }
        0
    }
    unsafe extern "C" fn dev_u(h: *mut c_void, v: *mut c_uint) -> c_int {
        note(h, "pm_get");
        unsafe { *v = 250_000 };
        0
    }
    unsafe extern "C" fn pm_set(h: *mut c_void, _mw: c_uint) -> c_int {
        note(h, "pm_set");
        0
    }
    unsafe extern "C" fn reasons(h: *mut c_void, r: *mut u64) -> c_int {
        note(h, "reasons");
        unsafe { *r = 0x4 };
        0
    }
    unsafe extern "C" fn util(h: *mut c_void, u: *mut [c_uint; 2]) -> c_int {
        note(h, "util");
        unsafe { (*u)[0] = 100 };
        0
    }
    unsafe extern "C" fn clock(h: *mut c_void, _t: c_uint, c: *mut c_uint) -> c_int {
        note(h, "clock");
        unsafe { *c = 1680 };
        0
    }
    unsafe extern "C" fn temp(h: *mut c_void, _t: c_uint, c: *mut c_uint) -> c_int {
        note(h, "temp");
        unsafe { *c = 60 };
        0
    }
    unsafe extern "C" fn text(h: *mut c_void, b: *mut c_char, n: c_uint) -> c_int {
        note(h, "text");
        if n > 1 {
            unsafe {
                *b = 0x78;
                *b.add(1) = 0;
            }
        }
        0
    }
    unsafe extern "C" fn pci(h: *mut c_void, _p: *mut PciInfo) -> c_int {
        note(h, "pci");
        0
    }
    unsafe extern "C" fn count(n: *mut c_uint) -> c_int {
        unsafe { *n = 1 };
        0
    }
    unsafe extern "C" fn drv(_b: *mut c_char, _n: c_uint) -> c_int {
        1
    }
    unsafe extern "C" fn gpc_get(h: *mut c_void, v: *mut c_int) -> c_int {
        note(h, "offset_get");
        unsafe { *v = 0 };
        0
    }
    unsafe extern "C" fn gpc_set(h: *mut c_void, _v: c_int) -> c_int {
        note(h, "offset_set");
        0
    }

    pub fn nvml() -> Nvml {
        Nvml {
            shutdown: None,
            error_string: None,
            driver_version: drv,
            count,
            by_bus,
            get_uuid: text,
            get_name: text,
            get_cc: None,
            get_index: None,
            get_pci: pci,
            get_power: power,
            get_pl: Some(dev_u),
            pm_get: Some(dev_u),
            pm_set: Some(pm_set),
            pm_minmax: None,
            throttle: Some(reasons),
            get_clock: clock,
            get_max_clock: None,
            get_temp: temp,
            get_util: Some(util),
            gpc_get: Some(gpc_get),
            gpc_set: Some(gpc_set),
            gpc_minmax: None,
            co_get: None,
            co_set: None,
            lgc_set: None,
            lgc_reset: None,
            lmc_set: None,
            lmc_reset: None,
            sup_mem: None,
            sup_gr: None,
            ev_create: None,
            ev_register: None,
            ev_wait: None,
            events: Mutex::new(Ev::default()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // every test has a bus of its own (the quiet state is global, the tests run in parallel)

    #[test]
    fn bus_ids_are_compared_normalized() {
        assert_eq!(quiet::norm_bus("00000000:0D:00.0").as_deref(), Some("0000:0d:00.0"));
        assert_eq!(quiet::norm_bus("0000:0d:00.0").as_deref(), Some("0000:0d:00.0"));
        assert_eq!(quiet::norm_bus("0d:00.0").as_deref(), Some("0000:0d:00.0"));
        assert_eq!(quiet::norm_bus("cpu:0"), None);
        assert!(quiet::same_bus("00000000:01:00.0", "0000:01:00.0"));
        assert!(!quiet::same_bus("0000:02:00.0", "0000:01:00.0"));
    }

    /// The AORUS 3080 Ti case: the worker reports "unknown error" -> no NVML call for the GPU from any caller until 10 s have
    /// passed AND its new engine context mined 3 s; then a fresh handle by the bus id, used by every handle the callers hold.
    #[test]
    fn a_worker_fault_starts_a_quiet_period_that_ends_after_the_time_and_the_mining() {
        let nv = mock::nvml();
        let bus = "0000:a1:00.0";
        let d = nv.by_bus(bus).unwrap();
        quiet::bind_dev(1001, bus); // FreeForGem: a GPU of a coin that reports its mining (NEB)
        let d2 = nv.by_bus("00000000:A1:00.0").unwrap(); // another caller's handle of the same GPU
        assert_ne!(d, d2);
        assert_eq!(nv.power_w(d), Some(200.0));
        let h1 = d.0 as usize;
        let before = mock::calls_for_bus(bus);
        quiet::gpu_mining(bus); // mining before the fault does not count
        quiet::gpu_fault("00000000:A1:00.0", "GPU0 scan: cudaEventSynchronize(ctx->ev1[pe]): unknown error");
        assert!(quiet::is_quiet(bus) && quiet::settling(bus));
        // every kind of caller: power sampler, PL keeper / throttle reasons, clocks, temperature, offsets, lost-GPU, by_bus
        assert_eq!(nv.power_w(d), None);
        assert_eq!(nv.power_w(d2), None);
        assert_eq!(nv.pm_limit_mw(d), None);
        assert_eq!(nv.throttle_reasons(d), None);
        assert_eq!(nv.util_gpu(d), None);
        assert_eq!(nv.clock_mhz(d), None);
        assert_eq!(nv.temp_c(d), None);
        assert!(is_busy(&nv.set_pm_limit_mw(d, 260_000).unwrap_err()));
        assert!(is_busy(&nv.set_offset(d, OffMode::Gpc, 300).unwrap_err()));
        assert!(is_busy(&nv.get_offset(d, OffMode::Gpc).unwrap_err()));
        assert!(nv.uuid(d).is_err());
        assert!(!nv.gpu_lost(d, bus), "no verdict in the quiet period");
        assert!(nv.by_bus(bus).is_err());
        assert_eq!(mock::calls_for_bus(bus), before, "no NVML call for the GPU in its quiet period");
        // the time alone does not end it: the new engine context has not mined
        quiet::age(bus, 30.0);
        assert_eq!(nv.power_w(d), None);
        // it mines, but not long enough yet
        quiet::gpu_mining(bus);
        quiet::age(bus, 1.0);
        assert_eq!(nv.power_w(d), None);
        assert_eq!(mock::calls_for_bus(bus), before);
        // 3 s of mining after 10+ s: the calls resume with a FRESH handle (a new lookup by the bus id)
        quiet::age(bus, 2.5);
        let looked = mock::lookups(bus);
        assert_eq!(nv.power_w(d), Some(200.0));
        assert_eq!(mock::lookups(bus), looked + 1, "the handle is taken fresh by the bus id");
        assert!(!quiet::is_quiet(bus));
        let calls = mock::CALLS.lock().unwrap().clone();
        let (last_h, _) = calls.iter().rev().find(|x| x.1 == "power" && x.0 >> 16 == h1 >> 16).copied().unwrap();
        assert_ne!(last_h, h1, "never the handle from before the fault");
        assert_ne!(last_h, d2.0 as usize);
        // the other caller's old handle maps to the fresh one too
        assert_eq!(nv.power_w(d2), Some(200.0));
        assert_eq!(mock::calls(h1).iter().filter(|c| **c == "power").count(), 1, "the old handle was used once, before the fault");
        assert!(mock::calls(d2.0 as usize).is_empty(), "the other caller's old handle is never called after the fault");
        assert_eq!(mock::lookups(bus), looked + 1, "one fresh lookup per quiet period");
        // the resume hooks may still set clocks / offset: readings of the state are no verdict for a moment
        assert!(quiet::settling(bus));
        quiet::age(bus, 6.0);
        assert!(!quiet::settling(bus));
    }

    #[test]
    fn mining_before_the_end_of_the_time_still_needs_the_time() {
        let nv = mock::nvml();
        let bus = "0000:a2:00.0";
        let d = nv.by_bus(bus).unwrap();
        quiet::bind_dev(1002, bus); // FreeForGem: a GPU of a coin that reports its mining (NEB)
        quiet::gpu_fault(bus, "the engine process ended");
        quiet::gpu_mining(bus);
        quiet::age(bus, 5.0); // mined 5 s, but only 5 s since the fault
        assert_eq!(nv.power_w(d), None);
        quiet::age(bus, 5.5);
        assert_eq!(nv.power_w(d), Some(200.0));
    }

    #[test]
    fn a_new_fault_starts_the_quiet_period_again() {
        let nv = mock::nvml();
        let bus = "0000:a3:00.0";
        let d = nv.by_bus(bus).unwrap();
        quiet::bind_dev(1003, bus); // FreeForGem: a GPU of a coin that reports its mining (NEB)
        quiet::gpu_fault(bus, "Xid 13");
        quiet::gpu_mining(bus);
        quiet::age(bus, 9.0);
        quiet::gpu_fault(bus, "unknown error"); // the retry of the worker failed too: the time and the mining start over
        quiet::age(bus, 4.0);
        assert_eq!(nv.power_w(d), None);
        quiet::gpu_mining(bus);
        quiet::age(bus, 7.0);
        assert_eq!(nv.power_w(d), Some(200.0));
        assert_eq!(quiet::faults(bus), 2);
    }

    /// iso.rs reports a fault by the CUDA ordinal (the engine process' call failed) before the process is dropped.
    #[test]
    fn an_engine_process_fault_by_the_ordinal() {
        let nv = mock::nvml();
        let bus = "0000:a4:00.0";
        let d = nv.by_bus(bus).unwrap();
        quiet::bind_dev(41, bus);
        quiet::fault_dev(42, "another GPU");
        assert_eq!(nv.power_w(d), Some(200.0));
        quiet::fault_dev(41, "GPU41: the engine process ended during a scan");
        assert!(quiet::is_quiet(bus));
        assert_eq!(nv.power_w(d), None);
    }

    /// A GPU whose engine never mines again (the worker gave up, Xid 79): its calls come back after QUIET_MAX, so the lost-GPU
    /// check sees it; no handle by the bus id then = lost.
    #[test]
    fn a_gpu_that_never_mines_again_is_seen_lost_after_the_longest_quiet_period() {
        let nv = mock::nvml();
        let bus = "0000:a5:00.0";
        let d = nv.by_bus(bus).unwrap();
        quiet::bind_dev(1005, bus); // FreeForGem: a GPU of a coin that reports its mining (NEB)
        quiet::gpu_fault(bus, "Xid 79");
        quiet::age(bus, 60.0);
        assert!(!nv.gpu_lost(d, bus));
        mock::LOST.lock().unwrap().push(quiet::norm_bus(bus).unwrap());
        quiet::age(bus, quiet::QUIET_MAX.as_secs_f64());
        assert!(nv.gpu_lost(d, bus), "no fresh handle by the bus id: lost");
        assert_eq!(nv.power_w(d), None);
        mock::LOST.lock().unwrap().clear();
    }

    /// The power-limit keeper (powerlimit.rs) does not re-apply in the quiet period (its read-back is "unknown") and makes no
    /// NVML call for the GPU; the throttle-reason query neither.
    #[test]
    fn the_power_limit_keeper_makes_no_call_in_the_quiet_period() {
        use crate::powerlimit::Hw;
        let nv = mock::nvml();
        let bus = "0000:a6:00.0";
        let d = nv.by_bus(bus).unwrap();
        quiet::bind_dev(1006, bus); // FreeForGem: a GPU of a coin that reports its mining (NEB)
        quiet::gpu_fault(bus, "unknown error");
        let before = mock::calls_for_bus(bus);
        assert_eq!(crate::powerlimit::keep_judge(260_000, nv.limit_mw(d), || true), crate::powerlimit::KeepAct::Holds);
        assert_eq!(nv.reasons(d), None);
        assert_eq!(nv.util(d), None);
        assert!(nv.set_limit_mw(d, 260_000).is_err());
        assert_eq!(mock::calls_for_bus(bus), before);
    }

    /// One NVML call per GPU at a time (several threads during a reset: the Windows risk).
    #[test]
    fn calls_of_one_gpu_are_serialized() {
        let nv: &'static Nvml = Box::leak(Box::new(mock::nvml()));
        let d = nv.by_bus("0000:a7:00.0").unwrap();
        mock::POWER_MS.store(3, Ordering::SeqCst);
        let hs: Vec<_> = (0..6).map(|_| std::thread::spawn(move || (0..5).for_each(|_| assert!(nv.power_w(d).is_some())))).collect();
        hs.into_iter().for_each(|h| h.join().unwrap());
        mock::POWER_MS.store(0, Ordering::SeqCst);
        assert_eq!(mock::MAX_IN_FLIGHT.load(Ordering::SeqCst), 1);
    }

    /// FreeForGem 1.8.0: a GPU of a coin without the mining signal (Pearl, NOID, CMFD, PROX, Ting: no `bind_dev`) has the time
    /// part of the quiet period only; reads give "unknown" in it, a set waits for its end (an auto-uv tuner sets the offset back
    /// right after the Xid: late, never lost), then a fresh handle by the bus id.
    #[test]
    fn a_gpu_of_a_coin_without_the_mining_signal_has_the_time_part_only() {
        let nv = mock::nvml();
        let bus = "0000:b1:00.0";
        let d = nv.by_bus(bus).unwrap();
        let h1 = d.0 as usize;
        quiet::gpu_fault(bus, "Xid 43 reported by the driver");
        let before = mock::calls_for_bus(bus);
        assert_eq!(nv.power_w(d), None);
        assert_eq!(nv.clock_mhz(d), None);
        assert_eq!(mock::calls_for_bus(bus), before, "no read in the quiet period");
        // a set 0.3 s before the end of the time part waits for it (no error), then goes with a fresh handle
        quiet::age(bus, quiet::QUIET_MIN.as_secs_f64() - 0.3);
        let t = Instant::now();
        let looked = mock::lookups(bus);
        nv.set_offset(d, OffMode::Gpc, 120).unwrap();
        assert!(t.elapsed() >= Duration::from_millis(250), "the set waited for the end of the time part");
        assert_eq!(mock::lookups(bus), looked + 1, "a fresh handle by the bus id");
        let last = mock::CALLS.lock().unwrap().iter().rev().find(|x| x.1 == "offset_set").map(|x| x.0).unwrap();
        assert_ne!(last, h1);
        assert!(!quiet::is_quiet(bus));
        // no mining signal needed: the reads come back after the time part alone
        quiet::gpu_fault(bus, "Xid 13");
        quiet::age(bus, quiet::QUIET_MIN.as_secs_f64() + 0.1);
        assert_eq!(nv.power_w(d), Some(200.0));
        // by_bus in the quiet period of such a GPU waits for the time part too, then ends it
        quiet::gpu_fault(bus, "Xid 31");
        quiet::age(bus, quiet::QUIET_MIN.as_secs_f64() - 0.2);
        assert!(nv.by_bus(bus).is_ok());
        assert!(!quiet::is_quiet(bus));
    }

    #[test]
    fn a_cpu_device_of_the_tests_has_no_quiet_period() {
        quiet::gpu_fault("cpu:0", "x");
        assert!(!quiet::is_quiet("cpu:0"));
        quiet::gpu_mining("cpu:0");
    }

    /// 1.0.13 (recover.rs) through the real quiet period and a mock NVML: the Rig01 sequence (an auto-uv step +365 faults,
    /// Xid 43; the tuner had +350 confirmed) and the PC_AORUS (Windows) case (the driver still in its reset: the probe PROCESS
    /// dies with 0xC0000005 twice).  Before the safe state: no NVML call for the GPU in this process (other threads' calls are
    /// refused); then a fresh handle, the clocks, the offset +335 set and read back, and only then the engine starts.
    #[test]
    fn rig01_and_windows_recovery_sets_the_safe_offset_before_the_new_engine() {
        use crate::recover::{Hw, Opts, Plan, SafeState};
        use std::cell::{Cell, RefCell};
        struct H<'a> {
            nv: &'a Nvml,
            d: Dev,
            bus: &'a str,
            probes: Cell<u32>,
            cur: Cell<i32>,
            log: RefCell<Vec<String>>,
            in_quiet_calls: Cell<usize>,
        }
        impl Hw for H<'_> {
            fn since_fault(&self) -> Option<Duration> {
                quiet::since_fault(self.bus)
            }
            fn probe(&self) -> Result<(), String> {
                // what the other threads see meanwhile: refused (the Windows fix: no call into nvml.dll during the reset)
                assert_eq!(self.nv.power_w(self.d), None);
                assert!(is_busy(&self.nv.set_offset(self.d, OffMode::Gpc, 365).unwrap_err()));
                self.in_quiet_calls.set(mock::calls_for_bus(self.bus));
                self.probes.set(self.probes.get() + 1);
                if self.probes.get() <= 2 { Err("the probe process ended with exit code: 0xc0000005".into()) } else { Ok(()) }
            }
            fn resume(&self) -> Result<(), String> {
                self.log.borrow_mut().push("resume".into());
                self.nv.resume_after_fault(self.bus).map_err(|e| format!("{e:#}"))
            }
            fn apply_clocks(&self) -> Result<(), String> {
                self.log.borrow_mut().push("clocks".into());
                self.nv.temp_c(self.d).map(|_| ()).ok_or_else(|| "refused".to_string())
            }
            fn set_offset(&self, v: i32) -> Result<(), String> {
                self.nv.set_offset(self.d, OffMode::Gpc, v).map_err(|e| format!("{e:#}"))?;
                self.cur.set(v);
                self.log.borrow_mut().push(format!("offset {v:+}"));
                Ok(())
            }
            fn read_offset(&self) -> Option<i32> {
                self.nv.get_offset(self.d, OffMode::Gpc).ok().map(|_| self.cur.get())
            }
            fn stopped(&self) -> bool {
                false
            }
            fn sleep(&self, d: Duration) {
                quiet::age(self.bus, d.as_secs_f64()); // the quiet period's clock, without waiting
            }
        }
        let nv = mock::nvml();
        let bus = "0000:b7:00.0";
        let d = nv.by_bus(bus).unwrap();
        let h1 = d.0 as usize;
        let looked = mock::lookups(bus);
        quiet::bind_dev(1007, bus); // FreeForGem: a GPU of a coin that reports its mining (NEB)
        quiet::gpu_fault(bus, "GPU1 scan: cudaEventSynchronize(ctx->ev1[pe]): an illegal memory access was encountered");
        let before = mock::calls_for_bus(bus);
        let hw = H { nv: &nv, d, bus, probes: Cell::new(0), cur: Cell::new(365), log: RefCell::new(vec![]), in_quiet_calls: Cell::new(0) };
        let plan = Plan { crashed: Some(365), confirmed: Some(350), base: 275, step: 15 };
        let mut lines = vec![];
        let r = crate::recover::recover_and_start(&hw, &plan, &Opts::default(), &mut |l| lines.push(l), |s: &SafeState| {
            assert_eq!(hw.cur.get(), 335, "the new engine starts at failed - 2 steps");
            assert!(!quiet::is_quiet(bus), "its NVML calls are back before the engine");
            s.clone()
        });
        assert!(matches!(r, Ok(SafeState::Ready { offset: Some(335), lowered: true, nvml: true, .. })), "{r:?}");
        assert_eq!(hw.probes.get(), 3, "two probe processes died (driver in its reset), the third answered");
        assert_eq!(hw.in_quiet_calls.get(), before, "no NVML call for the GPU in this process before the probe answered");
        assert_eq!(*hw.log.borrow(), vec!["resume", "clocks", "offset +335"]);
        assert_eq!(mock::lookups(bus), looked + 1, "one fresh handle");
        assert!(mock::calls_for_bus(bus) > before, "the calls after the resume reach NVML");
        // the old handle maps to the fresh one (not the pre-fault handle)
        assert!(mock::calls(h1).len() <= 1, "{:?}", mock::calls(h1));
        assert_eq!(nv.power_w(d), Some(200.0));
        assert!(lines.iter().any(|l| l.contains("does not answer")), "{lines:?}");
        // a second fault at the safe offset: the same way again (the 1.0.12 run burnt 3 tries at +365 here)
        quiet::gpu_fault(bus, "GPU1 scan: dtable build: an illegal memory access was encountered");
        let plan2 = Plan { crashed: Some(335), confirmed: Some(335), base: 275, step: 15 };
        hw.probes.set(2);
        let s2 = crate::recover::recover_after_fault(&hw, &plan2, &Opts::default(), &mut |_| {});
        assert!(matches!(s2, SafeState::Ready { offset: Some(305), lowered: true, .. }), "{s2:?}");
    }
}
