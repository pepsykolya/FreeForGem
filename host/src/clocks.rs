// FreeForGemPear — see LICENSE
//! Optional GPU clock control via NVML: lock the core clock, lock the memory clock and set a memory-clock
//! offset (`--lock-cclock` / `--lock-mclock` / `--moffset`).  Applied to the mining devices at start and
//! undone on exit (Ctrl-C/SIGTERM and after a crash, via a small marker file).  Needs root/Administrator.
//! Independent of `--auto-uv`; the kernel and mining are untouched.
use crate::gpu;
use crate::nvml::{Dev, Nvml};
use anyhow::Result;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Per mining device (in --devices order); `None` leaves that clock untouched.
#[derive(Clone, Default)]
pub struct ClockOpts {
    pub core_mhz: Vec<Option<u32>>,
    pub mem_mhz: Vec<Option<u32>>,
    pub mem_off: Vec<Option<i32>>,
}

impl ClockOpts {
    pub fn any(&self) -> bool {
        self.core_mhz.iter().any(Option::is_some)
            || self.mem_mhz.iter().any(Option::is_some)
            || self.mem_off.iter().any(Option::is_some)
    }
}

struct Item {
    dev: Dev,
    bus: String,
    core_locked: bool,
    mem_locked: bool,
    mem_off: bool,
}

struct State {
    nv: Nvml,
    marker: PathBuf,
    items: Vec<Item>,
}

fn state() -> &'static Mutex<Option<State>> {
    static S: Mutex<Option<State>> = Mutex::new(None);
    &S
}

fn lg(m: impl AsRef<str>) {
    println!("clocks: {}", m.as_ref());
}

fn marker_path(dir: &Path) -> PathBuf {
    dir.join("clocks-restore.json")
}

/// Undo clock locks left by a previous run that did not exit cleanly (crash / power loss): the locks persist
/// in the driver until reset or reboot, so clear exactly those we recorded, then delete the marker.
pub fn recover(dir: &Path) {
    let p = marker_path(dir);
    let Ok(txt) = std::fs::read_to_string(&p) else { return };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt) else {
        let _ = std::fs::remove_file(&p);
        return;
    };
    let nv = match Nvml::load() {
        Ok(n) => n,
        Err(_) => return,
    };
    let mut n = 0;
    for g in v.get("gpus").and_then(|x| x.as_array()).map(|a| a.as_slice()).unwrap_or(&[]) {
        let Some(bus) = g.get("bus").and_then(|x| x.as_str()) else { continue };
        let Ok(d) = nv.by_bus(bus) else { continue };
        if g.get("core_locked").and_then(|x| x.as_bool()).unwrap_or(false) {
            let _ = nv.reset_core_locked(d);
        }
        if g.get("mem_locked").and_then(|x| x.as_bool()).unwrap_or(false) {
            let _ = nv.reset_mem_locked(d);
        }
        if g.get("mem_off").and_then(|x| x.as_bool()).unwrap_or(false) {
            let _ = nv.set_mem_offset(d, 0);
        }
        n += 1;
    }
    if n > 0 {
        lg(format!("undid clock settings left by an earlier run ({n} GPU(s))"));
    }
    let _ = std::fs::remove_file(&p);
}

/// Applies the requested clock locks / memory offset to `devices`.  Returns a guard that restores them on drop;
/// `None` if nothing was requested, NVML is missing, or no device could be changed (then mining goes on as is).
pub fn apply(devices: &[i32], o: &ClockOpts, dir: &Path) -> Option<Guard> {
    if !o.any() {
        return None;
    }
    let nv = match Nvml::load() {
        Ok(n) => n,
        Err(e) => {
            lg(format!("NVML not available ({e:#}); clock settings skipped"));
            return None;
        }
    };
    let mut items = Vec::new();
    for (i, &dev) in devices.iter().enumerate() {
        let core = o.core_mhz.get(i).copied().flatten();
        let mem = o.mem_mhz.get(i).copied().flatten();
        let moff = o.mem_off.get(i).copied().flatten();
        if core.is_none() && mem.is_none() && moff.is_none() {
            continue;
        }
        let bus = match gpu::device_pci_bus_id(dev) {
            Ok(b) => b,
            Err(e) => {
                lg(format!("GPU{dev}: no PCI bus id ({e:#}); left alone"));
                continue;
            }
        };
        let d = match nv.by_bus(&bus) {
            Ok(d) => d,
            Err(e) => {
                lg(format!("GPU{dev} (bus {bus}): NVML handle failed ({e:#}); left alone"));
                continue;
            }
        };
        let mut it = Item { dev: d, bus: bus.clone(), core_locked: false, mem_locked: false, mem_off: false };
        if let Some(mhz) = core {
            match nv.set_core_locked(d, mhz) {
                Ok(()) => {
                    it.core_locked = true;
                    lg(format!("GPU{dev} (bus {bus}): core clock locked to {mhz} MHz"));
                }
                Err(e) => lg(format!("GPU{dev} (bus {bus}): cannot lock core clock ({e:#}); needs root/Administrator and a driver that supports it", )),
            }
        }
        if let Some(mhz) = mem {
            match nv.set_mem_locked(d, mhz) {
                Ok(()) => {
                    it.mem_locked = true;
                    lg(format!("GPU{dev} (bus {bus}): memory clock locked to {mhz} MHz"));
                }
                Err(e) => lg(format!("GPU{dev} (bus {bus}): cannot lock memory clock ({e:#})")),
            }
        }
        if let Some(v) = moff {
            match nv.set_mem_offset(d, v) {
                Ok(()) => {
                    it.mem_off = true;
                    lg(format!("GPU{dev} (bus {bus}): memory clock offset {v:+} MHz"));
                }
                Err(e) => lg(format!("GPU{dev} (bus {bus}): cannot set memory offset ({e:#}); needs driver 555+ and root/Administrator")),
            }
        }
        if it.core_locked || it.mem_locked || it.mem_off {
            items.push(it);
        }
    }
    if items.is_empty() {
        return None;
    }
    // marker for crash recovery (locks persist in the driver until reset/reboot)
    let marker = marker_path(dir);
    let doc = json!({
        "pid": std::process::id(),
        "gpus": items.iter().map(|it| json!({"bus": it.bus, "core_locked": it.core_locked, "mem_locked": it.mem_locked, "mem_off": it.mem_off})).collect::<Vec<_>>(),
    });
    let _ = std::fs::write(&marker, serde_json::to_string_pretty(&doc).unwrap());
    *state().lock().unwrap() = Some(State { nv, marker, items });
    Some(Guard)
}

/// Resets every clock setting this run applied, and removes the marker.  Safe to call more than once.
pub fn restore_all() {
    let Some(st) = state().lock().unwrap().take() else { return };
    for it in &st.items {
        if it.core_locked {
            let _ = st.nv.reset_core_locked(it.dev);
        }
        if it.mem_locked {
            let _ = st.nv.reset_mem_locked(it.dev);
        }
        if it.mem_off {
            let _ = st.nv.set_mem_offset(it.dev, 0);
        }
    }
    let _ = std::fs::remove_file(&st.marker);
    // logged only after every card is reset (the log pipe may be gone at shutdown)
    for it in &st.items {
        lg(format!("GPU bus {}: clock settings restored to default", it.bus));
    }
}

/// Restores clock settings when dropped (normal shutdown).  Signal handlers call `restore_all` directly.
pub struct Guard;
impl Drop for Guard {
    fn drop(&mut self) {
        restore_all();
    }
}

/// Expands a `--lock-cclock`/… value into one entry per device: a single number applies to all, a
/// comma-separated list is taken in `--devices` order and must have one value per device.
pub fn expand_u32(raw: &str, n: usize, flag: &str) -> Result<Vec<Option<u32>>> {
    expand(raw, n, flag, |s| s.parse::<u32>().ok().filter(|&v| v > 0))
}
pub fn expand_i32(raw: &str, n: usize, flag: &str) -> Result<Vec<Option<i32>>> {
    expand(raw, n, flag, |s| s.trim().trim_start_matches('+').parse::<i32>().ok())
}
fn expand<T: Copy>(raw: &str, n: usize, flag: &str, parse: impl Fn(&str) -> Option<T>) -> Result<Vec<Option<T>>> {
    let parts: Vec<&str> = raw.split(',').map(str::trim).collect();
    if parts.len() == 1 {
        let v = parse(parts[0]).ok_or_else(|| anyhow::anyhow!("{flag}: '{}' is not a valid number", parts[0]))?;
        return Ok(vec![Some(v); n]);
    }
    anyhow::ensure!(parts.len() == n, "{flag}: {} values for {n} device(s) (give one value for all, or one per device in --devices order)", parts.len());
    parts
        .iter()
        .map(|s| {
            if s.is_empty() || *s == "-" {
                Ok(None) // skip this device
            } else {
                parse(s).map(Some).ok_or_else(|| anyhow::anyhow!("{flag}: '{s}' is not a valid number"))
            }
        })
        .collect()
}
