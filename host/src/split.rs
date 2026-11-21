//! 1.8.0: different coins on different cards of one rig (`--split`, PLAN 3.1a; owner 21.11.2026: «ещё одна фишка нашего
//! майнера»).
//!
//! `freeforgem --split "neb:0,1;pearl:rest" --pool neb=stratum+tcp://neb.suprnova.cc:7804 --wallet neb=neb1... \
//!      --pool pearl=pearl-eu2.luckypool.io:3360 --wallet pearl=prl1... --auto-uv --lock-cclock neb=1680 --lock-cclock pearl=1410`
//!
//! This process is only a supervisor: it starts one miner process of this same binary per group of cards
//! (`--algo COIN --devices LIST` + that coin's options), with `FFG_GROUP=<group>` so the state files of each group are its
//! own (coins::group_file: restore markers per group; uv.json / gpu-faults.json / autolock.json are merged under a lock).
//! Each child owns its cards, pool session, dev fee (its own connection, 1 % of its cards' time), clocks, memory policy,
//! tuners and crash restarts; a child that ends is started again after a pause, the other groups never stop for it.
//!
//! Options of the command line go to every group unless their value is tagged with a coin: `--pool neb=URL` goes to the NEB
//! group only (as `--pool URL`), `--lock-cclock pearl=1410` to the Pearl group only. `--split-args COIN="--opt v --flag"`
//! gives a group options of its own (flags without a value too). `--coin-price neb=X,pearl=Y` is not a tag (passed as is).
//! The log: every line of a child carries the group in front, `[neb] [  60s] GPU0 ... | total 24.690 MA/s ...`;
//! hiveos/h-stats.sh reads the last stats line of every group.
//!
//! The canary of this module (the supervisor's own text, kept in the binary): see SPLIT_TAG.

use anyhow::{Result, anyhow, bail, ensure};
use std::collections::BTreeSet;
use std::io::BufRead;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::coins::{self, Coin};

/// The option that turns the supervisor on.
pub const SPLIT_FLAG: &str = "--split";
/// Options of one group: `--split-args neb="--table-mclock 810"`.
pub const SPLIT_ARGS_FLAG: &str = "--split-args";
/// Set in every child: a child never starts a split of its own (a `--split` left in its arguments is an error there).
pub const CHILD_ENV: &str = "FFG_SPLIT_CHILD";
/// The group of a child (coins::group reads it).
pub const GROUP_ENV: &str = "FFG_GROUP";
/// Values that look like a tag but are not one (`--coin-price neb=X,pearl=Y`).
const NOT_TAGGED: [&str; 1] = ["--coin-price"];
/// Removed from the children's arguments (the supervisor sets them per group).
const PER_GROUP: [&str; 5] = ["--devices", "--device", "--algo", "--coin", SPLIT_ARGS_FLAG];
/// The supervisor's own canary text (FFG 1.8.0 split; unique to FreeForGem).
pub static SPLIT_TAG: &str = "ffg-split/grp-v1 kx7Qe2";

#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub coin: Coin,
    /// the group name: the coin's name, `neb2` for a second NEB group (FFG_GROUP, the log prefix, the state file names)
    pub name: String,
    pub devs: Vec<u32>,
}

fn lg(m: impl AsRef<str>) {
    // coloured by its kind (color.rs) like any line; a light-blue name inside keeps the line's own colour after it
    let line = keep_outer(crate::color::paint(format!("split: {}", m.as_ref())));
    crate::conout::emit(false, format!("{line}
").into_bytes());
}

/// `ESC[33m ... ESC[96mneb ESC[0m ... ESC[0m` → the outer colour comes back after every inner reset.
fn keep_outer(line: String) -> String {
    match line.strip_prefix("[").and_then(|r| r.split_once('m')) {
        Some((code, body)) if line.ends_with(RESET) && body.len() >= RESET.len() => {
            let outer = format!("[{code}m");
            let body = &body[..body.len() - RESET.len()];
            format!("{outer}{}{RESET}", body.replace(RESET, &format!("{RESET}{outer}")))
        }
        _ => line,
    }
}

/// Light blue (owner 21.11.2026: «при сплите в логах майнера название монет или алгоритмов нужно выделить голубым цветом»):
/// the group in front of every line of a group's miner, and the coin names in the supervisor's lines.  No colour when
/// colours are off (--no-color, NO_COLOR, FFG_COLOR=0); h-stats.sh strips the codes before it parses.
const BLUE: &str = "[96m";
const RESET: &str = "[0m";

fn blue(s: &str) -> String {
    blue_if(crate::color::enabled(), s)
}

fn blue_if(on: bool, s: &str) -> String {
    if on { format!("{BLUE}{s}{RESET}") } else { s.to_string() }
}

fn group_tag(name: &str) -> String {
    format!("{} ", blue(&format!("[{name}]")))
}

/// The `--split` value of the command line (`--split X` or `--split=X`); none in a child of a split.
pub fn split_arg(argv: &[String]) -> Option<String> {
    if std::env::var_os(CHILD_ENV).is_some() {
        return None;
    }
    split_value(argv)
}

fn split_value(argv: &[String]) -> Option<String> {
    (0..argv.len()).find_map(|i| split_span(argv, i).map(|(v, _)| v))
}

/// A word that goes on a split spec written without quotes (`--split neb:0,1 pearl:rest`): `coin:...`, GPU numbers / ranges,
/// `rest`, `*`.
fn spec_word(a: &str) -> bool {
    if a.starts_with("--") || a.is_empty() {
        return false;
    }
    if let Some((c, _)) = a.split_once(':') {
        return coins::by_name(c.trim()).is_some();
    }
    a == "rest" || a == "*" || a.chars().all(|c| c.is_ascii_digit() || matches!(c, ',' | ';' | '-'))
}

/// `--split` at argument `i`: its value (the words of an unquoted spec joined by a space) and the number of arguments taken.
fn split_span(argv: &[String], i: usize) -> Option<(String, usize)> {
    let a = argv.get(i)?;
    let (mut v, mut n) = if a == SPLIT_FLAG {
        (argv.get(i + 1)?.clone(), 2)
    } else {
        (a.strip_prefix("--split=")?.to_string(), 1)
    };
    while let Some(w) = argv.get(i + n).filter(|w| spec_word(w)) {
        v.push(' ');
        v.push_str(w);
        n += 1;
    }
    Some((v, n))
}

fn parse_devs(s: &str) -> Result<Vec<u32>> {
    let mut v = Vec::new();
    for part in s.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        if let Some((a, b)) = part.split_once('-') {
            let (a, b): (u32, u32) = (a.trim().parse().map_err(|_| anyhow!("bad GPU number {a:?}"))?, b.trim().parse().map_err(|_| anyhow!("bad GPU number {b:?}"))?);
            ensure!(a <= b && b - a < 64, "bad GPU range {part:?}");
            v.extend(a..=b);
        } else {
            v.push(part.parse().map_err(|_| anyhow!("bad GPU number {part:?}"))?);
        }
    }
    ensure!(!v.is_empty(), "no GPUs in {s:?}");
    Ok(v)
}

/// `"neb:0,1;pearl:2-5"`, one group may take `rest` (or `*`): the cards no other group names (`count` = the GPUs of the rig,
/// from NVML; without it `rest` cannot be used and the numbers are not range-checked).
pub fn parse_spec(spec: &str, count: Option<u32>) -> Result<Vec<Group>> {
    let mut out: Vec<Group> = Vec::new();
    let mut rest: Option<Coin> = None;
    let mut rest_at = 0usize;
    let mut used = BTreeSet::new();
    // owner 21.11.2026: ';', ',' and white space are all separators ("neb:0,1;pearl:rest" = "neb:0 1, pearl:rest"); a word
    // with "coin:" starts a group, the words after it are its cards
    let mut ents: Vec<(String, Vec<String>)> = Vec::new();
    for w in spec.split(|c: char| c == ';' || c == ',' || c.is_whitespace()).filter(|w| !w.is_empty()) {
        if let Some((c, d)) = w.split_once(':') {
            ents.push((c.to_string(), if d.is_empty() { Vec::new() } else { vec![d.to_string()] }));
        } else if let Some(e) = ents.last_mut() {
            e.1.push(w.to_string());
        } else {
            bail!("--split {spec:?}: begins with {w:?}, not with COIN:GPUS (e.g. neb:0,1 pearl:rest)");
        }
    }
    for (c, dw) in &ents {
        let ent = format!("{c}:{}", dw.join(","));
        ensure!(!dw.is_empty(), "--split entry {ent:?}: COIN:GPUS (e.g. neb:0,1)");
        let d = dw.join(",");
        let c = c.as_str();
        ensure!(!(dw.len() > 1 && dw.iter().any(|x| x == "rest" || x == "*")), "--split entry {ent:?}: 'rest' takes the cards no other group names, without numbers");
        let coin = coins::by_name(c.trim()).ok_or_else(|| anyhow!("--split: unknown coin {:?}", c.trim()))?;
        let n = out.iter().filter(|g| g.coin == coin).count() + usize::from(rest == Some(coin));
        let name = if n == 0 { coins::spec(coin).name.to_string() } else { format!("{}{}", coins::spec(coin).name, n + 1) };
        let d = d.trim();
        if d == "rest" || d == "*" {
            ensure!(rest.is_none(), "--split: only one group can take the rest of the cards");
            rest = Some(coin);
            rest_at = out.len();
            out.push(Group { coin, name, devs: Vec::new() });
            continue;
        }
        let devs = parse_devs(d)?;
        for &x in &devs {
            if let Some(n) = count {
                ensure!(x < n, "--split: GPU{x} does not exist (the rig has {n} GPUs: 0..{})", n.saturating_sub(1));
            }
            ensure!(used.insert(x), "--split: GPU{x} is in two groups (one card mines one coin)");
        }
        out.push(Group { coin, name, devs });
    }
    ensure!(!out.is_empty(), "--split needs at least one group, e.g. --split \"neb:0,1;pearl:rest\"");
    if rest.is_some() {
        let n = count.ok_or_else(|| anyhow!("--split ...:rest needs the GPU count (NVML not available): list the cards"))?;
        let left: Vec<u32> = (0..n).filter(|x| !used.contains(x)).collect();
        ensure!(!left.is_empty(), "--split: no cards left for the 'rest' group ({})", out[rest_at].name);
        out[rest_at].devs = left;
    }
    Ok(out)
}

/// `coin=value` → (coin, value) when `coin` is a coin's name or alias.
fn tag_of(v: &str) -> Option<(Coin, &str)> {
    let (c, val) = v.split_once('=')?;
    if c.is_empty() || !c.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_') {
        return None;
    }
    coins::by_name(c).map(|coin| (coin, val))
}

/// `--split-args neb="--a 1 --b"` values of this coin, split at white space (quotes `"` / `'` group words).
fn words(s: &str) -> Vec<String> {
    let (mut out, mut cur, mut q, mut any) = (Vec::new(), String::new(), None::<char>, false);
    for ch in s.chars() {
        match (q, ch) {
            (Some(qc), c) if c == qc => q = None,
            (None, '"' | '\'') => {
                q = Some(ch);
                any = true;
            }
            (None, c) if c.is_whitespace() => {
                if any {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            (_, c) => {
                cur.push(c);
                any = true;
            }
        }
    }
    if any {
        out.push(cur);
    }
    out
}

/// The arguments of the miner process of group `g` (`idx` = its place in the split, for the API port).
pub fn child_args(argv: &[String], g: &Group, idx: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut extra: Vec<String> = Vec::new();
    let mut api_seen = false;
    let mut i = 0;
    while i < argv.len() {
        let a = &argv[i];
        let next = argv.get(i + 1);
        // the split itself
        if let Some((_, n)) = split_span(argv, i) {
            i += n;
            continue;
        }
        // set per group below
        if PER_GROUP.contains(&a.as_str()) {
            if a == SPLIT_ARGS_FLAG {
                if let Some((c, v)) = next.and_then(|n| tag_of(n)) {
                    if c == g.coin {
                        extra.extend(words(v));
                    }
                }
            }
            i += 2;
            continue;
        }
        if let Some((k, _)) = a.split_once('=') {
            if a.starts_with("--") && PER_GROUP.contains(&k) {
                i += 1;
                continue;
            }
        }
        // a tagged value: --opt coin=value (or --opt=coin=value)
        if a.starts_with("--") && !NOT_TAGGED.contains(&a.as_str()) {
            if let Some(n) = next.filter(|n| !n.starts_with("--")) {
                if let Some((c, v)) = tag_of(n) {
                    if c == g.coin {
                        out.push(a.clone());
                        out.push(api_port(a, v, idx, &mut api_seen));
                    }
                    i += 2;
                    continue;
                }
            }
            if let Some((k, val)) = a.split_once('=') {
                if !NOT_TAGGED.contains(&k) {
                    if let Some((c, v)) = tag_of(val) {
                        if c == g.coin {
                            out.push(k.to_string());
                            out.push(api_port(k, v, idx, &mut api_seen));
                        }
                        i += 1;
                        continue;
                    }
                }
            }
        }
        // untagged: to every group (an API port moves by the group's place, so two groups never share one)
        if (a == "--api" || a == "--api-port") && next.is_some() {
            out.push(a.clone());
            out.push(api_port(a, next.unwrap(), idx, &mut api_seen));
            i += 2;
            continue;
        }
        out.push(a.clone());
        i += 1;
    }
    out.extend(extra);
    // Ting serves its API on 4068 by default: a second group of the rig with an API of its own would fail to bind
    if g.coin == Coin::Ting && !api_seen && idx > 0 && !out.iter().any(|a| a == "--api-port" || a == "--api-bind") {
        out.push("--api-port".into());
        out.push((4068 + idx).to_string());
    }
    out.push("--algo".into());
    out.push(coins::spec(g.coin).name.into());
    out.push("--devices".into());
    out.push(g.devs.iter().map(u32::to_string).collect::<Vec<_>>().join(","));
    out
}

/// An API port value moved by `idx` (`4068` → `4069` for the second group; `HOST:PORT` likewise; `0` / `off` stay).
fn api_port(opt: &str, v: &str, idx: usize, seen: &mut bool) -> String {
    if opt != "--api" && opt != "--api-port" {
        return v.to_string();
    }
    *seen = true;
    let (host, port) = match v.rsplit_once(':') {
        Some((h, p)) => (Some(h), p),
        None => (None, v),
    };
    match port.parse::<u32>() {
        Ok(p) if p > 0 => {
            let p = (p as usize + idx).min(65535);
            host.map_or(p.to_string(), |h| format!("{h}:{p}"))
        }
        _ => v.to_string(),
    }
}

/// The groups of restore markers left in the state folder (`clocks-restore.<group>.json` ...).
fn marker_groups(dir: &std::path::Path) -> BTreeSet<String> {
    const STEMS: [&str; 5] = ["clocks-restore", "uv-restore", "ffgn-offset-restore", "noid-offset-restore", "cmfd-offset-restore"];
    let mut out = BTreeSet::new();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let n = e.file_name().to_string_lossy().to_string();
        for s in STEMS {
            if let Some(g) = n.strip_prefix(&format!("{s}.")).and_then(|r| r.strip_suffix(".json")) {
                if !g.is_empty() && !g.contains('.') {
                    out.insert(g.to_string());
                }
            }
        }
    }
    out
}

fn state_dir(argv: &[String]) -> std::path::PathBuf {
    if let Some(d) = argv.iter().position(|a| a == "--uv-dir").and_then(|i| argv.get(i + 1)) {
        return d.into();
    }
    let mut o = crate::uv::Opts::default();
    o.dir = None;
    crate::uv::state_dir(&o)
}

/// Markers of a group that is not in this split (its cards went to another group): put their clocks / offsets back now,
/// one short process per old group (`--split-recover`, FFG_GROUP set), before any group starts.
fn recover_stale(argv: &[String], groups: &[Group]) {
    let dir = state_dir(argv);
    let now: BTreeSet<&str> = groups.iter().map(|g| g.name.as_str()).collect();
    for old in marker_groups(&dir).into_iter().filter(|g| !now.contains(g.as_str())) {
        lg(format!("restore files of group '{old}' (not in this split) in {}: putting its clocks / offsets back", dir.display()));
        let Ok(exe) = std::env::current_exe() else { return };
        let mut cmd = Command::new(exe);
        cmd.arg("--split-recover").env(GROUP_ENV, &old).env(CHILD_ENV, "1").stdin(Stdio::null());
        if let Some(d) = argv.iter().position(|a| a == "--uv-dir").and_then(|i| argv.get(i + 1)) {
            cmd.args(["--uv-dir", d]);
        }
        match cmd.status() {
            Ok(s) if s.success() => {}
            Ok(s) => lg(format!("recovery of group '{old}' ended with {s} (its restore files stay)")),
            Err(e) => lg(format!("cannot run the recovery of group '{old}': {e}")),
        }
    }
}

/// `freeforgem --split-recover` (a child of the supervisor, FFG_GROUP = an old group): the crash recovery of every coin's
/// restore markers of that group, then exit.
pub fn recover_main(argv: &[String]) -> Result<()> {
    let dir = state_dir(argv);
    crate::clocks::recover(&dir);
    crate::offset::recover(&dir);
    crate::noid::offset::recover(&dir);
    crate::cmfd_offset::recover(&dir);
    let mut o = crate::uv::Opts::default();
    o.dir = Some(dir);
    crate::uv::recover(&o);
    Ok(())
}

struct Run {
    g: Group,
    args: Vec<String>,
    child: Option<Child>,
    /// when it may start (again)
    start_at: Instant,
    exits: Vec<Instant>,
}

fn spawn(r: &mut Run) -> Result<()> {
    let exe = std::env::current_exe().map_err(|e| anyhow!("cannot find the miner executable: {e}"))?;
    let mut cmd = Command::new(&exe);
    cmd.args(&r.args).env(GROUP_ENV, &r.g.name).env(CHILD_ENV, "1").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    // the child's own lines are coloured by this process (the prefix keeps the kind of line, color.rs)
    cmd.env("FFG_COLOR", "0");
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::CommandExt;
        // a supervisor that dies (kill -9, OOM) must not leave its miners running: the next start would put two miners on
        // one card; SIGINT = the child's normal shutdown (clocks / offsets put back)
        unsafe extern "C" {
            fn prctl(option: i32, arg2: u64, arg3: u64, arg4: u64, arg5: u64) -> i32;
        }
        unsafe {
            cmd.pre_exec(|| {
                prctl(1 /* PR_SET_PDEATHSIG */, 2 /* SIGINT */, 0, 0, 0);
                Ok(())
            });
        }
    }
    let mut ch = cmd.spawn().map_err(|e| anyhow!("cannot start {}: {e}", exe.display()))?;
    #[cfg(windows)]
    win_job::adopt(&ch);
    let tag = group_tag(&r.g.name);
    for (out, err) in [(ch.stdout.take().map(|o| Box::new(o) as Box<dyn std::io::Read + Send>), false), (ch.stderr.take().map(|e| Box::new(e) as Box<dyn std::io::Read + Send>), true)] {
        let Some(out) = out else { continue };
        let tag = tag.clone();
        let _ = std::thread::Builder::new().name(format!("split-{}", r.g.name)).spawn(move || {
            let mut br = std::io::BufReader::new(out);
            let mut buf = Vec::new();
            loop {
                buf.clear();
                match br.read_until(b'\n', &mut buf) {
                    Ok(0) | Err(_) => return,
                    Ok(_) => {
                        let s = String::from_utf8_lossy(&buf).trim_end_matches(['\r', '\n']).to_string();
                        // the child's line coloured by its kind (as any line of this process), the group in light blue in front
                        crate::conout::emit(err, format!("{tag}{}
", crate::color::paint(s)).into_bytes());
                    }
                }
            }
        });
    }
    lg(format!("group {} started (pid {}): GPU{} on {}", blue(&r.g.name), ch.id(), r.g.devs.iter().map(u32::to_string).collect::<Vec<_>>().join(",GPU"), blue(coins::spec(r.g.coin).name)));
    r.child = Some(ch);
    Ok(())
}

/// Windows: the groups' miners (and their per-GPU engine processes) live in a job object that is killed with the supervisor,
/// so a supervisor that dies (killed, crashed) never leaves miners behind (two miners on one card at the next start).  A
/// normal stop (Ctrl-C, console close) reaches every process of the console first; this is only the hard case.
#[cfg(windows)]
mod win_job {
    use std::os::windows::io::AsRawHandle;
    use std::sync::OnceLock;

    #[repr(C)]
    #[derive(Default)]
    struct Basic {
        per_process_user_time_limit: i64,
        per_job_user_time_limit: i64,
        limit_flags: u32,
        minimum_working_set_size: usize,
        maximum_working_set_size: usize,
        active_process_limit: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }
    #[repr(C)]
    #[derive(Default)]
    struct Extended {
        basic: Basic,
        io: [u64; 6],
        process_memory_limit: usize,
        job_memory_limit: usize,
        peak_process_memory_used: usize,
        peak_job_memory_used: usize,
    }
    const _: () = assert!(std::mem::size_of::<Extended>() == 144);
    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: i32 = 9;
    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x2000;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateJobObjectW(attrs: *mut std::ffi::c_void, name: *const u16) -> *mut std::ffi::c_void;
        fn SetInformationJobObject(job: *mut std::ffi::c_void, class: i32, info: *const std::ffi::c_void, len: u32) -> i32;
        fn AssignProcessToJobObject(job: *mut std::ffi::c_void, process: *mut std::ffi::c_void) -> i32;
    }

    struct Job(usize);
    static JOB: OnceLock<Option<Job>> = OnceLock::new();

    fn job() -> Option<usize> {
        JOB.get_or_init(|| unsafe {
            let h = CreateJobObjectW(std::ptr::null_mut(), std::ptr::null());
            if h.is_null() {
                return None;
            }
            let mut info = Extended::default();
            info.basic.limit_flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(h, JOB_OBJECT_EXTENDED_LIMIT_INFORMATION, &info as *const Extended as *const _, std::mem::size_of::<Extended>() as u32);
            // the handle is never closed: it closes when this process ends, and that is the point
            (ok != 0).then_some(Job(h as usize))
        })
        .as_ref()
        .map(|j| j.0)
    }

    pub fn adopt(ch: &std::process::Child) {
        let Some(j) = job() else {
            super::lg("cannot create the job object: a killed supervisor would leave its group miners running");
            return;
        };
        if unsafe { AssignProcessToJobObject(j as *mut _, ch.as_raw_handle() as *mut _) } == 0 {
            super::lg(format!("group miner pid {} not in the job object ({}): a killed supervisor would leave it running", ch.id(), std::io::Error::last_os_error()));
        }
    }
}

/// A pause before a group starts again: 10 s, 3 ends within 10 min = 10 min (rule 4a: a crash loop does not hammer the card).
fn backoff(exits: &mut Vec<Instant>) -> Duration {
    exits.retain(|t| t.elapsed() < Duration::from_secs(600));
    if exits.len() >= 3 { Duration::from_secs(600) } else { Duration::from_secs(10) }
}

#[cfg(unix)]
fn interrupt(ch: &Child) {
    let _ = Command::new("kill").args(["-INT", &ch.id().to_string()]).status();
}
#[cfg(not(unix))]
fn interrupt(_: &Child) {
    // Windows: Ctrl-C / the console close reaches every process of the console, the children included
}

/// The supervisor: runs until SIGINT / SIGTERM (Ctrl-C), then waits for every group's miner to put its clocks back.
pub fn run(spec: &str, argv: &[String]) -> Result<()> {
    std::hint::black_box(SPLIT_TAG);
    let count = crate::nvml::Nvml::load().ok().and_then(|n| n.device_count().ok());
    let groups = parse_spec(spec, count)?;
    if groups.len() == 1 {
        lg(format!("one group only ({}): the same as --algo {} --devices ...", blue(&groups[0].name), blue(coins::spec(groups[0].coin).name)));
    }
    if let Some(d) = argv.iter().position(|a| a == "--devices" || a == "--device") {
        bail!("--split sets the cards of every group: leave out {} (found at argument {})", argv[d], d + 1);
    }
    for g in &groups {
        lg(format!("group {}: GPU{} -> {} ({})", blue(&g.name), g.devs.iter().map(u32::to_string).collect::<Vec<_>>().join(",GPU"), blue(coins::spec(g.coin).name), coins::spec(g.coin).unit));
    }
    recover_stale(argv, &groups);
    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = stop.clone();
        ctrlc::set_handler(move || {
            if !stop.swap(true, Ordering::SeqCst) {
                eprintln!("split: stopping every group (each puts its clocks / offsets back)...");
            }
        })?;
    }
    let mut runs: Vec<Run> = groups.iter().enumerate().map(|(i, g)| Run { g: g.clone(), args: child_args(argv, g, i), child: None, start_at: Instant::now(), exits: Vec::new() }).collect();
    while !stop.load(Ordering::SeqCst) {
        for r in runs.iter_mut() {
            if let Some(ch) = r.child.as_mut() {
                if let Ok(Some(st)) = ch.try_wait() {
                    r.child = None;
                    if stop.load(Ordering::SeqCst) {
                        continue;
                    }
                    r.exits.push(Instant::now());
                    let wait = backoff(&mut r.exits);
                    r.start_at = Instant::now() + wait;
                    lg(format!("group {} ended ({st}); the other groups mine on; it starts again in {} s{}", blue(&r.g.name), wait.as_secs(), if wait.as_secs() >= 600 { " (3 ends in 10 min: PAUSED)" } else { "" }));
                }
            } else if Instant::now() >= r.start_at {
                if let Err(e) = spawn(r) {
                    r.exits.push(Instant::now());
                    let wait = backoff(&mut r.exits);
                    r.start_at = Instant::now() + wait;
                    lg(format!("group {} FAILED to start: {e:#}; again in {} s", blue(&r.g.name), wait.as_secs()));
                }
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    // stop: the children got the same Ctrl-C / SIGINT (one console / process group); a child that still runs after 5 s
    // gets one of its own, after 120 s it is killed (its markers are recovered at the next start of that group)
    let t0 = Instant::now();
    let mut nudged = false;
    loop {
        let alive: Vec<&mut Run> = runs.iter_mut().filter_map(|r| r.child.as_mut().is_some_and(|c| matches!(c.try_wait(), Ok(None))).then_some(r)).collect();
        if alive.is_empty() {
            break;
        }
        if !nudged && t0.elapsed() > Duration::from_secs(5) {
            nudged = true;
            for r in alive {
                if let Some(c) = r.child.as_ref() {
                    interrupt(c);
                }
            }
            continue;
        }
        if t0.elapsed() > Duration::from_secs(120) {
            for r in alive {
                lg(format!("group {} does not stop: killed (its clocks / offsets are put back at its next start)", r.g.name));
                if let Some(c) = r.child.as_mut() {
                    let _ = c.kill();
                    let _ = c.wait();
                }
            }
            break;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    lg("every group stopped");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn spec_groups_and_rest() {
        let g = parse_spec("neb:0,1;pearl:rest", Some(6)).unwrap();
        assert_eq!(g.len(), 2);
        assert_eq!((g[0].coin, g[0].name.as_str(), g[0].devs.clone()), (Coin::Neb, "neb", vec![0, 1]));
        assert_eq!((g[1].coin, g[1].name.as_str(), g[1].devs.clone()), (Coin::Pearl, "pearl", vec![2, 3, 4, 5]));
        let g = parse_spec("ting:0-2; neb:3 ;ting:4", None).unwrap();
        assert_eq!(g.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(), ["ting", "neb", "ting2"]);
        assert_eq!(g[0].devs, vec![0, 1, 2]);
        assert_eq!(parse_spec("nebhash:0;PRL:1", Some(2)).unwrap()[1].coin, Coin::Pearl);
    }

    #[test]
    fn spec_separators_are_equal() {
        let want = parse_spec("neb:0,1;pearl:rest", Some(4)).unwrap();
        for s in ["neb:0,1 pearl:rest", "neb:0 1 pearl:rest", "neb:0;1;pearl:rest", "neb:0,1,pearl:rest", " neb: 0 , 1 ;  pearl: rest ", "neb:0-1 pearl:*"] {
            assert_eq!(parse_spec(s, Some(4)).unwrap(), want, "{s:?}");
        }
        // unquoted on the command line (HiveOS extra config): the words after --split are the spec
        let a = v("--split neb:0,1 pearl:rest --pool neb=x:1");
        assert_eq!(split_value(&a).as_deref(), Some("neb:0,1 pearl:rest"));
        assert_eq!(split_value(&v("--split=neb:0 1 ting:2 --auto-uv")).as_deref(), Some("neb:0 1 ting:2"));
        let g = parse_spec("neb:0,1 pearl:rest", Some(3)).unwrap();
        assert_eq!(child_args(&a, &g[0], 0), v("--pool x:1 --algo neb --devices 0,1"));
        assert_eq!(child_args(&a, &g[1], 1), v("--algo pearl --devices 2"));
        assert!(parse_spec("0,1 neb:2", Some(3)).is_err(), "cards before a coin");
        assert!(parse_spec("neb:0 rest", Some(3)).is_err(), "rest with numbers");
    }

    #[test]
    fn spec_errors() {
        assert!(parse_spec("neb:0,1;pearl:1", Some(4)).is_err(), "one card in two groups");
        assert!(parse_spec("neb:0;pearl:rest;ting:rest", Some(4)).is_err(), "two rests");
        assert!(parse_spec("neb:0-3;pearl:rest", Some(4)).is_err(), "nothing left");
        assert!(parse_spec("neb:7", Some(4)).is_err(), "no such GPU");
        assert!(parse_spec("doge:0", Some(4)).is_err(), "unknown coin");
        assert!(parse_spec("neb:rest", None).is_err(), "rest without a count");
        assert!(parse_spec("", Some(2)).is_err());
        assert!(parse_spec("neb", Some(2)).is_err());
    }

    #[test]
    fn child_args_tags_and_per_group() {
        let argv = v("--split neb:0,1;pearl:2 --pool neb=stratum+tcp://neb.suprnova.cc:7804 --wallet neb=neb1q.w --pool pearl=pearl-eu2.luckypool.io:3360 --wallet pearl=prl1x.w --auto-uv --lock-cclock neb=1680 --lock-cclock=pearl=1410 --coin-price neb=0.1,pearl=2 --password x");
        let g = parse_spec("neb:0,1;pearl:2", Some(3)).unwrap();
        let n = child_args(&argv, &g[0], 0);
        assert_eq!(n, v("--pool stratum+tcp://neb.suprnova.cc:7804 --wallet neb1q.w --auto-uv --lock-cclock 1680 --coin-price neb=0.1,pearl=2 --password x --algo neb --devices 0,1"));
        let p = child_args(&argv, &g[1], 1);
        assert_eq!(p, v("--pool pearl-eu2.luckypool.io:3360 --wallet prl1x.w --auto-uv --lock-cclock 1410 --coin-price neb=0.1,pearl=2 --password x --algo pearl --devices 2"));
    }

    #[test]
    fn child_args_split_args_api_and_removed() {
        let mut argv = v("--split neb:0;ting:1 --algo pearl --devices 0,1 --api 127.0.0.1:4050");
        argv.extend(["--split-args".into(), "neb=--table-mclock 810 --no-color".into(), "--split-args".into(), "ting=\"--no-group\"".into()]);
        let g = parse_spec("neb:0;ting:1", Some(2)).unwrap();
        assert_eq!(child_args(&argv, &g[0], 0), v("--api 127.0.0.1:4050 --table-mclock 810 --no-color --algo neb --devices 0"));
        assert_eq!(child_args(&argv, &g[1], 1), v("--api 127.0.0.1:4051 --no-group --algo ting --devices 1"));
        // Ting's default API port (4068) moves for a second group
        let g = parse_spec("ting:0;ting:1", Some(2)).unwrap();
        assert_eq!(child_args(&v("--split ting:0;ting:1"), &g[1], 1), v("--api-port 4069 --algo ting --devices 1"));
        assert_eq!(child_args(&v("--split ting:0;ting:1"), &g[0], 0), v("--algo ting --devices 0"));
    }

    #[test]
    fn blue_names_keep_the_line_colour() {
        assert_eq!(blue_if(true, "[neb]"), "[96m[neb][0m");
        assert_eq!(blue_if(false, "[neb]"), "[neb]");
        let l = format!("[33mgroup {} ended, PAUSED[0m", blue_if(true, "neb"));
        assert_eq!(keep_outer(l), "[33mgroup [96mneb[0m[33m ended, PAUSED[0m");
        assert_eq!(keep_outer("plain".into()), "plain");
    }

    #[test]
    fn tag_rules() {
        assert_eq!(tag_of("neb=x"), Some((Coin::Neb, "x")));
        assert_eq!(tag_of("x=y"), None);
        assert_eq!(tag_of("stratum+tcp://a:1"), None);
        assert_eq!(tag_of("http://a/?neb=1"), None);
        assert_eq!(words("--a 1 'b c' \"d\""), vec!["--a", "1", "b c", "d"]);
    }

    #[test]
    fn markers_by_group() {
        let d = std::env::temp_dir().join(format!("ffg-split-test-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        for f in ["clocks-restore.json", "clocks-restore.neb.json", "uv-restore.pearl.json", "uv.json", "ffgn-offset-restore.neb2.json"] {
            std::fs::write(d.join(f), "{}").unwrap();
        }
        assert_eq!(marker_groups(&d).into_iter().collect::<Vec<_>>(), ["neb", "neb2", "pearl"]);
        let _ = std::fs::remove_dir_all(&d);
    }
}
