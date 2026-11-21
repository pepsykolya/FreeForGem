// FreeForGem — see LICENSE
//! The command line of a Ting / Daler (DLR) run (1.8.0): FFGT 0.2.2's option set and dispatch (it used FreeForGem's main.rs
//! parser), moved into the Ting module so that an FFGT command line works in FreeForGem with `--algo ting` (or unchanged with a
//! crypto-connection.net pool, a `ting://` gate URL, `--node-dir`), and the Pearl parser and loop of main.rs are not touched.
//! The internal child processes of Ting (one engine process per GPU `--ting-engine DEV`, the tuning check `--ting-uv-check`)
//! come through here too.

use crate::ting;
use crate::{autolock, color, failover, idle, templimit, update, uv};
use anyhow::{Result, bail, ensure};

struct Args {
    pools: Vec<String>,
    wallet: String,
    worker: String,
    password: String,
    minutes: Option<f64>,
    devices: Option<Vec<i32>>,
    uv: uv::Opts,
    lock_cclock: Option<String>,
    lock_mclock: Option<String>,
    moffset: Option<String>,
    no_mem_lock: bool,
    backend: Option<String>,
    pool_given: bool,
    hash_bench: Option<f64>,
    idle: idle::Opts,
    temp: templimit::Opts,
    node: Option<String>,            // --node HOST:PORT (Ting control channel, loopback)
    node_dir: Option<String>,        // --node-dir DATADIR (its control.token)
    node_token_file: Option<String>, // --node-token-file FILE
    node_remote: bool,               // --node-allow-remote
    blocks: Option<u64>,             // --blocks N: stop after N accepted blocks (tests)
    gate: Option<String>,            // --gate CONFIG.json: run the gate instead of mining
    api: Option<String>,             // --api-port N / --api-bind ADDR:PORT; None = off
    single_process: bool,            // --single-process
}

pub const HELP: &str = "\
FreeForGem - Ting / Daler (DLR) mining, the Ting module (from FFGT 0.2.2). NVIDIA GTX 10 and newer (CUDA), AMD (OpenCL).

USAGE:
    freeforgem --pool crypto-connection.net:4073 --wallet <TING ACCOUNT, 64 hex>[.<worker>] [OPTIONS]   pool
    freeforgem --algo ting --pool <gate HOST:PORT> --wallet any.<worker> --password <rig password>      through an ffgt-gate
               (or --pool ting://HOST:PORT without --algo)
    freeforgem --algo ting --node 127.0.0.1:18440 --node-dir <the node's data directory> [OPTIONS]      solo on a local node
    freeforgem --algo ting --gate gate.json                                                              run the gate
    freeforgem --algo ting --self-test [--devices N]        freeforgem --algo ting --hash-bench SECS [--devices N]

TING:
    --pool HOST:PORT     Ting stratum pool (plain TCP; crypto-connection.net:4073) or an ffgt-gate; a list (--pool A,B) =
                         failover / failback (the first is the primary, back to it after --failback-minutes, default 10)
    --wallet ACCOUNT[.WORKER]  the Ting account (64 hex digits; a `&net=...` tail of Ting ID is cut off); --worker NAME
    --password TEXT      x for a pool (default), the rig's password for an ffgt-gate
    --node HOST:PORT     solo on a local Ting node (control channel, HMAC token): the blocks pay the node's
                         `ting miner payout <account>`; --node-dir DIR (its control.token) or --node-token-file FILE;
                         --node-allow-remote for a tunnel endpoint on another address; --blocks N stops after N blocks
    --gate CONFIG.json   run the gate: a stratum server for several rigs, each rig (password) mapped to a node / payout account
    --backend cuda|opencl  default: CUDA when it sees a GPU, else OpenCL
    --no-group           no template grouping (default: up to 7 templates of one tip / jobs of one height searched together,
                         about +40 % solo and +25 % on the pool on RTX 30)
    --no-mem-lock        leave the memory clock alone (default: locked at the lowest clock that keeps the full core range,
                         810 MHz on RTX 30: SHA-256 does not use the video memory, about -10 % power; a memory lock of the HiveOS
                         OC profile, --lock-mclock or --moffset is kept as given; CMP 50HX: the driver has no memory lock)
    --single-process     every GPU's engine in the miner process. Default: one engine process per GPU, so a CUDA fault on one
                         GPU restarts only that GPU in a few seconds while the others keep mining (FFG_SINGLE_PROCESS=1 too)
    --self-test          the CPU conformance vector, the library's job build, every GPU's candidate set (single and grouped
                         kernels) vs the CPU; exit
    --hash-bench SECS    kernel throughput per GPU (GH/s, NVML watts, GH/W); exit
    --api-port N         read-only HTTP status /summary /gpus /metrics (default 4068 on 127.0.0.1, 0 = off); --api-bind ADDR:PORT
    Every candidate is re-hashed on the CPU before it is sent. Hash rate unit: GH/s, efficiency GH/W.
    Dev fee 1 %: one 60 s slot in every 100 minutes on crypto-connection.net, on a connection of its own; solo without that
    pool: the node's blocks pay the developer for the slot (crash-safe marker).

OPTIONS (as for the other coins, see freeforgem --help and MANUAL.md):
    --devices 0,1   --minutes N   --failback-minutes N   --noshare-minutes M   --reject-minutes K
    --lock-cclock N (RTX 3070: 1500 = best GH/W, 1800 = more GH/s)   --lock-mclock N   --moffset N
    --auto-uv   --uv-offset N   --uv-max N (default per card: Turing +225, CMP 50HX +330, CMP 30/40HX +150, others +495)
    --uv-min N   --uv-step N   --uv-coarse N   --uv-keep-last   --uv-parallel N|auto   --uv-unpause   --uv-reset   --uv-reset-all
    --uv-dir DIR   --uv-reval-hours H   --uv-warmup S   --uv-step-secs S   --uv-confirm-secs S   --auto-uv-dry-run
    --auto-lock   --auto-lock-range A-B[:S]|L,L..   --auto-lock-max-loss PCT   --auto-lock-goal eff|speed   --auto-lock-reset
    --temp-limit C[,C]   --temp-stop C2[,C2]   --idle   --idle-minutes N   --active MODE   --idle-pause-for L   --idle-cpu PCT
    --idle-ignore L   --idle-gpu PCT   --no-update-check   --check-update   --no-color   --help   --version
    auto-uv profiles are kept per GPU and core lock in uv.json section \"ting\" (FFGT's are imported once); a crash while
    mining at a tuned offset goes 2 steps below it, without tuning from scratch.
";

fn parse_devices(s: &str) -> Result<Vec<i32>> {
    let mut v = Vec::new();
    for p in uv::list_parts(s).into_iter().filter(|p| !p.is_empty()) {
        let d: i32 = p.parse().map_err(|_| anyhow::anyhow!("bad device id {p:?}"))?;
        ensure!(d >= 0, "bad device id {p:?}");
        // test hook: FFG_TEST_DUP_DEVICES=1 keeps a repeated id = two workers on one GPU (the per-GPU isolation test)
        if !v.contains(&d) || std::env::var("FFG_TEST_DUP_DEVICES").is_ok_and(|x| x == "1") {
            v.push(d);
        }
    }
    ensure!(!v.is_empty(), "--devices needs at least one device id");
    Ok(v)
}

/// The Ting run of FreeForGem: main.rs calls it before its own parser when coins::module_coin says Ting.
pub fn main() -> Result<()> {
    // colours: never in the child processes (the parent parses their lines), not with --no-color / NO_COLOR
    if std::env::args().any(|x| x == "--ting-uv-check" || x == "--no-color" || x == ting::iso::ENGINE_FLAG) {
        color::disable();
    }
    // an engine process of one GPU (started by the miner itself, iso.rs)
    if std::env::args().nth(1).as_deref() == Some(ting::iso::ENGINE_FLAG) {
        let dev = std::env::args().nth(2).and_then(|d| d.parse().ok()).unwrap_or(0);
        let v: Vec<String> = std::env::args().collect();
        let backend = v.iter().position(|x| x == "--backend").and_then(|i| v.get(i + 1)).and_then(|b| ting::parse_backend(Some(b)).ok().flatten());
        crate::conout::exit(ting::iso::child_main(dev, backend));
    }
    if std::env::args().any(|x| x == "--ting-uv-check") {
        let v: Vec<String> = std::env::args().collect();
        let get = |k: &str| v.iter().position(|x| x == k).and_then(|i| v.get(i + 1)).cloned();
        let dev = get("--devices").and_then(|s| s.split(',').next().and_then(|x| x.trim().parse().ok())).unwrap_or(0);
        let secs = get("--uv-secs").and_then(|s| s.parse().ok()).unwrap_or(15);
        let backend = ting::parse_backend(get("--backend").as_deref()).ok().flatten();
        ting::tune::set_tune_group(get("--uv-group").and_then(|s| s.parse().ok()).unwrap_or(1));
        crate::conout::exit(ting::tune::check_child(dev, secs, &get("--uv-ref").unwrap_or_default(), backend));
    }
    // per-GPU list options may be given without quotes (`--lock-cclock 1500 1650`): the numbers that follow are joined
    const LIST_OPTS: [&str; 10] = ["--devices", "--device", "--uv-offset", "--uv-max", "--uv-min", "--temp-limit", "--temp-stop", "--lock-cclock", "--lock-mclock", "--moffset"];
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut joined: Vec<String> = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        joined.push(raw[i].clone());
        if LIST_OPTS.contains(&raw[i].as_str()) && i + 1 < raw.len() {
            let mut v = raw[i + 1].clone();
            i += 2;
            while i < raw.len() && uv::is_list_value(&raw[i]) {
                v.push(' ');
                v.push_str(&raw[i]);
                i += 1;
            }
            joined.push(v);
            continue;
        }
        i += 1;
    }
    let argv_joined = joined.clone(); // --auto-lock builds the command line of its sweep's miner processes from it
    let mut args = joined.into_iter();
    let mut a = Args {
        pools: vec![],
        wallet: String::new(),
        worker: "rig".into(),
        password: "x".into(),
        minutes: None,
        devices: None,
        uv: uv::Opts::default(),
        lock_cclock: None,
        lock_mclock: None,
        moffset: None,
        no_mem_lock: false,
        backend: None,
        pool_given: false,
        hash_bench: None,
        idle: idle::Opts::default(),
        temp: templimit::Opts { limit: uv::PerGpu::Unset, stop: uv::PerGpu::Unset },
        node: None,
        node_dir: None,
        node_token_file: None,
        node_remote: false,
        blocks: None,
        gate: None,
        api: Some("127.0.0.1:4068".into()),
        single_process: false,
    };
    let mut test = false;
    let mut update_check = true;
    let mut al = autolock::Opts::default();
    while let Some(arg) = args.next() {
        let mut val = || args.next().ok_or_else(|| anyhow::anyhow!("missing value for {arg}"));
        match arg.as_str() {
            "--help" | "-h" => {
                print!("{HELP}");
                return Ok(());
            }
            "--version" | "-V" => {
                println!("freeforgem {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--self-test" => test = true,
            "--pool" => {
                let v = val()?;
                let list = failover::split_list(&v);
                ensure!(!list.is_empty(), "--pool needs an address");
                a.pools.extend(list);
                a.pool_given = true;
            }
            "--noshare-minutes" => failover::set_watchdog(Some(failover::parse_minutes(&val()?).map_err(|e| anyhow::anyhow!(e.replace("--failback-minutes", "--noshare-minutes")))?), None),
            "--reject-minutes" => failover::set_watchdog(None, Some(failover::parse_minutes(&val()?).map_err(|e| anyhow::anyhow!(e.replace("--failback-minutes", "--reject-minutes")))?)),
            "--failback-minutes" => failover::set_failback_minutes(failover::parse_minutes(&val()?).map_err(anyhow::Error::msg)?),
            // the coin is chosen before this parser (coins::module_coin): here only Ting's names are valid
            "--algo" | "--coin" => {
                let v = val()?.to_ascii_lowercase();
                ensure!(crate::coins::by_name(&v) == Some(crate::coins::Coin::Ting), "--algo {v:?} together with Ting options (a Ting pool URL, --node-dir): one coin per run (--algo ting)");
            }
            "--node" => a.node = Some(val()?),
            "--node-dir" => a.node_dir = Some(val()?),
            "--node-token-file" => a.node_token_file = Some(val()?),
            "--node-allow-remote" => a.node_remote = true,
            "--gate" => a.gate = Some(val()?),
            "--api-port" => {
                let v = val()?;
                a.api = if v == "0" || v == "off" { None } else { Some(format!("127.0.0.1:{}", v.parse::<u16>().map_err(|_| anyhow::anyhow!("--api-port needs a port number (0 = off)"))?)) };
            }
            "--api-bind" => a.api = Some(val()?),
            "--blocks" => a.blocks = Some(val()?.parse().map_err(|_| anyhow::anyhow!("--blocks needs a number"))?),
            "--no-group" => ting::set_grouping(false),
            "--no-mem-lock" => a.no_mem_lock = true,
            "--single-process" => a.single_process = true,
            "--hash-bench" => a.hash_bench = Some(val()?.parse().map_err(|_| anyhow::anyhow!("--hash-bench needs a number of seconds"))?),
            "--wallet" => a.wallet = val()?,
            "--worker" => a.worker = val()?,
            "--password" => a.password = val()?,
            "--minutes" => {
                let m: f64 = val()?.parse()?;
                ensure!(m > 0.0, "--minutes must be positive");
                a.minutes = Some(m);
            }
            "--devices" | "--device" => a.devices = Some(parse_devices(&val()?)?),
            "--auto-uv" => a.uv.auto = true,
            "--auto-uv-dry-run" => {
                a.uv.auto = true;
                a.uv.dry_run = true;
            }
            "--uv-reset" => a.uv.reset = true,
            "--uv-reset-all" => a.uv.reset_all = true,
            "--no-color" => color::disable(),
            "--uv-offset" => a.uv.fixed = uv::PerGpu::parse(&val()?, "--uv-offset")?,
            "--uv-max" => a.uv.max_offset = uv::PerGpu::parse(&val()?, "--uv-max")?,
            "--uv-min" => a.uv.min_offset = uv::PerGpu::parse(&val()?, "--uv-min")?,
            "--uv-step" => a.uv.step = val()?.trim().trim_start_matches('+').parse().map_err(|_| anyhow::anyhow!("--uv-step needs a positive integer (MHz)"))?,
            "--uv-keep-last" => a.uv.keep_last_good = true,
            "--uv-parallel-reset" | "--uv-unpause" => a.uv.parallel_reset = true,
            "--uv-parallel" => {
                let v = val()?;
                a.uv.parallel = if v.eq_ignore_ascii_case("auto") {
                    None
                } else {
                    let n: usize = v.trim().parse().map_err(|_| anyhow::anyhow!("--uv-parallel needs a number of GPUs (1..64) or auto"))?;
                    ensure!((1..=64).contains(&n), "--uv-parallel {n}: 1..64 GPUs or auto");
                    Some(n)
                };
            }
            "--temp-limit" => a.temp.limit = uv::PerGpu::parse(&val()?, "--temp-limit")?,
            "--temp-stop" => a.temp.stop = uv::PerGpu::parse(&val()?, "--temp-stop")?,
            "--uv-coarse" => {
                let c: i32 = val()?.trim().trim_start_matches('+').parse().map_err(|_| anyhow::anyhow!("--uv-coarse needs a non-negative integer (MHz)"))?;
                ensure!((0..=uv::HARD_CAP).contains(&c), "--uv-coarse must be between 0 and {}", uv::HARD_CAP);
                a.uv.coarse = Some(c);
            }
            "--lock-cclock" => a.lock_cclock = Some(val()?),
            "--lock-mclock" => a.lock_mclock = Some(val()?),
            "--moffset" => a.moffset = Some(val()?),
            "--uv-dir" => a.uv.dir = Some(val()?.into()),
            "--uv-reval-hours" => a.uv.reval_hours = val()?.parse()?,
            "--uv-warmup" => a.uv.warmup_secs = val()?.parse()?,
            "--uv-step-secs" => a.uv.step_secs = val()?.parse()?,
            "--uv-confirm-secs" => a.uv.confirm_secs = val()?.parse()?,
            "--no-update-check" => update_check = false,
            "--check-update" => crate::conout::exit(update::check_now()),
            "--idle" => a.idle.on = true,
            "--idle-minutes" => {
                let m: f64 = val()?.parse().map_err(|_| anyhow::anyhow!("--idle-minutes needs a number of minutes"))?;
                ensure!(m > 0.0, "--idle-minutes must be positive");
                a.idle.minutes = m;
            }
            "--active" => a.idle.active_pct = idle::parse_active(&val()?)?,
            "--idle-pause-for" => a.idle.pause_for = idle::parse_list(&val()?),
            "--idle-ignore" => a.idle.ignore = idle::parse_list(&val()?),
            "--idle-cpu" => a.idle.cpu_pct = idle::parse_pct(&val()?, "--idle-cpu")?,
            "--idle-gpu" => a.idle.gpu_pct = idle::parse_pct(&val()?, "--idle-gpu")?,
            "--backend" => a.backend = Some(val()?),
            x if x.starts_with("--auto-lock") || x == "--power-price" || x == "--coin-price" || x == "--pool-fee" => {
                if !autolock::arg(&mut al, x, &mut val)? {
                    bail!("unknown argument {arg} (see --algo ting --help)");
                }
            }
            "--pool-check" | "--gpu-state" => bail!("{arg} is not available for Ting in 1.8.0"),
            _ => bail!("unknown argument {arg} (see --algo ting --help)"),
        }
    }
    // --auto-lock (off unless asked): the core lock per GPU by measurement (autolock.rs); no profit goal for Ting yet
    if al.sub && !al.on {
        println!("auto-lock: WARNING --auto-lock-* options (and --power-price, --coin-price, --pool-fee) are ignored without --auto-lock");
    }
    ensure!(
        al.goal != autolock::Goal::Profit && !(al.tariff.is_some() || al.coin_price.is_some() || al.pool_fee.is_some()),
        "--auto-lock-goal profit / --power-price / --coin-price / --pool-fee are not available for Ting in 1.8.0: use --auto-lock-goal eff or speed"
    );
    if al.on && !test {
        autolock::times_from_env(&mut al);
        ensure!(!a.idle.on, "--auto-lock and --idle exclude each other (the sweep measures steady mining)");
        ensure!(!a.uv.dry_run, "--auto-lock and --auto-uv-dry-run exclude each other");
        if !a.uv.auto && !a.uv.fixed.is_set() {
            println!("auto-lock: --auto-lock turns on --auto-uv: every lock is measured at its own undervolt profile");
            a.uv.auto = true;
            al.add_uv = true;
        }
    }
    autolock::set(al, argv_joined);
    if update_check && !test && a.hash_bench.is_none() && a.gate.is_none() && (!a.wallet.is_empty() || a.node.is_some()) {
        update::start();
    }
    a.uv.validate()?;
    if let Some(d) = &a.devices {
        a.uv.check_counts(d.len())?;
    }
    if let Some(cfg) = &a.gate {
        return ting::gate::run(cfg);
    }
    idle::start(&a.idle)?;
    let backend = ting::parse_backend(a.backend.as_deref().filter(|b| *b != "auto"))?;
    if test {
        return ting::self_test(&a.devices, backend);
    }
    if let Some(s) = a.hash_bench {
        return ting::hash_bench(&a.devices, backend, s);
    }
    if (a.uv.reset || a.uv.reset_all) && !a.pool_given && a.node.is_none() && a.node_dir.is_none() {
        if a.uv.reset_all {
            uv::reset(&a.uv);
        } else {
            println!("auto-uv: --uv-reset forgets the profile of the clocks a mining run has (at its start, with --auto-uv); --uv-reset-all forgets every saved result now");
        }
        return Ok(());
    }
    ensure!(a.pool_given || a.node.is_some() || a.node_dir.is_some(), "Ting needs --pool HOST:PORT (crypto-connection.net:4073 or an ffgt-gate) or --node 127.0.0.1:18440 --node-dir DIR (solo; see --algo ting --help)");
    ensure!(!a.pool_given || (a.node.is_none() && a.node_dir.is_none()), "--pool (pool / ffgt-gate) and --node / --node-dir (solo on a local node) exclude each other");
    import_ffgt_state(&uv::state_dir(&a.uv), &ffgt_state_dirs());
    if let Some(b) = &a.api {
        ting::api::start(b);
    }
    let (account, worker) = match a.wallet.trim().rsplit_once('.') {
        Some((acc, w)) => (acc.to_string(), w.to_string()),
        None => (a.wallet.trim().to_string(), a.worker.clone()),
    };
    ting::mine(ting::Opts {
        pools: if a.pool_given { a.pools.clone() } else { vec![] },
        account: ting::clean_account(&account),
        worker,
        password: a.password.clone(),
        failback_min: failover::opts().failback_min,
        node: a.node,
        node_dir: a.node_dir,
        node_token_file: a.node_token_file,
        node_remote: a.node_remote,
        backend,
        minutes: a.minutes,
        blocks: a.blocks,
        devices: a.devices,
        lock_cclock: a.lock_cclock,
        lock_mclock: a.lock_mclock,
        moffset: a.moffset,
        no_mem_lock: a.no_mem_lock,
        temp: a.temp,
        uv: a.uv,
        single_process: a.single_process,
    })
}

/// The state folders of FFGT a rig may come from: HiveOS `/hive-config/ffgt` (FFGT's h-run.sh), $FFGT_STATE_DIR, an `ffgt*`
/// folder next to this one (Windows / plain Linux).
fn ffgt_state_dirs() -> Vec<std::path::PathBuf> {
    let mut v: Vec<std::path::PathBuf> = vec!["/hive-config/ffgt".into()];
    if let Some(d) = std::env::var_os("FFGT_STATE_DIR") {
        v.insert(0, d.into());
    }
    if let Some(parent) = std::env::current_exe().ok().and_then(|e| e.parent().and_then(|d| d.parent()).map(|d| d.to_path_buf())) {
        if let Ok(rd) = std::fs::read_dir(&parent) {
            let mut x: Vec<_> = rd.flatten().map(|e| e.path()).filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().to_ascii_lowercase().starts_with("ffgt"))).collect();
            x.sort();
            v.extend(x.into_iter().rev());
        }
    }
    v
}

/// A rig that mined Ting with FFGT keeps its tuned Ting offsets: on the first Ting start the "ting" / "ting_trying" sections of
/// FFGT's uv.json are copied into this miner's uv.json (only when it has no "ting" section yet; nothing else is touched, no
/// profile is tuned again; the profiles of FFGT 0.2.2 have the same kernel id and are used as they are).  The folder imported from.
pub fn import_ffgt_state(dir: &std::path::Path, from: &[std::path::PathBuf]) -> Option<std::path::PathBuf> {
    let sec = crate::profiles::TING;
    let dst = dir.join("uv.json");
    let mut cur: serde_json::Value = std::fs::read_to_string(&dst).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_else(|| serde_json::json!({}));
    if cur.get(sec.profiles).is_some_and(|v| v.as_object().is_some_and(|o| !o.is_empty())) {
        return None;
    }
    for d in from {
        let src = d.join("uv.json");
        if src == dst {
            continue;
        }
        let Some(v) = std::fs::read_to_string(&src).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()) else { continue };
        let Some(t) = v.get(sec.profiles).filter(|x| x.as_object().is_some_and(|o| !o.is_empty())) else { continue };
        if !cur.is_object() {
            cur = serde_json::json!({});
        }
        cur[sec.profiles] = t.clone();
        if let Some(t) = v.get(sec.trying) {
            cur[sec.trying] = t.clone();
        }
        let _ = std::fs::create_dir_all(dir);
        match uv::write_atomic(&dst, &serde_json::to_string_pretty(&cur).unwrap_or_default()) {
            Ok(()) => {
                println!("auto-uv: imported the Ting undervolt profiles of FFGT from {} into {} (no GPU is tuned again)", src.display(), dst.display());
                return Some(d.clone());
            }
            Err(e) => {
                eprintln!("auto-uv: cannot import the Ting profiles of FFGT from {} ({e:#})", src.display());
                return None;
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffgt_ting_profiles_are_imported_once_and_keep_the_other_coins() {
        let base = std::env::temp_dir().join(format!("ffg-tingimp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (ffg, ffgt) = (base.join("freeforgem"), base.join("ffgt"));
        std::fs::create_dir_all(&ffg).unwrap();
        std::fs::create_dir_all(&ffgt).unwrap();
        std::fs::write(ffg.join("uv.json"), r#"{"version":2,"profiles":{"GPU-a":{"1805":{"offset":300}}},"neb":{"GPU-a":{"1680":{"offset":210}}}}"#).unwrap();
        std::fs::write(ffgt.join("uv.json"), r#"{"version":2,"profiles":{"GPU-a":{"1805":{"offset":999}}},"ting":{"GPU-a":{"1800":{"offset":165}}},"ting_trying":{"GPU-a":{"at":195}}}"#).unwrap();
        assert_eq!(import_ffgt_state(&ffg, &[base.join("none"), ffgt.clone()]), Some(ffgt.clone()));
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(ffg.join("uv.json")).unwrap()).unwrap();
        assert_eq!(v["ting"]["GPU-a"]["1800"]["offset"], 165);
        assert_eq!(v["ting_trying"]["GPU-a"]["at"], 195);
        assert_eq!(v["profiles"]["GPU-a"]["1805"]["offset"], 300, "FFG's Pearl profiles stay (FFGT's Pearl section is not copied)");
        assert_eq!(v["neb"]["GPU-a"]["1680"]["offset"], 210);
        std::fs::write(ffgt.join("uv.json"), r#"{"ting":{"GPU-a":{"1800":{"offset":1}}}}"#).unwrap();
        assert_eq!(import_ffgt_state(&ffg, &[ffgt.clone()]), None, "once");
        let _ = std::fs::remove_dir_all(&base);
    }
}
