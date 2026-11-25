//! Fingerprinted staged writes. Compare-then-replace: the draft's base
//! fingerprint (sha256 + mtime) must still match disk, then the body is
//! written to a sibling `*.tmp` and atomically renamed over the target.
//! No frontmatter validation here — `boris validate` is authoritative.

use crate::error::{EngineError, Result};
use cv_core::engine::{Fingerprint, MoveOutcome, PageMove, RemoveOutcome, WriteOutcome};
use cv_core::sys::{Hash, Timestamp};
use cv_store::paths::{confined_file, confirm_confined_file, create_confined_parent};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

/// Windows: antivirus/indexer handles make rename fail transiently.
const SHARING_RETRIES: u32 = 8;
const SHARING_BACKOFF: Duration = Duration::from_millis(25);

pub fn fingerprint_bytes(bytes: &[u8], mtime: std::time::SystemTime) -> Fingerprint {
    let digest: [u8; 32] = Sha256::digest(bytes).into();
    let ms = mtime
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    Fingerprint {
        sha256: Hash(digest),
        mtime: Timestamp(ms),
    }
}

pub fn fingerprint(path: &Path) -> std::io::Result<Option<Fingerprint>> {
    match std::fs::read(path) {
        Ok(bytes) => {
            let mtime = std::fs::metadata(path)?.modified()?;
            Ok(Some(fingerprint_bytes(&bytes, mtime)))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Resolve a workspace-relative draft path without following links.
pub fn resolve(root: &Path, rel: &str) -> Result<PathBuf> {
    confine(rel, confined_file(root, Path::new(rel)))
}

fn confine(rel: &str, resolved: std::io::Result<PathBuf>) -> Result<PathBuf> {
    resolved.map_err(|e| match e.kind() {
        std::io::ErrorKind::InvalidInput => EngineError::PathEscape(rel.to_string()),
        _ => EngineError::Io(e),
    })
}

/// Is this a transient Windows sharing/lock violation worth retrying?
/// (`ERROR_ACCESS_DENIED` 5, `ERROR_SHARING_VIOLATION` 32,
/// `ERROR_LOCK_VIOLATION` 33.) Always `false` off Windows.
pub fn is_sharing_violation(e: &std::io::Error) -> bool {
    cfg!(windows) && matches!(e.raw_os_error(), Some(5 | 32 | 33))
}

/// Run `op`, retrying sharing violations with linear backoff.
pub fn retry_sharing<T>(mut op: impl FnMut() -> std::io::Result<T>) -> std::io::Result<T> {
    let mut attempt = 0;
    loop {
        match op() {
            Err(e) if is_sharing_violation(&e) && attempt < SHARING_RETRIES => {
                attempt += 1;
                std::thread::sleep(SHARING_BACKOFF * attempt);
            }
            r => return r,
        }
    }
}

/// `expected = None` means "create": an existing file is a stale base.
pub fn write_staged(
    root: &Path,
    rel: &str,
    body: &[u8],
    expected: Option<Fingerprint>,
) -> Result<WriteOutcome> {
    let target = resolve(root, rel)?;
    let check = |target: &Path| -> Result<Option<WriteOutcome>> {
        match (retry_sharing(|| fingerprint(target))?, expected) {
            (None, None) => Ok(None),
            (Some(found), Some(base)) if found == base => Ok(None),
            (Some(found), _) => Ok(Some(WriteOutcome::StaleBase { found })),
            (None, Some(_)) => Err(EngineError::Refused(format!(
                "{rel}: base fingerprint given but file no longer exists"
            ))),
        }
    };
    if let Some(stale) = check(&target)? {
        return Ok(stale);
    }
    let target = confine(rel, create_confined_parent(root, Path::new(rel)))?;
    let dir = target.parent().unwrap_or(root);
    let mut tmp = tempfile::Builder::new()
        .prefix(".cv-write-")
        .suffix(".tmp")
        .tempfile_in(dir)?;
    tmp.write_all(body)?;
    tmp.as_file().sync_all()?;
    // Re-check right before the swap to narrow the race with an external
    // editor or a swapped directory; the rename itself is atomic.
    let target = confine(rel, confirm_confined_file(root, Path::new(rel)))?;
    if let Some(stale) = check(&target)? {
        return Ok(stale);
    }
    let mut tmp = Some(tmp);
    retry_sharing(|| match tmp.take().expect("tmp present").persist(&target) {
        Ok(_) => Ok(()),
        Err(e) => {
            let err = std::io::Error::new(e.error.kind(), e.error.to_string());
            let raw = e.error.raw_os_error();
            tmp = Some(e.file);
            Err(raw.map(std::io::Error::from_raw_os_error).unwrap_or(err))
        }
    })?;
    let fp = retry_sharing(|| fingerprint(&target))?
        .ok_or_else(|| EngineError::Refused(format!("{rel}: vanished after write")))?;
    Ok(WriteOutcome::Written { fingerprint: fp })
}

/// Fingerprint-checked delete. A changed file stays on disk; a missing
/// one is an honest error, never a successful no-op.
pub fn remove_staged(root: &Path, rel: &str, expected: Fingerprint) -> Result<RemoveOutcome> {
    resolve(root, rel)?;
    let stale = retry_sharing(|| -> std::io::Result<Option<Fingerprint>> {
        let target = confirm_confined_file(root, Path::new(rel))?;
        match fingerprint(&target)? {
            Some(found) if found == expected => {
                std::fs::remove_file(&target)?;
                Ok(None)
            }
            Some(found) => Ok(Some(found)),
            None => Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "the file was removed since it was opened",
            )),
        }
    })?;
    Ok(match stale {
        Some(found) => RemoveOutcome::StaleBase { found },
        None => RemoveOutcome::Removed,
    })
}

fn fingerprint_confined(root: &Path, rel: &str) -> Result<Option<Fingerprint>> {
    let target = resolve(root, rel)?;
    Ok(retry_sharing(|| fingerprint(&target))?)
}

fn rollback_target(root: &Path, rel: &str, fp: Fingerprint) {
    let _ = remove_staged(root, rel, fp);
}

/// Write `body` at `to` and remove `from`, with both sides fingerprinted.
/// The destination write is staged + atomic; the pair cannot be atomic on
/// any supported filesystem, so a post-write source race rolls the
/// unchanged destination back before reporting the conflict.
pub fn move_staged(root: &Path, mv: &PageMove) -> Result<MoveOutcome> {
    if mv.from == mv.to {
        let found = fingerprint_confined(root, &mv.from)?
            .ok_or_else(|| EngineError::Refused(format!("{}: source no longer exists", mv.from)))?;
        if found != mv.base {
            return Ok(MoveOutcome::SourceStale { found });
        }
        // Still route through the staged writer: the body may pin an id.
        return match write_staged(root, &mv.to, mv.body.as_bytes(), Some(mv.base))? {
            WriteOutcome::Written { fingerprint } => Ok(MoveOutcome::Moved { fingerprint }),
            WriteOutcome::StaleBase { found } => Ok(MoveOutcome::SourceStale { found }),
        };
    }
    match fingerprint_confined(root, &mv.from)? {
        Some(found) if found == mv.base => {}
        Some(found) => return Ok(MoveOutcome::SourceStale { found }),
        None => {
            return Err(EngineError::Refused(format!(
                "{}: source no longer exists",
                mv.from
            )));
        }
    }
    if let Some(found) = fingerprint_confined(root, &mv.to)? {
        return Ok(MoveOutcome::TargetExists { found });
    }
    let written = match write_staged(root, &mv.to, mv.body.as_bytes(), None)? {
        WriteOutcome::Written { fingerprint } => fingerprint,
        WriteOutcome::StaleBase { found } => return Ok(MoveOutcome::TargetExists { found }),
    };
    match remove_staged(root, &mv.from, mv.base) {
        Ok(RemoveOutcome::Removed) => Ok(MoveOutcome::Moved {
            fingerprint: written,
        }),
        Ok(RemoveOutcome::StaleBase { found }) => {
            rollback_target(root, &mv.to, written);
            Ok(MoveOutcome::SourceStale { found })
        }
        Err(e) => {
            rollback_target(root, &mv.to, written);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A planted link below the root: a Unix symlink or a Windows directory
    /// symlink/junction — all carry the reparse attribute `is_link` checks.
    #[cfg(unix)]
    fn link_dir(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }
    #[cfg(windows)]
    fn link_dir(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::windows::fs::symlink_dir(target, link)
    }
    #[cfg(unix)]
    fn link_file(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }
    #[cfg(windows)]
    fn link_file(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::windows::fs::symlink_file(target, link)
    }

    #[test]
    fn create_then_update_then_stale() {
        let t = tempfile::tempdir().unwrap();
        let WriteOutcome::Written { fingerprint: fp1 } =
            write_staged(t.path(), "content/a.md", b"one", None).unwrap()
        else {
            panic!("create")
        };
        // Creating over an existing file is a stale base.
        assert!(matches!(
            write_staged(t.path(), "content/a.md", b"x", None).unwrap(),
            WriteOutcome::StaleBase { found } if found == fp1
        ));
        let WriteOutcome::Written { fingerprint: fp2 } =
            write_staged(t.path(), "content/a.md", b"two", Some(fp1)).unwrap()
        else {
            panic!("update")
        };
        assert_ne!(fp1.sha256, fp2.sha256);
        // Someone else edits; our old base is now stale and disk is untouched.
        std::fs::write(t.path().join("content/a.md"), b"theirs").unwrap();
        assert!(matches!(
            write_staged(t.path(), "content/a.md", b"mine", Some(fp2)).unwrap(),
            WriteOutcome::StaleBase { .. }
        ));
        assert_eq!(
            std::fs::read(t.path().join("content/a.md")).unwrap(),
            b"theirs"
        );
        let leftovers: Vec<_> = std::fs::read_dir(t.path().join("content"))
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn escapes_refused() {
        let t = tempfile::tempdir().unwrap();
        for bad in ["../x.md", "/etc/x", "", "a/../../b"] {
            assert!(
                matches!(resolve(t.path(), bad), Err(EngineError::PathEscape(_))),
                "{bad}"
            );
        }
        #[cfg(windows)]
        assert!(resolve(t.path(), r"C:\x.md").is_err());
    }

    #[test]
    fn staged_write_refuses_linked_parent_before_creating_directories() {
        let root = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        link_dir(other.path(), &root.path().join("linked")).unwrap();

        assert!(matches!(
            write_staged(root.path(), "linked/new/page.md", b"draft", None),
            Err(EngineError::PathEscape(_))
        ));
        assert_eq!(std::fs::read_dir(other.path()).unwrap().count(), 0);
    }

    /// Every entry under `dir` with file contents, for "unchanged" checks.
    fn tree(dir: &Path) -> Vec<(PathBuf, Option<Vec<u8>>)> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let meta = std::fs::symlink_metadata(&path).unwrap();
            if meta.is_dir() {
                out.push((path.clone(), None));
                out.extend(tree(&path));
            } else {
                out.push((path.clone(), Some(std::fs::read(&path).unwrap())));
            }
        }
        out.sort();
        out
    }

    #[test]
    fn staged_write_through_an_escaping_directory_link_leaves_the_outside_untouched() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("authorized_keys"), b"theirs").unwrap();
        std::fs::create_dir(outside.path().join("sub")).unwrap();
        link_dir(outside.path(), &root.path().join("link")).unwrap();
        let before = tree(outside.path());
        let base = fingerprint(&outside.path().join("authorized_keys")).unwrap();

        for (rel, expected) in [
            ("link/authorized_keys", None),
            ("link/authorized_keys", base),
            ("link/new.md", None),
            ("link/sub/new.md", None),
            ("link/sub/fresh/deeper/new.md", None),
        ] {
            assert!(
                matches!(
                    write_staged(root.path(), rel, b"planted", expected),
                    Err(EngineError::PathEscape(_))
                ),
                "{rel}"
            );
        }
        assert_eq!(tree(outside.path()), before);
    }

    #[test]
    fn staged_write_refuses_a_directory_link_that_stays_inside_the_root() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("real")).unwrap();
        link_dir(&root.path().join("real"), &root.path().join("alias")).unwrap();

        assert!(matches!(
            write_staged(root.path(), "alias/page.md", b"draft", None),
            Err(EngineError::PathEscape(_))
        ));
        assert_eq!(
            std::fs::read_dir(root.path().join("real")).unwrap().count(),
            0
        );
    }

    #[test]
    fn staged_write_refuses_a_linked_final_target_and_keeps_the_link() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let victim = outside.path().join("victim.md");
        std::fs::write(&victim, b"theirs").unwrap();
        let link = root.path().join("page.md");
        link_file(&victim, &link).unwrap();

        assert!(matches!(
            write_staged(root.path(), "page.md", b"draft", None),
            Err(EngineError::PathEscape(_))
        ));
        assert_eq!(std::fs::read(&victim).unwrap(), b"theirs");
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn staged_write_works_when_the_root_is_reached_through_a_link() {
        let real = tempfile::tempdir().unwrap();
        let holder = tempfile::tempdir().unwrap();
        let root = holder.path().join("workspace");
        link_dir(real.path(), &root).unwrap();

        assert!(matches!(
            write_staged(&root, "content/new/page.md", b"draft", None),
            Ok(WriteOutcome::Written { .. })
        ));
        assert_eq!(
            std::fs::read(real.path().join("content/new/page.md")).unwrap(),
            b"draft"
        );
    }

    #[test]
    fn staged_write_creates_nested_directories_inside_the_root() {
        let root = tempfile::tempdir().unwrap();
        assert!(matches!(
            write_staged(root.path(), "content/a/b/page.md", b"draft", None),
            Ok(WriteOutcome::Written { .. })
        ));
        assert_eq!(
            std::fs::read(root.path().join("content/a/b/page.md")).unwrap(),
            b"draft"
        );
    }

    #[test]
    fn sharing_violation_classification() {
        let e = std::io::Error::from_raw_os_error(32);
        assert_eq!(is_sharing_violation(&e), cfg!(windows));
        let mut n = 0;
        let r: std::io::Result<()> = retry_sharing(|| {
            n += 1;
            Err(std::io::Error::other("hard"))
        });
        assert!(r.is_err());
        assert_eq!(n, 1);
    }

    #[test]
    fn remove_staged_deletes_only_the_fingerprinted_file() {
        let t = tempfile::tempdir().unwrap();
        let WriteOutcome::Written {
            fingerprint: created,
        } = write_staged(t.path(), "content/a.md", b"one", None).unwrap()
        else {
            panic!("create")
        };
        std::fs::write(t.path().join("content/a.md"), b"theirs").unwrap();
        assert!(matches!(
            remove_staged(t.path(), "content/a.md", created).unwrap(),
            RemoveOutcome::StaleBase { .. }
        ));
        assert_eq!(
            std::fs::read(t.path().join("content/a.md")).unwrap(),
            b"theirs"
        );

        let fp = fingerprint(&t.path().join("content/a.md"))
            .unwrap()
            .unwrap();
        assert_eq!(
            remove_staged(t.path(), "content/a.md", fp).unwrap(),
            RemoveOutcome::Removed
        );
        assert!(!t.path().join("content/a.md").exists());
    }

    #[test]
    fn move_staged_moves_bytes_and_refuses_conflicts_without_overwriting() {
        let t = tempfile::tempdir().unwrap();
        let WriteOutcome::Written { fingerprint: base } =
            write_staged(t.path(), "content/a.md", b"one", None).unwrap()
        else {
            panic!("create")
        };
        let mv = PageMove {
            from: "content/a.md".into(),
            to: "content/nested/b.md".into(),
            body: "one".into(),
            base,
        };
        assert!(matches!(
            move_staged(t.path(), &mv).unwrap(),
            MoveOutcome::Moved { .. }
        ));
        assert!(!t.path().join("content/a.md").exists());
        assert_eq!(
            std::fs::read(t.path().join("content/nested/b.md")).unwrap(),
            b"one"
        );

        let WriteOutcome::Written { fingerprint: base } =
            write_staged(t.path(), "content/c.md", b"c", None).unwrap()
        else {
            panic!("create c")
        };
        let conflict = PageMove {
            from: "content/c.md".into(),
            to: "content/nested/b.md".into(),
            body: "c".into(),
            base,
        };
        assert!(matches!(
            move_staged(t.path(), &conflict).unwrap(),
            MoveOutcome::TargetExists { .. }
        ));
        assert_eq!(std::fs::read(t.path().join("content/c.md")).unwrap(), b"c");
        assert_eq!(
            std::fs::read(t.path().join("content/nested/b.md")).unwrap(),
            b"one"
        );

        std::fs::write(t.path().join("content/c.md"), b"theirs").unwrap();
        assert!(matches!(
            move_staged(t.path(), &conflict).unwrap(),
            MoveOutcome::SourceStale { .. }
        ));
        assert_eq!(
            std::fs::read(t.path().join("content/c.md")).unwrap(),
            b"theirs"
        );
        assert_eq!(
            std::fs::read(t.path().join("content/nested/b.md")).unwrap(),
            b"one"
        );
    }
}
