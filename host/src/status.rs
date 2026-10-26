// FreeForGem — see LICENSE
//! What each GPU's undervolt tuner is doing right now (1.7.2), for the periodic stats line ("GPU3 0.000 [tuning +150, step 3/10]")
//! and a summary line about once a minute ("auto-uv: tuning 8 GPU(s), ~6 min left; their hashrate shows once they finish").
//! Set by the Pearl, NOID and CMFD tuners; cleared when a GPU's tuning / check is over.
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

struct St {
    what: String,
    tuning: bool,
    eta: Option<Instant>,
}

struct All {
    gpus: BTreeMap<i32, St>,
    last_line: Option<Instant>,
    last_set: Vec<i32>,
}

fn all() -> &'static Mutex<All> {
    static S: Mutex<All> = Mutex::new(All { gpus: BTreeMap::new(), last_line: None, last_set: Vec::new() });
    &S
}

/// GPU `dev` is tuning (`tuning`) or checking, `what` for the stats line; `eta` = when it should be done (None: keep the old one).
pub fn set(dev: i32, what: String, tuning: bool, eta: Option<Instant>) {
    let mut a = all().lock().unwrap();
    let old = a.gpus.get(&dev).and_then(|s| s.eta);
    a.gpus.insert(dev, St { what, tuning, eta: eta.or(old) });
}

/// Is GPU `dev` in a tuning (not a single check)?
pub fn is_tuning(dev: i32) -> bool {
    all().lock().unwrap().gpus.get(&dev).is_some_and(|s| s.tuning)
}

pub fn clear(dev: i32) {
    all().lock().unwrap().gpus.remove(&dev);
}

/// " [tuning +150, step 3/10]" for the stats line, or "".
pub fn tag(dev: i32) -> String {
    all().lock().unwrap().gpus.get(&dev).map_or(String::new(), |s| format!(" [{}]", s.what))
}

/// Clears the state of `dev` when dropped (a tuning / check that ends on any path).
pub struct Guard(pub i32);
impl Drop for Guard {
    fn drop(&mut self) {
        clear(self.0);
    }
}

fn line(a: &All, now: Instant) -> Option<String> {
    if a.gpus.is_empty() {
        return None;
    }
    let tuning: Vec<i32> = a.gpus.iter().filter(|(_, s)| s.tuning).map(|(d, _)| *d).collect();
    let checking: Vec<i32> = a.gpus.iter().filter(|(_, s)| !s.tuning).map(|(d, _)| *d).collect();
    let names = |v: &[i32]| v.iter().map(|d| format!("GPU{d}")).collect::<Vec<_>>().join(", ");
    let left = a.gpus.values().filter_map(|s| s.eta).max().map(|e| e.saturating_duration_since(now));
    let mut parts = Vec::new();
    if !tuning.is_empty() {
        parts.push(format!("tuning {} GPU(s) ({})", tuning.len(), names(&tuning)));
    }
    if !checking.is_empty() {
        parts.push(format!("checking {} GPU(s) ({})", checking.len(), names(&checking)));
    }
    let eta = match left {
        Some(d) if d >= Duration::from_secs(90) => format!(", ~{} min left", (d.as_secs() + 59) / 60),
        Some(_) => ", about a minute left".to_string(),
        None => String::new(),
    };
    Some(format!("auto-uv: {}{eta}; their hashrate shows once they finish (the other GPUs mine){}", parts.join(", "), load_note()))
}

/// Linux: "; load average 3.1 / 2.8 (1 / 5 min, 8 CPU threads)", with a WARNING when the 5-min value is above 2x the CPU
/// threads (HiveOS reboots a rig at a high load average even with its watchdog off: /hive/bin/wd check_la).
fn load_note() -> String {
    let Some((l1, l5)) = loadavg() else { return String::new() };
    let n = std::thread::available_parallelism().map_or(1, |x| x.get());
    let warn = if l5 > 2.0 * n as f64 { " -- WARNING: high load average, HiveOS reboots a rig at a high one (even with its watchdog off)" } else { "" };
    format!("; load average {l1:.1} / {l5:.1} (1 / 5 min, {n} CPU threads){warn}")
}

pub fn loadavg() -> Option<(f64, f64)> {
    let s = std::fs::read_to_string("/proc/loadavg").ok()?;
    let mut it = s.split_whitespace().map(|x| x.parse::<f64>().ok());
    Some((it.next()??, it.next()??))
}

/// The summary line when it is due: at once when the set of busy GPUs changes, else about once a minute while any is busy.
pub fn summary_due() -> Option<String> {
    let now = Instant::now();
    let mut a = all().lock().unwrap();
    let set: Vec<i32> = a.gpus.keys().copied().collect();
    let changed = set != a.last_set;
    let due = a.last_line.is_none_or(|t| now.duration_since(t) >= Duration::from_secs(60));
    if set.is_empty() {
        a.last_set = set;
        return None;
    }
    if !(changed || due) {
        return None;
    }
    a.last_set = set;
    a.last_line = Some(now);
    line(&a, now)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_and_tags() {
        let now = Instant::now();
        let mut a = All { gpus: BTreeMap::new(), last_line: None, last_set: Vec::new() };
        assert_eq!(line(&a, now), None);
        for d in [3, 4] {
            a.gpus.insert(d, St { what: format!("tuning +150, step 3/10"), tuning: true, eta: Some(now + Duration::from_secs(330)) });
        }
        a.gpus.insert(7, St { what: "check +300".into(), tuning: false, eta: Some(now + Duration::from_secs(40)) });
        assert!(line(&a, now).unwrap().starts_with(
            "auto-uv: tuning 2 GPU(s) (GPU3, GPU4), checking 1 GPU(s) (GPU7), ~6 min left; their hashrate shows once they finish (the other GPUs mine)"
        ));
        set(91, "tuning +180, step 2/9".into(), true, Some(Instant::now() + Duration::from_secs(60)));
        assert_eq!(tag(91), " [tuning +180, step 2/9]");
        {
            let _g = Guard(91);
        }
        assert_eq!(tag(91), "");
    }
}
