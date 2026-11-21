// FreeForGem — see LICENSE
//! Colours in the console / miner log (1.7.2), by the kind of line: tuning and checks in progress cyan, passed / results green,
//! step-downs / pauses / warnings yellow, failures / Xid / faults / stopped workers red, accepted shares green, rejected red,
//! pool pauses / reconnects magenta.  FFGN 1.0.10 (the owner, 21.11.2026, all our miners): the lines of the automatic tuning
//! (`auto-uv...`, `auto-lock...`, also as `WARNING auto-uv: ...`) are YELLOW, and their failures / roll-backs (a FAILED step, an
//! Xid, a crash, a resume below a failed offset, a step back, a tuner switched off or paused) ORANGE (256-colour 208; bold yellow
//! on a terminal without 256 colours), so they stand apart from the red of real errors.  The periodic stats line (`[ 120s] GPU0 ... | total ...`) is never coloured: HiveOS
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
    Tune,     // yellow: auto-uv / auto-lock
    TuneFail, // orange: an auto-uv / auto-lock failure or roll-back
}

/// 1.0.10: a line of the automatic tuning (auto-uv / auto-lock, of any coin: `auto-uv (noid):` too)?
fn tuning_line(s: &str) -> bool {
    let t = s.strip_prefix("WARNING ").unwrap_or(s);
    t.starts_with("auto-uv") || t.starts_with("auto-lock")
}

/// 1.0.10: a failure / roll-back among the tuning lines (orange)
fn tuning_fail(s: &str) -> bool {
    // "no Xid" / "bad 0" in a passed step are not failures; nor the fallback note of the start line ("... its mining paused")
    let s = &s.replace("no Xid", "").replace("bad 0", "").replace("its mining paused", "");
    let has = |ws: &[&str]| ws.iter().any(|w| s.contains(w));
    s.starts_with("WARNING ")
        || has(&[
            "FAILED", "Xid", "crash", "CRASH", "failed", "failure", "steps below", "step back", "stepping back", "stepped back", "OFF", "PAUSED",
            "paused", "does not hold", "not held", "gives up", "give up", "cannot ", "not valid", "hung", "no verdict",
        ])
}

/// The kind of a log line, from its words (the first match in this order: bad, warn, good, pool, progress).
pub fn kind(s: &str) -> Kind {
    // the periodic stats line: never coloured (h-stats.sh parses it)
    if s.starts_with('[') && s.contains(" total ") {
        return Kind::Plain;
    }
    // 1.0.10: the automatic tuning has its own colours (yellow / orange)
    if tuning_line(s) {
        return if tuning_fail(s) { Kind::TuneFail } else { Kind::Tune };
    }
    let has = |ws: &[&str]| ws.iter().any(|w| s.contains(w));
    // a tuning result is green (red when it failed), whatever words it carries ("one step below the last good one" made the
    // NOID RESULT yellow on Rig01 beta6)
    if s.contains("RESULT") && !s.contains("UVRESULT") {
        return if s.contains("FAILED") { Kind::Bad } else { Kind::Good };
    }
    if has(&["FAILED", "Xid", "worker stopped", "CRASHED", "crashed", "Error:", "error:", "LEFT OUT", "share REJECTED", "rejected (", "COULD NOT", "panicked", "!! ", "cannot "]) {
        Kind::Bad
    } else if has(&["WARNING", "paused", "one step", "step down", "stepping back", "stepped", "lowered", "stands down", "below your offset", "no gain", "restarting itself", "MHz below", "FAILS", "not saved"]) {
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
        Kind::Tune => "33",
        Kind::TuneFail => orange(),
    };
    format!("\x1b[{code}m{s}\x1b[0m")
}

/// 1.0.10: orange (256 colours: 208) where the terminal has 256 colours, else bold yellow.  FFG_COLOR_256=0 / 1 decides; with
/// FFG_COLOR=1 (HiveOS h-run.sh: the log is read later in an ssh / web terminal) 256; TERM linux / dumb / vt100 / vt220 / ansi
/// (the Linux text console and the like): bold yellow; else 256 (Windows VT consoles and today's terminals have them).
fn orange() -> &'static str {
    if supports_256(std::env::var("FFG_COLOR_256").ok().as_deref(), std::env::var("FFG_COLOR").ok().as_deref(), std::env::var("TERM").ok().as_deref(), std::env::var_os("COLORTERM").is_some()) {
        "38;5;208"
    } else {
        "1;33"
    }
}

fn supports_256(force: Option<&str>, ffg_color: Option<&str>, term: Option<&str>, colorterm: bool) -> bool {
    match force {
        Some("0") => return false,
        Some("1") => return true,
        _ => {}
    }
    if colorterm || matches!(ffg_color, Some("1") | Some("always")) {
        return true;
    }
    match term {
        Some(t) if t.contains("256") || t.contains("direct") => true,
        Some("linux" | "dumb" | "vt100" | "vt220" | "ansi" | "cons25") => false,
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds() {
        assert_eq!(kind("[  60s] GPU0 80.9 (149 W) | total 647.0 TH/s | user sub 3 acc 3 rej 0"), Kind::Plain);
        assert_eq!(kind("auto-uv: GPU3 (bus 04, RTX 3070) step +255: FAILED (rerun_diff ...)"), Kind::TuneFail);
        assert_eq!(kind("faults: GPU4 (bus 05, RTX 3070) FAILED: Xid 43 reported by the driver"), Kind::Bad);
        assert_eq!(kind("!! GPU 3 worker stopped: pg_wait: ... illegal memory access"), Kind::Bad);
        assert_eq!(kind("auto-uv: GPU6 (bus 08, RTX 3070) auto-uv is paused for this GPU at these clocks after a failure (24 h left); keeping +275"), Kind::TuneFail);
        assert_eq!(kind("auto-uv: GPU0 (bus 0d, RTX 3070) step +180: ok (544 reruns, cpu ok 6), 76.1 W, 1410 MHz, 52 C"), Kind::Tune);
        assert_eq!(kind("auto-uv: GPU0 (bus 0d, RTX 3070) tuning: 20 s warm-up at +150, then +30 MHz steps up to +210"), Kind::Tune);
        assert_eq!(kind("[stratum:user] share ACCEPTED (id 113, job 5047b7f7_1500000)"), Kind::Good);
        assert_eq!(kind("pool pause [user] (canonical-tip-unverified): current job dropped, waiting for the next"), Kind::Pool);
        assert_eq!(kind("devfee: next slot in 54.7 min"), Kind::Plain);
        assert_eq!(kind("auto-uv (noid): GPU0 (bus 0d, RTX 3070) RESULT speed: offset +225 (edge +285, one step below the last good one); MH/W 0.357 -> 0.432 (+21.2%)"), Kind::Tune);
        assert_eq!(kind("auto-uv: GPU0 (bus 0d, RTX 3070) RESULT: FAILED, no offset down to +0 passes the check at these clocks"), Kind::TuneFail);
    }

    #[test]
    fn tuning_lines_are_yellow_and_their_failures_orange() {
        // yellow: the start, a step taken, the result, saved, a resume that passed
        for l in [
            "auto-uv: GPU4 (bus 05, RTX 3070, lock 1650) tuning +275..+495 WHILE MINING (+15 MHz steps, coarse +60, ...)",
            "auto-uv: GPU4 (bus 05, RTX 3070, lock 1650) step +335: ok while mining (201.3 MA/s counted, 41 results CPU-checked, bad 0, no Xid, 19 s), 131.0 W",
            "auto-uv: GPU4 (bus 05, RTX 3070, lock 1650) step +335: the power falls (132.4 W at +305 -> 131.0 W, -1.4 W): taken, saved (tuning in progress)",
            "auto-uv: GPU4 (bus 05, RTX 3070, lock 1650) RESULT: offset +275 -> +335, mining power at the base +275 134.0 W -> 131.0 W at +335",
            "auto-lock: GPU1 mining at lock 1400 (saved; --auto-lock-reset to sweep again)",
            "auto-uv: tuning 8 of 8 GPU(s) at a time (all at once: each GPU is tuned on its own mining load, no check processes)",
            // live test 21.11.2026: these two were orange ("paused" in the fallback note, "resuming at" of a saved profile)
            "auto-uv: tuning 1 of 1 GPU(s) at a time (all at once: each GPU is tuned on its own mining load, no check processes); every GPU keeps mining (a GPU without mining load within 240 s is checked in a check process, its mining paused)",
            "auto-uv: GPU1 (bus 0d, RTX 3070, lock 1680) resuming at +120 (the saved profile): a 15 s check, then this GPU mines (no new climb from the base)",
        ] {
            assert_eq!(kind(l), Kind::Tune, "{l}");
        }
        // orange: a failed step (Xid), a resume below a failed offset, a step back, auto-uv off / paused, auto-lock failures
        for l in [
            "auto-uv: GPU4 (bus 05, RTX 3070, lock 1650) step +395: FAILED (Xid 31 reported by the driver)",
            "auto-uv: GPU4 (bus 05, RTX 3070, lock 1650) resuming at +365 (2 steps below the failure at +395): a 30 s check, then this GPU mines",
            "auto-uv: GPU1 (bus 0d, RTX 3070, lock 1680) resuming at +240 (the crash while mining at +300 was 2 steps above +270; not above the last confirmed +240): a 30 s check",
            "auto-uv: GPU1 (bus 0d, RTX 3070, lock 1680) the mining worker of this GPU stopped after the fault (engine / CUDA error): no verdict on +225: the tuning stops here",
            "auto-uv: GPU4 (bus 05, RTX 3070, lock 1650) 1 candidate(s) failed the CPU check while mining: offset +335 -> +320, saved",
            "WARNING auto-uv: GPU4 (bus 05, RTX 3070): 3 failures in a row at its tuned offsets: auto-uv OFF for this GPU for 24 h (now at +0)",
            "auto-lock: GPU1 lock 1600 FAILED: Xid; next lock",
            "auto-lock:   1500    +200    60.000   100.0    0.6000  not valid: the clock did not hold the lock (power / thermal limit)",
        ] {
            assert_eq!(kind(l), Kind::TuneFail, "{l}");
        }
        // not tuning lines: unchanged
        assert_eq!(kind("!! GPU 3 worker stopped: pg_wait: ... illegal memory access"), Kind::Bad);
        assert_eq!(kind("faults: GPU4 (bus 05, RTX 3070) FAILED: Xid 43 reported by the driver"), Kind::Bad);
        // the codes: yellow 33; orange 208 with 256 colours, else bold yellow; nothing at all with --no-color
        assert!(supports_256(None, None, Some("xterm-256color"), false));
        assert!(supports_256(None, Some("1"), Some("linux"), false));
        assert!(!supports_256(None, None, Some("linux"), false));
        assert!(!supports_256(Some("0"), Some("1"), Some("xterm-256color"), true));
        assert!(supports_256(None, None, None, false));
        STATE.store(1, Ordering::SeqCst);
        let y = paint("auto-uv: GPU0 (bus 0d, RTX 3070) step +180: ok while mining".into());
        assert_eq!(y, "\x1b[33mauto-uv: GPU0 (bus 0d, RTX 3070) step +180: ok while mining\x1b[0m");
        let o = paint("auto-uv: GPU0 (bus 0d, RTX 3070) step +210: FAILED (Xid 31 reported by the driver)".into());
        assert!(o.starts_with("\x1b[38;5;208m") || o.starts_with("\x1b[1;33m"), "{o:?}");
        STATE.store(2, Ordering::SeqCst);
        assert_eq!(paint("auto-uv: GPU0 step +210: FAILED (Xid 31)".into()), "auto-uv: GPU0 step +210: FAILED (Xid 31)");
        STATE.store(0, Ordering::SeqCst);
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
