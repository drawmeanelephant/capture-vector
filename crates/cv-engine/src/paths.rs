//! Canonical filesystem paths in the form the app exposes. Windows
//! `canonicalize` returns `\\?\C:\…` verbatim paths; those are correct for
//! the syscalls that need them but hostile in the UI, recent list, and
//! child-process working directories.

use std::path::{Path, PathBuf};

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
