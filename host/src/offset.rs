// FreeForGemPear — see LICENSE
//! Keeper of the core V/F clock offset of the mining GPUs (NVML), used by the NEB undervolt (neb/mod.rs, neb/tune.rs):
//! `--uv-offset N` and `--auto-uv` change the offset only after the owner's offset is recorded in a marker file, and it is
//! put back on exit, on a signal and by the next start after a crash.
//! (FreeForGem 1.7.8: written for the NOID mode, whose kernel failed silently at offsets other coins ran fine with; the log
//! prefix "offset"; the marker is ffgn-offset-restore.json (the older name noid-offset-restore.json is still read once).)
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

pub(crate) fn nvml() -> Option<&'static Nvml> {
    static N: OnceLock<Option<Nvml>> = OnceLock::new();
    N.get_or_init(|| Nvml::load().ok()).as_ref()
}

fn state() -> &'static Mutex<Option<State>> {
    static S: Mutex<Option<State>> = Mutex::new(None);
    &S
}

fn lg(m: impl AsRef<str>) {
    println!("offset: {}", m.as_ref());
}

fn marker_path(dir: &Path) -> PathBuf {
    dir.join(crate::coins::group_file("ffgn-offset-restore.json")) // 1.8.0: per group of cards (coins::group_file)
}

/// The marker name of FFGN 0.9.x and FreeForGem (D-015): still read once, then removed.
const OLD_MARKER: &str = "noid-offset-restore.json";

/// Puts back the offsets a crashed run left changed (recorded in the marker), then deletes the marker.
pub fn recover(dir: &Path) {
    let old = dir.join(OLD_MARKER);
    if old.is_file() && !marker_path(dir).is_file() {
        let _ = std::fs::rename(&old, marker_path(dir));
    }
    let p = marker_path(dir);
    let Ok(txt) = std::fs::read_to_string(&p) else { return };
    let Some(nv) = nvml() else { return };
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt) {
        for g in v["gpus"].as_array().into_iter().flatten() {
            let (Some(bus), Some(orig)) = (g["bus"].as_str(), g["orig"].as_i64()) else { continue };
            if let Ok(d) = nv.by_bus(bus) {
                if let Ok((mode, cur, lo, hi)) = nv.probe_offset(d) {
                    // on HiveOS the profile's Core Clock value (1.7.2), not a value another program may have left
                    let orig = crate::profiles::owner_offset(nv, d, dir, orig as i32, lo, hi, "offset");
                    let _ = cur;
                    match nv.set_offset(d, mode, orig) {
                        Ok(()) => lg(format!("GPU bus {bus}: offset {orig:+} MHz put back (left changed by an earlier run that did not exit cleanly)")),
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
            lg("NVML not available: core offsets are not checked");
            return None;
        }
    };
    let mut cards = Vec::new();
    for bus in buses {
        let Ok(d) = nv.by_bus(bus) else { continue };
        match nv.probe_offset(d) {
            Ok((mode, cur, lo, hi)) => cards.push(Card { bus: bus.clone(), dev: d, mode, orig: crate::profiles::owner_offset(nv, d, dir, cur, lo, hi, "noid offset"), lo, hi }),
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

/// Test hook (tests only, 1.0.3): FFG_TEST_OFFSET_SIM=1 keeps the offsets this miner would set in memory and never writes them to
/// the GPU (the live test of the auto-uv start logic on a machine without administrator rights).  Never set in normal use.
fn sim() -> Option<&'static Mutex<std::collections::HashMap<String, i32>>> {
    static S: OnceLock<Option<Mutex<std::collections::HashMap<String, i32>>>> = OnceLock::new();
    S.get_or_init(|| std::env::var("FFG_TEST_OFFSET_SIM").is_ok_and(|v| v == "1").then(|| Mutex::new(Default::default()))).as_ref()
}

/// (owner's offset, driver range lo, hi) of a managed card.
pub fn info(bus: &str) -> Option<(i32, i32, i32)> {
    state().lock().unwrap().as_ref()?.cards.iter().find(|c| c.bus == bus).map(|c| (c.orig, c.lo, c.hi))
}

/// 1.0.12 (nvml.rs): the last offset this run set on a card (`set`), and one deferred in the card's quiet period after a fault;
/// `after_fault` sets it when the card's NVML calls resume (a driver reset may have dropped it).
fn wanted() -> &'static Mutex<Vec<(String, i32, bool)>> {
    static W: Mutex<Vec<(String, i32, bool)>> = Mutex::new(Vec::new());
    &W
}

fn note_wanted(bus: &str, v: i32, deferred: bool) {
    let mut w = wanted().lock().unwrap_or_else(|e| e.into_inner());
    w.retain(|x| !crate::nvml::quiet::same_bus(&x.0, bus));
    w.push((bus.to_string(), v, deferred));
}

/// 1.0.12: the card's NVML calls resume after its quiet period (nvml.rs resume hook, after this run's clock locks): the offset this
/// run wants there (deferred in the quiet period, or the last one set) is set again when the card reads another.
pub fn after_fault(bus: &str) {
    let want = wanted().lock().unwrap_or_else(|e| e.into_inner()).iter().find(|x| crate::nvml::quiet::same_bus(&x.0, bus)).map(|x| (x.0.clone(), x.1, x.2));
    let Some((mbus, v, deferred)) = want else { return };
    let now = {
        let g = state().lock().unwrap_or_else(|e| e.into_inner());
        let Some(c) = g.as_ref().and_then(|s| s.cards.iter().find(|c| c.bus == mbus)) else { return };
        if sim().is_some() {
            None
        } else {
            nvml().and_then(|n| n.get_offset(c.dev, c.mode).ok())
        }
    };
    if now == Some(v) && !deferred {
        return;
    }
    match set(&mbus, v) {
        Ok(()) => lg(format!(
            "GPU bus {mbus}: offset {v:+} MHz set again after the GPU's fault ({})",
            match now {
                Some(n) if n != v => format!("it read {n:+} after the driver's reset"),
                _ if deferred => "deferred while no NVML call was made for this GPU".to_string(),
                _ => "checked".to_string(),
            }
        )),
        Err(e) => lg(format!("GPU bus {mbus}: cannot set offset {v:+} again after the GPU's fault ({e:#})")),
    }
}

/// 1.0.13 (recover.rs): the offset last APPLIED on a card by this run (set and read back; the owner's at the start), not one
/// deferred in its quiet period: the offset the card ran at when it faulted.
fn applied_tab() -> &'static Mutex<Vec<(String, i32)>> {
    static A: Mutex<Vec<(String, i32)>> = Mutex::new(Vec::new());
    &A
}

fn note_applied(bus: &str, v: i32) {
    let mut a = applied_tab().lock().unwrap_or_else(|e| e.into_inner());
    a.retain(|x| !crate::nvml::quiet::same_bus(&x.0, bus));
    a.push((bus.to_string(), v));
}

/// The offset applied on a managed card now (as far as this run knows); None: not managed.
pub fn applied(bus: &str) -> Option<i32> {
    if let Some(v) = applied_tab().lock().unwrap_or_else(|e| e.into_inner()).iter().find(|x| crate::nvml::quiet::same_bus(&x.0, bus)).map(|x| x.1) {
        return Some(v);
    }
    info(bus).map(|i| i.0)
}

/// 1.0.13 (recover.rs): after a fault the card restarted at a safe offset below the crash: no set of this run goes above it
/// any more (the owner's rule 2: failed - 2 steps, then a short check and mining, no new climb in this run).
fn caps() -> &'static Mutex<Vec<(String, i32)>> {
    static C: Mutex<Vec<(String, i32)>> = Mutex::new(Vec::new());
    &C
}

pub fn set_cap(bus: &str, v: i32) {
    let mut c = caps().lock().unwrap_or_else(|e| e.into_inner());
    let old = c.iter().find(|x| crate::nvml::quiet::same_bus(&x.0, bus)).map(|x| x.1);
    c.retain(|x| !crate::nvml::quiet::same_bus(&x.0, bus));
    c.push((bus.to_string(), old.map_or(v, |o| o.min(v))));
}

pub fn cap(bus: &str) -> Option<i32> {
    caps().lock().unwrap_or_else(|e| e.into_inner()).iter().find(|x| crate::nvml::quiet::same_bus(&x.0, bus)).map(|x| x.1)
}

/// `v` as `set` applies it (the cap after a fault).
pub fn capped(bus: &str, v: i32) -> i32 {
    cap(bus).map_or(v, |c| v.min(c))
}

/// 1.0.13 (recover.rs): the card's offset read now, also right after its quiet period (`current` gives no reading then).
pub fn read_now(bus: &str) -> Option<i32> {
    let g = state().lock().unwrap_or_else(|e| e.into_inner());
    let c = g.as_ref()?.cards.iter().find(|c| c.bus == bus)?;
    if let Some(s) = sim() {
        return Some(s.lock().unwrap().get(bus).copied().unwrap_or(c.orig));
    }
    nvml()?.get_offset(c.dev, c.mode).ok()
}

pub fn current(bus: &str) -> Option<i32> {
    // 1.0.12: no reading in the card's quiet period after a fault, nor while its offset is being set again after it (a reading of a
    // reset card is not a change by someone else)
    if crate::nvml::quiet::settling(bus) {
        return None;
    }
    let g = state().lock().unwrap();
    let c = g.as_ref()?.cards.iter().find(|c| c.bus == bus)?;
    if let Some(s) = sim() {
        return Some(s.lock().unwrap().get(bus).copied().unwrap_or(c.orig));
    }
    nvml()?.get_offset(c.dev, c.mode).ok()
}

/// Sets the offset of a managed card and reads it back.
pub fn set(bus: &str, v: i32) -> Result<()> {
    let g = state().lock().unwrap();
    let Some(c) = g.as_ref().and_then(|s| s.cards.iter().find(|c| c.bus == bus)) else { bail!("GPU bus {bus}: offset not managed") };
    let nv = nvml().unwrap();
    let want = v;
    let v = capped(bus, v).clamp(c.lo, c.hi);
    if v < want.clamp(c.lo, c.hi) {
        lg(format!("GPU bus {bus}: offset {want:+} capped at {v:+} (the safe offset this GPU restarted at after its fault in this run)"));
    }
    if let Some(s) = sim() {
        s.lock().unwrap().insert(bus.to_string(), v);
        note_applied(bus, v);
        note_wanted(bus, v, false);
        return Ok(());
    }
    if let Err(e) = nv.set_offset(c.dev, c.mode, v) {
        // 1.0.12 (nvml.rs): no NVML call in the card's quiet period after a fault: the offset is set when its calls resume
        // (`after_fault`), before the card is judged again (auto-uv waits for the end of the quiet period)
        if crate::nvml::is_busy(&e) {
            note_wanted(bus, v, true);
            lg(format!("GPU bus {bus}: offset {v:+} MHz deferred: no NVML call for this GPU right after its fault (set when they resume)"));
            return Ok(());
        }
        return Err(e);
    }
    let r = nv.get_offset(c.dev, c.mode)?;
    if r != v {
        if nv.set_offset(c.dev, c.mode, c.orig).is_ok() {
            note_applied(bus, c.orig);
        }
        bail!("GPU bus {bus}: offset read back {r:+}, expected {v:+}; owner's {:+} put back", c.orig);
    }
    note_wanted(bus, v, false);
    note_applied(bus, v);
    Ok(())
}

/// Puts the owner's offsets back and removes the marker.  Safe to call more than once.
pub fn restore_all() {
    crate::nvml::quiet::set_exiting();
    let Some(st) = state().lock().ok().and_then(|mut g| g.take()) else { return };
    if sim().is_some() {
        let _ = std::fs::remove_file(&st.marker);
        return;
    }
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
