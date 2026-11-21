// FreeForGem — see LICENSE
//! The coins of FreeForGem in one place (1.8.0): names and aliases of `--algo` / `--coin`, the HiveOS algo name, the units of
//! the stats line, the memory-clock policy and how a run of the coin is dispatched.
//!
//! Two kinds of coin modules:
//! * the classic ones (Pearl, CMFD, NOID, PROX) are parsed by the command line of `main.rs` and dispatched at its end;
//! * coins with a command line of their own (`own_cli`: NEB, from FFGN; next: Ting) are dispatched BEFORE the Pearl parser
//!   (`module_coin`), so their options, child processes (NEB: one engine process per GPU, `--neb-engine DEV`; its tuning check
//!   `--neb-uv-check`) and mining loop live in their own module and the Pearl path is not touched at all.
//!
//! Adding a coin (the extension points, also listed in M1.md "For the Ting agent"):
//! 1. a variant of `Coin` and an entry of `SPECS` (name, aliases, HiveOS algo, unit, efficiency unit, memory policy, own_cli);
//! 2. for an own-CLI coin: its positive URL / internal-flag signs in `module_coin` (via a `looks_like_*` of its module) and its
//!    arm in `main.rs` `dispatch_module`;
//! 3. HiveOS: h-stats.sh maps the unit of the stats line to `_algo` / khs (one `elif` per unit), h-config.sh passes the Pass
//!    field where the coin needs it;
//! 4. the dev fee: its address / worker in devfee.rs (all fee constants of a coin live there or in the coin's `fee.rs`);
//! 5. the memory policy: `MemPolicy` (clocks.rs applies it; a new kind of policy is a new variant there, never a coin test
//!    inside clocks.rs);
//! 6. per-GPU process isolation (`isolation`, owner Q5 21.11.2026): one engine process per GPU, so a CUDA fault of one GPU
//!    restarts only that GPU.  On for NEB (and Ting); Pearl / NOID / CMFD / PROX stay single-process until after Pearl V4.
//!    `--single-process` (or FFG_SINGLE_PROCESS=1 / FFGN_SINGLE_PROCESS=1) forces it off for a run; `isolated` decides.

use crate::clocks::MemPolicy;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Coin {
    Pearl,
    Cmfd,
    Noid,
    Prox,
    Neb,
    Ting,
}

pub struct Spec {
    pub coin: Coin,
    /// the canonical `--algo` / `--coin` value
    pub name: &'static str,
    /// other accepted values (case-insensitive)
    pub aliases: &'static [&'static str],
    /// the algo HiveOS shows (h-stats.sh `_algo`); Pearl: the manifest's CUSTOM_ALGO
    pub hive_algo: &'static str,
    /// the rate unit of the stats line
    pub unit: &'static str,
    /// the efficiency unit (owner's rule 1: per watt)
    pub eff_unit: &'static str,
    /// what the host does with the memory clock while this coin mines (and puts back on exit)
    pub mem: MemPolicy,
    /// the module has a command line of its own (dispatched before the Pearl parser)
    pub own_cli: bool,
    /// per-GPU process isolation is the default of this coin (its module has the engine child: NEB `neb/iso.rs`)
    pub isolation: bool,
}

pub const SPECS: [Spec; 6] = [
    Spec { coin: Coin::Pearl, name: "pearl", aliases: &["prl", "pearlhash"], hive_algo: "pearlhash", unit: "TH/s", eff_unit: "TH/W", mem: MemPolicy::HiveProfile, own_cli: false, isolation: false },
    Spec { coin: Coin::Cmfd, name: "cmfd", aliases: &["forgematrix", "forgematrix_v4"], hive_algo: "forgematrix_v4", unit: "FW/s", eff_unit: "FW/kWh", mem: MemPolicy::NoHiveLock, own_cli: false, isolation: false },
    Spec { coin: Coin::Noid, name: "noid", aliases: &["parano1d", "poseidon2b"], hive_algo: "parano1d", unit: "MH/s", eff_unit: "MH/W", mem: MemPolicy::HiveProfile, own_cli: false, isolation: false },
    Spec { coin: Coin::Prox, name: "prox", aliases: &["proxima", "txid-blake2b"], hive_algo: "proxima", unit: "GH/s", eff_unit: "GH/W", mem: MemPolicy::HiveProfile, own_cli: false, isolation: false },
    Spec { coin: Coin::Neb, name: "neb", aliases: &["nebhash", "nebhash2", "nebhash-v2"], hive_algo: "nebhash", unit: "MA/s", eff_unit: "MA/W", mem: MemPolicy::FullClock, own_cli: true, isolation: true },
    // Ting / Daler (DLR), the FFGT module: the memory clock does not matter to SHA-256: the HiveOS profile's lock is kept, else
    // ting/mod.rs locks the lowest clock that keeps the full core range (as FFGT 0.2.1+; CMP 50HX: the driver has no memory lock)
    Spec { coin: Coin::Ting, name: "ting", aliases: &["dlr", "daler", "ting-sha256"], hive_algo: "ting", unit: "GH/s", eff_unit: "GH/W", mem: MemPolicy::HiveProfile, own_cli: true, isolation: true },
];

pub fn spec(c: Coin) -> &'static Spec {
    SPECS.iter().find(|s| s.coin == c).expect("every coin has a spec")
}

/// One engine process per GPU for this run of coin `c`: the coin's default, unless `--single-process` (`single_flag`) or
/// FFG_SINGLE_PROCESS=1 / FFGN_SINGLE_PROCESS=1.
pub fn isolated(c: Coin, single_flag: bool) -> bool {
    let env_off = ["FFG_SINGLE_PROCESS", "FFGN_SINGLE_PROCESS"].iter().any(|k| std::env::var(k).is_ok_and(|v| v == "1"));
    isolated_with(c, single_flag, env_off)
}

fn isolated_with(c: Coin, single_flag: bool, env_off: bool) -> bool {
    spec(c).isolation && !single_flag && !env_off
}

/// 1.8.0 (different coins on different cards, PLAN 3.1a): the group of cards this process mines for, from FFG_GROUP (set by
/// the split supervisor for each child; letters, digits, '-' and '_' only); None = the whole rig (one process).
pub fn group() -> Option<String> {
    group_of(std::env::var("FFG_GROUP").ok().as_deref())
}

fn group_of(v: Option<&str>) -> Option<String> {
    let g: String = v?.trim().chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').take(32).collect();
    (!g.is_empty()).then_some(g)
}

/// The name of a per-process state file (the restore markers of clocks and offsets: they describe what THIS process changed
/// and are undone by its next start): `name` for the whole rig, `stem.<group>.ext` for a group, so one group's start never
/// undoes the clocks of another group that is mining.
pub fn group_file(name: &str) -> String {
    group_file_of(name, group().as_deref())
}

fn group_file_of(name: &str, group: Option<&str>) -> String {
    match (group, name.rsplit_once('.')) {
        (Some(g), Some((stem, ext))) => format!("{stem}.{g}.{ext}"),
        (Some(g), None) => format!("{name}.{g}"),
        (None, _) => name.to_string(),
    }
}

/// The coin of an `--algo` / `--coin` value.
pub fn by_name(name: &str) -> Option<Coin> {
    let n = name.trim().to_ascii_lowercase();
    SPECS.iter().find(|s| s.name == n || s.aliases.contains(&n.as_str())).map(|s| s.coin)
}

/// The value of `--algo` / `--coin` in `argv` (the last one given), lower case.
fn algo_arg(argv: &[String]) -> Option<String> {
    let mut r = None;
    let mut i = 0;
    while i < argv.len() {
        if (argv[i] == "--algo" || argv[i] == "--coin") && i + 1 < argv.len() {
            r = Some(argv[i + 1].to_ascii_lowercase());
            i += 1;
        }
        i += 1;
    }
    r
}

/// The first --pool value (a list: its first entry).
fn first_pool(argv: &[String]) -> Option<String> {
    let i = argv.iter().position(|a| a == "--pool")?;
    let v = argv.get(i + 1)?;
    crate::failover::split_list(v).into_iter().next()
}

/// An own-CLI coin that this command line runs (argv without the program name), else None (the classic parser runs).
/// Order: the internal child flags of a module; an explicit `--algo` / `--coin` (a classic coin given explicitly wins over any
/// URL sign); then positive signs only (F-01..23 of FFGN: never a guess): the pool URL, `--node` / `--gate` (NEB solo / gate).
pub fn module_coin(argv: &[String]) -> Option<Coin> {
    if argv.first().is_some_and(|a| a == crate::neb::iso::ENGINE_FLAG) || argv.iter().any(|a| a == "--neb-uv-check") {
        return Some(Coin::Neb);
    }
    if argv.first().is_some_and(|a| a == crate::ting::iso::ENGINE_FLAG) || argv.iter().any(|a| a == "--ting-uv-check") {
        return Some(Coin::Ting);
    }
    if let Some(a) = algo_arg(argv) {
        return by_name(&a).filter(|c| spec(*c).own_cli);
    }
    if argv.iter().any(|a| a == "--node" || a == "--gate") {
        return Some(Coin::Neb);
    }
    if first_pool(argv).is_some_and(|p| crate::neb::looks_like_neb_pool(&p)) {
        return Some(Coin::Neb);
    }
    // Ting: a crypto-connection.net pool or a ting:// gate URL; Ting-only node options (--node alone is NEB's sign above)
    if first_pool(argv).is_some_and(|p| crate::ting::looks_like_ting_pool(&p)) || argv.iter().any(|a| a == "--node-dir" || a == "--node-token-file") {
        return Some(Coin::Ting);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn names_and_aliases() {
        for (n, c) in [("pearl", Coin::Pearl), ("PRL", Coin::Pearl), ("cmfd", Coin::Cmfd), ("noid", Coin::Noid), ("parano1d", Coin::Noid), ("prox", Coin::Prox), ("neb", Coin::Neb), ("NebHash", Coin::Neb), ("nebhash-v2", Coin::Neb)] {
            assert_eq!(by_name(n), Some(c), "{n}");
        }
        for n in ["ting", "DLR", "daler", "ting-sha256"] {
            assert_eq!(by_name(n), Some(Coin::Ting), "{n}");
        }
        assert_eq!((spec(Coin::Ting).hive_algo, spec(Coin::Ting).unit, spec(Coin::Ting).eff_unit), ("ting", "GH/s", "GH/W"));
        assert_eq!(spec(Coin::Neb).hive_algo, "nebhash");
        assert_eq!(spec(Coin::Neb).unit, "MA/s");
        assert!(SPECS.iter().all(|s| s.eff_unit.contains("/W") || s.coin == Coin::Cmfd), "rule 1: efficiency per watt");
    }

    #[test]
    fn memory_policy_per_coin() {
        // Pearl keeps the HiveOS profile's memory lock (810); NEB never runs at it; CMFD skips it (as 1.7.6)
        assert_eq!(spec(Coin::Pearl).mem, MemPolicy::HiveProfile);
        assert_eq!(spec(Coin::Neb).mem, MemPolicy::FullClock);
        assert_eq!(spec(Coin::Cmfd).mem, MemPolicy::NoHiveLock);
        assert_eq!(spec(Coin::Noid).mem, MemPolicy::HiveProfile);
    }

    #[test]
    fn restore_markers_per_group() {
        assert_eq!(group_file_of("clocks-restore.json", None), "clocks-restore.json");
        assert_eq!(group_file_of("clocks-restore.json", Some("neb")), "clocks-restore.neb.json");
        assert_eq!(group_of(Some(" pearl-2 ")), Some("pearl-2".into()));
        assert_eq!(group_of(Some("../x")), Some("x".into()), "no path in a group name");
        assert_eq!(group_of(Some("")), None);
        assert_eq!(group_of(None), None);
    }

    #[test]
    fn isolation_per_coin() {
        // Q5: on for NEB, off for Pearl / NOID / CMFD / PROX until after V4; --single-process / the env force it off
        assert!(isolated_with(Coin::Neb, false, false));
        assert!(!isolated_with(Coin::Neb, true, false));
        assert!(!isolated_with(Coin::Neb, false, true));
        assert!(isolated_with(Coin::Ting, false, false) && !isolated_with(Coin::Ting, true, false) && !isolated_with(Coin::Ting, false, true));
        for c in [Coin::Pearl, Coin::Noid, Coin::Cmfd, Coin::Prox] {
            assert!(!isolated_with(c, false, false), "{c:?}");
        }
    }

    #[test]
    fn own_cli_dispatch() {
        assert_eq!(module_coin(&v("--coin neb --pool x:1 --wallet neb1q")), Some(Coin::Neb));
        assert_eq!(module_coin(&v("--algo nebhash --pool x:1")), Some(Coin::Neb));
        assert_eq!(module_coin(&v("--pool stratum+tcp://neb.suprnova.cc:7804 --wallet neb1q.w")), Some(Coin::Neb));
        assert_eq!(module_coin(&v("--pool work://neb.ariabrain.com:3370 --wallet neb1q")), Some(Coin::Neb));
        assert_eq!(module_coin(&v("--node 127.0.0.1:3441")), Some(Coin::Neb));
        assert_eq!(module_coin(&v("--neb-engine 0")), Some(Coin::Neb));
        assert_eq!(module_coin(&v("--self-test --neb-uv-check --devices 0")), Some(Coin::Neb));
        // the classic coins: never NEB
        assert_eq!(module_coin(&v("--pool pearl-eu2.luckypool.io:3360 --wallet prl1q")), None);
        assert_eq!(module_coin(&v("--algo pearl --pool stratum+tcp://neb.suprnova.cc:7804")), None, "an explicit classic coin wins");
        assert_eq!(module_coin(&v("--pool stratum+ssl://pool.proximamine.com:3334")), None);
        assert_eq!(module_coin(&v("--algo noid --pool stratum+ssl://noid.example:443")), None);
        assert_eq!(module_coin(&v("--self-test")), None);
        assert_eq!(module_coin(&v("")), None);
        // Ting (FFGT module): explicit, by its pool / gate URL, by its node options, its child flags
        assert_eq!(module_coin(&v("--algo ting --pool 192.168.1.5:4173 --password p")), Some(Coin::Ting));
        assert_eq!(module_coin(&v("--pool crypto-connection.net:4073 --wallet 00ab.rig")), Some(Coin::Ting));
        assert_eq!(module_coin(&v("--pool ting://192.168.1.5:4173")), Some(Coin::Ting));
        assert_eq!(module_coin(&v("--algo ting --node 127.0.0.1:18440 --node-dir /var/lib/ting")), Some(Coin::Ting));
        assert_eq!(module_coin(&v("--node 127.0.0.1:18440 --node-dir /var/lib/ting")), Some(Coin::Neb), "--node alone stays NEB's sign");
        assert_eq!(module_coin(&v("--node-dir /var/lib/ting")), Some(Coin::Ting));
        assert_eq!(module_coin(&v("--ting-engine 0")), Some(Coin::Ting));
        assert_eq!(module_coin(&v("--ting-uv-check --devices 0 --uv-secs 15")), Some(Coin::Ting));
        assert_eq!(module_coin(&v("--algo pearl --pool crypto-connection.net:4073")), None, "an explicit classic coin wins");
        // a failover list: the first entry decides
        assert_eq!(module_coin(&v("--pool neb.suprnova.cc:7804,neb.suprnova.cc:7805")), Some(Coin::Neb));
    }
}
