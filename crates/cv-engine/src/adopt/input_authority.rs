use super::AdoptError;
use cv_store::paths::portable_relative;
use std::path::Path;

/// The content-input authority adoption and its gate must share exactly.
/// Empty, whitespace-padded, absolute, and non-portable spellings are not
/// normalized after the fact: they are pathological workspace evidence and
/// refuse before the recovery journal or any content byte is written.
/// Internal spaces and Unicode are legitimate and pass byte-exactly.
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
    Ok(raw)
}
