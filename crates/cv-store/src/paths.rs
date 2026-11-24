//! Resolve paths from project data without following links below the root.

use std::fs::{self, Metadata};
use std::io::{self, ErrorKind};
use std::path::{Component, Path, PathBuf};

/// Reject symlinks and Windows reparse points, including junctions.
pub fn is_link(meta: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    meta.file_type().is_symlink()
}

fn refused() -> io::Error {
    io::Error::new(
        ErrorKind::InvalidInput,
        "not a confined project-relative path",
    )
}

/// Resolve a project-relative path. Missing components are allowed for
/// creation. Existing components must be ordinary directories or files;
/// the chosen root itself may be a link. This checks the tree as found,
/// not against concurrent replacement of directory entries.
pub fn confined(root: &Path, relative: &Path) -> io::Result<PathBuf> {
    resolve(root, relative, false)
}

/// Resolve a path that must be a regular file if it already exists.
pub fn confined_file(root: &Path, relative: &Path) -> io::Result<PathBuf> {
    resolve(root, relative, true)
}

fn resolve(root: &Path, relative: &Path, file: bool) -> io::Result<PathBuf> {
    let mut resolved = std::path::absolute(root)?;
    let mut components = relative.components().peekable();
    let mut has_name = false;
    while let Some(component) = components.next() {
        let name = match component {
            Component::CurDir => continue,
            Component::Normal(name) => name,
            _ => return Err(refused()),
        };
        #[cfg(windows)]
        if !portable_name(name.to_str().ok_or_else(refused)?) {
            return Err(refused());
        }
        has_name = true;
        resolved.push(name);
        match fs::symlink_metadata(&resolved) {
            Ok(m) if is_link(&m) => return Err(refused()),
            Ok(m) if m.is_dir() && (!file || components.peek().is_some()) => {}
            Ok(m) if m.is_file() && components.peek().is_none() => {}
            Ok(_) => return Err(refused()),
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    if !has_name {
        return Err(refused());
    }
    Ok(resolved)
}

/// Validate a persisted `/`-separated path regardless of the host OS.
pub fn portable_relative(path: &str) -> io::Result<&Path> {
    if !path.split('/').all(portable_name) {
        return Err(refused());
    }
    Ok(Path::new(path))
}

fn portable_name(name: &str) -> bool {
    if name.is_empty()
        || name.ends_with(['.', ' '])
        || name.chars().any(|c| {
            c.is_ascii_control() || matches!(c, '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*')
        })
    {
        return false;
    }
    let stem = name.split('.').next().unwrap_or(name).trim_end_matches(' ');
    let stem = stem.to_ascii_uppercase();
    !matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) && !stem
        .strip_prefix("COM")
        .or_else(|| stem.strip_prefix("LPT"))
        .is_some_and(|n| {
            matches!(
                n,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_have_the_same_interpretation_across_platforms() {
        for path in ["content/index.html", "posts/東京.html", ".cv/publish"] {
            assert!(portable_relative(path).is_ok(), "{path}");
        }
        for path in [
            "",
            "/page",
            "a//b",
            "a/../b",
            r"a\b",
            "C:page",
            "page:stream",
            "aux.md",
            "a. /page",
        ] {
            assert!(portable_relative(path).is_err(), "{path}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn project_path_validation_refuses_linked_directories() {
        let root = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(other.path(), root.path().join("linked")).unwrap();

        assert!(confined_file(root.path(), Path::new("linked/nested/page.md")).is_err());
        assert_eq!(std::fs::read_dir(other.path()).unwrap().count(), 0);
        assert!(confined_file(root.path(), Path::new("real/nested/page.md")).is_ok());
    }
}
