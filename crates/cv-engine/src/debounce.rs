//! Burst coalescing for follow-up work, such as the IR projection after a
//! watch build. Requests inside one window run the job once; requests
//! that land while the job runs queue exactly one more run.

use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

#[derive(Default)]
struct State {
    /// When the oldest request not yet served arrived.
    pending: Option<Instant>,
    closed: bool,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Runs `job` on its own thread, at most once per `window` after the
/// first request of a burst. The window counts from the first request,
/// not the last, so a steady stream of requests cannot starve the job.
/// Dropping the handle serves a pending request at once, then ends the
/// thread.
pub struct Debounced {
    shared: Arc<Shared>,
}

impl Debounced {
    pub fn spawn(
        name: &str,
        window: Duration,
        mut job: impl FnMut() + Send + 'static,
    ) -> std::io::Result<Self> {
        let shared = Arc::new(Shared::default());
        let worker = shared.clone();
        std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                loop {
                    let mut s = worker.lock();
                    let first = loop {
                        match s.pending {
                            Some(first) => break first,
                            None if s.closed => return,
                            None => s = worker.wake.wait(s).unwrap_or_else(|e| e.into_inner()),
                        }
                    };
                    let due = first + window;
                    while !s.closed {
                        let now = Instant::now();
                        if now >= due {
                            break;
                        }
                        s = worker
                            .wake
                            .wait_timeout(s, due - now)
                            .unwrap_or_else(|e| e.into_inner())
                            .0;
                    }
                    s.pending = None;
                    drop(s);
                    job();
                }
            })?;
        Ok(Self { shared })
    }

    /// Ask for one run. Folds into a run that has not started yet.
    pub fn request(&self) {
        let mut s = self.shared.lock();
        s.pending.get_or_insert_with(Instant::now);
        self.shared.wake.notify_one();
    }
}

impl Drop for Debounced {
    /// Does not wait for the job: it may be blocked on a lock the dropping
    /// thread holds (a build stopping the watch whose sink owns this).
    fn drop(&mut self) {
        self.shared.lock().closed = true;
        self.shared.wake.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;

    const WINDOW: Duration = Duration::from_millis(200);
    const T: Duration = Duration::from_secs(10);

    fn counting(window: Duration) -> (Debounced, Arc<AtomicUsize>, mpsc::Receiver<usize>) {
        let runs = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = mpsc::channel();
        let counter = runs.clone();
        let d = Debounced::spawn("test-debounce", window, move || {
            let n = counter.fetch_add(1, Ordering::SeqCst) + 1;
            let _ = tx.send(n);
        })
        .unwrap();
        (d, runs, rx)
    }

    #[test]
    fn a_burst_of_requests_runs_the_job_once() {
        let (d, runs, rx) = counting(WINDOW);
        for _ in 0..50 {
            d.request();
        }
        assert_eq!(rx.recv_timeout(T).unwrap(), 1);
        assert!(rx.recv_timeout(WINDOW * 3).is_err(), "no second run");
        assert_eq!(runs.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn requests_during_a_run_queue_exactly_one_more() {
        let (started_tx, started) = mpsc::channel();
        let (release, gate) = mpsc::channel::<()>();
        let gate = Mutex::new(gate);
        let runs = Arc::new(AtomicUsize::new(0));
        let counter = runs.clone();
        let d = Debounced::spawn("test-debounce", Duration::from_millis(10), move || {
            counter.fetch_add(1, Ordering::SeqCst);
            started_tx.send(()).unwrap();
            gate.lock().unwrap().recv().unwrap();
        })
        .unwrap();
        d.request();
        started.recv_timeout(T).unwrap();
        for _ in 0..20 {
            d.request();
        }
        release.send(()).unwrap();
        started.recv_timeout(T).unwrap();
        release.send(()).unwrap();
        assert!(started.recv_timeout(WINDOW).is_err(), "no third run");
        assert_eq!(runs.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn dropping_the_handle_serves_a_pending_request_without_the_window() {
        let (d, runs, rx) = counting(Duration::from_secs(60));
        d.request();
        drop(d);
        assert_eq!(rx.recv_timeout(T).unwrap(), 1);
        assert!(rx.recv_timeout(WINDOW).is_err());
        assert_eq!(runs.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn dropping_an_idle_handle_runs_nothing() {
        let (d, runs, rx) = counting(WINDOW);
        drop(d);
        assert!(rx.recv_timeout(WINDOW * 2).is_err());
        assert_eq!(runs.load(Ordering::SeqCst), 0);
    }
}
