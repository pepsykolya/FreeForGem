// FreeForGemPear — see LICENSE
//! NOID mode: core V/F clock offset of the mining GPUs (NVML).
//!
//! The NOID kernel (a pure integer ALU + CLMAD stream) fails at undervolt offsets that Pearl runs fine with, and it fails
//! SILENTLY near the edge: no Xid, no crash, the quick self-test passes, but some launches return wrong hit sets (RTX 3070 at
//! 1560 MHz: +250 gave 168 bad launches out of 5984).  So in NOID mode a positive owner's offset is set to 0 for the run unless
//! --auto-uv found a NOID-safe value for the card (tune.rs) or --uv-offset N gives one; the owner's offset is recorded in a
//! marker file before the first change and put back on exit (and by the next start after a crash).
use crate::nvml::{Dev, Nvml, OffMode};
use anyhow::{Result, bail};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

pub struct Card {
    pub bus: String,
    pub dev: Dev,
    pub mode: OffMode,
    pub orig: i32,
    pub lo: i32,
    pub hi: i32,
}

struct State {
    marker: PathBuf,
    cards: Vec<Card>,
}

fn nvml() -> Option<&'static Nvml> {
    static N: OnceLock<Option<Nvml>> = OnceLock::new();
    N.get_or_init(|| Nvml::load().ok()).as_ref()
}

fn state() -> &'static Mutex<Option<State>> {
    static S: Mutex<Option<State>> = Mutex::new(None);
    &S
}

fn lg(m: impl AsRef<str>) {
    println!("noid offset: {}", m.as_ref());
}

fn marker_path(dir: &Path) -> PathBuf {
    dir.join("noid-offset-restore.json")
}

/// Puts back the offsets a crashed NOID run left changed (recorded in the marker), then deletes the marker.
pub fn recover(dir: &Path) {
    let p = marker_path(dir);
    let Ok(txt) = std::fs::read_to_string(&p) else { return };
    let Some(nv) = nvml() else { return };
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt) {
        for g in v["gpus"].as_array().into_iter().flatten() {
            let (Some(bus), Some(orig)) = (g["bus"].as_str(), g["orig"].as_i64()) else { continue };
            if let Ok(d) = nv.by_bus(bus) {
                if let Ok((mode, _, _, _)) = nv.probe_offset(d) {
                    match nv.set_offset(d, mode, orig as i32) {
                        Ok(()) => lg(format!("GPU bus {bus}: offset {orig:+} MHz put back (left changed by an earlier NOID run)")),
                        Err(e) => lg(format!("GPU bus {bus}: cannot put back offset {orig:+} ({e:#})")),
                    }
                }
            }
        }
    }
    let _ = std::fs::remove_file(&p);
}

/// Records the owner's offsets of `buses` (marker written before anything changes).  Cards whose offset NVML cannot read are
/// left out (logged).  Returns the NVML handle and the cards it manages.
pub fn begin(buses: &[String], dir: &Path) -> Option<&'static Nvml> {
    let nv = match nvml() {
        Some(n) => n,
        None => {
            lg("NVML not available: core offsets are not checked (NOID needs no undervolt offset, or a NOID-tuned one)");
            return None;
        }
    };
    let mut cards = Vec::new();
    for bus in buses {
        let Ok(d) = nv.by_bus(bus) else { continue };
        match nv.probe_offset(d) {
            Ok((mode, cur, lo, hi)) => cards.push(Card { bus: bus.clone(), dev: d, mode, orig: cur, lo, hi }),
            Err(e) => lg(format!("GPU bus {bus}: offset not readable ({e:#}); left as is")),
        }
    }
    let marker = marker_path(dir);
    let doc = json!({"pid": std::process::id(), "gpus": cards.iter().map(|c| json!({"bus": c.bus, "orig": c.orig})).collect::<Vec<_>>()});
    if let Some(d) = marker.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    if let Err(e) = std::fs::write(&marker, serde_json::to_string_pretty(&doc).unwrap()) {
        lg(format!("cannot write {} ({e}); offsets are still put back on a normal exit", marker.display()));
    }
    *state().lock().unwrap() = Some(State { marker, cards });
    Some(nv)
}

/// (owner's offset, driver range lo, hi) of a managed card.
pub fn info(bus: &str) -> Option<(i32, i32, i32)> {
    state().lock().unwrap().as_ref()?.cards.iter().find(|c| c.bus == bus).map(|c| (c.orig, c.lo, c.hi))
}

pub fn current(bus: &str) -> Option<i32> {
    let g = state().lock().unwrap();
    let c = g.as_ref()?.cards.iter().find(|c| c.bus == bus)?;
    nvml()?.get_offset(c.dev, c.mode).ok()
}

/// Sets the offset of a managed card and reads it back.
pub fn set(bus: &str, v: i32) -> Result<()> {
    let g = state().lock().unwrap();
    let Some(c) = g.as_ref().and_then(|s| s.cards.iter().find(|c| c.bus == bus)) else { bail!("GPU bus {bus}: offset not managed") };
    let nv = nvml().unwrap();
    let v = v.clamp(c.lo, c.hi);
    nv.set_offset(c.dev, c.mode, v)?;
    let r = nv.get_offset(c.dev, c.mode)?;
    if r != v {
        let _ = nv.set_offset(c.dev, c.mode, c.orig);
        bail!("GPU bus {bus}: offset read back {r:+}, expected {v:+}; owner's {:+} put back", c.orig);
    }
    Ok(())
}

/// Puts the owner's offsets back and removes the marker.  Safe to call more than once.
pub fn restore_all() {
    let Some(st) = state().lock().ok().and_then(|mut g| g.take()) else { return };
    let Some(nv) = nvml() else { return };
    let mut lines = Vec::new();
    let mut failed = false;
    for c in &st.cards {
        let now = nv.get_offset(c.dev, c.mode).ok();
        if now == Some(c.orig) {
            continue;
        }
        match nv.set_offset(c.dev, c.mode, c.orig) {
            Ok(()) => lines.push(format!("GPU bus {}: core offset {:+} MHz put back", c.bus, c.orig)),
            Err(e) => {
                failed = true;
                lines.push(format!("GPU bus {}: cannot put back offset {:+} ({e:#}); the marker stays and the next start puts it back", c.bus, c.orig));
            }
        }
    }
    if !failed {
        let _ = std::fs::remove_file(&st.marker);
    }
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
