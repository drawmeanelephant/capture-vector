//! Resolve paths from project data without following links below the root.
//!
//! Policy: every component below the root must be an ordinary directory or
//! file; a symlink or reparse point is refused even when it points back
//! inside the root. The root itself may be reached through a link.

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

/// Resolve a staged-write target like [`confined_file`] and create its
/// missing parent directories one level at a time, checking each level is
/// a real directory before descending into it, so nothing is created
/// through a link. Ends with [`confirm_confined_file`]. A missing root is
/// created as before; the root is trusted and may be a link.
pub fn create_confined_parent(root: &Path, relative: &Path) -> io::Result<PathBuf> {
    let target = confined_file(root, relative)?;
    let mut dir = std::path::absolute(root)?;
    let below_root = target
        .parent()
        .and_then(|parent| parent.strip_prefix(&dir).ok())
        .ok_or_else(refused)?;
    fs::create_dir_all(&dir)?;
    for name in below_root.components() {
        dir.push(name);
        // `create_dir` never follows a link at the last component: an
        // existing link or file reports `AlreadyExists` and is refused below.
        match fs::create_dir(&dir) {
            Ok(()) => {}
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
        let meta = fs::symlink_metadata(&dir)?;
        if is_link(&meta) || !meta.is_dir() {
            return Err(refused());
        }
    }
    confirm_confined_file(root, relative)
}

/// Re-check a staged-write target right before staging or renaming: it
/// must still resolve per [`confined_file`], and the canonical form of its
/// parent must lie under the canonical root.
///
/// std offers no directory handles (`openat`/`O_NOFOLLOW`), so a process
/// that swaps an already checked directory for a link between this check
/// and the next create, stage, or rename can still redirect that single
/// operation. Callers re-check before each step to keep the window small.
pub fn confirm_confined_file(root: &Path, relative: &Path) -> io::Result<PathBuf> {
    let target = confined_file(root, relative)?;
    let parent = target.parent().ok_or_else(refused)?;
    if !fs::canonicalize(parent)?.starts_with(fs::canonicalize(root)?) {
        return Err(refused());
    }
    Ok(target)
}

/// Install a fully written replacement file in the same directory.
/// `std::fs::rename` replaces an existing file atomically on Unix and
/// through the Windows rename facilities on Windows.
pub fn replace_file(staged: &Path, target: &Path) -> io::Result<()> {
    fs::rename(staged, target)?;
    #[cfg(unix)]
    if let Some(parent) = target.parent() {
        std::fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

/// Base for rebuildable indexes: OS app-data, `CV_APP_DATA` overriding
/// (tests and hermetic runs point it at a scratch dir). Index files
/// must never live under `.cv/` — synced folders corrupt their WAL and
/// journal artifacts.
fn app_data_base() -> io::Result<PathBuf> {
    if let Some(d) = std::env::var_os("CV_APP_DATA") {
        return Ok(PathBuf::from(d));
    }
    #[cfg(windows)]
    {
        return std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .ok_or_else(|| io::Error::new(ErrorKind::NotFound, "APPDATA is unset"));
    }
    #[cfg(target_os = "macos")]
    {
        return home_dir().map(|h| h.join("Library/Application Support"));
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(d) = std::env::var_os("XDG_DATA_HOME") {
            return Ok(PathBuf::from(d));
        }
        return home_dir().map(|h| h.join(".local/share"));
    }
    #[allow(unreachable_code)]
    Err(io::Error::new(
        ErrorKind::Unsupported,
        "no app-data convention on this platform",
    ))
}

#[cfg(unix)]
fn home_dir() -> io::Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(ErrorKind::NotFound, "HOME is unset"))
}

/// Stable per-project key: a hash of the canonical root, so distinct
/// checkouts never share an index and a renamed project gets a fresh one.
fn project_key(project_root: &Path) -> io::Result<String> {
    use sha2::{Digest, Sha256};
    let canonical = project_root.canonicalize()?;
    let digest = Sha256::digest(canonical.as_os_str().as_encoded_bytes());
    Ok(digest[..8].iter().map(|b| format!("{b:02x}")).collect())
}

/// `…/capture-vector/<project-key>/<namespace>` under OS app-data —
/// the directory a lane's rebuildable index lives in. Resolve only;
/// use [`create_project_index_dir`] to open it for use.
pub fn project_index_dir(project_root: &Path, namespace: &str) -> io::Result<PathBuf> {
    Ok(app_data_base()?
        .join("capture-vector")
        .join(project_key(project_root)?)
        .join(namespace))
}

/// Create (or reuse) the index dir. Every level is owner-only on unix:
/// an index mirrors private workspace content — note bodies, journal
/// noun JSON — and a umask-022 default would leave it readable by other
/// local users.
pub fn create_project_index_dir(project_root: &Path, namespace: &str) -> io::Result<PathBuf> {
    let base = app_data_base()?.join("capture-vector");
    let keyed = base.join(project_key(project_root)?);
    let dir = keyed.join(namespace);
    for level in [&base, &keyed, &dir] {
        std::fs::create_dir_all(level)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(level, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(dir)
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

    /// Directory links only: a Windows directory symlink or junction and a
    /// Unix symlink all carry the reparse bit `is_link` checks. std has no
    /// cross-platform link API, so the tests share one helper.
    #[cfg(unix)]
    fn link_dir(target: &Path, link: &Path) -> io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }
    #[cfg(windows)]
    fn link_dir(target: &Path, link: &Path) -> io::Result<()> {
        std::os::windows::fs::symlink_dir(target, link)
    }

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

    #[test]
    fn project_index_dir_stays_outside_cv_and_is_keyed_per_project() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let ledger = project_index_dir(a.path(), "ledger").unwrap();
        assert_ne!(ledger, project_index_dir(b.path(), "ledger").unwrap());
        assert_ne!(ledger, project_index_dir(a.path(), "research").unwrap());
        let s = ledger.to_str().unwrap();
        assert!(!s.contains(".cv"), "{s}");
        assert_eq!(ledger.file_name().unwrap(), "ledger");
    }

    #[cfg(unix)]
    #[test]
    fn created_index_dirs_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let dir = create_project_index_dir(root.path(), "ledger").unwrap();
        for level in [dir.as_path(), dir.parent().unwrap()] {
            assert_eq!(
                level.metadata().unwrap().permissions().mode() & 0o777,
                0o700,
                "{level:?}"
            );
        }
    }

    #[test]
    fn project_path_validation_refuses_linked_directories() {
        let root = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        link_dir(other.path(), &root.path().join("linked")).unwrap();

        assert!(confined_file(root.path(), Path::new("linked/nested/page.md")).is_err());
        assert_eq!(std::fs::read_dir(other.path()).unwrap().count(), 0);
        assert!(confined_file(root.path(), Path::new("real/nested/page.md")).is_ok());
    }

    #[test]
    fn staged_parent_creation_builds_missing_directories_inside_the_root() {
        let root = tempfile::tempdir().unwrap();
        let target = create_confined_parent(root.path(), Path::new("a/b/c/page.md")).unwrap();
        assert_eq!(target, root.path().join("a/b/c/page.md"));
        assert!(root.path().join("a/b/c").is_dir());
        assert!(!target.exists());
    }

    #[test]
    fn staged_parent_creation_refuses_a_linked_directory_without_creating_outside() {
        let root = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        link_dir(other.path(), &root.path().join("linked")).unwrap();

        let err = create_confined_parent(root.path(), Path::new("linked/new/deeper/page.md"))
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidInput);
        assert_eq!(std::fs::read_dir(other.path()).unwrap().count(), 0);
    }

    #[test]
    fn staged_parent_creation_works_when_the_root_is_reached_through_a_link() {
        let real = tempfile::tempdir().unwrap();
        let holder = tempfile::tempdir().unwrap();
        let root = holder.path().join("workspace");
        link_dir(real.path(), &root).unwrap();

        create_confined_parent(&root, Path::new("content/new/page.md")).unwrap();
        assert!(real.path().join("content/new").is_dir());
    }

    #[test]
    fn confirmation_refuses_a_directory_swapped_for_a_link_after_creation() {
        let root = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        create_confined_parent(root.path(), Path::new("content/page.md")).unwrap();
        std::fs::rename(root.path().join("content"), root.path().join("moved")).unwrap();
        link_dir(other.path(), &root.path().join("content")).unwrap();

        let err = confirm_confined_file(root.path(), Path::new("content/page.md")).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidInput);
    }
}
