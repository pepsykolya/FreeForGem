// FFGN — see LICENSE
//! Developer fee of FFGN: 1 % of the mining time (crate::devfee schedule: one 60 s slot in every 100 minutes at a random moment).
//!
//! ALL NEB FEE CONSTANTS LIVE IN THIS FILE (and devfee.rs for the schedule).  The LICENSE forbids removing, disabling,
//! reducing or otherwise altering the developer fee, its address, its schedule or its accounting.
//!
//! * Pool mode (neb-pool/1): the fee login `FEE_ADDRESS.FEE_WORKER` is authorized in the SAME session as the user's
//!   (spec/pool.md section 4: "a miner software with a dev fee authorizes its own login as well and submits that part of its
//!   attempts under it"); during the slot the devices mine headers of the fee login (extranonce2 with its top bit set, so
//!   they never repeat the user's) and the shares are submitted under it.  A pool that refuses the fee login: slot skipped.
//! * Solo (work service of the user's node): the node pays its own `--miner-lock`, which the miner cannot change.  During the
//!   slot the devices mine on a public pool (`FEE_POOLS`: Aria, Suprnova) under the fee login, on a second connection opened
//!   shortly before the slot.  If no fee pool answers, the slot is skipped and the user keeps mining (never blocked).
//! * Work-protocol pools (Aria): one login per connection, so the fee login has a connection of its own to the user's pool.
//! * Fee fallback (as FreeForGem 1.7.8 PROX): once the fee login in the user's session is refused or brings no job within the
//!   slot's ready window, the following slots use a connection of its own to the user's pool for the rest of the run.
//! A build without FFGN_FEE_POOLS has no solo fee path (solo slots skipped, pool fee unaffected); build.sh /
//! build-windows.ps1 refuse to package a release without it.

pub const FEE_WORKER: &str = "ffgn";
/// The FFGN dev-fee address (DEVFEE wallet, created by neb-sec 2026-11-16; public address only, its wallet is not on the miner).
pub const FEE_ADDRESS: &str = "neb1fh4c7f95phnphxsh6tqv26cy2wzrf0ga4ljrs9uhc04mlyetr2xqw4nhfp";
/// Pools for the solo fee slots (a list, failover as for the user's pools): Aria (NEB work protocol) and Suprnova (stratum).
/// FFGN_FEE_POOLS at build time replaces the list.
pub const FEE_POOLS: &str = match option_env!("FFGN_FEE_POOLS") {
    Some(s) => s,
    None => "work://neb.ariabrain.com:3370,stratum+tcp://neb.suprnova.cc:7804",
};

pub struct Config {
    pub login: String,
    pub password: String,
    /// solo: the fee pools
    pub endpoints: Vec<String>,
}

/// None = development build without a fee address.
pub fn config(solo: bool) -> Option<Config> {
    if FEE_ADDRESS.is_empty() {
        return None;
    }
    let login = format!("{FEE_ADDRESS}.{FEE_WORKER}");
    if solo {
        let endpoints: Vec<String> = FEE_POOLS.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        if endpoints.is_empty() {
            println!("devfee: this build has no FFGN fee pool for solo mining: the solo fee slots are skipped");
            return None;
        }
        return Some(Config { login, password: String::new(), endpoints });
    }
    let endpoints = FEE_POOLS.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    Some(Config { login, password: String::new(), endpoints })
}
