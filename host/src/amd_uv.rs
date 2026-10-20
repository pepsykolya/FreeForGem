// FreeForGemPear — see LICENSE
//! `--auto-uv` with the AMD backend: delegates to the separate tuner `amduv` (ADLX on Windows; amdgpu sysfs on Linux later),
//! shipped next to the binary.  The miner only starts it and waits:
//!  * `amduv apply --guard-pid <miner pid>` applies the saved result for this card / driver / search library (a quick self-check
//!    unless it was verified within the re-validation interval) and then stays alive: when the miner exits — cleanly, by a crash
//!    or by a kill — it puts the owner's clocks / voltage / power limit back (restore file + crash recovery inside amduv);
//!  * without a saved result: `amduv tune` first (frequency ladder + voltage steps, every point self-checked bit-exact in a child
//!    process; ~15–30 min once per card / driver / library), then `apply`.
//! Nothing here touches the GPU itself.
use anyhow::{Result, bail};
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

fn lg(msg: impl AsRef<str>) {
    println!("auto-uv (amd): {}", msg.as_ref());
}

fn tool() -> Option<PathBuf> {
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let p = dir.join(if cfg!(windows) { "amduv.exe" } else { "amduv" });
    p.exists().then_some(p)
}

fn lib_path() -> Option<PathBuf> {
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let p = dir.join(if cfg!(windows) { "pearlgpu_amd.dll" } else { "libpearlgpu_amd.so" });
    p.exists().then_some(p)
}

/// Runs `amduv <args>` to the end, forwarding its log; returns its exit code.
fn run(tool: &PathBuf, args: &[String]) -> Result<i32> {
    let st = Command::new(tool).args(args).stdin(Stdio::null()).status()?;
    Ok(st.code().unwrap_or(-1))
}

/// Applies (tuning first if needed) and returns the guard process, which must outlive mining; `None` = card left as it is.
pub fn start(device: i32, tune_if_missing: bool, goal: &str) -> Option<Child> {
    let Some(t) = tool() else {
        lg("amduv not found next to the binary; mining at the current clocks and voltage");
        return None;
    };
    let mut base = vec!["--dev".to_string(), device.to_string()];
    if let Some(l) = lib_path() {
        base.push("--lib".into());
        base.push(l.display().to_string());
    }
    for attempt in 0..2 {
        let mut args = vec!["apply".to_string(), "--guard-pid".to_string(), std::process::id().to_string()];
        args.extend(base.iter().cloned());
        let mut child = match Command::new(&t).args(&args).stdin(Stdio::null()).stdout(Stdio::piped()).spawn() {
            Ok(c) => c,
            Err(e) => {
                lg(format!("cannot start amduv: {e}"));
                return None;
            }
        };
        let out = child.stdout.take().unwrap();
        let mut rd = BufReader::new(out);
        let mut guarding = false;
        let mut no_result = false;
        let mut line = String::new();
        while rd.read_line(&mut line).unwrap_or(0) > 0 {
            print!("{line}");
            if line.contains("guarding:") {
                guarding = true;
                break;
            }
            if line.contains("no saved result") || line.contains("run 'amduv tune' again") {
                no_result = true;
            }
            line.clear();
        }
        if guarding {
            // keep draining its output (the restore message at exit) without blocking the miner
            std::thread::spawn(move || {
                for l in rd.lines().map_while(|l| l.ok()) {
                    println!("{l}");
                }
            });
            return Some(child);
        }
        let _ = child.wait();
        if no_result && tune_if_missing && attempt == 0 {
            lg(format!("no saved result for this card: tuning now (goal {goal}; takes ~15-30 min, once per card/driver/library)"));
            let mut a = vec!["tune".to_string(), "--goal".to_string(), goal.to_string()];
            a.extend(base.iter().cloned());
            match run(&t, &a) {
                Ok(0) => continue,
                Ok(c) => lg(format!("tuning ended without a result (exit {c}); mining at the current settings")),
                Err(e) => lg(format!("cannot run amduv tune: {e}")),
            }
        }
        return None;
    }
    None
}

/// `--uv-reset` with AMD: forget amduv's saved results.
pub fn reset() -> Result<()> {
    let Some(t) = tool() else { bail!("amduv not found next to the binary") };
    let ini = t.with_file_name("amduv.ini");
    match std::fs::remove_file(&ini) {
        Ok(()) => lg(format!("saved results deleted ({})", ini.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => lg(format!("no saved results ({})", ini.display())),
        Err(e) => bail!("cannot delete {}: {e}", ini.display()),
    }
    Ok(())
}
