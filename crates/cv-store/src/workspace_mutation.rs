//! Workspace-instance-scoped cross-process mutation locking.
//!
//! The lock file lives inside `.cv`, so a rename carries the same lock inode
//! with the surviving workspace instance while a byte copy receives an
//! independent copy. That is exactly the instance scope adoption needs: it
//! is not a bare path lock, and it does not unify divergent copies. The file
//! is never deleted or truncated; kernel file locks are released when even a
//! crashing holder's descriptor closes, so there is no stale-lock cleanup
//! state to interpret. Acquisition is bounded by the same inherited-fd
//! settling window used by run locks; a live concurrent writer is reported
//! as `WouldBlock` rather than silently overlapped.

use std::fs::File;
use std::path::{Path, PathBuf};

pub const FILE: &str = ".cv/workspace-mutation.lock";

fn path(root: &Path) -> PathBuf {
    root.join(FILE)
}

fn refuse_symlink(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("{} is a symlink", path.display()),
        )),
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Held exclusive mutation capability for one workspace instance. Drop
/// releases the kernel lock; process death releases it even without Drop.
#[derive(Debug)]
pub struct WorkspaceMutationLock {
    _file: File,
}

/// Create the lock carrier at intentional workspace-open/mint time. Lookup
/// and adoption locking never create this file: creating it during a merely
/// observational or foreign-path operation would itself be an unowned write.
pub(crate) fn ensure_file(root: &Path) -> std::io::Result<()> {
    refuse_symlink(&root.join(".cv"))?;
    refuse_symlink(&path(root))?;
    crate::open::open_create_no_follow(&path(root)).map(|_| ())
}

/// Acquire the workspace mutation critical section. The lock file must
/// already exist because intentional workspace opening creates it; absence
/// is degraded evidence and fails closed rather than being recreated here.
pub fn lock(root: &Path) -> std::io::Result<WorkspaceMutationLock> {
    refuse_symlink(&root.join(".cv"))?;
    refuse_symlink(&path(root))?;
    let file = crate::open::open_existing_no_follow(&path(root), true)?;
    match crate::lock::try_lock_settled(&file) {
        Ok(()) => Ok(WorkspaceMutationLock { _file: file }),
        Err(std::fs::TryLockError::WouldBlock) => Err(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            format!(
                "{} is held by another workspace mutation writer",
                path(root).display()
            ),
        )),
        Err(std::fs::TryLockError::Error(e)) => Err(e),
    }
}
