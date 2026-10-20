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

// ---------------- backend library (loaded at run time) ----------------
// The search library is loaded dynamically, so one binary runs on NVIDIA (pearlgpu, CUDA) and AMD (pearlgpu_amd, OpenCL) GPUs
// and a machine without one vendor's driver does not fail to start.  `--backend auto|cuda|amd` / FFGP_BACKEND pick it; auto takes
// the CUDA library if it loads and sees a device, else the AMD library.  Both export the same C ABI (gpu/pearlgpu.h).

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Backend {
    Cuda,
    Amd,
}

impl Backend {
    pub fn name(self) -> &'static str {
        match self {
            Backend::Cuda => "cuda",
            Backend::Amd => "amd",
        }
    }
    fn lib_file(self) -> &'static str {
        match (self, cfg!(windows)) {
            (Backend::Cuda, true) => "pearlgpu.dll",
            (Backend::Amd, true) => "pearlgpu_amd.dll",
            (Backend::Cuda, false) => "libpearlgpu.so",
            (Backend::Amd, false) => "libpearlgpu_amd.so",
        }
    }
}

mod dl {
    use std::ffi::{CString, c_void};
    #[cfg(windows)]
    unsafe extern "system" {
        fn LoadLibraryW(name: *const u16) -> *mut c_void;
        fn GetProcAddress(h: *mut c_void, name: *const std::ffi::c_char) -> *mut c_void;
        fn GetLastError() -> u32;
    }
    #[cfg(not(windows))]
    #[link(name = "dl")]
    unsafe extern "C" {
        fn dlopen(name: *const std::os::raw::c_char, flags: i32) -> *mut c_void;
        fn dlsym(h: *mut c_void, name: *const std::os::raw::c_char) -> *mut c_void;
        fn dlerror() -> *const std::os::raw::c_char;
    }
    pub struct Lib(*mut c_void);
    unsafe impl Send for Lib {}
    unsafe impl Sync for Lib {}
    impl Lib {
        pub fn open(path: &std::path::Path) -> Result<Lib, String> {
            #[cfg(windows)]
            {
                use std::os::windows::ffi::OsStrExt;
                let w: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
                let h = unsafe { LoadLibraryW(w.as_ptr()) };
                if h.is_null() { Err(format!("LoadLibrary error {}", unsafe { GetLastError() })) } else { Ok(Lib(h)) }
            }
            #[cfg(not(windows))]
            {
                let c = CString::new(path.to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
                let h = unsafe { dlopen(c.as_ptr(), 2 /* RTLD_NOW */) };
                if h.is_null() {
                    let e = unsafe { dlerror() };
                    Err(if e.is_null() { "dlopen failed".into() } else { unsafe { std::ffi::CStr::from_ptr(e) }.to_string_lossy().into_owned() })
                } else {
                    Ok(Lib(h))
                }
            }
        }
        pub fn sym(&self, name: &str) -> Option<*mut c_void> {
            let c = CString::new(name).ok()?;
            #[cfg(windows)]
            let p = unsafe { GetProcAddress(self.0, c.as_ptr()) };
            #[cfg(not(windows))]
            let p = unsafe { dlsym(self.0, c.as_ptr()) };
            if p.is_null() { None } else { Some(p) }
        }
    }
}

macro_rules! pg_api {
    ($($f:ident: fn($($an:ident: $at:ty),*) -> $r:ty;)*) => {
        struct Api {
            kind: Backend,
            _lib: dl::Lib,
            $($f: unsafe extern "C" fn($($at),*) -> $r,)*
        }
        impl Api {
            fn load(kind: Backend, lib: dl::Lib) -> Result<Api, String> {
                Ok(Api {
                    kind,
                    $($f: {
                        let p = lib.sym(stringify!($f)).ok_or_else(|| format!("{} has no {}", kind.lib_file(), stringify!($f)))?;
                        unsafe { std::mem::transmute::<*mut std::ffi::c_void, unsafe extern "C" fn($($at),*) -> $r>(p) }
                    },)*
                    _lib: lib,
                })
            }
        }
        $(unsafe fn $f($($an: $at),*) -> $r { unsafe { (api().$f)($($an),*) } })*
    };
}

pg_api! {
    pg_device_count: fn() -> i32;
    pg_sm_count: fn(device: i32) -> i32;
    pg_kdim: fn() -> i32;
    pg_device_name: fn(device: i32, out: *mut std::os::raw::c_char, len: i32) -> i32;
    pg_init: fn(device: i32) -> i32;
    pg_set_job: fn(br_t: *const i8, q1: *const i32, q2: *const i32, n: i32, k: i32) -> i32;
    pg_attempt: fn(seed: *const u8, p1: *const i32, p2: *const i32, m: i32, target: *const u32, hits: *mut Hit, max_hits: i32) -> i32;
    pg_launch: fn(slot: i32, seed: *const u8, p1: *const i32, p2: *const i32, m: i32, target: *const u32) -> i32;
    pg_wait: fn(slot: i32, hits: *mut Hit, max_hits: i32) -> i32;
    pg_debug_al: fn(seed: *const u8, m: i32, row0: i32, rows: i32, out: *mut i8) -> i32;
    pg_last_error: fn() -> *const std::os::raw::c_char;
    pg_device_pci_bus_id: fn(device: i32, out: *mut std::os::raw::c_char, len: i32) -> i32;
    pg_device_uuid: fn(device: i32, out: *mut std::os::raw::c_char, len: i32) -> i32;
    pg_kernel_label: fn() -> *const std::os::raw::c_char;
    pg_kernel_label_for: fn(device: i32) -> *const std::os::raw::c_char;
    pg_row_align: fn() -> i32;
    pg_check_run: fn(seed: *const u8, m: i32, reps: i32, digest: *mut u64, diff_words: *mut u64) -> i32;
}

static API: std::sync::OnceLock<Api> = std::sync::OnceLock::new();

fn open_backend(kind: Backend) -> Result<Api, String> {
    let file = kind.lib_file();
    // next to the executable first (release layout), then the system search path
    let mut errs = Vec::new();
    let mut cands = Vec::new();
    if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())) {
        cands.push(dir.join(file));
    }
    cands.push(std::path::PathBuf::from(file));
    for c in cands {
        match dl::Lib::open(&c) {
            Ok(l) => return Api::load(kind, l),
            Err(e) => errs.push(format!("{}: {e}", c.display())),
        }
    }
    Err(errs.join("; "))
}

/// Loads the search library once: `pref` = "cuda" | "amd" | "auto" (None: $FFGP_BACKEND, else auto).  Later calls return the
/// backend already loaded.
pub fn select(pref: Option<&str>) -> Result<Backend> {
    if let Some(a) = API.get() {
        return Ok(a.kind);
    }
    let env = std::env::var("FFGP_BACKEND").ok();
    let want = pref.map(str::to_owned).or(env).unwrap_or_else(|| "auto".into()).to_ascii_lowercase();
    let api = match want.as_str() {
        "cuda" | "nvidia" => open_backend(Backend::Cuda).map_err(|e| anyhow::anyhow!("CUDA backend: {e}"))?,
        "amd" | "opencl" => open_backend(Backend::Amd).map_err(|e| anyhow::anyhow!("AMD backend: {e}"))?,
        "auto" => {
            let mut why = Vec::new();
            let mut fallback = None;
            let mut chosen = None;
            for kind in [Backend::Cuda, Backend::Amd] {
                match open_backend(kind) {
                    Ok(a) => {
                        if unsafe { (a.pg_device_count)() } > 0 {
                            chosen = Some(a);
                            break;
                        }
                        why.push(format!("{}: no devices", kind.name()));
                        fallback.get_or_insert(a);
                    }
                    Err(e) => why.push(format!("{}: {e}", kind.name())),
                }
            }
            match chosen.or(fallback) {
                Some(a) => a,
                None => bail!("no GPU search library could be loaded ({})", why.join("; ")),
            }
        }
        x => bail!("unknown backend {x:?} (auto, cuda, amd)"),
    };
    let kind = api.kind;
    let _ = API.set(api);
    Ok(kind)
}

fn api() -> &'static Api {
    if API.get().is_none() {
        if let Err(e) = select(None) {
            panic!("{e:#}");
        }
    }
    API.get().unwrap()
}

/// The loaded backend (loads it with the default selection if nothing was selected yet).
pub fn backend() -> Backend {
    api().kind
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

/// Label of the kernel `init(device)` will select, without initialising (the undervolt tuner runs on a thread that never called `init`,
/// where `kernel_label()` would name gen13 on an Ada card).
pub fn kernel_label_for(device: i32) -> String {
    unsafe {
        let p = pg_kernel_label_for(device);
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
