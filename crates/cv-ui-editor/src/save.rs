//! Fingerprinted staged saves.
//!
//! A save is a ticket taken from the buffer (`Buffer::begin_save`), handed
//! to a [`ContentWriter`] off the UI thread, and folded back with
//! `Buffer::finish_save`. Edits made while the write is in flight keep the
//! buffer dirty: the saved revision is the ticket's, not the latest.
//!
//! [`FsWriter`] implements the cv-core Windows pin directly: stage into a
//! sibling `*.tmp`, `sync_all`, re-check the base fingerprint, rename over
//! the target; a sharing violation / access-denied on the rename retries
//! [`RENAME_RETRIES`] times on a fixed [`RENAME_BACKOFF`], then fails
//! honestly. Frontmatter is never validated here — `boris validate` judges.

use crate::buffer::Buffer;
use cv_core::engine::{Engine, Fingerprint, PageDraft, WriteOutcome};
use cv_core::sys::{Hash, Timestamp};
use cv_store::paths::{confined_file, confirm_confined_file, create_confined_parent};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Retries after the first failed rename (cv-core pin: 5 × 50 ms).
pub const RENAME_RETRIES: u32 = 5;
pub const RENAME_BACKOFF: Duration = Duration::from_millis(50);

/// The editor's write port. Blocking; callers run it on a worker thread.
pub trait ContentWriter: Send + Sync {
    fn write(&self, draft: &PageDraft) -> cv_core::Result<WriteOutcome>;
}

/// Route saves through an engine's `write_content`.
pub struct EngineWriter<E>(pub E);

impl<E: Engine + Sync> ContentWriter for EngineWriter<E> {
    fn write(&self, draft: &PageDraft) -> cv_core::Result<WriteOutcome> {
        self.0.write_content(draft, draft.base)
    }
}

/// What the buffer shows about its last save attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveStatus {
    Idle,
    Saving {
        revision: u64,
    },
    Saved {
        revision: u64,
    },
    /// Disk moved under the buffer; nothing was written.
    Conflict {
        found: Fingerprint,
    },
    /// The write failed (including retry exhaustion); nothing was dropped.
    Failed {
        message: String,
    },
}

/// A snapshot of the buffer to write.
#[derive(Debug, Clone)]
pub struct SaveTicket {
    pub revision: u64,
    pub draft: PageDraft,
}

impl Buffer {
    pub fn begin_save(&self) -> SaveTicket {
        SaveTicket {
            revision: self.revision(),
            draft: PageDraft {
                path: self.path().to_string(),
                body: self.text(),
                base: self.base(),
            },
        }
    }

    /// Fold a write result back. Returns the status to display.
    pub fn finish_save(
        &mut self,
        ticket: &SaveTicket,
        result: cv_core::Result<WriteOutcome>,
    ) -> SaveStatus {
        match result {
            Ok(WriteOutcome::Written { fingerprint }) => {
                self.mark_saved(ticket.revision, fingerprint);
                SaveStatus::Saved {
                    revision: ticket.revision,
                }
            }
            Ok(WriteOutcome::StaleBase { found }) => SaveStatus::Conflict { found },
            Err(e) => SaveStatus::Failed {
                message: e.to_string(),
            },
        }
    }
}

/// Run one save synchronously (worker-thread helper).
pub fn save_blocking(
    writer: &dyn ContentWriter,
    ticket: &SaveTicket,
) -> cv_core::Result<WriteOutcome> {
    writer.write(&ticket.draft)
}

pub fn fingerprint_bytes(bytes: &[u8], mtime: SystemTime) -> Fingerprint {
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

pub fn fingerprint_file(path: &Path) -> std::io::Result<Option<Fingerprint>> {
    match std::fs::read(path) {
        Ok(bytes) => {
            let mtime = std::fs::metadata(path)?.modified()?;
            Ok(Some(fingerprint_bytes(&bytes, mtime)))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Windows `ERROR_ACCESS_DENIED` (5) / `ERROR_SHARING_VIOLATION` (32):
/// the transient failures AV filters and open reader handles cause.
pub fn is_sharing_violation(e: &std::io::Error) -> bool {
    cfg!(windows) && matches!(e.raw_os_error(), Some(5) | Some(32))
}

/// Run `op`, retrying transient failures per the pin. `sleep` is injected
/// so tests can count waits without sleeping.
pub fn retry_pinned<T>(
    mut op: impl FnMut() -> std::io::Result<T>,
    transient: impl Fn(&std::io::Error) -> bool,
    mut sleep: impl FnMut(Duration),
) -> std::io::Result<T> {
    let mut attempt = 0;
    loop {
        match op() {
            Err(e) if transient(&e) && attempt < RENAME_RETRIES => {
                attempt += 1;
                sleep(RENAME_BACKOFF);
            }
            Err(e) if transient(&e) => {
                return Err(std::io::Error::new(
                    e.kind(),
                    format!("{e} (gave up after {RENAME_RETRIES} retries × {RENAME_BACKOFF:?})"),
                ));
            }
            other => return other,
        }
    }
}

/// Staged filesystem writer rooted at a project directory.
pub struct FsWriter {
    root: PathBuf,
}

impl FsWriter {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

fn io(rel: &str, what: &str, e: std::io::Error) -> cv_core::Error {
    cv_core::Error::Other(format!("{rel}: {what}: {e}"))
}

impl ContentWriter for FsWriter {
    fn write(&self, draft: &PageDraft) -> cv_core::Result<WriteOutcome> {
        let rel = draft.path.as_str();
        let target =
            confined_file(&self.root, Path::new(rel)).map_err(|e| io(rel, "resolve", e))?;
        let check = |target: &Path| -> cv_core::Result<Option<WriteOutcome>> {
            let found = retry_pinned(
                || fingerprint_file(target),
                is_sharing_violation,
                std::thread::sleep,
            )
            .map_err(|e| io(rel, "fingerprint", e))?;
            match (found, draft.base) {
                (None, None) => Ok(None),
                (Some(found), Some(base)) if found == base => Ok(None),
                (Some(found), _) => Ok(Some(WriteOutcome::StaleBase { found })),
                (None, Some(_)) => Err(cv_core::Error::Other(format!(
                    "{rel}: the file was removed since it was opened"
                ))),
            }
        };
        if let Some(stale) = check(&target)? {
            return Ok(stale);
        }
        let target = create_confined_parent(&self.root, Path::new(rel))
            .map_err(|e| io(rel, "create dir", e))?;
        let dir = target.parent().unwrap_or(&self.root);
        let mut tmp = tempfile::Builder::new()
            .prefix(".cv-write-")
            .suffix(".tmp")
            .tempfile_in(dir)
            .map_err(|e| io(rel, "stage", e))?;
        tmp.write_all(draft.body.as_bytes())
            .and_then(|()| tmp.as_file().sync_all())
            .map_err(|e| io(rel, "stage", e))?;
        let mut tmp = Some(tmp);
        // Re-check the base on every attempt: a writer holding the file
        // during a retry may have changed it before releasing, and a
        // directory may have been swapped for a link.
        let stale = retry_pinned(
            || -> std::io::Result<Option<WriteOutcome>> {
                let target = confirm_confined_file(&self.root, Path::new(rel))?;
                match (fingerprint_file(&target)?, draft.base) {
                    (None, None) => {}
                    (Some(found), Some(base)) if found == base => {}
                    (Some(found), _) => return Ok(Some(WriteOutcome::StaleBase { found })),
                    (None, Some(_)) => {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::NotFound,
                            "the file was removed since it was opened",
                        ));
                    }
                }
                let staged = tmp
                    .take()
                    .ok_or_else(|| std::io::Error::other("no staged save"))?;
                match staged.persist(&target) {
                    Ok(_) => Ok(None),
                    Err(e) => {
                        tmp = Some(e.file);
                        Err(e.error)
                    }
                }
            },
            is_sharing_violation,
            std::thread::sleep,
        )
        .map_err(|e| io(rel, "rename", e))?;
        if let Some(stale) = stale {
            return Ok(stale);
        }
        let fingerprint = fingerprint_file(&target)
            .map_err(|e| io(rel, "fingerprint", e))?
            .ok_or_else(|| cv_core::Error::Other(format!("{rel}: vanished after rename")))?;
        Ok(WriteOutcome::Written { fingerprint })
    }
}

#[cfg(test)]
#[path = "save_tests.rs"]
mod tests;
