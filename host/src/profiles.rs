// FreeForGem — see LICENSE
//! The profile store shared by the three auto-uv tuners (Pearl, NOID, CMFD), 1.7.6.
//!
//! One `uv.json` holds the sections of all of them; each tuner keeps its own `Sections` (where its profiles and its crash markers
//! live) and its own entry type (`Profile`).  This module owns what was copied three times before: loading the file, the
//! atomic write, "the profile in use on this card", save (a result of the same clocks replaces the old one, the others stay),
//! forget, the crash marker (`{offset, good}`) and the list of profiles for `climb::hint_start`.
//!
//! The on-disk format is NOT changed by 1.7.6: every key, every field, and the key order (serde_json sorts the keys) are the
//! ones 1.7.5 wrote, and a new field would have to be optional.  A file written here is read by 1.7.5 (the golden tests in
//! `tests/data/` are real 1.7.5 files: load, save, compare).
//!
//! Where the tuners differ they keep it: the key of a profile (`profile_key` / `eff@1400` / `1805`), when two profiles are "the
//! same clocks", and the migrations of the sections of older versions (run on the loaded `Value` before the store is built).
use crate::climb::HintProfile;
use crate::nvml::Dev;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Where one tuner keeps its data in uv.json.
#[derive(Clone, Copy, Debug)]
pub struct Sections {
    /// uuid -> profile key -> entry
    pub profiles: &'static str,
    /// uuid -> {"offset", "good"?}: the offset under check (a crash leaves it)
    pub trying: &'static str,
    /// "version" written at a save (Pearl: 2); None: the field is left as it is
    pub version: Option<u32>,
}

pub const PEARL: Sections = Sections { profiles: "profiles", trying: "trying", version: Some(2) };
pub const NOID: Sections = Sections { profiles: "noid", trying: "noid_trying", version: None };
pub const CMFD: Sections = Sections { profiles: "cmfd_profiles", trying: "cmfd_trying", version: None };

/// An entry type of a tuner: its JSON form and when two entries are the profile of the same clocks.
pub trait Profile: Clone {
    fn from_json(v: &Value) -> Option<Self>;
    fn to_json(&self) -> Value;
    fn same_clocks(&self, other: &Self) -> bool;
}

/// What a previous run left in a crash marker: the offset it died at, and the highest step that had passed before.
/// (1.5.2 wrote a bare number; later versions write {"offset", "good"}; NOID before 1.7.6 wrote no "good".)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Crash {
    pub at: i32,
    pub good: Option<i32>,
}

pub fn results_path(dir: &Path) -> PathBuf {
    dir.join("uv.json")
}

/// The whole uv.json (an unreadable / missing file = an empty one), nothing migrated.
pub fn load_value(dir: &Path) -> Value {
    std::fs::read_to_string(results_path(dir))
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({"version": 1, "gpus": {}}))
}

pub fn write_value(dir: &Path, v: &Value) -> Result<(), String> {
    crate::uv::write_atomic(&results_path(dir), &serde_json::to_string_pretty(v).unwrap()).map_err(|e| format!("{e:#}"))
}

pub struct Store {
    pub dir: PathBuf,
    sec: Sections,
    v: Mutex<Value>,
    used: Mutex<HashMap<String, String>>, // uuid -> key of the profile this run uses
    good: Mutex<HashMap<String, i32>>,    // uuid -> highest step that passed in the tuning now running (goes into the crash marker)
    log: fn(&str),
}

impl Store {
    /// Loads uv.json; `migrate` (the tuner's own, for the sections of older versions) changes the loaded value and returns how
    /// many results it moved; the file is rewritten then.
    pub fn load(dir: &Path, sec: Sections, log: fn(&str), migrate: impl FnOnce(&mut Value) -> usize) -> Store {
        let mut v = load_value(dir);
        let moved = migrate(&mut v);
        let s = Store { dir: dir.to_path_buf(), sec, v: Mutex::new(Value::Null), used: Mutex::new(HashMap::new()), good: Mutex::new(HashMap::new()), log };
        if moved > 0 {
            s.write_to(&v);
        }
        *s.v.lock().unwrap() = v;
        s
    }

    fn write_to(&self, v: &Value) {
        if let Err(e) = write_value(&self.dir, v) {
            (self.log)(&format!("cannot save {}: {e}", results_path(&self.dir).display()));
        }
    }

    /// A copy of the whole file as it is now (tests, migrations, other tuners' sections).
    #[cfg(test)]
    pub fn snapshot(&self) -> Value {
        self.v.lock().unwrap().clone()
    }

    /// All profiles of the card whose key starts with `prefix` ("" = all), sorted by key.
    pub fn list<E: Profile>(&self, uuid: &str, prefix: &str) -> Vec<(String, E)> {
        let v = self.v.lock().unwrap();
        let mut out: Vec<(String, E)> = v[self.sec.profiles][uuid]
            .as_object()
            .map(|m| m.iter().filter(|(k, _)| k.starts_with(prefix)).filter_map(|(k, x)| E::from_json(x).map(|e| (k.clone(), e))).collect())
            .unwrap_or_default();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    pub fn get<E: Profile>(&self, uuid: &str, key: &str) -> Option<E> {
        self.v.lock().unwrap()[self.sec.profiles][uuid].get(key).and_then(E::from_json)
    }

    /// The profile this run uses on the card (chosen at start, or the one just saved).
    pub fn current<E: Profile>(&self, uuid: &str) -> Option<E> {
        let k = self.used.lock().unwrap().get(uuid).cloned()?;
        self.get(uuid, &k)
    }

    pub fn use_key(&self, uuid: &str, key: &str) {
        self.used.lock().unwrap().insert(uuid.into(), key.into());
    }

    pub fn clear_use(&self, uuid: &str) {
        self.used.lock().unwrap().remove(uuid);
    }

    /// Saves `e` under `key`: a profile of the same clocks (among those whose key starts with `scope`; NOID: the mode) is
    /// replaced, the others stay untouched.  The saved profile becomes the one in use.
    pub fn save<E: Profile>(&self, uuid: &str, scope: &str, key: &str, e: &E) {
        let mut v = self.v.lock().unwrap();
        if let Some(ver) = self.sec.version {
            v["version"] = json!(ver);
        }
        if !v[self.sec.profiles].is_object() {
            v[self.sec.profiles] = json!({});
        }
        if !v[self.sec.profiles][uuid].is_object() {
            v[self.sec.profiles][uuid] = json!({});
        }
        let same: Vec<String> = v[self.sec.profiles][uuid]
            .as_object()
            .unwrap()
            .iter()
            .filter(|(k, x)| k.starts_with(scope) && k.as_str() != key && E::from_json(x).is_some_and(|o| o.same_clocks(e)))
            .map(|(k, _)| k.clone())
            .collect();
        let m = v[self.sec.profiles][uuid].as_object_mut().unwrap();
        for k in same {
            m.remove(&k);
        }
        m.insert(key.to_string(), e.to_json());
        self.write_to(&v);
        drop(v);
        self.use_key(uuid, key);
    }

    /// Forgets the profiles whose key is in `keys`; the profile in use is dropped as well.  Returns whether the file changed.
    pub fn forget(&self, uuid: &str, keys: &[String]) -> bool {
        let mut v = self.v.lock().unwrap();
        let mut any = false;
        if let Some(m) = v[self.sec.profiles][uuid].as_object_mut() {
            for k in keys {
                any |= m.remove(k).is_some();
            }
        }
        if any {
            self.write_to(&v);
        }
        drop(v);
        self.clear_use(uuid);
        any
    }

    /// `--uv-reset-all` for this tuner's sections: the profiles and the markers go.  Returns whether anything was there.
    pub fn forget_all(&self) -> bool {
        let mut v = self.v.lock().unwrap();
        let mut any = false;
        if let Some(m) = v.as_object_mut() {
            any |= m.remove(self.sec.profiles).is_some();
            any |= m.remove(self.sec.trying).is_some();
        }
        if any {
            self.write_to(&v);
        }
        any
    }

    // ----- crash marker: the offset under check, written before the check and removed after it -------------------------

    /// The highest step that passed in the tuning now running (goes into the marker of the next check).
    pub fn set_good(&self, uuid: &str, good: Option<i32>) {
        let mut g = self.good.lock().unwrap();
        match good {
            Some(x) => g.insert(uuid.into(), x),
            None => g.remove(uuid),
        };
    }

    pub fn mark_trying(&self, uuid: &str, at: i32) {
        let good = self.good.lock().unwrap().get(uuid).copied();
        let mut v = self.v.lock().unwrap();
        if !v[self.sec.trying].is_object() {
            v[self.sec.trying] = json!({});
        }
        v[self.sec.trying][uuid] = match good {
            Some(g) => json!({"offset": at, "good": g}),
            None => json!({"offset": at}),
        };
        self.write_to(&v);
    }

    pub fn clear_trying(&self, uuid: &str) {
        let mut v = self.v.lock().unwrap();
        let had = v[self.sec.trying].as_object_mut().is_some_and(|t| t.remove(uuid).is_some());
        if had {
            self.write_to(&v);
        }
    }

    pub fn has_trying(&self, uuid: &str) -> bool {
        self.v.lock().unwrap()[self.sec.trying].get(uuid).is_some()
    }

    pub fn trying_ids(&self) -> Vec<String> {
        self.v.lock().unwrap()[self.sec.trying].as_object().map(|t| t.keys().cloned().collect()).unwrap_or_default()
    }

    /// What a previous run was checking when it died, if anything (the marker is removed).
    pub fn take_trying(&self, uuid: &str) -> Option<Crash> {
        let t = self.v.lock().unwrap()[self.sec.trying].get(uuid).cloned()?;
        self.clear_trying(uuid);
        parse_marker(&t)
    }

    /// Profiles of the card as `climb::hint_start` wants them; `conv` maps (key, entry) to the profile (None: not a candidate).
    pub fn hints<E: Profile>(&self, uuid: &str, prefix: &str, conv: impl Fn(&str, &E) -> Option<HintProfile>) -> Vec<HintProfile> {
        self.list::<E>(uuid, prefix).iter().filter_map(|(k, e)| conv(k, e)).collect()
    }
}

/// {"offset": N, "good": M?} or a bare number (1.5.2).
pub fn parse_marker(t: &Value) -> Option<Crash> {
    let at = t.as_i64().or_else(|| t["offset"].as_i64())? as i32;
    Some(Crash { at, good: t["good"].as_i64().map(|g| g as i32) })
}

/// 1.7.5 (PC_BOX): the offset on the card is one of this card's saved tuned profiles and that profile was made from another
/// offset: a leftover of our own (a run killed by a power cut whose restore marker already held the tuned value, or the miner
/// before FFG), not the owner's offset.  Taking it for the owner's put +270 back at every start and, unlocked, crashed the GPU.
/// 1.7.6: all three tuners write `orig_offset` in their profiles, so every section counts (a Pearl offset left on the card
/// before a NOID run is as foreign to NOID as to Pearl).  Returns the offset recorded before the tuning.
pub fn leftover_orig(db: &Value, uuid: &str, cur: i32) -> Option<i32> {
    let mut es: Vec<(i32, i32)> = Vec::new();
    for sec in [PEARL.profiles, NOID.profiles, CMFD.profiles] {
        for e in db[sec][uuid].as_object().into_iter().flat_map(|m| m.values()) {
            if let (Some(o), Some(orig)) = (e["offset"].as_i64(), e["orig_offset"].as_i64()) {
                es.push((o as i32, orig as i32));
            }
        }
    }
    let hit = es.iter().find(|&&(o, orig)| o == cur && o != 0 && orig != cur)?;
    // only when every profile agrees on the owner's offset (no guessing between two different ones)
    es.iter().all(|&(_, orig)| orig == hit.1).then_some(hit.1)
}

/// The owner's offset to put back on exit (NOID / CMFD): on HiveOS the OC profile's Core Clock value, else the offset on the card
/// - unless that is a leftover of a tuned profile of this miner (`leftover_orig`), then the offset from before the tuning.
pub fn owner_offset(nv: &crate::nvml::Nvml, d: Dev, dir: &Path, cur: i32, lo: i32, hi: i32, who: &str) -> i32 {
    let o = crate::hiveoc::owner_offset(nv, d, cur, lo, hi);
    let hive = crate::hiveoc::load().and_then(|h| nv.index(d).map(|i| h.core_offset(i as usize))).filter(|v| (lo..=hi).contains(v));
    if hive.is_some() {
        return o;
    }
    let Ok(uuid) = nv.uuid(d) else { return o };
    match leftover_orig(&load_value(dir), &uuid, o) {
        Some(x) => {
            println!("{who}: the offset {o:+} found on the card (bus {}) is a tuned profile of this miner left behind by an earlier run; the owner's offset put back on exit is the one from before the tuning: {x:+}", nv.pci(d).map_or(0, |p| p.1));
            x
        }
        None => o,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, PartialEq)]
    struct E {
        lock: u32,
        offset: i32,
    }
    impl Profile for E {
        fn from_json(v: &Value) -> Option<E> {
            Some(E { lock: v["core_lock"].as_u64()? as u32, offset: v["offset"].as_i64()? as i32 })
        }
        fn to_json(&self) -> Value {
            json!({"core_lock": self.lock, "offset": self.offset})
        }
        fn same_clocks(&self, o: &E) -> bool {
            self.lock == o.lock
        }
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ffg-profiles-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }
    fn quiet(_: &str) {}

    #[test]
    fn save_replace_forget_and_marker() {
        let d = tmp("basic");
        let s = Store::load(&d, NOID, quiet, |_| 0);
        s.save("U1", "eff@", "eff@1400", &E { lock: 1400, offset: 150 });
        s.save("U1", "eff@", "eff@1500", &E { lock: 1500, offset: 120 });
        s.save("U1", "speed@", "speed@1400", &E { lock: 1400, offset: 300 });
        // the same clocks in the same scope replace the old result (another key, e.g. lock became known)
        s.save("U1", "eff@", "eff@~1400", &E { lock: 1400, offset: 165 });
        let l = s.list::<E>("U1", "eff@");
        assert_eq!(l.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(), ["eff@1500", "eff@~1400"]);
        assert_eq!(s.list::<E>("U1", "").len(), 3);
        assert_eq!(s.current::<E>("U1").unwrap().offset, 165);
        // marker: no good -> {"offset"} only (what 1.7.5 wrote and reads), with good -> both
        s.mark_trying("U1", 255);
        assert_eq!(s.snapshot()["noid_trying"]["U1"], json!({"offset": 255}));
        s.set_good("U1", Some(240));
        s.mark_trying("U1", 270);
        assert_eq!(s.snapshot()["noid_trying"]["U1"], json!({"offset": 270, "good": 240}));
        assert!(s.has_trying("U1") && s.trying_ids() == ["U1"]);
        // it is on disk
        let s2 = Store::load(&d, NOID, quiet, |_| 0);
        assert_eq!(s2.take_trying("U1"), Some(Crash { at: 270, good: Some(240) }));
        assert_eq!(s2.take_trying("U1"), None);
        assert!(!Store::load(&d, NOID, quiet, |_| 0).has_trying("U1"));
        assert!(s2.forget("U1", &["eff@1500".into()]));
        assert!(!s2.forget("U1", &["eff@1500".into()]));
        assert_eq!(Store::load(&d, NOID, quiet, |_| 0).list::<E>("U1", "").len(), 2);
        assert!(s2.forget_all());
        assert!(Store::load(&d, NOID, quiet, |_| 0).list::<E>("U1", "").is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn old_markers() {
        assert_eq!(parse_marker(&json!(195)), Some(Crash { at: 195, good: None })); // 1.5.2: a bare number
        assert_eq!(parse_marker(&json!({"offset": 195})), Some(Crash { at: 195, good: None }));
        assert_eq!(parse_marker(&json!({"offset": 195, "good": 165})), Some(Crash { at: 195, good: Some(165) }));
        assert_eq!(parse_marker(&json!({"good": 1})), None);
    }

    #[test]
    fn other_sections_are_kept() {
        let d = tmp("keep");
        std::fs::write(results_path(&d), r#"{"version":2,"profiles":{"U":{"1805":{"x":1}}},"trying":{"U":{"offset":5}}}"#).unwrap();
        let s = Store::load(&d, CMFD, quiet, |_| 0);
        s.save("U", "", "1805", &E { lock: 1805, offset: 255 });
        let v = s.snapshot();
        assert_eq!(v["profiles"]["U"]["1805"], json!({"x": 1}));
        assert_eq!(v["trying"]["U"], json!({"offset": 5}));
        assert_eq!(v["version"], json!(2)); // CMFD leaves the version alone
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn leftover_in_any_section() {
        let e = |offset: i32, orig: i32| json!({"offset": offset, "orig_offset": orig});
        // PC_BOX: Pearl profile made from +0 at +270; the card shows +270 at the start
        let db = json!({"profiles": {"A": {"1560/mem810": e(270, 0)}}, "noid": {"A": {"eff@1400": e(150, 0)}}, "cmfd_profiles": {"A": {"1805": e(240, 0)}}});
        assert_eq!(leftover_orig(&db, "A", 270), Some(0)); // a Pearl offset left before a NOID / CMFD run
        assert_eq!(leftover_orig(&db, "A", 150), Some(0)); // a NOID one left before a Pearl run
        assert_eq!(leftover_orig(&db, "A", 240), Some(0));
        assert_eq!(leftover_orig(&db, "A", 100), None); // the owner's own
        assert_eq!(leftover_orig(&db, "B", 270), None);
        assert_eq!(leftover_orig(&db, "A", 0), None);
        // profiles that disagree on the owner's offset: no guess
        let db2 = json!({"profiles": {"A": {"1560/mem810": e(270, 0)}}, "noid": {"A": {"eff@1400": e(150, 120)}}});
        assert_eq!(leftover_orig(&db2, "A", 270), None);
        // an offset equal to the profile's own original is the owner's
        let db3 = json!({"noid": {"A": {"eff@1400": e(150, 150)}}});
        assert_eq!(leftover_orig(&db3, "A", 150), None);
    }
}
