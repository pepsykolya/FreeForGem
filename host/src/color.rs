// FreeForGem — see LICENSE
//! Colours in the console / miner log (1.7.2), by the kind of line: tuning and checks in progress cyan, passed / results green,
//! step-downs / pauses / warnings yellow, failures / Xid / faults / stopped workers red, accepted shares green, rejected red,
//! pool pauses / reconnects magenta.  The periodic stats line (`[ 120s] GPU0 ... | total ...`) is never coloured: HiveOS
//! h-stats.sh parses it (and strips ANSI codes anyway).
//! On: a terminal (Windows: when VT processing can be enabled), or FFG_COLOR=1 (h-run.sh sets it: the HiveOS miner screen and
//! `miner log` show ANSI colours through tee).  Off: NO_COLOR (any value), --no-color, FFG_COLOR=0, check child processes.
use std::sync::atomic::{AtomicU8, Ordering};

// 0 = not decided yet, 1 = on, 2 = off
static STATE: AtomicU8 = AtomicU8::new(0);

pub fn disable() {
    STATE.store(2, Ordering::SeqCst);
}

fn decide() -> bool {
    if std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    match std::env::var("FFG_COLOR").ok().as_deref() {
        Some("0") => return false,
        Some("1") | Some("always") => {
            #[cfg(windows)]
            let _ = enable_vt();
            return true;
        }
        _ => {}
    }
    use std::io::IsTerminal;
    if !std::io::stdout().is_terminal() {
        return false;
    }
    #[cfg(windows)]
    {
        return enable_vt();
    }
    #[allow(unreachable_code)]
    true
}

/// Windows: ENABLE_VIRTUAL_TERMINAL_PROCESSING on the console of stdout; false when that is not possible.
#[cfg(windows)]
fn enable_vt() -> bool {
    use std::ffi::c_void;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetStdHandle(n: u32) -> *mut c_void;
        fn GetConsoleMode(h: *mut c_void, m: *mut u32) -> i32;
        fn SetConsoleMode(h: *mut c_void, m: u32) -> i32;
    }
    unsafe {
        let h = GetStdHandle(-11i32 as u32); // STD_OUTPUT_HANDLE
        let mut m = 0u32;
        if h.is_null() || GetConsoleMode(h, &mut m) == 0 {
            return false;
        }
        SetConsoleMode(h, m | 0x0004) != 0
    }
}

pub fn enabled() -> bool {
    match STATE.load(Ordering::SeqCst) {
        1 => true,
        2 => false,
        _ => {
            let on = decide();
            // a concurrent disable() wins
            let _ = STATE.compare_exchange(0, if on { 1 } else { 2 }, Ordering::SeqCst, Ordering::SeqCst);
            STATE.load(Ordering::SeqCst) == 1
        }
    }
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Kind {
    Plain,
    Progress, // cyan
    Good,     // green
    Warn,     // yellow
    Bad,      // red
    Pool,     // magenta
}

/// The kind of a log line, from its words (the first match in this order: bad, warn, good, pool, progress).
pub fn kind(s: &str) -> Kind {
    // the periodic stats line: never coloured (h-stats.sh parses it)
    if s.starts_with('[') && s.contains(" total ") {
        return Kind::Plain;
    }
    let has = |ws: &[&str]| ws.iter().any(|w| s.contains(w));
    // a tuning result is green (red when it failed), whatever words it carries ("one step below the last good one" made the
    // NOID RESULT yellow on Rig01 beta6)
    if s.contains("RESULT") && !s.contains("UVRESULT") {
        return if s.contains("FAILED") { Kind::Bad } else { Kind::Good };
    }
    if has(&["FAILED", "Xid", "worker stopped", "CRASHED", "crashed", "Error:", "error:", "LEFT OUT", "share REJECTED", "rejected (", "COULD NOT", "panicked", "!! ", "cannot "]) {
        Kind::Bad
    } else if has(&["WARNING", "paused", "one step", "step down", "stepping back", "stepped", "lowered", "stands down", "below your offset", "waits for RAM", "no gain", "restarting itself", "MHz below", "FAILS", "not saved"]) {
        Kind::Warn
    } else if has(&[": ok", "passed", "PASSED", "saved as profile", "RESULT", "applied", "ACCEPTED", "restored", "put back", "self-test PASSED"]) {
        Kind::Good
    } else if has(&["pool pause", "reconnect", "connection closed", "connecting to", "disconnected", "DISCONNECTED"]) {
        Kind::Pool
    } else if has(&["tuning", "check", "baseline", "step +", "core clock under mining load", "profiles:", "confirmation"]) {
        Kind::Progress
    } else {
        Kind::Plain
    }
}

/// The line with its colour (unchanged when colours are off or the line is plain).
pub fn paint(s: String) -> String {
    if !enabled() {
        return s;
    }
    let code = match kind(&s) {
        Kind::Plain => return s,
        Kind::Progress => "36",
        Kind::Good => "32",
        Kind::Warn => "33",
        Kind::Bad => "31",
        Kind::Pool => "35",
    };
    format!("\x1b[{code}m{s}\x1b[0m")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds() {
        assert_eq!(kind("[  60s] GPU0 80.9 (149 W) | total 647.0 TH/s | user sub 3 acc 3 rej 0"), Kind::Plain);
        assert_eq!(kind("auto-uv: GPU3 (bus 04, RTX 3070) step +255: FAILED (rerun_diff ...)"), Kind::Bad);
        assert_eq!(kind("faults: GPU4 (bus 05, RTX 3070) FAILED: Xid 43 reported by the driver"), Kind::Bad);
        assert_eq!(kind("!! GPU 3 worker stopped: pg_wait: ... illegal memory access"), Kind::Bad);
        assert_eq!(kind("auto-uv: GPU6 (bus 08, RTX 3070) auto-uv is paused for this GPU at these clocks after a failure (24 h left); keeping +275"), Kind::Warn);
        assert_eq!(kind("auto-uv: GPU0 (bus 0d, RTX 3070) step +180: ok (544 reruns, cpu ok 6), 76.1 W, 1410 MHz, 52 C"), Kind::Good);
        assert_eq!(kind("auto-uv: GPU0 (bus 0d, RTX 3070) tuning: 20 s warm-up at +150, then +30 MHz steps up to +210"), Kind::Progress);
        assert_eq!(kind("[stratum:user] share ACCEPTED (id 113, job 5047b7f7_1500000)"), Kind::Good);
        assert_eq!(kind("pool pause [user] (canonical-tip-unverified): current job dropped, waiting for the next"), Kind::Pool);
        assert_eq!(kind("devfee: next slot in 54.7 min"), Kind::Plain);
        assert_eq!(kind("auto-uv (noid): GPU0 (bus 0d, RTX 3070) RESULT speed: offset +225 (edge +285, one step below the last good one); MH/W 0.357 -> 0.432 (+21.2%)"), Kind::Good);
        assert_eq!(kind("auto-uv: GPU0 (bus 0d, RTX 3070) RESULT: FAILED, no offset down to +0 passes the check at these clocks"), Kind::Bad);
    }

    #[test]
    fn stats_line_stays_parseable() {
        // whatever the setting, the stats line comes back byte for byte (h-stats.sh: ' total N TH/s', 'GPUn N')
        STATE.store(1, Ordering::SeqCst);
        let l = "[ 230s] GPU0 81.085 (149 W, 0.543 TH/W)  GPU1 80.967 | total 647.0 TH/s (1637 att/s) | 1148 W, 0.563 TH/W | user sub 9 acc 9 rej 0".to_string();
        assert_eq!(paint(l.clone()), l);
        assert_eq!(paint("x step +180: ok".into()), "\x1b[32mx step +180: ok\x1b[0m");
        STATE.store(2, Ordering::SeqCst);
        assert_eq!(paint("x step +180: ok".into()), "x step +180: ok");
        STATE.store(0, Ordering::SeqCst);
    }
}
