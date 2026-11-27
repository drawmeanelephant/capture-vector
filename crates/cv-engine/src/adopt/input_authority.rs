use super::AdoptError;
use crate::paths::{canonical_parts, normalize_component};
use cv_store::paths::portable_relative;
use std::path::Path;

fn metadata_input(raw: &str) -> bool {
    let parts = Path::new(raw)
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(n) => Some(normalize_component(&n.to_string_lossy())),
            _ => None,
        })
        .collect::<Vec<_>>();
    parts.first().is_some_and(|p| p == ".cv" || p == ".boris")
}

/// The content-input authority adoption and its gate must share exactly.
/// Empty, whitespace-padded, absolute, non-portable, whole-workspace, and
/// metadata/lane spellings are not normalized after the fact: they are
/// pathological workspace evidence and refuse before the recovery journal
/// or any content byte is written. Internal spaces and Unicode are
/// legitimate and pass byte-exactly.
pub(super) fn content_input_authority(raw: &str) -> Result<&str, AdoptError> {
    if raw.is_empty()
        || raw == "."
        || raw.trim().is_empty()
        || raw != raw.trim()
        || Path::new(raw).is_absolute()
    {
        return Err(AdoptError::Content(format!(
            "content input {raw:?} is not a portable workspace-relative authority"
        )));
    }
    portable_relative(raw)
        .map_err(|_| AdoptError::Content(format!("content input {raw:?} is not portable")))?;
    if metadata_input(raw) {
        return Err(AdoptError::Content(format!(
            "content input {raw:?} overlaps workspace metadata or an output lane"
        )));
    }
    Ok(raw)
}

/// Re-check the already validated lexical authority against the target's
/// filesystem before the mutation carrier exists. A symlinked input prefix
/// must not resolve into `.cv` or `.boris` under Unicode/case-canonical
/// comparison, even when that symlink is currently broken because `.cv`
/// has not been created yet.
pub(super) fn content_input_target_authority(
    target_root: &Path,
    raw: &str,
) -> Result<(), AdoptError> {
    let input = canonical_parts(&target_root.join(raw)).map_err(|e| {
        AdoptError::Content(format!(
            "content input {raw:?} cannot be canonicalized for metadata validation: {e}"
        ))
    })?;
    for metadata in [".cv", ".boris"] {
        let metadata = canonical_parts(&target_root.join(metadata)).map_err(|e| {
            AdoptError::Content(format!(
                "workspace metadata path {metadata:?} cannot be canonicalized: {e}"
            ))
        })?;
        if input.starts_with(&metadata) {
            return Err(AdoptError::Content(format!(
                "content input {raw:?} resolves into workspace metadata or an output lane"
            )));
        }
    }
    Ok(())
}
