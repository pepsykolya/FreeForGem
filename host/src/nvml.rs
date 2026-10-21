// FreeForGemPear — see LICENSE
//! NVML loaded at run time (libnvidia-ml.so.1 / nvml.dll): no link-time dependency, the miner starts without it.
//! Only the calls the opt-in undervolt tuner (uv.rs) needs.
use anyhow::{Result, anyhow, bail};
use std::ffi::{CStr, CString, c_char, c_int, c_uint, c_void};
use std::sync::Mutex;

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
    get_clock: unsafe extern "C" fn(*mut c_void, c_uint, *mut c_uint) -> c_int,
    get_max_clock: Option<unsafe extern "C" fn(*mut c_void, c_uint, *mut c_uint) -> c_int>,
    get_temp: unsafe extern "C" fn(*mut c_void, c_uint, *mut c_uint) -> c_int,
    gpc_get: Option<unsafe extern "C" fn(*mut c_void, *mut c_int) -> c_int>,
    gpc_set: Option<unsafe extern "C" fn(*mut c_void, c_int) -> c_int>,
    gpc_minmax: Option<unsafe extern "C" fn(*mut c_void, *mut c_int, *mut c_int) -> c_int>,
    co_get: Option<unsafe extern "C" fn(*mut c_void, *mut ClockOffset) -> c_int>,
    co_set: Option<unsafe extern "C" fn(*mut c_void, *mut ClockOffset) -> c_int>,
    lgc_set: Option<unsafe extern "C" fn(*mut c_void, c_uint, c_uint) -> c_int>,
    lgc_reset: Option<unsafe extern "C" fn(*mut c_void) -> c_int>,
    lmc_set: Option<unsafe extern "C" fn(*mut c_void, c_uint, c_uint) -> c_int>,
    lmc_reset: Option<unsafe extern "C" fn(*mut c_void) -> c_int>,
    ev_create: Option<unsafe extern "C" fn(*mut *mut c_void) -> c_int>,
    ev_register: Option<unsafe extern "C" fn(*mut c_void, u64, *mut c_void) -> c_int>,
    ev_wait: Option<unsafe extern "C" fn(*mut c_void, *mut EventData, c_uint) -> c_int>,
    events: Mutex<(*mut c_void, Vec<(usize, u64)>)>, // event set, drained Xid events (device handle, Xid)
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
            get_clock: req!("nvmlDeviceGetClockInfo"),
            get_max_clock: opt!("nvmlDeviceGetMaxClockInfo"),
            get_temp: req!("nvmlDeviceGetTemperature"),
            gpc_get: opt!("nvmlDeviceGetGpcClkVfOffset"),
            gpc_set: opt!("nvmlDeviceSetGpcClkVfOffset"),
            gpc_minmax: opt!("nvmlDeviceGetGpcClkMinMaxVfOffset"),
            co_get: opt!("nvmlDeviceGetClockOffsets"),
            co_set: opt!("nvmlDeviceSetClockOffsets"),
            lgc_set: opt!("nvmlDeviceSetGpuLockedClocks"),
            lgc_reset: opt!("nvmlDeviceResetGpuLockedClocks"),
            lmc_set: opt!("nvmlDeviceSetMemoryLockedClocks"),
            lmc_reset: opt!("nvmlDeviceResetMemoryLockedClocks"),
            ev_create: opt!("nvmlEventSetCreate"),
            ev_register: opt!("nvmlDeviceRegisterEvents"),
            ev_wait: opt!("nvmlEventSetWait_v2"),
            events: Mutex::new((std::ptr::null_mut(), Vec::new())),
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
    pub fn by_bus(&self, bus: &str) -> Result<Dev> {
        let c = CString::new(bus)?;
        let mut h = std::ptr::null_mut();
        self.ck(unsafe { (self.by_bus)(c.as_ptr(), &mut h) }, &format!("nvmlDeviceGetHandleByPciBusId({bus})"))?;
        Ok(Dev(h))
    }

    pub fn uuid(&self, d: Dev) -> Result<String> {
        let mut b = [0 as c_char; 96];
        self.ck(unsafe { (self.get_uuid)(d.0, b.as_mut_ptr(), b.len() as c_uint) }, "nvmlDeviceGetUUID")?;
        Ok(unsafe { CStr::from_ptr(b.as_ptr()) }.to_string_lossy().into_owned())
    }

    pub fn name(&self, d: Dev) -> Result<String> {
        let mut b = [0 as c_char; 96];
        self.ck(unsafe { (self.get_name)(d.0, b.as_mut_ptr(), b.len() as c_uint) }, "nvmlDeviceGetName")?;
        Ok(unsafe { CStr::from_ptr(b.as_ptr()) }.to_string_lossy().into_owned())
    }

    /// CUDA compute capability (major, minor), e.g. (8, 6) for RTX 30, (8, 9) for RTX 40, (12, 0) for RTX 50.
    pub fn compute_capability(&self, d: Dev) -> Option<(i32, i32)> {
        let f = self.get_cc?;
        let (mut ma, mut mi) = (0, 0);
        (unsafe { f(d.0, &mut ma, &mut mi) } == 0).then_some((ma, mi))
    }

    pub fn index(&self, d: Dev) -> Option<u32> {
        let f = self.get_index?;
        let mut i = 0;
        (unsafe { f(d.0, &mut i) } == 0).then_some(i)
    }

    /// (domain, bus, device) of the handle.
    pub fn pci(&self, d: Dev) -> Result<(u32, u32, u32)> {
        let mut p: PciInfo = unsafe { std::mem::zeroed() };
        self.ck(unsafe { (self.get_pci)(d.0, &mut p) }, "nvmlDeviceGetPciInfo")?;
        Ok((p.domain, p.bus, p.device))
    }

    /// PCI device id of the GPU (e.g. 0x1e09 = TU102 of the CMP 50HX); NVML reports (device << 16) | vendor.
    pub fn pci_device_id(&self, d: Dev) -> Option<u16> {
        let mut p: PciInfo = unsafe { std::mem::zeroed() };
        (unsafe { (self.get_pci)(d.0, &mut p) } == 0).then_some((p.pci_device_id >> 16) as u16)
    }

    pub fn power_w(&self, d: Dev) -> Option<f64> {
        let mut mw = 0;
        (unsafe { (self.get_power)(d.0, &mut mw) } == 0).then(|| mw as f64 / 1000.0)
    }

    pub fn clock_mhz(&self, d: Dev) -> Option<u32> {
        let mut c = 0;
        (unsafe { (self.get_clock)(d.0, 0, &mut c) } == 0).then_some(c) // NVML_CLOCK_GRAPHICS
    }

    /// Current memory clock (NVML_CLOCK_MEM).
    pub fn mem_clock_mhz(&self, d: Dev) -> Option<u32> {
        let mut c = 0;
        (unsafe { (self.get_clock)(d.0, 2, &mut c) } == 0).then_some(c)
    }

    /// Highest memory clock of the board (nvmlDeviceGetMaxClockInfo, NVML_CLOCK_MEM).
    pub fn max_mem_clock_mhz(&self, d: Dev) -> Option<u32> {
        let f = self.get_max_clock?;
        let mut c = 0;
        (unsafe { f(d.0, 2, &mut c) } == 0).then_some(c)
    }

    pub fn temp_c(&self, d: Dev) -> Option<u32> {
        let mut t = 0;
        (unsafe { (self.get_temp)(d.0, 0, &mut t) } == 0).then_some(t) // NVML_TEMPERATURE_GPU
    }

    /// Finds the working V/F offset API of this driver; returns (mode, current offset, min, max).
    pub fn probe_offset(&self, d: Dev) -> Result<(OffMode, i32, i32, i32)> {
        let mut why = Vec::new();
        if let (Some(g), Some(_)) = (self.gpc_get, self.gpc_set) {
            let mut cur = 0;
            let r = unsafe { g(d.0, &mut cur) };
            if r == 0 {
                let (mut lo, mut hi) = (-1000, 1000);
                if let Some(mm) = self.gpc_minmax {
                    if unsafe { mm(d.0, &mut lo, &mut hi) } != 0 {
                        (lo, hi) = (-1000, 1000);
                    }
                }
                return Ok((OffMode::Gpc, cur, lo, hi));
            }
            why.push(format!("GetGpcClkVfOffset: {}", self.err(r)));
        }
        if let (Some(g), Some(_)) = (self.co_get, self.co_set) {
            let mut o = ClockOffset { version: CLOCK_OFFSET_V1, clock_type: 0, pstate: 0, offset: 0, min: 0, max: 0 };
            let r = unsafe { g(d.0, &mut o) };
            if r == 0 {
                return Ok((OffMode::Clock, o.offset, o.min, o.max));
            }
            why.push(format!("GetClockOffsets: {}", self.err(r)));
        }
        if why.is_empty() {
            bail!("this NVML has no V/F offset API");
        }
        bail!("V/F offset not available on this GPU/driver ({})", why.join("; "))
    }

    pub fn get_offset(&self, d: Dev, m: OffMode) -> Result<i32> {
        match m {
            OffMode::Gpc => {
                let mut v = 0;
                self.ck(unsafe { self.gpc_get.unwrap()(d.0, &mut v) }, "nvmlDeviceGetGpcClkVfOffset")?;
                Ok(v)
            }
            OffMode::Clock => {
                let mut o = ClockOffset { version: CLOCK_OFFSET_V1, clock_type: 0, pstate: 0, offset: 0, min: 0, max: 0 };
                self.ck(unsafe { self.co_get.unwrap()(d.0, &mut o) }, "nvmlDeviceGetClockOffsets")?;
                Ok(o.offset)
            }
        }
    }

    pub fn set_offset(&self, d: Dev, m: OffMode, v: i32) -> Result<()> {
        match m {
            OffMode::Gpc => self.ck(unsafe { self.gpc_set.unwrap()(d.0, v) }, "nvmlDeviceSetGpcClkVfOffset"),
            OffMode::Clock => {
                let mut o = ClockOffset { version: CLOCK_OFFSET_V1, clock_type: 0, pstate: 0, offset: v, min: 0, max: 0 };
                self.ck(unsafe { self.co_set.unwrap()(d.0, &mut o) }, "nvmlDeviceSetClockOffsets")
            }
        }
    }

    /// Lock the core (graphics) clock to a fixed MHz (nvmlDeviceSetGpuLockedClocks, like `nvidia-smi -lgc N,N`).
    pub fn set_core_locked(&self, d: Dev, mhz: u32) -> Result<()> {
        let f = self.lgc_set.ok_or_else(|| anyhow!("nvmlDeviceSetGpuLockedClocks not in this driver"))?;
        self.ck(unsafe { f(d.0, mhz, mhz) }, "nvmlDeviceSetGpuLockedClocks")
    }
    pub fn reset_core_locked(&self, d: Dev) -> Result<()> {
        let f = self.lgc_reset.ok_or_else(|| anyhow!("nvmlDeviceResetGpuLockedClocks not in this driver"))?;
        self.ck(unsafe { f(d.0) }, "nvmlDeviceResetGpuLockedClocks")
    }
    /// Lock the memory clock to a fixed MHz (nvmlDeviceSetMemoryLockedClocks, like `nvidia-smi -lmc N,N`).
    pub fn set_mem_locked(&self, d: Dev, mhz: u32) -> Result<()> {
        let f = self.lmc_set.ok_or_else(|| anyhow!("nvmlDeviceSetMemoryLockedClocks not in this driver"))?;
        self.ck(unsafe { f(d.0, mhz, mhz) }, "nvmlDeviceSetMemoryLockedClocks")
    }
    pub fn reset_mem_locked(&self, d: Dev) -> Result<()> {
        let f = self.lmc_reset.ok_or_else(|| anyhow!("nvmlDeviceResetMemoryLockedClocks not in this driver"))?;
        self.ck(unsafe { f(d.0) }, "nvmlDeviceResetMemoryLockedClocks")
    }
    /// Memory-clock V/F offset in MHz (nvmlDeviceSetClockOffsets with NVML_CLOCK_MEM; needs driver 555+).
    pub fn set_mem_offset(&self, d: Dev, v: i32) -> Result<()> {
        let f = self.co_set.ok_or_else(|| anyhow!("nvmlDeviceSetClockOffsets not in this driver (memory offset needs driver 555+)"))?;
        let mut o = ClockOffset { version: CLOCK_OFFSET_V1, clock_type: 2, pstate: 0, offset: v, min: 0, max: 0 }; // NVML_CLOCK_MEM = 2
        self.ck(unsafe { f(d.0, &mut o) }, "nvmlDeviceSetClockOffsets(MEM)")
    }

    /// Subscribes to critical Xid events of `d` (best effort).
    pub fn watch_xid(&self, d: Dev) -> bool {
        let (Some(create), Some(reg)) = (self.ev_create, self.ev_register) else { return false };
        let mut g = self.events.lock().unwrap();
        if g.0.is_null() {
            let mut set = std::ptr::null_mut();
            if unsafe { create(&mut set) } != 0 {
                return false;
            }
            g.0 = set;
        }
        unsafe { reg(d.0, XID_CRITICAL, g.0) == 0 }
    }

    /// Critical Xid errors seen on `d` since the last call.
    pub fn take_xids(&self, d: Dev) -> Vec<u64> {
        let Some(wait) = self.ev_wait else { return Vec::new() };
        let mut g = self.events.lock().unwrap();
        if g.0.is_null() {
            return Vec::new();
        }
        loop {
            let mut e = EventData { device: std::ptr::null_mut(), event_type: 0, event_data: 0, gpu_instance: 0, compute_instance: 0 };
            if unsafe { wait(g.0, &mut e, 0) } != 0 {
                break;
            }
            if e.event_type & XID_CRITICAL != 0 {
                g.1.push((e.device as usize, e.event_data));
            }
        }
        let mut mine = Vec::new();
        g.1.retain(|&(dev, x)| {
            if dev == d.0 as usize {
                mine.push(x);
                false
            } else {
                true
            }
        });
        mine
    }
}

impl Drop for Nvml {
    fn drop(&mut self) {
        if let Some(f) = self.shutdown {
            unsafe { f() };
        }
    }
}
