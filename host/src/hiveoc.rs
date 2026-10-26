// FreeForGem — see LICENSE
//! Read-only view of the HiveOS NVIDIA overclock profile (`/hive-config/nvidia-oc.conf`, or `$NVIDIA_OC_CONF`), the file that
//! HiveOS' `nvidia-oc` applies at every miner start and whenever the OC is changed in the web UI.  Used for two things:
//!  * `--lock-cclock` / `--lock-mclock`: the lock HiveOS set is what the miner puts back on exit (NVML cannot read a lock back);
//!  * `--auto-uv`: an offset that suddenly equals the profile's Core Clock value (0 when the field is empty) is HiveOS
//!    re-applying the OC, not the owner choosing a new offset by hand.
//! The rules mirror `nvidia-oc` (hive 0.6-231): lists are space separated, one value per NVIDIA GPU in NVML index order, a
//! shorter list repeats its last value; `CLOCK` >= 500 is a fixed core clock (+ the pill-args number), not an offset;
//! `MEM` = -3000 (LOWMEM) locks the memory to 810; `LMEM`, else `DEF_MEMCLOCK`, else no memory lock; `PLIMIT` 1 = GPU disabled
//! (memory 810, no core lock).  Nothing here changes the GPU.
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const MIN_FIXCLOCK: i64 = 500;
const LOWMEM: i64 = -3000;

pub struct HiveOc {
    vars: HashMap<String, String>,
}

/// Path of the HiveOS NVIDIA OC profile, if this looks like HiveOS (or $NVIDIA_OC_CONF points at a file).
fn conf_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("NVIDIA_OC_CONF").map(PathBuf::from) {
        return p.is_file().then_some(p);
    }
    let p = PathBuf::from("/hive-config/nvidia-oc.conf");
    (cfg!(target_os = "linux") && p.is_file()).then_some(p)
}

/// The profile as it is on disk now; `None` when this is not HiveOS (or the file is unreadable).
pub fn load() -> Option<HiveOc> {
    let path = conf_path()?;
    let txt = std::fs::read_to_string(&path).ok()?;
    Some(HiveOc { vars: parse(&txt) })
}

/// `KEY=value`, `KEY="v1 v2"`, `KEY='v'` lines of a sourced bash file (comments and anything else ignored).
fn parse(txt: &str) -> HashMap<String, String> {
    let mut m = HashMap::new();
    for l in txt.lines() {
        let l = l.trim();
        let l = l.strip_prefix("export ").unwrap_or(l);
        if l.starts_with('#') {
            continue;
        }
        let Some((k, v)) = l.split_once('=') else { continue };
        if k.is_empty() || !k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        let v = v.trim();
        let v = if v.len() >= 2 && ((v.starts_with('"') && v.ends_with('"')) || (v.starts_with('\'') && v.ends_with('\''))) { &v[1..v.len() - 1] } else { v };
        m.insert(k.to_string(), v.trim().to_string());
    }
    m
}

impl HiveOc {
    #[cfg(test)]
    fn from_str(txt: &str) -> HiveOc {
        HiveOc { vars: parse(txt) }
    }

    fn scalar(&self, key: &str) -> Option<&str> {
        self.vars.get(key).map(String::as_str).filter(|s| !s.is_empty())
    }

    /// Value of list `key` for NVIDIA GPU `i` (padded with the last element, like nvidia-oc); `None` = list empty / not a number.
    fn item(&self, key: &str, i: usize) -> Option<i64> {
        let v: Vec<&str> = self.scalar(key)?.split_whitespace().collect();
        let s = v.get(i).or(v.last())?;
        s.parse::<i64>().ok()
    }

    fn disabled(&self, i: usize) -> bool {
        self.item("PLIMIT", i) == Some(1)
    }

    /// The number nvidia-oc adds to a fixed CLOCK (pill command line used as a clock when the pill itself is off).
    fn def_fixclock(&self) -> i64 {
        if self.scalar("OHGODAPILL_ENABLED").is_none() && self.scalar("OHGODAPILL_START_TIMEOUT").is_none() {
            if let Some(a) = self.scalar("OHGODAPILL_ARGS") {
                if let Ok(n) = a.parse::<i64>() {
                    return n;
                }
            }
        }
        0
    }

    /// Core clock lock nvidia-oc sets on NVIDIA GPU `i` (NVML index): `Some(mhz)`, or `None` = no lock (it resets it).
    pub fn core_lock(&self, i: usize) -> Option<u32> {
        if self.disabled(i) {
            return None;
        }
        let clock = self.item("CLOCK", i).unwrap_or(0);
        let mhz = if clock >= MIN_FIXCLOCK { clock + self.def_fixclock() } else { self.item("LCLOCK", i).unwrap_or(0) };
        u32::try_from(mhz).ok().filter(|&m| m > 0)
    }

    /// Memory clock lock nvidia-oc sets on NVIDIA GPU `i`: `Some(mhz)`, or `None` = no lock.
    pub fn mem_lock(&self, i: usize) -> Option<u32> {
        if self.disabled(i) || self.item("MEM", i) == Some(LOWMEM) {
            return Some(810);
        }
        let v = if self.scalar("LMEM").is_some() { self.item("LMEM", i) } else { self.scalar("DEF_MEMCLOCK").and_then(|s| s.parse().ok()) };
        v.and_then(|m| u32::try_from(m).ok()).filter(|&m| m > 0)
    }

    /// Core clock offset nvidia-oc sets on NVIDIA GPU `i` (0 when the Core Clock field is empty).
    pub fn core_offset(&self, i: usize) -> i32 {
        let clock = self.item("CLOCK", i).unwrap_or(0);
        let v = if clock >= MIN_FIXCLOCK { self.def_fixclock() } else { clock };
        v.clamp(i32::MIN as i64, i32::MAX as i64) as i32
    }
}

/// OC applies per hour that get a verified offset back (NOID / CMFD tuners; the Pearl one has the same rule in uv.rs); more:
/// the tuner stands down on that GPU.
pub const OC_RESETS_MAX: usize = 4;

/// Is `v` (read back instead of the offset this run set on NVML device `d`) the work of an OC apply rather than the owner's own
/// choice?  HiveOS nvidia-oc writes the profile's Core Clock value at every miner start and OC change (0 when the field is
/// empty); other OC tools put the owner's offset (`orig`, from before the miner started) back.  Returns how it was recognised.
pub fn reset_kind(nv: &crate::nvml::Nvml, d: crate::nvml::Dev, v: i32, orig: i32) -> Option<&'static str> {
    let hive = load().and_then(|h| nv.index(d).map(|i| h.core_offset(i as usize)));
    if hive == Some(v) {
        Some("the value of the HiveOS OC profile")
    } else if v == 0 {
        Some("offset 0: HiveOS nvidia-oc with an empty Core Clock field, or an OC reset")
    } else if v == orig {
        Some("your offset from before the miner started")
    } else {
        None
    }
}

/// Rate limit of the re-applies after OC applies: an OC tool that keeps resetting the offset is not fought for ever.
#[derive(Default)]
pub struct ResetLimit(Mutex<Vec<Instant>>);

impl ResetLimit {
    /// true (and counted) while fewer than OC_RESETS_MAX re-applies happened within the last hour.
    pub fn allow(&self) -> bool {
        let mut r = self.0.lock().unwrap();
        r.retain(|t| t.elapsed() < Duration::from_secs(3600));
        if r.len() >= OC_RESETS_MAX {
            return false;
        }
        r.push(Instant::now());
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hive_profile_rules() {
        // Rig02-like: core lock 1680, mem lock 810, empty Core Clock offset
        let h = HiveOc::from_str("# OC\nCLOCK=\"\"\nLCLOCK=\"1680\"\nMEM=\nLMEM=\"810\"\nPLIMIT=\nFAN=\"\"\n");
        for i in 0..4 {
            assert_eq!(h.core_lock(i), Some(1680));
            assert_eq!(h.mem_lock(i), Some(810));
            assert_eq!(h.core_offset(i), 0);
        }
        // per card lists, padding with the last value, LOWMEM, fixed clock in CLOCK, disabled card
        let h = HiveOc::from_str("CLOCK=\"150 225 1500 0\"\nLCLOCK=\"1470 1410 0\"\nMEM=\"0 -3000\"\nLMEM=\"5001 0\"\nPLIMIT=\"0 0 0 1\"\n");
        assert_eq!((h.core_lock(0), h.core_offset(0), h.mem_lock(0)), (Some(1470), 150, Some(5001)));
        assert_eq!((h.core_lock(1), h.core_offset(1), h.mem_lock(1)), (Some(1410), 225, Some(810)));
        assert_eq!((h.core_lock(2), h.core_offset(2), h.mem_lock(2)), (Some(1500), 0, Some(810))); // MEM padded: -3000
        assert_eq!((h.core_lock(3), h.mem_lock(3)), (None, Some(810)));
        // nothing set: no locks, offset 0
        let h = HiveOc::from_str("CLOCK=\nLCLOCK=\nMEM=\nPLIMIT=\n");
        assert_eq!((h.core_lock(0), h.mem_lock(0), h.core_offset(0)), (None, None, 0));
        // DEF_MEMCLOCK without LMEM; pill args as fixed-clock addend
        let h = HiveOc::from_str("CLOCK=\"1400\"\nOHGODAPILL_ARGS=\"100\"\nDEF_MEMCLOCK=\"5001\"\n");
        assert_eq!((h.core_lock(0), h.core_offset(0), h.mem_lock(0)), (Some(1500), 100, Some(5001)));
    }

    #[test]
    fn reset_rate_limit() {
        let l = ResetLimit::default();
        for _ in 0..OC_RESETS_MAX {
            assert!(l.allow());
        }
        assert!(!l.allow());
    }
}
