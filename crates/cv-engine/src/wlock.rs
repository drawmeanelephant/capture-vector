//! Process-lifetime workspace ownership via `flock` on
//! `.cv/workspace.lock`. The kernel releases the lock when the holder
//! dies, so there is no stale-lock case to break. Callers use ownership
//! to decide whether destructive repair sweeps are race-free: only a
//! process holding the lock can know no other live process writes here.
//!
//! Windows is unchecked: flock has no std/libc primitive there and
//! Windows behavior is honestly unproven throughout this crate.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ownership {
    /// This process holds the workspace lock.
    Owned,
    /// Another live process holds it.
    Foreign,
    /// No locking primitive on this platform, or the lock file is
    /// unavailable.
    Unchecked,
}

/// Acquire (or re-report) this process's ownership of the workspace at
/// `root`. Held locks are cached per canonical path for the process
/// lifetime, so repeated calls in-process neither self-conflict (flock
/// is keyed on the open file description, not the process) nor drop the
/// lock early.
#[cfg(unix)]
pub fn ownership(root: &Path) -> Ownership {
    use std::sync::{Mutex, OnceLock};
    static LOCKS: OnceLock<Mutex<HashMap<PathBuf, std::fs::File>>> = OnceLock::new();
    let Ok(canon) = std::fs::canonicalize(root) else {
        return Ownership::Unchecked;
    };
    let locks = LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut locks = locks.lock().unwrap();
    if locks.contains_key(&canon) {
        return Ownership::Owned;
    }
    let lock_path = canon.join(".cv").join("workspace.lock");
    if let Some(parent) = lock_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let Ok(file) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
    else {
        return Ownership::Unchecked;
    };
    use std::os::unix::io::AsRawFd;
    // SAFETY: flock on a valid fd; non-blocking exclusive.
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc == 0 {
        locks.insert(canon, file);
        Ownership::Owned
    } else {
        Ownership::Foreign
    }
}

#[cfg(not(unix))]
pub fn ownership(_root: &Path) -> Ownership {
    Ownership::Unchecked
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// Child entry point: with this env var set, the test binary just
    /// holds the workspace lock and sleeps, the way a second Capture
    /// Vector process would.
    const HOLD_ENV: &str = "CV_TEST_HOLD_WORKSPACE_LOCK";

    #[test]
    fn hold_the_lock_when_asked() {
        let Some(root) = std::env::var_os(HOLD_ENV) else {
            return; // parent or ordinary run: nothing to do
        };
        let root = PathBuf::from(root);
        assert_eq!(ownership(&root), Ownership::Owned);
        std::thread::sleep(std::time::Duration::from_secs(30));
    }

    /// The cross-process exclusion proof: while a real child process
    /// holds the lock this process cannot acquire it, and once the child
    /// exits the kernel release makes acquisition succeed. Spawning lives
    /// here because cv-engine is the only crate allowed to spawn.
    #[test]
    fn a_live_holder_excludes_other_processes_until_it_dies() {
        if std::env::var_os(HOLD_ENV).is_some() {
            return; // we are the child
        }
        let t = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(t.path()).unwrap();
        // The child needs a valid target test binary invocation.
        let exe = std::env::current_exe().unwrap();
        let mut child = std::process::Command::new(exe)
            .arg("wlock::tests::hold_the_lock_when_asked")
            .arg("--exact")
            .arg("--nocapture")
            .env(HOLD_ENV, &root)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        // Wait until the child provably holds the lock: our own acquire
        // attempts report Foreign. NOTE: because this test process may
        // itself be multi-threaded, ownership is per-process and our
        // attempts never disturb the child's.
        let mut foreign = false;
        for _ in 0..100 {
            std::thread::sleep(std::time::Duration::from_millis(20));
            // Probe with a raw flock on a fresh fd rather than the cached
            // registry: the registry would insert on success, and we must
            // not hold the lock before the child dies in case ownership()
            // is needed as Foreign by an outer test.
            let lock_path = root.join(".cv").join("workspace.lock");
            if lock_path.exists() && raw_probe(&lock_path) == Ownership::Foreign {
                foreign = true;
                break;
            }
        }
        assert!(foreign, "the child holds the lock, we are excluded");
        child.kill().unwrap();
        child.wait().unwrap();
        let lock_path = root.join(".cv").join("workspace.lock");
        let mut owned = false;
        for _ in 0..100 {
            std::thread::sleep(std::time::Duration::from_millis(20));
            if raw_probe(&lock_path) == Ownership::Owned {
                owned = true;
                break;
            }
        }
        assert!(owned, "the kernel released the lock with the child");
    }

    /// A fresh non-blocking flock probe on its own fd, uncached. On
    /// success the fd is dropped immediately, releasing the lock again —
    /// this is only a probe of acquirability.
    fn raw_probe(lock_path: &Path) -> Ownership {
        use std::os::unix::io::AsRawFd;
        let Ok(file) = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)
        else {
            return Ownership::Unchecked;
        };
        // SAFETY: flock on a valid fd; non-blocking exclusive. The fd is
        // dropped at scope end, releasing any acquired lock.
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if rc == 0 {
            Ownership::Owned
        } else {
            Ownership::Foreign
        }
    }
}
