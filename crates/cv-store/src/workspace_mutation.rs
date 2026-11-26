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

#[cfg(unix)]
fn identity_of_file(file: &File) -> std::io::Result<Option<(u64, u64)>> {
    use std::os::unix::fs::MetadataExt;
    let metadata = file.metadata()?;
    Ok(Some((metadata.dev(), metadata.ino())))
}

#[cfg(windows)]
fn identity_of_file(file: &File) -> std::io::Result<Option<(u64, u64)>> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };
    let mut info = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
    let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, info.as_mut_ptr()) };
    if ok == 0 {
        return Err(std::io::Error::last_os_error());
    }
    let info = unsafe { info.assume_init() };
    Ok(Some((
        info.dwVolumeSerialNumber.into(),
        ((info.nFileIndexHigh as u64) << 32) | u64::from(info.nFileIndexLow),
    )))
}

#[cfg(not(any(unix, windows)))]
fn identity_of_file(_: &File) -> std::io::Result<Option<(u64, u64)>> {
    Ok(None)
}

impl WorkspaceMutationLock {
    /// Physical identity of the lock carrier actually held by this guard.
    /// Comparing this with the current path's carrier detects replacement of
    /// `.cv` even when token bytes and workspace-directory identity match.
    pub fn identity(&self) -> std::io::Result<Option<(u64, u64)>> {
        identity_of_file(&self._file)
    }
}

/// Physical identity of the mutation-lock carrier currently denoted by this
/// workspace path, opened no-follow but never locked or created.
pub fn current_identity(root: &Path) -> std::io::Result<Option<(u64, u64)>> {
    refuse_symlink(&root.join(".cv"))?;
    refuse_symlink(&path(root))?;
    let file = crate::open::open_existing_no_follow(&path(root), true)?;
    identity_of_file(&file)
}

/// Lock an intentional ordinary content mutation. Unlike adoption, which is
/// path/instance critical and must fail closed when evidence is absent, a
/// content write is itself intentional workspace mutation and may create the
/// lock carrier in a pre-lock-carrier workspace before serializing.
pub fn lock_for_content_mutation(root: &Path) -> std::io::Result<WorkspaceMutationLock> {
    // A content write is already an intentional workspace mutation, so it may
    // establish the carrier in a legacy or engine-test workspace that has no
    // `.cv` yet. Create only the immediate `.cv` child: never resurrect a
    // missing workspace root, and let the no-follow open/refusal checks below
    // reject symlinked evidence.
    let cv = root.join(".cv");
    match std::fs::symlink_metadata(&cv) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if root.is_dir() {
                match std::fs::create_dir(&cv) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(e) => return Err(e),
                }
            }
        }
        Err(e) => return Err(e),
    }
    ensure_file(root)?;
    lock(root)
}

/// Create the lock carrier at intentional workspace-open/mint time. Lookup
/// and adoption locking never create this file: creating it during a merely
/// observational or foreign-path operation would itself be an unowned write.
/// Ordinary content mutations may use it because the mutation itself is
/// already intentional workspace writes.
pub fn ensure_file(root: &Path) -> std::io::Result<()> {
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
