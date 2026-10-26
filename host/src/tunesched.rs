// FreeForGem — see LICENSE
//! One GPU tunes at a time (1.7.2 beta3): while one card is tuned (or, for CMFD and NOID, checked), the other GPUs mine at
//! their base / verified offset; the rest wait in a queue, first come first served.  `--uv-parallel N` lets N tune at once
//! (`all` = every GPU at once, the behaviour before).  Pearl's short start quick checks (~40 s) do not take a slot.
//! A first tuning of n GPUs therefore takes about n x the per-card time (Pearl RTX 3070: ~10-15 min per card), while the rig
//! mines on the other n-1 cards.
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::Duration;

/// How many GPUs may tune at once (`--uv-parallel`; usize::MAX = all).
static PARALLEL: AtomicUsize = AtomicUsize::new(1);

pub fn set_parallel(n: usize) {
    PARALLEL.store(n.max(1), Ordering::SeqCst);
}

pub fn parallel() -> usize {
    PARALLEL.load(Ordering::SeqCst)
}

/// `--uv-parallel` value: a number >= 1 or `all`.
pub fn parse(s: &str) -> anyhow::Result<usize> {
    if s.eq_ignore_ascii_case("all") {
        return Ok(usize::MAX);
    }
    let n: usize = s.trim().parse().map_err(|_| anyhow::anyhow!("--uv-parallel: '{s}' is not a number of GPUs (or 'all')"))?;
    anyhow::ensure!(n >= 1, "--uv-parallel must be at least 1");
    Ok(n)
}

/// The queue itself, without threads (unit-tested).
#[derive(Default, Debug)]
pub struct Queue {
    pub running: Vec<i32>,
    pub waiting: VecDeque<i32>,
    pub done: usize,
    pub total: usize,
}

impl Queue {
    /// A GPU asks for a slot (counted once).
    pub fn enqueue(&mut self, dev: i32) {
        self.waiting.push_back(dev);
        self.total += 1;
    }
    /// May `dev` start now?  Only the head of the queue, while fewer than `max` run.
    pub fn try_start(&mut self, dev: i32, max: usize) -> bool {
        if self.running.len() < max && self.waiting.front() == Some(&dev) {
            self.waiting.pop_front();
            self.running.push(dev);
            true
        } else {
            false
        }
    }
    pub fn finish(&mut self, dev: i32) {
        self.running.retain(|&d| d != dev);
        self.done += 1;
    }
    /// Leaves the queue without running (a stop).
    pub fn leave(&mut self, dev: i32) {
        self.waiting.retain(|&d| d != dev);
        self.total = self.total.saturating_sub(1);
    }
    fn names(v: impl Iterator<Item = i32>) -> String {
        v.map(|d| format!("GPU{d}")).collect::<Vec<_>>().join(", ")
    }
    /// "GPU3 tuning (2/8); GPU4, GPU5 queued, mining meanwhile"
    pub fn line(&self, dev: i32, what: &str) -> String {
        let pos = self.done + self.running.len();
        let mut s = format!("GPU{dev} {what} starts ({pos}/{})", self.total);
        let others: Vec<i32> = self.running.iter().copied().filter(|&d| d != dev).collect();
        if !others.is_empty() {
            s.push_str(&format!("; also running: {}", Self::names(others.into_iter())));
        }
        if !self.waiting.is_empty() {
            s.push_str(&format!("; {} queued, mining meanwhile", Self::names(self.waiting.iter().copied())));
        }
        s
    }
}

fn sched() -> &'static (Mutex<Queue>, Condvar) {
    static S: OnceLock<(Mutex<Queue>, Condvar)> = OnceLock::new();
    S.get_or_init(|| (Mutex::new(Queue::default()), Condvar::new()))
}

/// The slot of one GPU; frees it on drop.
pub struct Ticket(i32);

impl Drop for Ticket {
    fn drop(&mut self) {
        let (m, cv) = sched();
        m.lock().unwrap().finish(self.0);
        cv.notify_all();
    }
}

/// Waits for a slot for `dev` (the GPU mines meanwhile); `log` gets the queue lines.  None after a stop.
pub fn acquire(dev: i32, what: &str, stop: &AtomicBool, log: impl Fn(String)) -> Option<Ticket> {
    let (m, cv) = sched();
    let mut q = m.lock().unwrap();
    q.enqueue(dev);
    let max = parallel();
    if !q.try_start(dev, max) {
        let ahead: Vec<i32> = q.running.iter().copied().chain(q.waiting.iter().copied().take_while(|&d| d != dev)).collect();
        log(format!(
            "GPU{dev} {what} queued behind {} (at most {} at a time{}): it mines meanwhile",
            Queue::names(ahead.into_iter()),
            if max == usize::MAX { "all".to_string() } else { max.to_string() },
            if max == 1 { "; --uv-parallel N for more" } else { "" }
        ));
        loop {
            q = cv.wait_timeout(q, Duration::from_millis(500)).unwrap().0;
            if stop.load(Ordering::Relaxed) {
                q.leave(dev);
                cv.notify_all();
                return None;
            }
            if q.try_start(dev, parallel()) {
                break;
            }
        }
    }
    log(q.line(dev, what));
    Some(Ticket(dev))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_at_a_time_in_order() {
        // 8 GPUs without a profile: one tunes, seven queue (Rig01 NOID beta2: all 8 tuned at once, 0 MH/s for 10 min)
        let mut q = Queue::default();
        for d in 0..8 {
            q.enqueue(d);
        }
        assert!(q.try_start(0, 1));
        for d in 1..8 {
            assert!(!q.try_start(d, 1), "GPU{d} must wait");
        }
        assert_eq!(q.line(0, "tuning"), "GPU0 tuning starts (1/8); GPU1, GPU2, GPU3, GPU4, GPU5, GPU6, GPU7 queued, mining meanwhile");
        assert!(!q.try_start(2, 1), "not the head of the queue");
        q.finish(0);
        assert!(q.try_start(1, 1));
        assert_eq!(q.line(1, "tuning"), "GPU1 tuning starts (2/8); GPU2, GPU3, GPU4, GPU5, GPU6, GPU7 queued, mining meanwhile");
        for d in 2..8 {
            q.finish(d - 1);
            assert!(q.try_start(d, 1));
            assert_eq!(q.running, vec![d]);
        }
        q.finish(7);
        assert!(q.running.is_empty() && q.waiting.is_empty() && q.done == 8);
    }

    #[test]
    fn parallel_n_and_all() {
        let mut q = Queue::default();
        for d in 0..8 {
            q.enqueue(d);
        }
        assert!(q.try_start(0, 3) && q.try_start(1, 3) && q.try_start(2, 3));
        assert!(!q.try_start(3, 3));
        q.finish(1);
        assert!(q.try_start(3, 3));
        let mut all = Queue::default();
        for d in 0..8 {
            all.enqueue(d);
        }
        for d in 0..8 {
            assert!(all.try_start(d, usize::MAX));
        }
        assert_eq!(parse("all").unwrap(), usize::MAX);
        assert_eq!(parse("2").unwrap(), 2);
        assert!(parse("0").is_err() && parse("x").is_err());
    }

    #[test]
    fn threads_never_overlap_with_one_slot() {
        // 8 threads as 8 GPUs: at most one inside at any time, all of them get their turn
        set_parallel(1);
        let inside = std::sync::Arc::new(AtomicUsize::new(0));
        let peak = std::sync::Arc::new(AtomicUsize::new(0));
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let hs: Vec<_> = (100..108)
            .map(|d| {
                let (inside, peak, stop) = (inside.clone(), peak.clone(), stop.clone());
                std::thread::spawn(move || {
                    let t = acquire(d, "tuning", &stop, |_| {}).unwrap();
                    let n = inside.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(n, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(30));
                    inside.fetch_sub(1, Ordering::SeqCst);
                    drop(t);
                })
            })
            .collect();
        for h in hs {
            h.join().unwrap();
        }
        assert_eq!(peak.load(Ordering::SeqCst), 1);
    }
}
