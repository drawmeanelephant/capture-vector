//! Canonical filesystem paths in the form the app exposes. Windows
//! `canonicalize` returns `\\?\C:\…` verbatim paths; those are correct for
//! the syscalls that need them but hostile in the UI, recent list, and
//! child-process working directories.

use std::path::{Path, PathBuf};

use unicode_casefold::UnicodeCaseFold;
use unicode_normalization::UnicodeNormalization;

/// Unicode/canonical comparison form for one path component. NFC first
/// catches composed/decomposed aliases; full case folding then catches
/// aliases ordinary lowercase misses (`ß`/`SS`, sigma forms, ligatures);
/// the final NFC pass keeps the folded form canonical.
pub(crate) fn normalize_component(raw: &str) -> String {
    let folded: String = raw.nfc().case_fold().collect();
    folded.nfc().collect()
}

/// Normalized comparison components. The root separator is common to all
/// absolute paths on one host; Windows prefixes (drive/share) are retained
/// because they distinguish roots. Non-normal components are not path
/// names and never participate in overlap comparisons.
pub(crate) fn normalized_parts(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|c| match c {
            std::path::Component::Prefix(p) => {
                Some(normalize_component(&p.as_os_str().to_string_lossy()))
            }
            std::path::Component::Normal(n) => Some(normalize_component(&n.to_string_lossy())),
            _ => None,
        })
        .collect()
}

/// Canonicalize the deepest existing prefix, then reattach the not-yet-
/// existing suffix lexically. This resolves symlinks where filesystem
/// identity exists without pretending a missing suffix has one.
pub(crate) fn canonicalize_with_missing(path: &Path) -> std::io::Result<PathBuf> {
    fn go(path: &Path, depth: usize) -> std::io::Result<PathBuf> {
        let mut cur: PathBuf = path.components().collect();
        let mut missing = Vec::new();
        loop {
            if cur.try_exists()? {
                cur = cur.canonicalize()?;
                break;
            }
            // A broken symlink still has resolution semantics once its target
            // appears. Resolve it deliberately instead of treating the link
            // itself as a missing lexical component.
            match std::fs::symlink_metadata(&cur) {
                Ok(meta) if meta.file_type().is_symlink() => {
                    if depth >= 40 {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "symlink resolution depth exceeded",
                        ));
                    }
                    let target = std::fs::read_link(&cur)?;
                    let resolved = if target.is_absolute() {
                        target
                    } else {
                        cur.parent().unwrap_or(path).join(target)
                    };
                    cur = go(&resolved, depth + 1)?;
                    break;
                }
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
            let Some(name) = cur.file_name().map(|n| n.to_os_string()) else {
                break;
            };
            missing.push(name);
            if !cur.pop() {
                break;
            }
        }
        for name in missing.into_iter().rev() {
            cur.push(name);
        }
        Ok(cur)
    }
    go(path, 0)
}

/// Unicode/case-normalized components of `path` after resolving every
/// existing component that can be resolved.
pub(crate) fn canonical_parts(path: &Path) -> std::io::Result<Vec<String>> {
    canonicalize_with_missing(path).map(|p| normalized_parts(&p))
}

/// Remove only the Windows drive-letter verbatim prefix (`\\?\C:\` →
/// `C:\`). UNC verbatim paths keep their prefix: stripping it would change
/// how the path resolves.
pub fn strip_verbatim(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        if let Some(s) = path.to_str()
            && let Some(rest) = s.strip_prefix(r"\\?\")
            && rest.as_bytes().get(1) == Some(&b':')
        {
            return PathBuf::from(rest);
        }
    }
    path
}

/// `canonicalize`, then put drive-letter paths in display form.
pub fn canonical_display(root: &Path) -> std::io::Result<PathBuf> {
    std::fs::canonicalize(root).map(strip_verbatim)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_display_round_trips_an_ordinary_directory() {
        let t = tempfile::tempdir().unwrap();
        let p = canonical_display(t.path()).unwrap();
        assert!(p.is_absolute());
        #[cfg(windows)]
        assert!(!p.to_string_lossy().starts_with(r"\\?\"));
        assert_eq!(p, strip_verbatim(std::fs::canonicalize(t.path()).unwrap()));
    }

    #[cfg(windows)]
    #[test]
    fn only_drive_letter_verbatim_prefixes_are_stripped() {
        assert_eq!(
            strip_verbatim(PathBuf::from(r"\\?\C:\work\site")),
            PathBuf::from(r"C:\work\site")
        );
        assert_eq!(
            strip_verbatim(PathBuf::from(r"\\?\UNC\server\share\site")),
            PathBuf::from(r"\\?\UNC\server\share\site")
        );
    }
}
