//! Workspace-instance evidence carried inside `.cv/`.
//!
//! Device+file-index evidence follows renames on normal filesystems but is
//! not a lifetime identity: POSIX permits reuse after deletion. This small
//! minted token is the lifetime evidence outcome ownership needs. It is not
//! a security boundary — a byte copy of `.cv` is the same logical lineage —
//! but deleting and recreating a workspace mints a different instance.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub const FILE: &str = ".cv/workspace-instance";
pub const LOCK: &str = ".cv/workspace-instance.lock";
const TMP: &str = ".cv/workspace-instance.tmp";
static COUNTER: AtomicU64 = AtomicU64::new(0);

fn path(root: &Path, rel: &str) -> PathBuf {
    root.join(rel)
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

fn valid(id: &str) -> bool {
    id.len() >= 24
        && id.len() <= 96
        && id.starts_with("cvwi-")
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn mint() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    format!("cvwi-{nanos:032x}-{:x}-{n:x}", std::process::id())
}

fn read(root: &Path) -> std::io::Result<Option<String>> {
    match std::fs::read_to_string(path(root, FILE)) {
        Ok(raw) => {
            let id = raw.trim();
            if valid(id) {
                Ok(Some(id.to_string()))
            } else {
                Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "{} is not a valid workspace instance id",
                        path(root, FILE).display()
                    ),
                ))
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Read the workspace-instance token, minting it under a cross-process lock
/// when this `.cv` does not have one yet. Malformed evidence is never
/// overwritten silently; callers must treat the instance as unproven.
pub fn ensure(root: &Path) -> std::io::Result<String> {
    refuse_symlink(&root.join(".cv"))?;
    refuse_symlink(&path(root, FILE))?;
    refuse_symlink(&path(root, LOCK))?;
    refuse_symlink(&path(root, TMP))?;
    std::fs::create_dir_all(root.join(".cv"))?;
    let lock = crate::open::open_create_no_follow(&path(root, LOCK))?;
    lock.lock()?;
    if let Some(id) = read(root)? {
        return Ok(id);
    }

    let id = mint();
    let tmp = path(root, TMP);
    match std::fs::remove_file(&tmp) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        writeln!(f, "{id}")?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path(root, FILE))?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_survives_workspace_rename_and_recreation_mints_a_new_one() {
        let base = tempfile::tempdir().unwrap();
        let original = base.path().join("original");
        std::fs::create_dir_all(&original).unwrap();
        let first = ensure(&original).unwrap();
        assert_eq!(first, ensure(&original).unwrap(), "reads are stable");

        let moved = base.path().join("moved");
        std::fs::rename(&original, &moved).unwrap();
        assert_eq!(first, ensure(&moved).unwrap(), "rename keeps .cv lineage");

        std::fs::create_dir_all(&original).unwrap();
        let recreated = ensure(&original).unwrap();
        assert_ne!(first, recreated, "recreation is a new instance");
    }

    #[test]
    fn malformed_instance_evidence_is_not_silently_replaced() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(".cv")).unwrap();
        std::fs::write(root.path().join(FILE), "foreign").unwrap();
        assert!(ensure(root.path()).is_err());
        assert_eq!(
            std::fs::read_to_string(root.path().join(FILE)).unwrap(),
            "foreign"
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_instance_evidence_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(".cv")).unwrap();
        std::os::unix::fs::symlink(outside.path().join("instance"), root.path().join(FILE))
            .unwrap();
        assert!(ensure(root.path()).is_err());
    }
}
