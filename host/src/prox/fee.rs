// FreeForGem — see LICENSE
//! PROX developer fee routing (1.7.8): the fee slot (devfee.rs schedule) is a second login on the SAME pool the user mines on,
//! like Pearl / NOID (`devfee::fee_wallet(pool)`):
//!   proximamine   FEE_WALLET_PROX.FEE_WORKER_PROX with the dev account's pool secret (FEE_PASS_PROX)
//!   InnovLab      FEE_WALLET_PROX.FEE_WORKER_PROX, pass x (InnovLab protocol, innov.rs)
//!   other stratum FEE_WALLET_PROX.FEE_WORKER_PROX, pass x
//! The dev account on proximamine (FEE_POOL_PROX) is the FALLBACK: always when the user mines solo (a node) or on suprnova (its
//! authorize is gated to another miner's user agent), and for the rest of the run once the same-pool fee login is refused, brings no
//! job within the slot's ready window, or its shares are rejected.  The slot never blocks the user (devfee.rs skip rules).
use super::pool::{self, Event, Login, Submit};
use crate::devfee;
use crate::noid::pool::PoolUrl;
use crate::stratum::{Src, Stats};
use anyhow::Result;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Proto {
    Stratum,
    Innov,
}

#[derive(Clone, Debug)]
pub struct Target {
    pub url: PoolUrl,
    pub proto: Proto,
    pub login: String,
    pub pass: String,
}

impl Target {
    pub fn describe(&self) -> String {
        format!("{}{}", self.url, if self.proto == Proto::Innov { " (InnovLab)" } else { "" })
    }
}

fn fee_login() -> String {
    format!("{}.{}", devfee::FEE_WALLET_PROX, devfee::FEE_WORKER_PROX)
}

pub fn is_proximamine(u: &PoolUrl) -> bool {
    u.host.to_ascii_lowercase().ends_with("proximamine.com")
}

pub fn is_suprnova(u: &PoolUrl) -> bool {
    u.host.to_ascii_lowercase().contains("suprnova")
}

/// The dev account on proximamine (the fallback).
pub fn dev_target() -> Result<Target> {
    // FEE_PASS_PROX (the dev account's pool secret) is injected at build time, see devfee.rs
    Ok(Target { url: super::parse_pool(devfee::FEE_POOL_PROX)?, proto: Proto::Stratum, login: fee_login(), pass: devfee::FEE_PASS_PROX.into() })
}

/// The fee login on the user's own pool; None = only the fallback (solo, suprnova).
pub fn same_pool_target(user: Option<(&PoolUrl, Proto)>) -> Option<Target> {
    let (u, proto) = user?;
    if is_suprnova(u) {
        return None;
    }
    let pass = if is_proximamine(u) && proto == Proto::Stratum { devfee::FEE_PASS_PROX } else { "x" };
    Some(Target { url: u.clone(), proto, login: fee_login(), pass: pass.into() })
}

/// One fee login thread: its own enable flag, submit channel and job events (a disabled one is drained and ignored).
pub struct Client {
    pub target: Target,
    pub enabled: Arc<AtomicBool>,
    pub refused: Arc<AtomicU64>,
    pub tx: Sender<Submit>,
    pub ev: Receiver<(Src, Event)>,
}

pub fn spawn(t: Target, name: &'static str, stats: Arc<Stats>, stop: Arc<AtomicBool>, hashrate: Arc<AtomicU64>) -> Client {
    spawn_following(vec![t], None, name, stats, stop, hashrate)
}

/// The fee login on the user's own pool list: one target per entry (the dev account's secret differs per pool); it connects to the
/// entry the user is on (`follow`) whenever its slot starts.
pub fn spawn_following(ts: Vec<Target>, follow: Option<Arc<crate::failover::Link>>, name: &'static str, stats: Arc<Stats>, stop: Arc<AtomicBool>, hashrate: Arc<AtomicU64>) -> Client {
    let (tx, rx) = mpsc::channel::<Submit>();
    let (etx, erx) = mpsc::channel::<(Src, Event)>();
    let enabled = Arc::new(AtomicBool::new(false));
    let refused = Arc::new(AtomicU64::new(0));
    let t = ts[0].clone();
    let l = Login {
        name,
        url: t.url.clone(),
        urls: ts.iter().map(|x| x.url.clone()).collect(),
        passes: ts.iter().map(|x| x.pass.clone()).collect(),
        link: crate::failover::Link::new(ts.len()),
        follow,
        login: t.login.clone(),
        password: t.pass.clone(),
        enabled: enabled.clone(),
        stop,
        refused: refused.clone(),
        hashrate,
    };
    let sess: pool::Session = if t.proto == Proto::Innov { super::innov::session } else { pool::session_stratum };
    pool::spawn_with(l, Src::Fee, etx, rx, stats, sess);
    Client { target: t, enabled, refused, tx, ev: erx }
}

/// The fee clients of a run: the same-pool one (if any) first, the proximamine fallback.
pub struct Route {
    pub same: Option<Client>,
    pub dev: Client,
    pub on_dev: bool,
}

impl Route {
    pub fn active(&self) -> &Client {
        match (&self.same, self.on_dev) {
            (Some(s), false) => s,
            _ => &self.dev,
        }
    }
    /// Enables the connection of the active client only.
    pub fn want(&self, on: bool) {
        let a = self.active() as *const Client;
        for c in self.same.iter().chain(std::iter::once(&self.dev)) {
            c.enabled.store(on && std::ptr::eq(c, a), Ordering::Relaxed);
        }
    }
    /// Falls back to proximamine for the rest of the run (logged); false when already there.
    pub fn fall_back(&mut self, why: &str) -> bool {
        if self.on_dev || self.same.is_none() {
            return false;
        }
        self.on_dev = true;
        if let Some(s) = &self.same {
            s.enabled.store(false, Ordering::Relaxed);
        }
        println!("devfee: {why} on {}: the fee falls back to the dev account on {} for this run", self.same.as_ref().unwrap().target.describe(), self.dev.target.describe());
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn targets() {
        let pm = super::super::parse_pool("stratum+ssl://pool.proximamine.com:3334").unwrap();
        let t = same_pool_target(Some((&pm, Proto::Stratum))).unwrap();
        assert_eq!(t.pass, devfee::FEE_PASS_PROX);
        assert!(t.login.ends_with(".ffg"));
        let sn = super::super::parse_pool("stratum+tcp://proxi.suprnova.cc:3355").unwrap();
        assert!(same_pool_target(Some((&sn, Proto::Stratum))).is_none());
        let other = super::super::parse_pool("stratum+tcp://10.0.0.1:3333").unwrap();
        assert_eq!(same_pool_target(Some((&other, Proto::Stratum))).unwrap().pass, "x");
        let inn = super::super::parse_pool("stratum+ssl://eu3.innovlab.cc:19701").unwrap();
        let t = same_pool_target(Some((&inn, Proto::Innov))).unwrap();
        assert_eq!((t.pass.as_str(), t.proto), ("x", Proto::Innov));
        assert!(same_pool_target(None).is_none());
    }
}
