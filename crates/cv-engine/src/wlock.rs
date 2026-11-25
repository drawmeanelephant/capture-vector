//! Workspace ownership via `flock` on `.cv/workspace.lock`, scoped to the
//! single active workspace of the process. The kernel releases the lock
//! when the holder dies, so there is no stale-lock case to break; and
//! acquiring a different workspace drops the previous handle, so the
//! workspace the user switched away from is released rather than wedged
//! for the process lifetime. Callers use ownership to decide whether
//! destructive repair sweeps are race-free: only a process holding the
//! lock can know no other live process writes here. Mutation paths treat
//! ownership as a write capability: `Foreign` and `Unsafe` workspaces are
//! read-only.
//!
//! Windows is unchecked: flock has no std/libc primitive there and
//! Windows behavior is honestly unproven throughout this crate.

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
    /// `.cv` is a symlink: sidecar state in potentially untrusted cloned
    /// content must never be followed blindly, so the workspace cannot be
    /// owned — or written — here at all.
    Unsafe,
}

/// Acquire (or re-report) this process's ownership of the workspace at
/// `root`. Held locks are cached per canonical path for the process
/// lifetime, so repeated calls in-process neither self-conflict (flock
/// is keyed on the open file description, not the process) nor drop the
/// lock early.
#[cfg(unix)]
pub fn ownership(root: &Path) -> Ownership {
    use std::sync::{Mutex, OnceLock};
    // One active workspace per process: acquiring a different workspace
    // replaces the slot and drops the previous File, so the kernel
    // releases the earlier lock instead of wedging the workspace the user
    // switched away from. Multi-window use of several workspaces in one
    // process is not a supported shape today.
    static LOCK: OnceLock<Mutex<Option<(PathBuf, std::fs::File)>>> = OnceLock::new();
    let Ok(canon) = std::fs::canonicalize(root) else {
        return Ownership::Unchecked;
    };
    let slot = LOCK.get_or_init(|| Mutex::new(None));
    let mut slot = slot.lock().unwrap();
    if slot.as_ref().is_some_and(|(path, _)| path == &canon) {
        return Ownership::Owned;
    }
    let cv = canon.join(".cv");
    // `.cv` is sidecar state in potentially untrusted cloned content.
    // Inspect before creating anything: a symlinked `.cv` is never
    // followed — no lock file materializes in its target.
    if let Ok(meta) = std::fs::symlink_metadata(&cv)
        && meta.file_type().is_symlink()
    {
        return Ownership::Unsafe;
    }
    let lock_path = cv.join("workspace.lock");
    if let Some(parent) = lock_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // Re-check after creation: a hostile or careless process could have
    // swapped `.cv` for a symlink in between.
    if let Ok(meta) = std::fs::symlink_metadata(&cv)
        && meta.file_type().is_symlink()
    {
        return Ownership::Unsafe;
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
        // Replacing the slot drops any previous File here, releasing the
        // earlier workspace's lock exactly once.
        *slot = Some((canon, file));
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

    #[test]
    fn a_symlinked_cv_is_never_followed_for_locking() {
        let t = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(t.path()).unwrap();
        let outside = t.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, root.join(".cv")).unwrap();
        assert_eq!(ownership(&root), Ownership::Unsafe);
        assert!(
            !outside.join("workspace.lock").exists(),
            "no lock materializes through the symlink"
        );
    }

    #[test]
    fn acquiring_a_new_workspace_releases_the_previous_lock() {
        let t1 = tempfile::tempdir().unwrap();
        let r1 = std::fs::canonicalize(t1.path()).unwrap();
        let t2 = tempfile::tempdir().unwrap();
        let r2 = std::fs::canonicalize(t2.path()).unwrap();
        assert_eq!(ownership(&r1), Ownership::Owned);
        assert_eq!(ownership(&r2), Ownership::Owned);
        // r1's lock was dropped with the slot switch: a fresh probe can
        // acquire it, proving the kernel release happened.
        let probe = r1.join(".cv").join("workspace.lock");
        assert_eq!(raw_probe(&probe), Ownership::Owned);
    }
}
