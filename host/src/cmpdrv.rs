// FreeForGem — see LICENSE
//! `--cmp-driver`: one-shot, opt-in install of an unlocked NVIDIA driver on a HiveOS rig with NVIDIA CMP cards
//! (50HX and the like). Under the stock driver the compute pipes of these cards run at a small part of their speed; the
//! pinned NVIDIA Open Kernel Module build below lifts that. The miner NEVER does this on its own: mining does not call
//! here, nothing is installed unless the operator passes `--cmp-driver` (or puts `cmp-unlock-driver` in the HiveOS
//! "Extra config arguments", see hiveos/h-run.sh). The flow: check HiveOS + root + a CMP card on the PCI bus, download
//! the pinned .run to disk, check its SHA-256 against the value baked in here, install it with the stock
//! `nvidia-driver-update <file> --force`, then reboot. Any failed check changes nothing and exits non-zero.
//!
//! "Already installed" is decided by the BUILD TAG of the kernel module in /proc/driver/nvidia/version, not by the
//! version: the stock driver of the same version reports the same VERSION but another tag.
//!
//! Env (all optional): FFG_CMP_DRIVER_FORCE=1 install even if the unlock build is in place / no CMP card found;
//! FFG_CMP_DRIVER_NO_REBOOT=1 install but do not reboot; FFG_CMP_DRIVER_DRY_RUN=1 run every check, log what would be
//! done, change nothing; FFG_CMP_DRIVER_TAGS=a,b the accepted build tags instead of the built-in list;
//! FFG_CMP_DRIVER_URL=... download from another place (the SHA-256 stays pinned, so another file is refused).
use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

// ---- the pin: everything that names the driver is here ----------------------------------------------------------
// The SHA-256 below is what makes the file trustworthy; the URL is only where to get it (owner's decision for 1.7.7: this host).
const DRIVER_URL: &str = "https://kavatar.kasplay.online/dl/NVIDIA-Linux-x86_64-610.43.03-cmp-unlock-v2.run";
const DRIVER_SHA256: &str = "9807c37c528191a8b90cd5ef93d4817f393c841b9a0362c65f18d65dfb490400";
/// VERSION as it reads in /proc/driver/nvidia/version.
const DRIVER_VERSION: &str = "610.43.03";
/// Build tags (the `(user@host)` of /proc/driver/nvidia/version) of the unlock module that count as "already
/// installed". A rebuilt driver adds its tag here (and re-pins URL + SHA-256 above).
const UNLOCK_BUILD_TAGS: &[&str] = &["shaxtar@shahtyor"];
// -----------------------------------------------------------------------------------------------------------------

const HIVE_UPDATE: &str = "/hive/sbin/nvidia-driver-update";
const PROC_VERSION: &str = "/proc/driver/nvidia/version";
const PCI_DEVICES: &str = "/sys/bus/pci/devices";
/// NVIDIA vendor id and the PCI device ids of the CMP family (50HX 1e09; 30HX 1f0b; 40HX 248a/220d; 90HX 20c2; 170HX 2082).
const PCI_VENDOR_NVIDIA: &str = "0x10de";
const CMP_DEVICE_IDS: &[&str] = &["0x1f0b", "0x1e09", "0x248a", "0x220d", "0x20c2", "0x2082"];

fn envset(k: &str) -> bool {
    std::env::var(k).map(|v| v != "0" && !v.is_empty()).unwrap_or(false)
}

/// What a run decided / did.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// the unlock build is in place (and no force): nothing to do
    AlreadyInstalled,
    /// dry run: all checks passed, an install would follow
    DryRun,
    /// installed (and rebooted, or told to reboot)
    Installed,
}

/// Everything a run reads from the system, so that a test can point it at a fake tree.
pub struct Ctx {
    pub linux: bool,
    pub pci_root: PathBuf,
    pub proc_version: PathBuf,
    pub hive_update: PathBuf,
    /// None = read it from /proc/self/status
    pub euid: Option<u32>,
    pub force: bool,
    pub dry_run: bool,
    pub no_reboot: bool,
    pub tags: Vec<String>,
}

impl Ctx {
    fn from_env() -> Ctx {
        let tags = match std::env::var("FFG_CMP_DRIVER_TAGS") {
            Ok(s) if !s.trim().is_empty() => s.split(',').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect(),
            _ => UNLOCK_BUILD_TAGS.iter().map(|t| t.to_string()).collect(),
        };
        Ctx {
            linux: cfg!(target_os = "linux"),
            pci_root: PathBuf::from(PCI_DEVICES),
            proc_version: PathBuf::from(PROC_VERSION),
            hive_update: PathBuf::from(HIVE_UPDATE),
            euid: None,
            force: envset("FFG_CMP_DRIVER_FORCE"),
            dry_run: envset("FFG_CMP_DRIVER_DRY_RUN"),
            no_reboot: envset("FFG_CMP_DRIVER_NO_REBOOT"),
            tags,
        }
    }
}

/// Entry for main: returns the process exit code (0 ok / already done, non-zero on any failure).
pub fn install() -> i32 {
    match run(&Ctx::from_env()) {
        Ok(_) => 0,
        Err(e) => {
            println!("--cmp-driver: {e:#}");
            1
        }
    }
}

/// Parsed first line of /proc/driver/nvidia/version, e.g.
/// `NVRM version: NVIDIA UNIX Open Kernel Module for x86_64  610.43.03  Release Build  (user@host)  Mon ...`
#[derive(Debug, PartialEq, Eq)]
pub struct DriverInfo {
    pub version: Option<String>,
    pub build_tag: Option<String>,
}

pub fn parse_proc_version(text: &str) -> Option<DriverInfo> {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty())?;
    // the version: the first token that is dotted digits ("610.43.03", "595.91.07")
    let version = line
        .split_whitespace()
        .find(|t| t.contains('.') && t.split('.').all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())))
        .map(str::to_string);
    // the build tag: the first parenthesised text after "Build"
    let after = line.find("Build").map(|i| &line[i..]).unwrap_or(line);
    let build_tag = after
        .find('(')
        .and_then(|i| after[i + 1..].find(')').map(|j| after[i + 1..i + 1 + j].trim().to_string()))
        .filter(|t| !t.is_empty());
    Some(DriverInfo { version, build_tag })
}

/// Is this the unlock build? By the tag; the version must match as well (a tag of ours on another version is not the pin).
pub fn is_unlock_build(info: &DriverInfo, tags: &[String]) -> bool {
    info.version.as_deref() == Some(DRIVER_VERSION) && info.build_tag.as_deref().map(|t| tags.iter().any(|x| x == t)).unwrap_or(false)
}

fn run(c: &Ctx) -> Result<Outcome> {
    println!("--cmp-driver: CMP unlock driver install (opt-in){}", if c.dry_run { " [dry run: nothing is changed]" } else { "" });
    if !c.linux {
        bail!("only on a HiveOS/Linux rig");
    }
    if !c.hive_update.exists() {
        bail!("{} not found - this runs on HiveOS (use nvidia-driver-update yourself elsewhere)", c.hive_update.display());
    }
    let euid = match c.euid {
        Some(e) => e,
        None => euid()?,
    };
    if euid != 0 {
        bail!("must run as root (sudo); installing a kernel driver needs it");
    }

    let cmp = cmp_cards(&c.pci_root);
    if cmp.is_empty() && !c.force {
        bail!("no NVIDIA CMP card found on the PCI bus - refusing to replace the driver (FFG_CMP_DRIVER_FORCE=1 to override)");
    }
    println!("--cmp-driver: CMP card(s): {}", if cmp.is_empty() { "none (forced)".into() } else { cmp.join(", ") });

    if let Some(info) = std::fs::read_to_string(&c.proc_version).ok().and_then(|s| parse_proc_version(&s)) {
        println!(
            "--cmp-driver: current driver {} build {}",
            info.version.as_deref().unwrap_or("?"),
            info.build_tag.as_deref().unwrap_or("?")
        );
        if is_unlock_build(&info, &c.tags) {
            if !c.force {
                println!("--cmp-driver: the CMP unlock driver {DRIVER_VERSION} is already installed - nothing to do (FFG_CMP_DRIVER_FORCE=1 to reinstall)");
                return Ok(Outcome::AlreadyInstalled);
            }
        } else if info.version.as_deref() == Some(DRIVER_VERSION) {
            println!("--cmp-driver: driver {DRIVER_VERSION} is present but not the CMP unlock build - installing the unlock");
        }
    }

    let url = std::env::var("FFG_CMP_DRIVER_URL").ok().filter(|u| !u.trim().is_empty()).unwrap_or_else(|| DRIVER_URL.to_string());
    if c.dry_run {
        println!("--cmp-driver: dry run - would download {url}");
        println!(
            "--cmp-driver: dry run - would check SHA-256 {DRIVER_SHA256}, run {} <file> --force, then {}",
            c.hive_update.display(),
            if c.no_reboot { "NOT reboot (FFG_CMP_DRIVER_NO_REBOOT)" } else { "reboot" }
        );
        return Ok(Outcome::DryRun);
    }

    let dst = download_dir().join(".ffg-cmp-unlock.run");
    let _ = std::fs::remove_file(&dst);
    println!("--cmp-driver: downloading {url}");
    download(&url, &dst).with_context(|| format!("download to {}", dst.display()))?;

    println!("--cmp-driver: verifying SHA-256");
    let got = sha256_file(&dst).with_context(|| format!("hash {}", dst.display()))?;
    if !got.eq_ignore_ascii_case(DRIVER_SHA256) {
        let _ = std::fs::remove_file(&dst);
        bail!("SHA-256 mismatch: got {got}, expected {DRIVER_SHA256} - file deleted, nothing installed");
    }
    println!("--cmp-driver: SHA-256 OK ({DRIVER_SHA256})");

    println!("--cmp-driver: installing via {} (this stops GPU work and takes minutes)", c.hive_update.display());
    let st = Command::new(&c.hive_update).arg(&dst).arg("--force").status().with_context(|| format!("run {}", c.hive_update.display()))?;
    let _ = std::fs::remove_file(&dst);
    if !st.success() {
        bail!("{} exited with {st} - driver not changed (or left half-installed; check the rig)", c.hive_update.display());
    }
    println!("--cmp-driver: driver installed");

    if c.no_reboot {
        println!("--cmp-driver: FFG_CMP_DRIVER_NO_REBOOT set - reboot the rig yourself to load the new driver");
        return Ok(Outcome::Installed);
    }
    println!("--cmp-driver: rebooting to load the new driver");
    let _ = Command::new("reboot").status();
    // reboot is async; give it a moment so we do not return into mining
    std::thread::sleep(std::time::Duration::from_secs(30));
    Ok(Outcome::Installed)
}

/// Effective uid from /proc/self/status (no libc dependency).
fn euid() -> Result<u32> {
    let s = std::fs::read_to_string("/proc/self/status").context("read /proc/self/status")?;
    for line in s.lines() {
        if let Some(rest) = line.strip_prefix("Uid:") {
            if let Some(eff) = rest.split_whitespace().nth(1) {
                return Ok(eff.parse().unwrap_or(1));
            }
        }
    }
    bail!("no Uid in /proc/self/status");
}

/// CMP cards under a sysfs PCI root (vendor + device id); returns the bus addresses found, sorted.
pub fn cmp_cards(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(root) else { return out };
    for e in rd.flatten() {
        let p = e.path();
        let ven = std::fs::read_to_string(p.join("vendor")).unwrap_or_default().trim().to_ascii_lowercase();
        if ven != PCI_VENDOR_NVIDIA {
            continue;
        }
        let dev = std::fs::read_to_string(p.join("device")).unwrap_or_default().trim().to_ascii_lowercase();
        if CMP_DEVICE_IDS.contains(&dev.as_str()) {
            out.push(e.file_name().to_string_lossy().into_owned());
        }
    }
    out.sort();
    out
}

/// A disk directory to stage the download in (not tmpfs: the file is ~540 MiB).
fn download_dir() -> PathBuf {
    for d in ["/hive-drivers-pack", "/hive/tmp"] {
        if Path::new(d).is_dir() {
            return PathBuf::from(d);
        }
    }
    std::env::current_exe().ok().and_then(|p| p.parent().map(|x| x.to_path_buf())).unwrap_or_else(|| PathBuf::from("."))
}

/// Download url -> path with curl, then wget; whichever is present. Fails if neither works.
fn download(url: &str, dst: &Path) -> Result<()> {
    if which("curl") {
        let st = Command::new("curl").args(["-fL", "--retry", "3", "--connect-timeout", "20", "-o"]).arg(dst).arg(url).status().context("run curl")?;
        if st.success() {
            return Ok(());
        }
        println!("--cmp-driver: curl failed ({st}), trying wget");
    }
    if which("wget") {
        let st = Command::new("wget").args(["-O"]).arg(dst).arg(url).status().context("run wget")?;
        if st.success() {
            return Ok(());
        }
        bail!("wget exited with {st}");
    }
    bail!("neither curl nor wget is available to download the driver");
}

fn which(cmd: &str) -> bool {
    Command::new(cmd).arg("--version").stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
}

/// Streaming SHA-256 of a file -> lowercase hex.
fn sha256_file(p: &Path) -> Result<String> {
    let mut f = std::fs::File::open(p)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    let d: [u8; 32] = h.finalize().into();
    Ok(d.iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const UNLOCK: &str = "NVRM version: NVIDIA UNIX Open Kernel Module for x86_64  610.43.03  Release Build  (shaxtar@shahtyor)  Sun Nov  1 10:00:00 UTC 2026\nGCC version:  gcc version 12\n";
    const STOCK: &str = "NVRM version: NVIDIA UNIX Open Kernel Module for x86_64  610.43.03  Release Build  (dvs-builder@U22-I3-AE18-23-3)  Tue Oct 14 2026\n";
    const OLD: &str = "NVRM version: NVIDIA UNIX x86_64 Kernel Module  595.91.07  Tue Jan  1 00:00:00 UTC 2026\n";

    fn tags() -> Vec<String> {
        UNLOCK_BUILD_TAGS.iter().map(|t| t.to_string()).collect()
    }

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ffg-cmpdrv-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn pci(root: &Path, addr: &str, ven: &str, dev: &str) {
        let d = root.join(addr);
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("vendor"), format!("{ven}\n")).unwrap();
        fs::write(d.join("device"), format!("{dev}\n")).unwrap();
    }

    #[test]
    fn parse_unlock_and_stock_tags() {
        let u = parse_proc_version(UNLOCK).unwrap();
        assert_eq!(u.version.as_deref(), Some("610.43.03"));
        assert_eq!(u.build_tag.as_deref(), Some("shaxtar@shahtyor"));
        let s = parse_proc_version(STOCK).unwrap();
        assert_eq!(s.version.as_deref(), Some("610.43.03"));
        assert_eq!(s.build_tag.as_deref(), Some("dvs-builder@U22-I3-AE18-23-3"));
        let o = parse_proc_version(OLD).unwrap();
        assert_eq!(o.version.as_deref(), Some("595.91.07"));
        assert_eq!(o.build_tag, None);
        assert!(parse_proc_version("").is_none());
    }

    #[test]
    fn same_version_tells_unlock_from_stock() {
        assert!(is_unlock_build(&parse_proc_version(UNLOCK).unwrap(), &tags()));
        assert!(!is_unlock_build(&parse_proc_version(STOCK).unwrap(), &tags()));
        assert!(!is_unlock_build(&parse_proc_version(OLD).unwrap(), &tags()));
        // our tag on another version is not the pin
        let other = UNLOCK.replace("610.43.03", "595.91.07");
        assert!(!is_unlock_build(&parse_proc_version(&other).unwrap(), &tags()));
    }

    #[test]
    fn old_and_future_tags_are_both_accepted() {
        let both = vec!["shaxtar@shahtyor".to_string(), "ffg-cmp@build".to_string()];
        let future = UNLOCK.replace("shaxtar@shahtyor", "ffg-cmp@build");
        assert!(is_unlock_build(&parse_proc_version(&future).unwrap(), &both));
        assert!(is_unlock_build(&parse_proc_version(UNLOCK).unwrap(), &both));
        assert!(!is_unlock_build(&parse_proc_version(&future).unwrap(), &tags()));
    }

    #[test]
    fn pci_scan_fake_sysfs() {
        let r = scratch("pci");
        pci(&r, "0000:01:00.0", "0x10de", "0x1e09"); // CMP 50HX
        pci(&r, "0000:02:00.0", "0x10de", "0x2484"); // RTX 3070, not CMP
        pci(&r, "0000:03:00.0", "0x1002", "0x1e09"); // other vendor, same device id
        pci(&r, "0000:04:00.0", "0x10de", "0x2082"); // CMP 170HX
        assert_eq!(cmp_cards(&r), vec!["0000:01:00.0", "0000:04:00.0"]);
        assert!(cmp_cards(&r.join("missing")).is_empty());
        let _ = fs::remove_dir_all(&r);
    }

    /// A Ctx over a fake tree: a fake updater file, a root euid, a non-CMP card (+ a CMP card), the given /proc text.
    fn ctx(name: &str, proc_text: Option<&str>, with_cmp: bool) -> (Ctx, PathBuf) {
        let d = scratch(name);
        let pcid = d.join("pci");
        fs::create_dir_all(&pcid).unwrap();
        pci(&pcid, "0000:02:00.0", "0x10de", "0x2484");
        if with_cmp {
            pci(&pcid, "0000:01:00.0", "0x10de", "0x1e09");
        }
        let upd = d.join("nvidia-driver-update");
        fs::write(&upd, "#!/bin/sh\nexit 0\n").unwrap();
        let pv = d.join("version");
        if let Some(t) = proc_text {
            fs::write(&pv, t).unwrap();
        }
        let c = Ctx { linux: true, pci_root: pcid, proc_version: pv, hive_update: upd, euid: Some(0), force: false, dry_run: true, no_reboot: true, tags: tags() };
        (c, d)
    }

    #[test]
    fn noop_when_unlock_build_is_in_place() {
        let (mut c, d) = ctx("noop", Some(UNLOCK), true);
        assert_eq!(run(&c).unwrap(), Outcome::AlreadyInstalled);
        c.dry_run = false; // a real run is a no-op too: nothing downloaded, nothing installed
        assert_eq!(run(&c).unwrap(), Outcome::AlreadyInstalled);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn dry_run_on_stock_future_tag_and_force() {
        let (mut c, d) = ctx("dry", Some(STOCK), true);
        assert_eq!(run(&c).unwrap(), Outcome::DryRun); // same version, stock tag: an install is planned
        c.tags = vec!["ffg-cmp@build".into()]; // a future tag only: the old build no longer counts
        fs::write(&c.proc_version, UNLOCK).unwrap();
        assert_eq!(run(&c).unwrap(), Outcome::DryRun);
        c.tags = tags();
        c.force = true; // force goes past "already installed"
        assert_eq!(run(&c).unwrap(), Outcome::DryRun);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn refusals_change_nothing() {
        let (mut c, d) = ctx("refuse", Some(STOCK), false);
        assert!(format!("{:#}", run(&c).unwrap_err()).contains("no NVIDIA CMP card"));
        c.force = true;
        assert_eq!(run(&c).unwrap(), Outcome::DryRun); // forced without a card
        let (mut c2, d2) = ctx("refuse2", Some(STOCK), true);
        c2.euid = Some(1000);
        assert!(format!("{:#}", run(&c2).unwrap_err()).contains("root"));
        c2.euid = Some(0);
        let real = c2.hive_update.clone();
        c2.hive_update = d2.join("absent");
        assert!(format!("{:#}", run(&c2).unwrap_err()).contains("HiveOS"));
        c2.hive_update = real;
        c2.linux = false;
        assert!(format!("{:#}", run(&c2).unwrap_err()).contains("Linux"));
        let _ = (fs::remove_dir_all(&d), fs::remove_dir_all(&d2));
    }

    #[test]
    fn sha256_of_a_file() {
        let d = scratch("sha");
        let f = d.join("x");
        fs::write(&f, b"abc").unwrap();
        assert_eq!(sha256_file(&f).unwrap(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        let _ = fs::remove_dir_all(&d);
    }
}
