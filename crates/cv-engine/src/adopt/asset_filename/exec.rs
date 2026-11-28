//! Global-phase execution of a journaled transformation. Phases run in
//! order and each is idempotent: every step first checks whether the
//! journaled end state is already on disk, and a step that finds its work
//! done records it as resumed rather than applied. Backup links carry the
//! bytes through the whole operation, so no phase can strand the only copy:
//! the source is unlinked only after the destination holds the journaled
//! identity, and the backup is removed only after the destination is
//! verified.
//!
//! Hard links are the only move primitive — there is no copy fallback and
//! no rename over an existing file. If the filesystem cannot link or
//! report identity, planning already refused the run; a link failure here
//! is an error, not a silent degradation.
//!
//! Markdown post-image bytes are the lab candidate's, staged to a sibling
//! temp whose identity the journal records before the compare-and-replace.
//! Replace is rename(2)-atomic but the compare before it is best-effort:
//! nothing portable can bind "still the pre-image" to the rename itself, so
//! the code re-verifies immediately before renaming and treats anything
//! else as foreign.

use super::{AdoptError, AdoptionSource, journal_error, malformed};
use cv_store::imports as store;
use cv_store::paths::{confined_file, confirm_confined_file, create_confined_parent, is_link};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// Byte + identity state of one ordinary file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FileState {
    pub sha256: String,
    pub bytes: u64,
    pub identity: store::FileIdentity,
}

impl FileState {
    fn matches(&self, sha256: &str, bytes: u64, identity: store::FileIdentity) -> bool {
        self.sha256 == sha256 && self.bytes == bytes && self.identity == identity
    }
}

fn state(path: &Path) -> Result<Option<FileState>, AdoptError> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    if is_link(&meta) || !meta.is_file() {
        return Err(journal_error(format!(
            "{} is not an ordinary file",
            path.display()
        )));
    }
    let mut file = std::fs::File::open(path)?;
    let identity = crate::file_identity::identity_of_file(&file).ok_or_else(|| {
        AdoptError::Run(format!(
            "this platform cannot report a file identity for {}; the asset transformation cannot run",
            path.display()
        ))
    })?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(Some(FileState {
        sha256: store::sha256_hex(&bytes),
        bytes: bytes.len() as u64,
        identity: store::FileIdentity {
            volume: identity.0,
            index: identity.1,
        },
    }))
}

fn input_path(input_root: &Path, rel: &str) -> Result<PathBuf, AdoptError> {
    confined_file(input_root, Path::new(rel))
        .map_err(|e| AdoptError::Content(format!("{rel}: {e}")))
}

fn foreign(message: impl Into<String>) -> AdoptError {
    AdoptError::Foreign(vec![message.into()])
}

/// `link` a hard link; an `AlreadyExists` error means a same-shape path
/// is present, which is never silently overwritten — the caller re-reads
/// state to classify.
fn link(from: &Path, to: &Path) -> Result<(), AdoptError> {
    std::fs::hard_link(from, to).map_err(|e| link_error(e, from, to))
}

fn link_error(e: std::io::Error, from: &Path, to: &Path) -> AdoptError {
    #[cfg(unix)]
    let crosses_devices =
        e.kind() == std::io::ErrorKind::CrossesDevices || e.raw_os_error() == Some(libc::EXDEV);
    #[cfg(not(unix))]
    let crosses_devices = e.kind() == std::io::ErrorKind::CrossesDevices;
    if e.kind() == std::io::ErrorKind::AlreadyExists {
        foreign(format!("{} appeared while linking", to.display()))
    } else if crosses_devices {
        AdoptError::Run(format!(
            "cannot link {} from {}: the destination lives on a different volume than the run directory ({e}); the transformation has no copy fallback — unmount the foreign volume or move the content input onto the run volume, then retry",
            to.display(),
            from.display()
        ))
    } else {
        AdoptError::Io(e)
    }
}

pub(super) struct Outcome {
    pub written: usize,
    pub resumed: usize,
    /// Destination/path → per-op status, for provenance.
    pub statuses: BTreeMap<String, store::AssetOperationStatus>,
}

pub(super) struct ExecCtx<'a> {
    pub target_root: &'a Path,
    /// Absolute, already-confined content-input directory.
    pub input_root: &'a Path,
    pub lock: &'a store::RunLock,
    pub src: &'a AdoptionSource,
}

impl ExecCtx<'_> {
    fn source(&self, mv: &store::PlannedAssetMove) -> Result<PathBuf, AdoptError> {
        input_path(self.input_root, &mv.source)
    }
    fn dest(&self, mv: &store::PlannedAssetMove) -> Result<PathBuf, AdoptError> {
        create_confined_parent(self.input_root, Path::new(&mv.dest))
            .map_err(|e| AdoptError::Content(format!("{}: {e}", mv.dest)))
    }
    fn backup(&self, mv: &store::PlannedAssetMove) -> Result<PathBuf, AdoptError> {
        confined_file(self.target_root, Path::new(&mv.backup))
            .map_err(|e| AdoptError::Content(format!("{}: {e}", mv.backup)))
    }
    fn page(&self, md: &store::PlannedMarkdown) -> Result<PathBuf, AdoptError> {
        input_path(self.input_root, &md.page)
    }
    fn temp(&self, md: &store::PlannedMarkdown) -> Result<PathBuf, AdoptError> {
        input_path(self.input_root, &md.temp)
    }
    fn moves<'s>(
        &'s self,
        t: &'s store::PendingTransformation,
    ) -> impl Iterator<Item = &'s store::PlannedAssetMove> {
        t.components.iter().flat_map(|c| c.asset_moves.iter())
    }
    fn markdown<'s>(
        &'s self,
        t: &'s store::PendingTransformation,
    ) -> impl Iterator<Item = &'s store::PlannedMarkdown> {
        t.components.iter().flat_map(|c| c.markdown.iter())
    }
}

fn set_phase(
    ctx: &ExecCtx<'_>,
    journal: &mut store::PendingAdoption,
    phase: store::TransformationPhase,
) -> Result<(), AdoptError> {
    let t = journal
        .transformation
        .as_mut()
        .ok_or_else(|| malformed("journal lost its transformation snapshot"))?;
    if t.phase >= phase {
        return Ok(());
    }
    t.phase = phase;
    store::write_pending(ctx.lock, journal)?;
    Ok(())
}

fn phase_backups(ctx: &ExecCtx<'_>, t: &store::PendingTransformation) -> Result<(), AdoptError> {
    for mv in ctx.moves(t) {
        let backup = ctx.backup(mv)?;
        match state(&backup)? {
            Some(b) if b.matches(&mv.sha256, mv.bytes, mv.identity) => {}
            Some(_) => {
                return Err(foreign(format!(
                    "{} occupies the journaled backup path",
                    mv.backup
                )));
            }
            None => {
                let source = ctx.source(mv)?;
                match state(&source)? {
                    Some(s) if s.matches(&mv.sha256, mv.bytes, mv.identity) => {
                        if let Some(parent) = backup.parent() {
                            std::fs::create_dir_all(parent)?;
                        }
                        link(&source, &backup)?;
                        match state(&backup)? {
                            Some(b) if b.matches(&mv.sha256, mv.bytes, mv.identity) => {}
                            _ => {
                                return Err(journal_error(format!(
                                    "backup {} was linked but does not carry the journaled identity",
                                    mv.backup
                                )));
                            }
                        }
                    }
                    Some(_) => {
                        return Err(foreign(format!(
                            "{} is at the journaled move source but is not the journaled file",
                            mv.source
                        )));
                    }
                    None => {
                        // The source is already unlinked: legal only when the
                        // destination already carries the journaled identity —
                        // otherwise the bytes this move owns are gone.
                        let dest = ctx.dest(mv)?;
                        match state(&dest)? {
                            Some(d) if d.matches(&mv.sha256, mv.bytes, mv.identity) => {}
                            _ => {
                                return Err(journal_error(format!(
                                    "source {} is gone and neither backup nor destination carries its journaled identity",
                                    mv.source
                                )));
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

fn phase_dests(
    ctx: &ExecCtx<'_>,
    t: &store::PendingTransformation,
    outcome: &mut Outcome,
) -> Result<(), AdoptError> {
    for mv in ctx.moves(t) {
        let dest = ctx.dest(mv)?;
        match state(&dest)? {
            Some(d) if d.matches(&mv.sha256, mv.bytes, mv.identity) => {
                outcome
                    .statuses
                    .insert(mv.dest.clone(), store::AssetOperationStatus::Resumed);
                outcome.resumed += 1;
            }
            Some(_) => {
                return Err(foreign(format!(
                    "{} occupies a journaled destination but is not the journaled file",
                    mv.dest
                )));
            }
            None => {
                let source = ctx.source(mv)?;
                if let Some(s) = state(&source)?
                    && !s.matches(&mv.sha256, mv.bytes, mv.identity)
                {
                    return Err(foreign(format!(
                        "{} is at the journaled move source but is not the journaled file",
                        mv.source
                    )));
                }
                // The destination links the backup, not the source: the link
                // pair keeps the journaled identity even if the source was
                // unlinked by the interrupted attempt.
                let backup = ctx.backup(mv)?;
                match state(&backup)? {
                    Some(b) if b.matches(&mv.sha256, mv.bytes, mv.identity) => {
                        link(&backup, &dest)?;
                        match state(&dest)? {
                            Some(d) if d.matches(&mv.sha256, mv.bytes, mv.identity) => {}
                            _ => {
                                return Err(journal_error(format!(
                                    "destination {} was linked but does not carry the journaled identity",
                                    mv.dest
                                )));
                            }
                        }
                    }
                    _ => {
                        return Err(journal_error(format!(
                            "backup {} is missing or foreign; the move cannot proceed",
                            mv.backup
                        )));
                    }
                }
                outcome
                    .statuses
                    .insert(mv.dest.clone(), store::AssetOperationStatus::Applied);
                outcome.written += 1;
            }
        }
    }
    Ok(())
}

fn phase_markdown(
    ctx: &ExecCtx<'_>,
    journal: &mut store::PendingAdoption,
    outcome: &mut Outcome,
) -> Result<(), AdoptError> {
    // Take the component list out so the journal file can be updated for
    // staged identities mid-phase.
    let pages: Vec<store::PlannedMarkdown> = journal
        .transformation
        .as_ref()
        .map(|t| ctx.markdown(t).cloned().collect())
        .unwrap_or_default();
    for op in &pages {
        let temp = ctx.temp(op)?;
        let temp_state = state(&temp)?;
        if let Some(s) = &temp_state {
            let ours = op
                .staged_identity
                .is_some_and(|staged| staged == s.identity)
                && s.sha256 == op.post_sha256
                && s.bytes == op.post_bytes;
            if !ours {
                return Err(foreign(format!(
                    "{} is an unexplained staged-write temporary file; it was left in place",
                    op.temp
                )));
            }
        }
        let page = ctx.page(op)?;
        let Some(page_state) = state(&page)? else {
            return Err(journal_error(format!(
                "journaled page {} is missing",
                op.page
            )));
        };
        if page_state.sha256 == op.post_sha256 && page_state.bytes == op.post_bytes {
            if temp_state.is_some() {
                return Err(foreign(format!(
                    "{} was already rewritten yet a staged temp also survives; the temp was left in place",
                    op.page
                )));
            }
            outcome
                .statuses
                .insert(op.page.clone(), store::AssetOperationStatus::Resumed);
            outcome.resumed += 1;
            continue;
        }
        if !page_state.matches(&op.pre_sha256, op.pre_bytes, op.pre_identity) {
            return Err(foreign(format!(
                "{} changed after the transformation journal was written",
                op.page
            )));
        }
        if temp_state.is_none() {
            // Stage the post-image to a sibling temp; the journal records
            // its identity before the rename so a crash-left temp is ours.
            let post = super::evidence::read_candidate_bytes(ctx.target_root, ctx.src, &op.page)?;
            if store::sha256_hex(&post) != op.post_sha256 || post.len() as u64 != op.post_bytes {
                return Err(malformed(format!(
                    "candidate page {} no longer carries its post-image bytes",
                    op.page
                )));
            }
            write_temp(&temp, &post)?;
            let Some(staged) = state(&temp)? else {
                return Err(journal_error(format!(
                    "staged temp {} vanished after creation",
                    op.temp
                )));
            };
            if staged.sha256 != op.post_sha256 || staged.bytes != op.post_bytes {
                return Err(journal_error(format!(
                    "staged temp {} does not carry the journaled post-image",
                    op.temp
                )));
            }
            // Record the temp's identity in the journal before exposing it
            // to the rename: a crash now leaves a recognizable, never
            // silently deleted artifact.
            let t = journal
                .transformation
                .as_mut()
                .ok_or_else(|| malformed("journal lost its transformation snapshot"))?;
            for component in &mut t.components {
                for md in &mut component.markdown {
                    if md.page == op.page {
                        md.staged_identity = Some(staged.identity);
                    }
                }
            }
            store::write_pending(ctx.lock, journal)?;
        }
        // Compare-before-replace: re-verify the page is still the journaled
        // pre-image immediately before renaming. Portable atomic
        // compare-and-replace does not exist; this narrows, not closes, the
        // window and anything else fails closed on the next check.
        let page = confirm_confined_file(ctx.input_root, Path::new(&op.page))
            .map_err(|e| AdoptError::Content(format!("{}: {e}", op.page)))?;
        let checked = state(&page)?;
        if !checked.is_some_and(|s| s.matches(&op.pre_sha256, op.pre_bytes, op.pre_identity)) {
            return Err(foreign(format!(
                "{} changed while its rewrite was being staged",
                op.page
            )));
        }
        cv_store::paths::replace_file(&temp, &page)?;
        outcome
            .statuses
            .insert(op.page.clone(), store::AssetOperationStatus::Applied);
        outcome.written += 1;
    }
    Ok(())
}

/// Stage `bytes` at `path` with create-new, no-follow semantics. A
/// pre-existing entry is an error, never opened.
fn write_temp(path: &Path, bytes: &[u8]) -> Result<(), AdoptError> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NOCTTY);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_FLAG_OPEN_REPARSE_POINT.
        options.custom_flags(0x0020_0000);
    }
    let mut file = options.open(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            foreign(format!(
                "{} appeared while staging the rewrite temp",
                path.display()
            ))
        } else {
            AdoptError::Io(e)
        }
    })?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn phase_unlink(ctx: &ExecCtx<'_>, t: &store::PendingTransformation) -> Result<(), AdoptError> {
    for mv in ctx.moves(t) {
        // Reverify before unlinking: the destination must carry the
        // journaled identity or the source stays untouched.
        let dest = ctx.dest(mv)?;
        match state(&dest)? {
            Some(d) if d.matches(&mv.sha256, mv.bytes, mv.identity) => {}
            _ => {
                return Err(journal_error(format!(
                    "destination {} does not carry the journaled identity; source {} stays in place",
                    mv.dest, mv.source
                )));
            }
        }
        let backup = ctx.backup(mv)?;
        match state(&backup)? {
            Some(b) if b.matches(&mv.sha256, mv.bytes, mv.identity) => {}
            None => {}
            Some(_) => {
                return Err(foreign(format!(
                    "{} occupies the journaled backup path",
                    mv.backup
                )));
            }
        }
        let source = ctx.source(mv)?;
        match state(&source)? {
            None => {}
            Some(s) if s.matches(&mv.sha256, mv.bytes, mv.identity) => {
                std::fs::remove_file(&source)?;
            }
            Some(_) => {
                return Err(foreign(format!(
                    "{} is at the journaled move source but is not the journaled file; it stays in place",
                    mv.source
                )));
            }
        }
    }
    Ok(())
}

fn phase_verify(ctx: &ExecCtx<'_>, t: &store::PendingTransformation) -> Result<(), AdoptError> {
    for mv in ctx.moves(t) {
        let dest = ctx.dest(mv)?;
        match state(&dest)? {
            Some(d) if d.matches(&mv.sha256, mv.bytes, mv.identity) => {}
            _ => {
                return Err(journal_error(format!(
                    "destination {} failed post-write verification",
                    mv.dest
                )));
            }
        }
        let source = ctx.source(mv)?;
        if state(&source)?.is_some() {
            return Err(foreign(format!(
                "{} reappeared at the moved source path after its unlink",
                mv.source
            )));
        }
        // The target is verified; only now does the backup go.
        let backup = ctx.backup(mv)?;
        match state(&backup)? {
            Some(b) if b.matches(&mv.sha256, mv.bytes, mv.identity) => {
                std::fs::remove_file(&backup)?;
            }
            None => {}
            Some(_) => {
                return Err(foreign(format!(
                    "{} occupies the journaled backup path",
                    mv.backup
                )));
            }
        }
    }
    for md in ctx.markdown(t) {
        let page = ctx.page(md)?;
        match state(&page)? {
            Some(p) if p.sha256 == md.post_sha256 && p.bytes == md.post_bytes => {}
            _ => {
                return Err(journal_error(format!(
                    "page {} failed post-write verification",
                    md.page
                )));
            }
        }
        let temp = ctx.temp(md)?;
        if state(&temp)?.is_some() {
            return Err(foreign(format!(
                "{} is an unexplained staged-write temporary file; it was left in place",
                md.temp
            )));
        }
    }
    // Prune the now-empty backup directory.
    for component in &t.components {
        for mv in &component.asset_moves {
            if let Some(parent) = ctx.backup(mv)?.parent().map(|p| p.to_path_buf()) {
                let _ = std::fs::remove_dir(parent);
            }
        }
    }
    Ok(())
}

/// Run every journaled phase in order. On a fresh attempt each phase does
/// its work and advances the journal; on a resume attempt each phase
/// re-verifies the work a crashed attempt already did. The journaled
/// phase is advisory for ordering only — the executor always converges
/// from the live state.
pub(super) fn execute(
    ctx: &ExecCtx<'_>,
    journal: &mut store::PendingAdoption,
) -> Result<Outcome, AdoptError> {
    let mut outcome = Outcome {
        written: 0,
        resumed: 0,
        statuses: BTreeMap::new(),
    };
    let t_snapshot = |journal: &store::PendingAdoption| -> store::PendingTransformation {
        journal
            .transformation
            .clone()
            .expect("transformation journal present")
    };
    let t = t_snapshot(journal);
    phase_backups(ctx, &t)?;
    set_phase(ctx, journal, store::TransformationPhase::BackupsLinked)?;
    let t = t_snapshot(journal);
    phase_dests(ctx, &t, &mut outcome)?;
    set_phase(ctx, journal, store::TransformationPhase::DestinationsLinked)?;
    phase_markdown(ctx, journal, &mut outcome)?;
    set_phase(ctx, journal, store::TransformationPhase::MarkdownWritten)?;
    let t = t_snapshot(journal);
    phase_unlink(ctx, &t)?;
    set_phase(ctx, journal, store::TransformationPhase::SourcesUnlinked)?;
    let t = t_snapshot(journal);
    phase_verify(ctx, &t)?;
    set_phase(ctx, journal, store::TransformationPhase::Verified)?;
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::ErrorKind;

    #[cfg(unix)]
    #[test]
    fn exdev_classifies_as_cross_volume_run_failure() {
        let e = std::io::Error::from_raw_os_error(libc::EXDEV);
        let err = link_error(e, Path::new("backup/000"), Path::new("mounted/dest.png"));
        let AdoptError::Run(message) = err else {
            panic!("EXDEV must classify as a run failure, got {err:?}");
        };
        assert!(message.contains("mounted/dest.png"), "{message}");
        assert!(message.contains("different volume"), "{message}");
        assert!(message.contains("no copy fallback"), "{message}");
    }

    #[test]
    fn already_exists_stays_foreign_not_cross_volume() {
        let e = std::io::Error::new(ErrorKind::AlreadyExists, "occupied");
        let err = link_error(e, Path::new("backup/000"), Path::new("dest.png"));
        assert!(matches!(err, AdoptError::Foreign(_)), "{err:?}");
    }

    #[test]
    fn unrelated_link_errors_stay_io() {
        let e = std::io::Error::new(ErrorKind::PermissionDenied, "denied");
        let err = link_error(e, Path::new("backup/000"), Path::new("dest.png"));
        assert!(matches!(err, AdoptError::Io(_)), "{err:?}");
    }
}
