//! Single shutdown gate for the supervisor.
//!
//! Signals, console events, and internal failures notify this coordinator.
//! The runtime thread is the only caller of service shutdown.

use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

#[derive(Debug)]
struct Flag {
    requested: bool,
    duplicates: u64,
}

#[derive(Debug)]
struct Inner {
    flag: Mutex<Flag>,
    wake: Condvar,
}

/// Idempotent shutdown request shared by every termination source.
#[derive(Clone, Debug)]
pub struct ShutdownCoordinator {
    inner: Arc<Inner>,
}

impl ShutdownCoordinator {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                flag: Mutex::new(Flag {
                    requested: false,
                    duplicates: 0,
                }),
                wake: Condvar::new(),
            }),
        }
    }

    /// Record a shutdown request.
    ///
    /// The first call returns `true` and wakes [`Self::wait`]. Later calls
    /// return `false` and do not start a second shutdown.
    pub fn request(&self) -> bool {
        let mut flag = self.lock();
        if flag.requested {
            flag.duplicates = flag.duplicates.saturating_add(1);
            return false;
        }
        flag.requested = true;
        drop(flag);
        self.inner.wake.notify_all();
        true
    }

    pub fn is_requested(&self) -> bool {
        self.lock().requested
    }

    pub fn duplicate_count(&self) -> u64 {
        self.lock().duplicates
    }

    /// Block until [`Self::request`] has been called.
    pub fn wait(&self) {
        let mut flag = self.lock();
        while !flag.requested {
            flag = self
                .inner
                .wake
                .wait(flag)
                .unwrap_or_else(|error| error.into_inner());
        }
    }

    /// Block until shutdown is requested or `timeout` elapses.
    ///
    /// Returns `true` when shutdown was requested. A timeout returns `false`
    /// without spinning.
    pub fn wait_timeout(&self, timeout: Duration) -> bool {
        let mut flag = self.lock();
        let deadline = Instant::now() + timeout;
        while !flag.requested {
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            let (next, wait_result) = self
                .inner
                .wake
                .wait_timeout(flag, deadline.saturating_duration_since(now))
                .unwrap_or_else(|error| error.into_inner());
            flag = next;
            if wait_result.timed_out() && !flag.requested {
                return false;
            }
        }
        true
    }

    fn lock(&self) -> MutexGuard<'_, Flag> {
        self.inner
            .flag
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }
}

impl Default for ShutdownCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::ShutdownCoordinator;
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, Instant};

    #[test]
    fn first_request_wins_and_repeats_do_not() {
        let shutdown = ShutdownCoordinator::new();
        assert!(!shutdown.is_requested());
        assert!(shutdown.request());
        assert!(shutdown.is_requested());
        assert!(!shutdown.request());
        assert!(!shutdown.request());
        assert_eq!(shutdown.duplicate_count(), 2);
        assert!(shutdown.wait_timeout(Duration::from_millis(1)));
    }

    #[test]
    fn wait_timeout_returns_without_a_request() {
        let shutdown = ShutdownCoordinator::new();
        let started = Instant::now();
        assert!(!shutdown.wait_timeout(Duration::from_millis(40)));
        assert!(started.elapsed() >= Duration::from_millis(30));
        assert!(!shutdown.is_requested());
    }

    #[test]
    fn wait_unblocks_when_shutdown_is_requested() {
        let shutdown = ShutdownCoordinator::new();
        let waiting = shutdown.clone();
        let (started_tx, started_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        thread::spawn(move || {
            started_tx.send(()).unwrap();
            waiting.wait();
            done_tx.send(()).unwrap();
        });
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(shutdown.request());
        done_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    }
}
