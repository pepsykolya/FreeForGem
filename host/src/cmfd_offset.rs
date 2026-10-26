// FreeForGem — see LICENSE
//! CMFD mode: core V/F clock offset of the mining GPUs.
//!
//! A positive V/F offset (the undervolt that Pearl runs well with: +200 on RTX 3070, +330..345 on RTX 4070 Ti) makes the
//! CMFD engine fail at once ("illegal memory access", "misaligned address", or silently wrong results): its tensor-core
//! path loads the GPU much harder. So in CMFD mode the host sets a positive offset back to 0 (stock) before the engine
//! starts and puts the previous value back on exit; a crash leaves a marker file that the next start undoes.
//! An explicit `--uv-offset N` is applied as given instead (the user takes the risk). Negative offsets are left alone.
//! Needs root/Administrator like every NVML setting; without it mining goes on with a warning.
//! `--auto-uv` (NVIDIA): `apply_managed` records every mining card (also those already at 0 or below) before the first change,
//! and the CMFD tuner (cmfd_uv.rs) changes the offsets only through `set`: the same marker and the same restore put the
//! owner's offset back on exit, on a crash (next start) or after a second Ctrl-C.
use crate::gpu;
use crate::nvml::{Dev, Nvml, OffMode};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

struct Item {
    dev: Dev,
    bus: String,
    mode: OffMode,
    orig: i32,
    cur: i32,
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
    println!("cmfd offset: {}", m.as_ref());
}

fn marker_path(dir: &Path) -> PathBuf {
    dir.join("cmfd-offset-restore.json")
}

/// Puts back the offsets a crashed CMFD run left changed (recorded in the marker), then deletes the marker.
pub fn recover(dir: &Path) {
    let p = marker_path(dir);
    let Ok(txt) = std::fs::read_to_string(&p) else { return };
    let Ok(nv) = Nvml::load() else { return };
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt) {
        for g in v["gpus"].as_array().into_iter().flatten() {
            let (Some(bus), Some(orig)) = (g["bus"].as_str(), g["orig"].as_i64()) else { continue };
            if let Ok(d) = nv.by_bus(bus) {
                if let Ok((mode, _, _, _)) = nv.probe_offset(d) {
                    match nv.set_offset(d, mode, orig as i32) {
                        Ok(()) => lg(format!("GPU bus {bus}: offset {orig:+} MHz put back (left changed by an earlier CMFD run)")),
                        Err(e) => lg(format!("GPU bus {bus}: cannot put back offset {orig:+} ({e:#})")),
                    }
                }
            }
        }
    }
    let _ = std::fs::remove_file(&p);
}

/// Offset to use: `fixed` (explicit --uv-offset for that GPU, by its position in `devices`) or 0 when the current offset is
/// positive. Returns a guard that restores.
pub fn apply(devices: &[i32], fixed_all: &crate::uv::PerGpu, dir: &Path) -> Option<Guard> {
    let nv = match Nvml::load() {
        Ok(n) => n,
        Err(e) => {
            lg(format!("NVML not available ({e:#}); offsets not checked. CMFD needs stock clocks (no undervolt offset)"));
            return None;
        }
    };
    let mut items = Vec::new();
    for (pos, &dev) in devices.iter().enumerate() {
        let fixed = fixed_all.get(pos);
        let Ok(bus) = gpu::device_pci_bus_id(dev) else { continue };
        let Ok(d) = nv.by_bus(&bus) else { continue };
        let (mode, cur, lo, hi) = match nv.probe_offset(d) {
            Ok(x) => x,
            Err(e) => {
                lg(format!("GPU{dev}: offset not readable ({e:#}); left as is"));
                continue;
            }
        };
        let target = match fixed {
            Some(v) => v.clamp(lo, hi),
            None if cur > 0 => 0,
            None => {
                lg(format!("GPU{dev}: core offset {cur:+} MHz, nothing to change"));
                continue;
            }
        };
        if target == cur {
            lg(format!("GPU{dev}: core offset already {cur:+} MHz"));
            continue;
        }
        match nv.set_offset(d, mode, target) {
            Ok(()) => {
                if fixed.is_some() {
                    lg(format!("GPU{dev}: core offset {cur:+} -> {target:+} MHz (--uv-offset; at your own risk for CMFD); put back on exit"));
                } else {
                    lg(format!(
                        "GPU{dev}: core offset {cur:+} -> 0 MHz (an undervolt offset makes CMFD compute wrong results; --uv-offset N overrides); put back on exit"
                    ));
                }
                items.push(Item { dev: d, bus: bus.clone(), mode, orig: cur, cur: target });
            }
            Err(e) => lg(format!(
                "GPU{dev}: cannot change core offset {cur:+} ({e:#}); run as root/Administrator or set the offset to 0 yourself, otherwise CMFD will likely fail on this GPU"
            )),
        }
    }
    if items.is_empty() {
        return None;
    }
    let marker = marker_path(dir);
    let doc = json!({
        "pid": std::process::id(),
        "gpus": items.iter().map(|it| json!({"bus": it.bus, "orig": it.orig})).collect::<Vec<_>>(),
    });
    let _ = std::fs::write(&marker, serde_json::to_string_pretty(&doc).unwrap());
    *state().lock().unwrap() = Some(State { nv, marker, items });
    Some(Guard)
}

/// `--auto-uv` for CMFD: like `apply` without a fixed value (a positive offset goes to 0, the CMFD base), but every card whose
/// offset can be read is recorded, so any later change by the tuner (`set`) is undone by the same restore. Returns the guard
/// and, per device (same order as `devices`), the base offset the card now runs at (None: not manageable, left alone).
pub fn apply_managed(devices: &[i32], dir: &Path) -> (Option<Guard>, Vec<Option<i32>>) {
    let mut bases = vec![None; devices.len()];
    let nv = match Nvml::load() {
        Ok(n) => n,
        Err(e) => {
            lg(format!("NVML not available ({e:#}); offsets not checked. CMFD needs stock clocks (no undervolt offset)"));
            return (None, bases);
        }
    };
    let mut items = Vec::new();
    for (pos, &dev) in devices.iter().enumerate() {
        let Ok(bus) = gpu::device_pci_bus_id(dev) else { continue };
        let Ok(d) = nv.by_bus(&bus) else { continue };
        let (mode, cur, _, _) = match nv.probe_offset(d) {
            Ok(x) => x,
            Err(e) => {
                lg(format!("GPU{dev}: offset not readable ({e:#}); left as is"));
                continue;
            }
        };
        let target = if cur > 0 { 0 } else { cur };
        // permission probe (writes the value the card already has when nothing changes)
        match nv.set_offset(d, mode, target) {
            Ok(()) => {
                if target != cur {
                    lg(format!("GPU{dev}: core offset {cur:+} -> 0 MHz (CMFD base; --auto-uv tunes the CMFD offset from here); put back on exit"));
                }
                items.push(Item { dev: d, bus: bus.clone(), mode, orig: cur, cur: target });
                bases[pos] = Some(target);
            }
            Err(e) => lg(format!(
                "GPU{dev}: cannot change core offset {cur:+} ({e:#}); run as root/Administrator{}",
                if cur > 0 { " or set the offset to 0 yourself, otherwise CMFD will likely fail on this GPU" } else { "" }
            )),
        }
    }
    if items.is_empty() {
        return (None, bases);
    }
    let marker = marker_path(dir);
    let doc = json!({
        "pid": std::process::id(),
        "gpus": items.iter().map(|it| json!({"bus": it.bus, "orig": it.orig})).collect::<Vec<_>>(),
    });
    if let Err(e) = std::fs::write(&marker, serde_json::to_string_pretty(&doc).unwrap()) {
        // without the marker a crash could leave a tuned offset behind: give the cards back now and do not tune
        lg(format!("cannot write {} ({e}); offsets left as the owner had them, no tuning", marker.display()));
        for it in &items {
            let _ = nv.set_offset(it.dev, it.mode, it.orig);
        }
        return (None, vec![None; devices.len()]);
    }
    *state().lock().unwrap() = Some(State { nv, marker, items });
    (Some(Guard), bases)
}

/// Sets the core offset of a card recorded by `apply_managed` (by PCI bus id) and reads it back. Refused after the restore.
pub fn set(bus: &str, v: i32) -> anyhow::Result<()> {
    let mut g = state().lock().unwrap();
    let st = g.as_mut().ok_or_else(|| anyhow::anyhow!("offsets already restored"))?;
    let it = st.items.iter_mut().find(|it| it.bus == bus).ok_or_else(|| anyhow::anyhow!("GPU bus {bus} is not managed"))?;
    st.nv.set_offset(it.dev, it.mode, v)?;
    std::thread::sleep(std::time::Duration::from_millis(150));
    let rb = st.nv.get_offset(it.dev, it.mode)?;
    it.cur = rb;
    anyhow::ensure!(rb == v, "read-back {rb:+} differs from the value just set ({v:+})");
    Ok(())
}

/// The offset the card has now (read from the driver; None: not managed or already restored).
pub fn read(bus: &str) -> Option<i32> {
    let g = state().lock().ok()?;
    let st = g.as_ref()?;
    let it = st.items.iter().find(|it| it.bus == bus)?;
    st.nv.get_offset(it.dev, it.mode).ok()
}

/// Is `v`, read back on a managed card instead of the offset this run set, an OC apply (HiveOS nvidia-oc: the profile's Core
/// Clock value or 0; another tool: the owner's offset)?  See hiveoc::reset_kind.
pub fn reset_kind(bus: &str, v: i32) -> Option<&'static str> {
    let g = state().lock().ok()?;
    let st = g.as_ref()?;
    let it = st.items.iter().find(|it| it.bus == bus)?;
    crate::hiveoc::reset_kind(&st.nv, it.dev, v, it.orig)
}

/// The owner's offset of a managed card (what the restore puts back).
pub fn orig(bus: &str) -> Option<i32> {
    state().lock().ok()?.as_ref()?.items.iter().find(|it| it.bus == bus).map(|it| it.orig)
}

/// Offset this run set on the card (None: not managed or already restored).
pub fn current(bus: &str) -> Option<i32> {
    state().lock().ok()?.as_ref()?.items.iter().find(|it| it.bus == bus).map(|it| it.cur)
}

/// Puts the original offsets back and removes the marker. Safe to call more than once.
pub fn restore_all() {
    let Some(st) = state().lock().ok().and_then(|mut g| g.take()) else { return };
    let mut lines = Vec::new();
    let mut failed = false;
    for it in &st.items {
        match st.nv.set_offset(it.dev, it.mode, it.orig) {
            Ok(()) => lines.push(format!("GPU bus {}: core offset {:+} MHz put back", it.bus, it.orig)),
            Err(e) => {
                failed = true;
                lines.push(format!("GPU bus {}: cannot put back offset {:+} ({e:#}); the marker stays and the next start puts it back", it.bus, it.orig));
            }
        }
    }
    if !failed {
        let _ = std::fs::remove_file(&st.marker);
    }
    // logged only after every card is set back (the log pipe may be gone at shutdown)
    for l in lines {
        lg(l);
    }
}

pub struct Guard;
impl Drop for Guard {
    fn drop(&mut self) {
        restore_all();
    }
}
