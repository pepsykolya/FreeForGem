// FreeForGem — see LICENSE
//! Console output that can never stop the miner.  println!/eprintln!/print! (shadowed in main.rs) put each line into a bounded
//! queue; one writer thread owns stdout/stderr.  If the console stops reading (Windows QuickEdit selection, a pipe nobody
//! drains), the writer blocks and the queue fills up; further lines are dropped and counted, the worker threads never wait.
//! When the console reads again, "console output was blocked, N lines dropped" is printed.  One queue keeps the order of
//! stdout and stderr lines.  [`exit`] / [`flush`] drain the queue (at most 1 s) so the last lines are not lost.

use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

/// Messages the queue holds before it starts dropping.
const CAPACITY: usize = 4096;
/// How long the exit paths wait for the queue to drain.
const FLUSH_TIMEOUT: Duration = Duration::from_secs(1);

struct Msg {
    err: bool,
    bytes: Vec<u8>,
}

/// The queue plus its writer thread; `Console::new` is used by the process-wide instance and by the tests.
pub struct Console {
    tx: SyncSender<Msg>,
    queued: Arc<AtomicU64>,
    written: Arc<AtomicU64>,
    dropped: Arc<AtomicU64>,
}

impl Console {
    pub fn new(cap: usize, mut out: Box<dyn Write + Send>, mut err: Box<dyn Write + Send>) -> Console {
        let (tx, rx) = sync_channel::<Msg>(cap);
        let queued = Arc::new(AtomicU64::new(0));
        let written = Arc::new(AtomicU64::new(0));
        let dropped = Arc::new(AtomicU64::new(0));
        let (w, d) = (written.clone(), dropped.clone());
        let _ = std::thread::Builder::new().name("console".into()).spawn(move || {
            while let Ok(m) = rx.recv() {
                // write errors (closed pipe) are ignored, as before; a blocked write blocks only this thread
                let s: &mut dyn Write = if m.err { &mut *err } else { &mut *out };
                let _ = s.write_all(&m.bytes);
                let _ = s.flush();
                w.fetch_add(1, Ordering::SeqCst);
                let n = d.swap(0, Ordering::SeqCst);
                if n > 0 {
                    let _ = writeln!(out, "console output was blocked, {n} lines dropped");
                    let _ = out.flush();
                }
            }
        });
        Console { tx, queued, written, dropped }
    }

    /// Never blocks.  Returns false when the line was dropped.
    pub fn push(&self, err: bool, bytes: Vec<u8>) -> bool {
        self.queued.fetch_add(1, Ordering::SeqCst);
        match self.tx.try_send(Msg { err, bytes }) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                self.queued.fetch_sub(1, Ordering::SeqCst);
                self.dropped.fetch_add(1, Ordering::SeqCst);
                false
            }
        }
    }

    #[cfg(test)]
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::SeqCst)
    }

    /// Waits until everything queued so far is written, at most `timeout`.  True when the queue drained.
    pub fn flush(&self, timeout: Duration) -> bool {
        let end = Instant::now() + timeout;
        while self.written.load(Ordering::SeqCst) < self.queued.load(Ordering::SeqCst) {
            if Instant::now() >= end {
                return false;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        true
    }
}

fn global() -> &'static Console {
    static C: OnceLock<Console> = OnceLock::new();
    C.get_or_init(|| Console::new(CAPACITY, Box::new(std::io::stdout()), Box::new(std::io::stderr())))
}

/// Used by the print macros.
pub fn emit(err: bool, bytes: Vec<u8>) {
    global().push(err, bytes);
}

/// Waits (at most 1 s) for the console writer; call before the process ends.
pub fn flush() {
    global().flush(FLUSH_TIMEOUT);
}

/// `std::process::exit` that first lets the queued lines out.
pub fn exit(code: i32) -> ! {
    flush();
    std::process::exit(code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Clone)]
    struct Gate(Arc<Mutex<Vec<u8>>>, Arc<Mutex<()>>);
    impl Write for Gate {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            let _g = self.1.lock().unwrap(); // blocked while the test holds the gate
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn blocked_writer_never_blocks_producers() {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let gate = Arc::new(Mutex::new(()));
        let g = gate.lock().unwrap();
        let w = Gate(buf.clone(), gate.clone());
        let c = Console::new(100, Box::new(w.clone()), Box::new(w));
        let t = Instant::now();
        let mut ok = 0;
        for i in 0..10_000 {
            if c.push(i % 2 == 1, format!("line {i}\n").into_bytes()) {
                ok += 1;
            }
        }
        assert!(t.elapsed() < Duration::from_secs(2), "producer waited for the blocked console");
        assert!(ok <= 102, "queue is bounded, accepted {ok}");
        assert!(c.dropped() >= 9_800);
        assert!(!c.flush(Duration::from_millis(50)));
        drop(g); // the console reads again
        assert!(c.flush(Duration::from_secs(5)));
        std::thread::sleep(Duration::from_millis(50));
        let s = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
        assert!(s.starts_with("line 0\n"), "order kept");
        assert!(s.find("line 1\n").unwrap() > s.find("line 0\n").unwrap());
        assert!(s.contains("console output was blocked, "), "{s}");
        assert!(s.contains(" lines dropped"));
    }

    #[test]
    fn order_and_flush_without_overflow() {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let w = Gate(buf.clone(), Arc::new(Mutex::new(())));
        let c = Console::new(64, Box::new(w.clone()), Box::new(w));
        for i in 0..50 {
            assert!(c.push(i % 3 == 0, format!("{i}\n").into_bytes()));
        }
        assert!(c.flush(Duration::from_secs(2)));
        let s = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
        let want: String = (0..50).map(|i| format!("{i}\n")).collect();
        assert_eq!(s, want);
    }
}
