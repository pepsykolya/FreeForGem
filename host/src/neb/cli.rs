// FreeForGem — see LICENSE
//! The command line of a NEB run (1.8.0): FFGN 1.0.11's `main.rs` (its option set, help and dispatch), moved into the NEB module
//! so that a NEB command line of FFGN works unchanged in FreeForGem (`--algo neb` / `--coin neb`, or a NEB pool URL, `--node`,
//! `--gate`), and the Pearl parser and loop of main.rs are not touched.  The internal child processes of NEB (one engine process
//! per GPU `--neb-engine DEV`, the tuning check `--neb-uv-check`) come through here too.

use crate::{autolock, color, failover, idle, templimit, update, uv};
use crate::neb;
use anyhow::{Result, bail, ensure};

struct Args {
    pools: Vec<String>, // every --pool entry (a comma / space separated list or repeated --pool): primary first, then backups
    wallet: String,
    worker: String,
    password: String,
    minutes: Option<f64>,
    devices: Option<Vec<i32>>,
    uv: uv::Opts,
    lock_cclock: Option<String>, // --lock-cclock: core clock MHz (one value, or one per device)
    lock_mclock: Option<String>, // --lock-mclock: memory clock MHz
    table_mclock: Option<String>, // --table-mclock: memory clock MHz while the digest-table path runs (NEB, 1.0.11)
    moffset: Option<String>,     // --moffset: memory clock offset MHz (may be negative)
    power_limit: uv::PerGpu,     // FFGN 1.0.12: --power-limit / --pl: power limit W (one value, or one per device); optional, never on by default
    backend: Option<String>,     // --backend cuda|cpu
    pool_given: bool,
    pool_check: bool,            // --pool-check: log in to every --pool entry (login + a job), print the verdicts, exit
    hash_bench: Option<f64>,     // --hash-bench SECS
    idle: idle::Opts,
    temp: templimit::Opts,
    node: Option<String>,            // --node HOST:PORT[,..]: the work service of a NEB node (solo)
    blocks: Option<u64>,             // --blocks N: stop after N accepted blocks (tests)
    dataset: Option<String>,         // neb-datasets.bin
    cpu_threads: usize,              // --backend cpu: threads (0 = all)
    gate: bool,                      // run as ffgn-gate (work service of a node -> neb-pool/1 for rigs)
    gate_listen: String,
    gate_token_file: Option<String>,
    gate_bits: u32,
    api: Option<String>,             // --api [HOST:]PORT
    allow_other_dataset: bool,       // development: a dataset file of neither network
    keep_hive_mem: bool,             // --keep-hive-mem (D-007)
    single_process: bool,            // 1.0.11: --single-process
}

const HELP: &str = "\
FreeForGem - NEB (Neural Engine Byte, nebhash v2) mining, the NEB module (from FFGN)

USAGE:
    freeforgem --pool HOST:PORT --wallet neb1... [--worker NAME] [OPTIONS]      a neb-pool/1 pool (several: failover list)
    freeforgem --node 127.0.0.1:3441 [OPTIONS]                                  solo: the work service of your NEB node
    freeforgem --pool GATE:3442 --password TOKEN [OPTIONS]                      solo for rigs through an ffgn gate beside the node
    freeforgem --gate --node 127.0.0.1:3441 --gate-token-file F [--gate-listen 0.0.0.0:3442]   run the gate (beside the node)
    freeforgem --self-test [--devices N]      freeforgem --hash-bench SECS [--devices N]      freeforgem --pool-check --pool ...

NEB:
    --pool HOST:PORT     neb-pool/1 pool: HOST:PORT or stratum+tcp://HOST:PORT (plain TCP), stratum+ssl://HOST:PORT (TLS);
                         a list (--pool A,B or --pool A --pool B) = failover / failback (the first is the primary)
    --wallet neb1...     the address the pool pays (required with --pool)
    --worker NAME        worker name (login = ADDRESS.NAME; default: none)
    --password TEXT      pool password (default x); for an ffgn gate: the gate token
    --node HOST:PORT[,..]  solo: the work service of a NEB node (neb-node --work-port, default 3441, listens on 127.0.0.1 only).
                         The block pays the node's --miner-lock. Shares go to the node, solutions are submitted and answered
    --dataset FILE       neb-datasets.bin of NEB 1.0.1 (default: next to the binary, in the working folder, or the HiveOS
                         miner folder). Main network: 16,087,037 bytes, BLAKE3 790b57fb... (two KEP1 datasets: Kepler, TESS);
                         the file of NEB 1.0.0 is refused; the testnet file (NEB 0.3.1) is accepted for the test network
    --backend cuda|cpu   cuda = the NEB GPU engine (nebgpu, default); cpu = the C reference on CPU threads (tests)
    --cpu-threads N      threads of --backend cpu (default: all)
    --self-test          C reference vs the NEB vectors, the engine's self-test, device hits = reference on the dataset; exit
    --hash-bench SECS    attempts per second per GPU on both networks (KEP1: main network, both slots; RET2: test network
                         only), with NVML watts; exit
    --blocks N           stop after N accepted blocks (solo tests)
    --api [HOST:]PORT    read-only HTTP status: /summary /gpus /metrics (default host 127.0.0.1)
    --gate               run as an ffgn gate: serve the work service of --node to rigs (neb-pool/1, token, a nonce range per
                         rig, every share re-checked); --gate-listen ADDR (default 127.0.0.1:3442; give 0.0.0.0:3442 to serve
                         the LAN), --gate-token-file FILE,
                         --gate-slots-bits B (2^B rigs at most, default 6)
    Every nonce a GPU reports is evaluated again with the NEB C reference before it is sent; models are checked on the
    device when they arrive and one batch in 64 is audited. Hash rate unit: MA/s = 10^6 attempts per second.
    Dev fee 1 %: one 60 s slot in every 100 minutes, always on a connection of its own (pool: the user's pool list; solo or
    through a gate: the public NEB pools).

OPTIONS:
    --devices 0,1,...    GPUs to mine on (default: all visible; honours CUDA_VISIBLE_DEVICES)
    --minutes N          stop after N minutes (default: run until interrupted)
    --gpu-state          print the core offset, the clocks and the power of every GPU (NVML), change nothing, exit
    --failback-minutes N  while on a backup pool, check the primary every N minutes (default 10, 0 = off) and go back
                         after 2 good checks in a row (login + a job on a separate connection, nothing is submitted)
    --noshare-minutes M  no accepted share of yours for M minutes while the GPUs hash (and at least 10 shares were expected at the
                         measured hash rate and the pool's share difficulty): the pool is not crediting -> reconnect, the same
                         again -> the next --pool entry (a single pool: reconnect with a warning). Default 15, 0 = off
    --reject-minutes K   only rejected shares (at least 5, none accepted) for K minutes: the same. Default 5, 0 = off
    --pool-check         log in to every --pool entry (connect + login + a job), print the verdicts and exit (no GPU needed)
    --auto-uv            opt-in: find and apply a safe GPU undervolt (NVIDIA: V/F offset, needs root/admin); every step is checked
                         in a child process: the NEB engine at full load, GPU tickets bit-exact with the C reference; see MANUAL.md
    --uv-offset N        apply a fixed V/F offset of N MHz (after a quick check) instead of tuning
    --idle               Windows: mine only while nobody uses the PC (keyboard, mouse, gamepad, a fullscreen
                         game, other programs at work on the CPU or GPU);
                         a locked screen counts as idle. While the PC is in use the GPUs get the owner's clocks and
                         offsets back and no undervolt check starts
    --idle-minutes N     minutes without input before mining starts again (default 5)
    --active MODE        while the PC is in use: pause (default) or light:PCT (mine PCT % of the time, 1-99; light = 30)
    --idle-pause-for L   programs that pause mining while they run even with no input, e.g. cs2.exe,blender.exe
    --idle-cpu PCT       other programs using more than PCT % of the CPU for 3 s = in use (default 25, 0 = off)
    --idle-ignore L      programs whose CPU / GPU load does not count as use (known mining programs are ignored already),
                         e.g. myminer.exe
    --idle-gpu PCT       another program using a GPU engine over PCT % = in use (default 10, 0 = off); any video
                         decoding (a film, a stream) counts too
    --temp-limit C[,C]   keep the GPU at or below C degrees (40..100; one value for all GPUs or a list in --devices order, '-' = off):
                         a controller lowers the share of the time the GPU works (duty cycle) so the core temperature stays
                         2 C below C; meant for laptops / mobile GPUs whose fans cannot be set. NVIDIA. Mining starts soft (30 %) and ramps up; the hashrate shown is the real one
    --temp-stop C2[,C2]  hard stop: at C2 the GPU stops mining until it has cooled to the limit - 6 (default: limit + 10, at
                         most 90, at least limit + 3)
    --no-update-check    do not look for a newer FreeForGem release (default: a GitHub check ~1 min after the start,
                         then every ~12 h; a newer one is only reported: in the log and in HiveOS next to the version)
    --check-update       check for a newer release now, then exit
    --no-color           no colours in the log (also NO_COLOR=1 or FFG_COLOR=0; FFG_COLOR=1 forces them, e.g. through tee)
    --uv-reset           forget the saved undervolt profile of the clocks this run has (core lock / clock under load);
                         the profiles of other locks stay
    --uv-reset-all       forget every saved undervolt result (all locks)
                         Profiles: auto-uv keeps one result per core clock lock (--lock-cclock N; without it the clock under
                         mining load picks the profile), per GPU; the start log lists them (profiles: 1410:+450, 1805:+300).
                         Switching locks reuses each lock's verified result.
                         Checks and tunings run while the miner mines: only the GPU being checked / tuned pauses, the others mine.
                         On HiveOS the offset put back on exit is the OC profile's Core Clock value.
                         A GPU that crashes while mining (Xid) is named in the log and remembered in gpu-faults.json: a
                         crashed undervolt offset is tuned below, a crash at your own offset twice in a row lowers it
                         (HiveOS profile value, then 60 MHz steps), a GPU with nothing left to lower is left out for 1 h
    --uv-max N           highest offset the tuner tries (at most 600). Default: GeForce Turing (RTX 20xx / GTX 16xx, CUDA
                         CC 7.5) +240 (since FFGN 1.0.12: a 12x RTX 2060 SUPER rig crashed from +165 to +450, its stable cards
                         mine at +75..+255), all others (CMP too) 495; --uv-max N overrides the default
    --uv-min N           offset the tuner starts from; a start that fails steps down 30 MHz (default: RTX 3070/3060 Ti by
                         the core lock, RTX 3080/3090 +180; else the HiveOS/driver offset or 120; RTX 40: at least 200,
                         RTX 50: at least 300)
                         --uv-max, --uv-min and --uv-offset take one value for all GPUs, or one per GPU in --devices
                         order (without --devices: GPU0, GPU1, ...), e.g. --uv-max 225,495,150; '-' = that GPU's default
    --uv-step N          MHz per tuning step (default 15)
    --uv-keep-last       after a failed step keep the last good step (default: last good step minus one step)
    --uv-parallel N      GPUs tuned (or re-checked from a saved profile) at the same time; the others mine meanwhile.
                         Default auto: 1 on rigs of 6+ GPUs, else 2; 1 -> 2 -> 4 after 2 GPUs tuned without trouble, never
                         above the free CPU threads nor the RAM of the checks; trouble while tuning (a GPU lost from the
                         bus, a hanging check, all GPUs stalled at once, wrong results of a mining GPU) -> 1, remembered
                         for the rig (uv-parallel.json). The check processes start 3 s apart
    --uv-unpause         forget the remembered caution and the guards of auto-uv (a rig paused after 2 restarts / reboots
                         during a tuning within an hour, GPUs off for 24 h after 3 failures in a row); also --uv-reset-all.
                         Each run tunes at most 10 min per GPU and 60 min in all; the rest goes on at the next start.
                         Since FFGN 1.0.12: a GPU that crashed (or failed a check at its offset) within the last 24 h does not
                         restart straight at its saved offset: it resumes at the last offset that mined 1 h without a fault,
                         else half way between its start offset and the saved one, and climbs back from there (never above
                         the saved offset). An Xid 79 (a GPU fell off the bus: check its riser / power cable, reboot the
                         rig) makes the whole rig cautious for 6 h, kept across restarts (uv-xid79.json): no climbing, one
                         GPU at a time, [AUTO-UV CAUTIOUS Xid 79] on the stats line; and for 24 h every GPU resumes below its
                         saved offset as above. --uv-unpause / --uv-reset-all forget it
    --uv-coarse N        climbing step in MHz (default: 2 x --uv-step = 30): the search climbs in N MHz steps and, at the first
                         failing step, tries --uv-step steps inside that interval only (about half the steps, the same edge).
                         A driver fault (Xid / hang) on a coarse step is not refined: the result is one step below the last
                         good one. 0 (or <= --uv-step) = one step size
    --uv-dir DIR         directory of the undervolt state files (default: next to the binary, $FFGP_STATE_DIR)
    --uv-reval-hours H   re-validate the undervolt every H hours while mining (default 6, 0 = never)
    --auto-uv-dry-run    only log which GPUs --auto-uv would change; changes nothing
    --lock-cclock N      lock the core clock to N MHz (one value for all, or a list in --devices order: a shorter list repeats its last value, - skips a GPU); needs root/Administrator
                         auto-uv profiles are per lock: a new lock is tuned anew
    --lock-mclock N      lock the memory clock to N MHz (same syntax). NEB needs the full memory clock: a HiveOS OC profile
                         that locks the memory low (e.g. 810 MHz) is replaced by the card's maximum while mining and put back
                         on exit; a memory clock far below the maximum is reported in the log
    --table-mclock N     NEB: lock the memory clock to N MHz (e.g. 810) only while a GPU mines on the pure digest-table path,
                         which barely uses the memory (RTX 3070: -0.5 % rate, -10 W, +8-10 % MA/W); every other path, the
                         full-path trial and each new model's table build run at the full memory clock. Same list syntax;
                         needs root/Administrator. With it the automatic L=3 is taken only when it beats the pure table by
                         more than 10 % (the lock's per-watt gain; env NEBGPU_L3_AUTO_GAIN overrides)
    --tbig MODE          NEB: partial L=3 digest table of the TESS slot (engine 0.8.1): auto (default) = built in slices while
                         the pair table mines, sized per model from the measured build speed and the measured mining time
                         per model, none when it would not pay back (+1 % net or less, e.g. slow / Turing cards); on = all
                         the free VRAM at the model start; off. Exact: the same tickets as the full path
    --tbig-gb G          NEB: at most G GB for the TESS triples (default: the free VRAM minus a reserve for the Kepler table)
    --keep-hive-mem      keep the HiveOS OC profile's memory lock even when it is below the maximum (NEB mines much slower)
    --single-process     every GPU's engine in the miner process (before 1.0.11). Default: one engine process per GPU, so a
                         CUDA fault on one GPU (Xid 31/43, an illegal memory access) restarts only that GPU in a few seconds
                         while the others keep mining (FFG_SINGLE_PROCESS=1 or FFGN_SINGLE_PROCESS=1 does the same)
    --reboot-on-lost-gpu MIN   opt-in (default off; root / Administrator): a GPU LOST - fallen off the bus (Xid 79), its
                         engine cannot reopen the device after the retries (DEAD), or NVML no longer sees it - for MIN
                         minutes reboots the rig: HiveOS sreboot (/hive/sbin/sreboot), else systemctl reboot; Windows
                         shutdown /r /t 60. Never in the first 10 min of the miner, at most once per 60 min (the time is
                         kept in reboot-on-lost-gpu.json in the state folder, across reboots); a WARNING line with the GPU,
                         its bus, how long it is lost and the command comes first (log, stats line, API). Off with
                         --single-process or FFG_NO_REBOOT=1 (then it only logs). Recommended on rigs: 10.
                         Always (without it): a lost GPU shows 0 on the stats line / API / HiveOS, and when every GPU is
                         down or not mining for 5 min although there is work, the miner exits with code 3 so that HiveOS
                         (its miner restart and Watchdog), start.ps1 or systemd takes over
    --ramp-ms N          after a pause (pool or node lost, no job, idle mode, an auto-uv check) the GPUs go back to full load
                         one by one, N ms apart (default: 500 on rigs of 4+ GPUs, else 0); a new block's model is built
                         30 ms apart per GPU on such rigs (no synchronous power step of all cards)
    --kep1-kernel K      KEP1 attempt kernel: auto (default: both kernels are timed on the first KEP1 model of each GPU and
                         the faster is kept; the warp kernel needs RTX 30 / A-series or newer), dp4a, warp
    --moffset N          memory clock offset in MHz, may be negative (same syntax; needs driver 555+)
                         on exit a lock that was there before is put back: the HiveOS OC profile lock, or
                         $FFG_RESTORE_CCLOCK / $FFG_RESTORE_MCLOCK (also $FFGP_RESTORE_*; one value or one per device,
                         0 = none); else reset
                         --auto-uv: an OC apply (HiveOS) that resets the tuned offset while mining gets
                         it back after a quick check; a stop during a check is not a failure
    --power-limit N      (alias --pl N) optional, off by default (the lock + offset are the working limiters): set the GPU
                         power limit to N W (NVML, like nvidia-smi -pl; needs root/Administrator): one value for all, or one
                         per device in --devices order, e.g. --pl 140,115,140 ('-' = leave that GPU's limit as it is, e.g.
                         the HiveOS one; a list must have one value per GPU). Clamped to the card's range (logged). The limit
                         before is put back on exit, on Ctrl-C and at the next start after a crash
                         (power-limit-restore-<bus>.json per GPU; on HiveOS the OC profile's power limit); an OC apply that
                         changes it while mining is undone (logged; at most 4 times an hour per GPU). auto-uv measures with
                         the limit in force; when it holds the clocks down under load (power cap) the log says so once
    --auto-lock          opt-in (NVIDIA, root/Administrator; turns on --auto-uv): choose the core
                         clock lock per GPU by measurement. Each lock of the range, highest first, is mined at its own auto-uv
                         profile (a new lock is tuned first), then 45 s settle + 120 s measured (hash rate, NVML watts, clock);
                         shares go to the pool as usual, the other GPUs mine at their locks. Chosen: the best MA/W
                         within --auto-lock-max-loss % of the top hash rate; lower locks are not measured once one is below
                         that. Saved per GPU, coin, board, driver, memory lock and kernel in autolock.json (next to uv.json):
                         later starts apply it at once. Each round runs as a miner process of its own (log lines follow).
                         A GPU with its own --lock-cclock value is not swept (--lock-cclock 1400,- sweeps the second GPU only)
    --auto-lock-range A-B[:S]|L,L..  locks to try (default 1800-1200 step 100, capped by the card's maximum clock; CMP cards: top 1500)
    --auto-lock-max-loss PCT   allowed hash rate loss for efficiency (default 10)
    --auto-lock-goal eff|speed  best efficiency within the loss (default), or the top hash rate
    --auto-lock-reset    forget the measured locks of these GPUs and sweep again (the auto-uv profiles stay)
    --help               show this text
    --version            show the version

";

fn parse_devices(s: &str) -> Result<Vec<i32>> {
    let mut v = Vec::new();
    for p in crate::uv::list_parts(s).into_iter().filter(|p| !p.is_empty()) {
        let d: i32 = p.parse().map_err(|_| anyhow::anyhow!("bad device id {p:?}"))?;
        ensure!(d >= 0, "bad device id {p:?}");
        // test hook (faults.rs scenario s5d): FFG_TEST_DUP_DEVICES=1 keeps a repeated id = two workers on one GPU, a simulated second device
        if !v.contains(&d) || std::env::var("FFG_TEST_DUP_DEVICES").is_ok_and(|x| x == "1") {
            v.push(d);
        }
    }
    ensure!(!v.is_empty(), "--devices needs at least one device id");
    Ok(v)
}

/// The NEB run of FreeForGem: the whole command line (FFGN's option set) is parsed here; main.rs calls it before its own parser
/// when coins::module_coin says NEB.
pub fn main() -> Result<()> {
    // colours: never in the check child processes (the parent parses their lines), not with --no-color / NO_COLOR
    if std::env::args().any(|x| x == "--neb-uv-check" || x == "--no-color" || x == neb::iso::ENGINE_FLAG) {
        color::disable();
    }
    // 1.0.11: an engine process of one GPU (started by the miner itself, iso.rs)
    if std::env::args().nth(1).as_deref() == Some(neb::iso::ENGINE_FLAG) {
        let dev = std::env::args().nth(2).and_then(|d| d.parse().ok()).unwrap_or(0);
        crate::conout::exit(neb::iso::child_main(dev));
    }
    if std::env::args().any(|x| x == "--neb-uv-check") {
        let v: Vec<String> = std::env::args().collect();
        let get = |k: &str| v.iter().position(|x| x == k).and_then(|i| v.get(i + 1)).cloned();
        let dev = get("--devices").and_then(|s| s.split(',').next().and_then(|x| x.trim().parse().ok())).unwrap_or(0);
        let secs = get("--uv-secs").and_then(|s| s.parse().ok()).unwrap_or(15);
        crate::conout::exit(neb::tune::check_child(dev, secs, &get("--uv-ref").unwrap_or_default(), None));
    }
    // per-GPU list options may be given without quotes (`--lock-cclock 1500 1650`, HiveOS Extra config / .bat values split on
    // spaces): the numbers that follow such an option are joined to its value (uv::is_list_value)
    const LIST_OPTS: [&str; 13] = ["--devices", "--device", "--uv-offset", "--uv-max", "--uv-min", "--temp-limit", "--temp-stop", "--lock-cclock", "--lock-mclock", "--moffset", "--table-mclock", "--power-limit", "--pl"];
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
        worker: "worker1".into(),
        password: "x".into(),
        minutes: None,
        devices: None,
        uv: uv::Opts::default(),
        lock_cclock: None,
        lock_mclock: None,
        table_mclock: None,
        moffset: None,
        power_limit: uv::PerGpu::Unset,
        backend: None,
        pool_given: false,
        pool_check: false,
        hash_bench: None,
        idle: idle::Opts::default(),
        temp: templimit::Opts { limit: uv::PerGpu::Unset, stop: uv::PerGpu::Unset },
        node: None,
        blocks: None,
        dataset: None,
        cpu_threads: 0,
        gate: false,
        gate_listen: "127.0.0.1:3442".into(),
        gate_token_file: None,
        gate_bits: 6,
        api: None,
        allow_other_dataset: false,
        keep_hive_mem: false,
        single_process: false,
    };
    let mut test = false;
    let mut gpu_state = false;
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
            "--pool-check" => a.pool_check = true,
            "--noshare-minutes" => failover::set_watchdog(Some(failover::parse_minutes(&val()?).map_err(|e| anyhow::anyhow!(e.replace("--failback-minutes", "--noshare-minutes")))?), None),
            "--reject-minutes" => failover::set_watchdog(None, Some(failover::parse_minutes(&val()?).map_err(|e| anyhow::anyhow!(e.replace("--failback-minutes", "--reject-minutes")))?)),
            "--failback-minutes" => failover::set_failback_minutes(failover::parse_minutes(&val()?).map_err(anyhow::Error::msg)?),
            // the coin is chosen before this parser (coins::module_coin): here only NEB's names are valid
            "--algo" | "--coin" => {
                let v = val()?.to_ascii_lowercase();
                ensure!(crate::coins::by_name(&v) == Some(crate::coins::Coin::Neb), "--algo {v:?} together with NEB options (a NEB pool URL, --node, --gate): one coin per run (--algo neb)");
            }
            "--node" => a.node = Some(val()?),
            "--dataset" => a.dataset = Some(val()?),
            "--cpu-threads" => a.cpu_threads = val()?.parse().map_err(|_| anyhow::anyhow!("--cpu-threads needs a number"))?,
            "--gate" => a.gate = true,
            "--gate-listen" => a.gate_listen = val()?,
            "--gate-token-file" => a.gate_token_file = Some(val()?),
            "--gate-slots-bits" => a.gate_bits = val()?.parse().map_err(|_| anyhow::anyhow!("--gate-slots-bits needs a number 1-16"))?,
            "--api" | "--api-port" => a.api = Some(val()?),
            "--allow-other-dataset" => a.allow_other_dataset = true,
            "--keep-hive-mem" => a.keep_hive_mem = true,
            "--single-process" => a.single_process = true,
            "--reboot-on-lost-gpu" => crate::lostgpu::set_minutes(crate::lostgpu::parse_minutes(&val()?)?),
            "--ramp-ms" => {
                // 1.0.5: ms between GPUs that go back to full load after a pause (0 = all at once; auto: 500 on 6+ GPUs)
                let v: u64 = val()?.trim().parse().map_err(|_| anyhow::anyhow!("--ramp-ms needs a number of milliseconds"))?;
                ensure!(v <= 10_000, "--ramp-ms {v}: at most 10000");
                unsafe { std::env::set_var("FFG_RAMP_MS", v.to_string()) };
            }
            "--tbig" => {
                // FFGN 1.0.12 (nebgpu 0.8.1, R112.md): the partial L=3 table of the TESS slot. auto (default): built in slices
                // while the pair table mines, as many triples as pay back in a model's measured mining time against the measured
                // build speed (none below +1 % net, e.g. on slow / Turing cards); on: all the free VRAM at once (0.8.0); off.
                // The engine reads NEBGPU_TBIG when it opens a GPU (every engine process inherits it).
                let v = val()?.to_ascii_lowercase();
                let e = match v.as_str() {
                    "off" | "0" => "0",
                    "auto" => "2",
                    "on" | "1" => "1",
                    _ => anyhow::bail!("--tbig {v:?}: off, auto or on"),
                };
                unsafe { std::env::set_var("NEBGPU_TBIG", e) };
            }
            "--tbig-gb" => {
                // FFGN 1.0.12: upper limit of the TESS triple section in GB (default: the free VRAM minus a reserve)
                let g: f64 = val()?.trim().parse().map_err(|_| anyhow::anyhow!("--tbig-gb needs a number of GB"))?;
                ensure!((0.0..=256.0).contains(&g), "--tbig-gb {g}: 0 to 256");
                unsafe { std::env::set_var("NEBGPU_TBIG_GB", g.to_string()) };
            }
            "--kep1-kernel" => {
                // 1.0.5: the engine reads NEBGPU_KEP1_PATH when it opens a GPU (this process and the check children)
                let v = val()?.to_ascii_lowercase();
                ensure!(matches!(v.as_str(), "auto" | "dp4a" | "warp"), "--kep1-kernel {v:?}: auto, dp4a or warp");
                unsafe { std::env::set_var("NEBGPU_KEP1_PATH", &v) };
            }
            "--debug-supr" => {
                val()?;
            }
            "--blocks" => a.blocks = Some(val()?.parse().map_err(|_| anyhow::anyhow!("--blocks needs a number"))?),
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
                let c: i32 = val()?.trim().trim_start_matches('+').parse().map_err(|_| anyhow::anyhow!("--uv-coarse needs a non-negative integer (MHz, 0 = one step size as before 1.7.6)"))?;
                ensure!((0..=uv::HARD_CAP).contains(&c), "--uv-coarse must be between 0 and {}", uv::HARD_CAP);
                a.uv.coarse = Some(c);
            }
            "--lock-cclock" => a.lock_cclock = Some(val()?),
            "--lock-mclock" => a.lock_mclock = Some(val()?),
            "--table-mclock" => {
                let v = val()?;
                // FFGN 1.0.12 (R111.md section 10): with the low memory clock on the pure table, the automatic L=3 (which runs at
                // the full memory clock) must beat the table by more than the lock's per-watt gain (~+10 %); the engine reads
                // NEBGPU_L3_AUTO_GAIN when a model's L=3 trial ends (every engine process inherits it). An own value wins.
                if crate::clocks::l3_gain_wanted(&v) && std::env::var_os("NEBGPU_L3_AUTO_GAIN").is_none() {
                    unsafe { std::env::set_var("NEBGPU_L3_AUTO_GAIN", crate::clocks::TABLE_MCLOCK_L3_GAIN) };
                }
                a.table_mclock = Some(v);
            }
            "--moffset" => a.moffset = Some(val()?),
            "--power-limit" | "--pl" => a.power_limit = crate::powerlimit::parse(&val()?, &arg)?,
            "--uv-dir" => a.uv.dir = Some(val()?.into()),
            "--uv-reval-hours" => a.uv.reval_hours = val()?.parse()?,
            "--uv-warmup" => a.uv.warmup_secs = val()?.parse()?,
            "--uv-step-secs" => a.uv.step_secs = val()?.parse()?,
            "--uv-confirm-secs" => a.uv.confirm_secs = val()?.parse()?,
            "--gpu-state" => gpu_state = true,
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
                    bail!("unknown argument {arg} (see --help)");
                }
            }
            _ => bail!("unknown argument {arg} (see --help)"),
        }
    }
    // --auto-lock (1.7.8, off unless asked): choose the core lock per GPU by measurement (autolock.rs)
    if al.sub && !al.on {
        println!("auto-lock: WARNING --auto-lock-* options (and --power-price, --coin-price, --pool-fee) are ignored without --auto-lock");
    } else if al.on && al.goal != autolock::Goal::Profit && (al.tariff.is_some() || al.coin_price.is_some() || al.pool_fee.is_some()) {
        println!("auto-lock: WARNING --power-price / --coin-price / --pool-fee are used by --auto-lock-goal profit only");
    }
    ensure!(
        al.goal != autolock::Goal::Profit && !(al.tariff.is_some() || al.coin_price.is_some() || al.pool_fee.is_some()),
        "--auto-lock-goal profit / --power-price / --coin-price / --pool-fee are not available for NEB in 1.8.0 (NEB prices / network data: the profit switching of the next release): use --auto-lock-goal eff or speed"
    );
    if al.on && al.goal == autolock::Goal::Profit {
        ensure!(
            al.tariff.is_some(),
            "--auto-lock-goal profit needs the price of electricity: --power-price X ($ per kWh), or day and night: --power-price 0.12,0.053@23-07 (0.053 from 23:00 to 07:00)"
        );
    }
    if al.on && !test && !gpu_state {
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
    // a newer release -> a log line and update-available for HiveOS (update.rs); not for check processes / tests
    if update_check && !test && !gpu_state && (!a.wallet.is_empty() || a.node.is_some()) {
        update::start();
    }
    // undervolt options first: a typo must not wait for the GPU library
    a.uv.validate()?;
    if let Some(d) = &a.devices {
        a.uv.check_counts(d.len())?; // without --devices: checked once the GPUs are counted
        a.power_limit.check_count(d.len(), "--power-limit")?;
    }
    // NEB: its own GPU library (nebgpu) or the CPU reference; work from a node, a pool or a gate
    let backend = neb::parse_backend(a.backend.as_deref())?;
    let threads = if a.cpu_threads > 0 { a.cpu_threads } else { std::thread::available_parallelism().map_or(1, |n| n.get()) };
    let nodes: Vec<String> = a.node.as_deref().map(failover::split_list).unwrap_or_default();
    if a.gate {
        ensure!(!nodes.is_empty(), "--gate needs --node 127.0.0.1:3441 (the work service of the node beside the gate)");
        let tf = a.gate_token_file.clone().ok_or_else(|| anyhow::anyhow!("--gate needs --gate-token-file FILE (the token the rigs give as --password)"))?;
        let (file, _) = neb::load_dataset(a.dataset.as_deref(), a.allow_other_dataset)?;
        return neb::gate::run(neb::gate::Opts { listen: a.gate_listen, node: nodes, token: neb::gate::read_token(&tf)?, dataset: file, bits: a.gate_bits, minutes: a.minutes });
    }
    idle::start(&a.idle)?;
    if !a.pool_check && !test && !gpu_state && a.hash_bench.is_none() {
        import_ffgn_state(&uv::state_dir(&a.uv), &ffgn_state_dirs());
    }
    let o = neb::Opts {
        node: nodes,
        pools: if a.pool_given { a.pools.clone() } else { vec![] },
        wallet: a.wallet.trim().to_string(),
        worker: if a.worker == "worker1" { String::new() } else { a.worker.clone() },
        password: a.password.clone(),
        dataset: a.dataset.clone(),
        backend,
        cpu_threads: threads,
        minutes: a.minutes,
        blocks: a.blocks,
        devices: a.devices.clone(),
        lock_cclock: a.lock_cclock.clone(),
        lock_mclock: a.lock_mclock.clone(),
        table_mclock: a.table_mclock.clone(),
        moffset: a.moffset.clone(),
        power_limit: a.power_limit.clone(),
        temp: a.temp,
        uv: a.uv,
        allow_other_dataset: a.allow_other_dataset,
        keep_hive_mem: a.keep_hive_mem,
        single_process: a.single_process,
        api: a.api.clone(),
    };
    if a.pool_check {
        return neb::pool_check(&o);
    }
    if gpu_state {
        return neb::gpu_state(&o);
    }
    if (o.uv.reset || o.uv.reset_all) && o.node.is_empty() && o.pools.is_empty() && !test && a.hash_bench.is_none() {
        // without a work source: --uv-reset-all forgets every saved undervolt result now (uv.json, also auto-lock's
        // autolock.json stays); --uv-reset acts on the profile of the clocks of a mining run, at its start
        if o.uv.reset_all {
            uv::reset(&o.uv);
        } else {
            println!("auto-uv: --uv-reset forgets the profile of the clocks a mining run has (at its start, with --auto-uv); --uv-reset-all forgets every saved result now");
        }
        return Ok(());
    }
    if let Some(f) = std::env::args().skip_while(|x| x != "--debug-supr").nth(1) {
        return neb::supr::debug(&f, o.dataset.as_deref());
    }
    if test {
        return neb::self_test(&o);
    }
    if let Some(s) = a.hash_bench {
        return neb::hash_bench(&o, s);
    }
    neb::mine(o)
}

/// The state folders of FFGN a rig may come from: HiveOS `/hive-config/ffgn`, an `ffgn*` folder next to this one (Windows / plain
/// Linux), $FFGN_STATE_DIR.
fn ffgn_state_dirs() -> Vec<std::path::PathBuf> {
    let mut v: Vec<std::path::PathBuf> = vec!["/hive-config/ffgn".into()];
    if let Some(d) = std::env::var_os("FFGN_STATE_DIR") {
        v.insert(0, d.into());
    }
    if let Some(parent) = std::env::current_exe().ok().and_then(|e| e.parent().and_then(|d| d.parent()).map(|d| d.to_path_buf())) {
        if let Ok(rd) = std::fs::read_dir(&parent) {
            let mut x: Vec<_> = rd.flatten().map(|e| e.path()).filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().to_ascii_lowercase().starts_with("ffgn"))).collect();
            x.sort();
            v.extend(x.into_iter().rev());
        }
    }
    v
}

/// 1.8.0: a rig that mined NEB with FFGN keeps its tuned NEB offsets: on the first NEB start the "neb" / "neb_trying" sections
/// of FFGN's uv.json are copied into this miner's uv.json (only when it has no "neb" section yet; nothing else is touched, no
/// profile is tuned again).  Returns the folder imported from.
pub fn import_ffgn_state(dir: &std::path::Path, from: &[std::path::PathBuf]) -> Option<std::path::PathBuf> {
    let dst = dir.join("uv.json");
    let mut cur: serde_json::Value = std::fs::read_to_string(&dst).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_else(|| serde_json::json!({}));
    if cur.get(crate::profiles::NEB.profiles).is_some_and(|v| v.as_object().is_some_and(|o| !o.is_empty())) {
        return None;
    }
    for d in from {
        let src = d.join("uv.json");
        if src == dst {
            continue;
        }
        let Some(v) = std::fs::read_to_string(&src).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()) else { continue };
        let Some(neb) = v.get(crate::profiles::NEB.profiles).filter(|x| x.as_object().is_some_and(|o| !o.is_empty())) else { continue };
        if !cur.is_object() {
            cur = serde_json::json!({});
        }
        cur[crate::profiles::NEB.profiles] = neb.clone();
        if let Some(t) = v.get(crate::profiles::NEB.trying) {
            cur[crate::profiles::NEB.trying] = t.clone();
        }
        let _ = std::fs::create_dir_all(dir);
        match crate::uv::write_atomic(&dst, &serde_json::to_string_pretty(&cur).unwrap_or_default()) {
            Ok(()) => {
                println!("auto-uv: imported the NEB undervolt profiles of FFGN from {} into {} (no GPU is tuned again)", src.display(), dst.display());
                return Some(d.clone());
            }
            Err(e) => {
                eprintln!("auto-uv: cannot import the NEB profiles of FFGN from {} ({e:#})", src.display());
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
    fn ffgn_neb_profiles_are_imported_once_and_keep_the_other_coins() {
        let base = std::env::temp_dir().join(format!("ffg-nebimp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (ffg, ffgn) = (base.join("freeforgem"), base.join("ffgn"));
        std::fs::create_dir_all(&ffg).unwrap();
        std::fs::create_dir_all(&ffgn).unwrap();
        // FFG has Pearl profiles; FFGN has a NEB profile and a crash marker
        std::fs::write(ffg.join("uv.json"), r#"{"version":2,"profiles":{"GPU-a":{"1805":{"offset":300}}}}"#).unwrap();
        std::fs::write(ffgn.join("uv.json"), r#"{"neb":{"GPU-a":{"1680":{"offset":210}}},"neb_trying":{"GPU-a":{"at":240}}}"#).unwrap();
        assert_eq!(import_ffgn_state(&ffg, &[base.join("none"), ffgn.clone()]), Some(ffgn.clone()));
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(ffg.join("uv.json")).unwrap()).unwrap();
        assert_eq!(v["neb"]["GPU-a"]["1680"]["offset"], 210);
        assert_eq!(v["neb_trying"]["GPU-a"]["at"], 240);
        assert_eq!(v["profiles"]["GPU-a"]["1805"]["offset"], 300, "the Pearl profiles stay");
        assert_eq!(v["version"], 2);
        // once: a later start (FFG has its own NEB section now) imports nothing
        std::fs::write(ffgn.join("uv.json"), r#"{"neb":{"GPU-a":{"1680":{"offset":999}}}}"#).unwrap();
        assert_eq!(import_ffgn_state(&ffg, &[ffgn.clone()]), None);
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(ffg.join("uv.json")).unwrap()).unwrap();
        assert_eq!(v["neb"]["GPU-a"]["1680"]["offset"], 210);
        // a fresh FFG folder (no uv.json yet): created
        let fresh = base.join("fresh");
        assert_eq!(import_ffgn_state(&fresh, &[ffgn.clone()]), Some(ffgn));
        let _ = std::fs::remove_dir_all(&base);
    }
}
