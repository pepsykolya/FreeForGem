//! 1.0.12 (FIELD111.md anomaly 3, the 12x 2060S rig 21.11.2026): a rig whose GPUs mine far below their own rate without saying so.
//! After an Xid 79 (a GPU fell off the PCIe bus) the driver flags a reboot (Xid 154) and the lost GPU does not come back without
//! one; the other 11 GPUs mined on the full (plain) path at ~9.85 MA/s instead of the digest table at ~32 MA/s until the reboot.
//! Two loud signals, on the log, the stats line (HiveOS h-stats) and the API warnings:
//! - `REBOOT NEEDED`: an Xid 79 was seen in this run;
//! - `NO TABLE`: a GPU that mined on the digest table earlier in this run has had no table for `NO_TABLE_SECS` (the engine retries
//!   its table at least every third model since nebgpu 0.7.1, so a long time without it means a degraded driver or card).

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Instant;

/// a GPU without its digest table for this long (after it had one in this run) is reported
pub const NO_TABLE_SECS: u64 = 600;

#[derive(Default)]
struct Gpu {
    had_table: bool,
    since: Option<Instant>, // the first scan of the current stretch without the table
    warned: bool,
}

#[derive(Default)]
struct State {
    reboot: Option<String>,
    gpus: BTreeMap<i32, Gpu>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let mut g = STATE.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(State::default))
}

/// an Xid 79 of `label` (a GPU fell off the bus): the rig needs a reboot
pub fn xid79(label: &str) {
    with(|s| {
        if s.reboot.is_none() {
            s.reboot = Some(format!("Xid 79 {}", label.split(' ').next().unwrap_or(label)));
        }
    });
}

/// GPU `dev` scans with the digest table now (`table`) or not; returns the warning line to log once when it crosses NO_TABLE_SECS
pub fn note_path(dev: i32, table: bool) -> Option<String> {
    with(|s| step(s.gpus.entry(dev).or_default(), table, Instant::now()).then(|| {
        format!(
            "GPU{dev}: WARNING no digest table for {} min although this GPU mined on it earlier in this run: it mines on a much slower path; a degraded driver (after an Xid 79 a REBOOT is needed) or card - see the stats line tag [NO TABLE]",
            NO_TABLE_SECS / 60
        )
    }))
}

/// one observation; true once when the stretch without the table reaches NO_TABLE_SECS
fn step(g: &mut Gpu, table: bool, now: Instant) -> bool {
    if table {
        g.had_table = true;
        g.since = None;
        g.warned = false;
        return false;
    }
    if !g.had_table {
        return false;
    }
    let t0 = *g.since.get_or_insert(now);
    if !g.warned && now.duration_since(t0).as_secs() >= NO_TABLE_SECS {
        g.warned = true;
        return true;
    }
    false
}

/// the stats-line tags: "REBOOT NEEDED: Xid 79 GPU1" and "NO TABLE: GPU2,GPU5"
pub fn tags() -> Vec<String> {
    with(|s| {
        let mut v = Vec::new();
        if let Some(r) = &s.reboot {
            v.push(format!("REBOOT NEEDED: {r}"));
        }
        let slow: Vec<String> = s.gpus.iter().filter(|(_, g)| g.warned).map(|(d, _)| format!("GPU{d}")).collect();
        if !slow.is_empty() {
            v.push(format!("NO TABLE: {} (no digest table for {}+ min: driver degraded?)", slow.join(","), NO_TABLE_SECS / 60));
        }
        v
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_gpu_without_its_table_for_ten_minutes_is_reported_once() {
        let t = Instant::now();
        let mut g = Gpu::default();
        // never had the table (a card the table does not suit): never reported
        assert!(!step(&mut g, false, t));
        assert!(!step(&mut g, false, t + Duration::from_secs(3600)));
        // the table, then a model on cheap / plain for a moment (the measuring phase): not reported
        assert!(!step(&mut g, true, t));
        assert!(!step(&mut g, false, t + Duration::from_secs(1)));
        assert!(!step(&mut g, true, t + Duration::from_secs(20)));
        // no table from 100 s on: reported once at 700 s
        assert!(!step(&mut g, false, t + Duration::from_secs(100)));
        assert!(!step(&mut g, false, t + Duration::from_secs(699)));
        assert!(step(&mut g, false, t + Duration::from_secs(700)));
        assert!(!step(&mut g, false, t + Duration::from_secs(800)));
        assert!(g.warned);
        // the table is back: cleared
        assert!(!step(&mut g, true, t + Duration::from_secs(900)));
        assert!(!g.warned);
    }

    #[test]
    fn xid79_tags_the_rig_for_a_reboot() {
        xid79("GPU1 (bus 02, RTX 2060 SUPER)");
        xid79("GPU7 (bus 0b)");
        assert!(tags().iter().any(|t| t == "REBOOT NEEDED: Xid 79 GPU1"), "{:?}", tags());
    }
}
