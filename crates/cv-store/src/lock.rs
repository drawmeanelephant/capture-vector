//! Advisory writer locks that tolerate a briefly inherited descriptor.
//!
//! A child process spawned by another thread holds a copy of every
//! descriptor that was open when it forked, and keeps it until its own
//! `exec` closes the close-on-exec ones. An exclusive lock released by its
//! owner in that window still reads as held, so a workspace reopened while
//! the engine is spawning could refuse its own sidecar. The retry window
//! only covers that hand-off; a real second writer still gets `WouldBlock`.

use std::fs::{File, TryLockError};
use std::time::{Duration, Instant};

const SETTLE: Duration = Duration::from_millis(250);
const POLL: Duration = Duration::from_millis(5);

/// [`File::try_lock`], retried for up to 250 ms while the lock is held.
pub fn try_lock_settled(file: &File) -> Result<(), TryLockError> {
    let deadline = Instant::now() + SETTLE;
    loop {
        match file.try_lock() {
            Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(POLL);
            }
            other => return other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::OpenOptions;
    use std::path::Path;

    fn open(path: &Path) -> File {
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .unwrap()
    }

    #[test]
    fn a_lock_released_within_the_window_is_acquired() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("writer.lock");
        let holder = open(&path);
        holder.try_lock().unwrap();
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(40));
            drop(holder);
        });
        assert!(try_lock_settled(&open(&path)).is_ok());
        release.join().unwrap();
    }

    #[test]
    fn a_lock_held_past_the_window_still_refuses() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("writer.lock");
        let holder = open(&path);
        holder.try_lock().unwrap();
        let started = Instant::now();
        assert!(matches!(
            try_lock_settled(&open(&path)),
            Err(TryLockError::WouldBlock)
        ));
        assert!(started.elapsed() >= SETTLE);
    }
}
