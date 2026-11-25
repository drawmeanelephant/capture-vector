//! File-identity editor lookup. Case-only spellings can name the same
//! file on case-insensitive volumes, while remaining distinct files on
//! case-sensitive volumes. Device+file-index identity separates those two
//! facts instead of lowercasing paths globally.

use cv_store::paths::confined_file;
use cv_ui_editor::BufferId;
use cv_widgets::editor::host;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Identity {
    volume: u64,
    file: u64,
}

#[cfg(unix)]
fn identity(path: &Path) -> Option<Identity> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::metadata(path).ok()?;
    Some(Identity {
        volume: m.dev(),
        file: m.ino(),
    })
}

#[cfg(windows)]
fn identity(path: &Path) -> Option<Identity> {
    use std::os::windows::fs::MetadataExt;
    let m = std::fs::metadata(path).ok()?;
    Some(Identity {
        volume: m.volume_serial_number()?.into(),
        file: m.file_index()?,
    })
}

#[cfg(not(any(unix, windows)))]
fn identity(_: &Path) -> Option<Identity> {
    None
}

fn confined(root: &Path, rel: &str) -> Option<PathBuf> {
    confined_file(root, Path::new(rel)).ok()
}

/// Find an editor buffer for `rel` by real file identity. Falling back to
/// the session's normalized string lookup keeps non-file/test buffers
/// usable on platforms that expose no file index.
pub fn buffer_for(root: &Path, rel: &str) -> Option<BufferId> {
    let ses = host::session();
    let ses = ses.borrow();
    let Some(target) = confined(root, rel).and_then(|p| identity(&p)) else {
        return ses.find(rel);
    };
    ses.ids()
        .find(|id| {
            ses.get(*id).is_some_and(|entry| {
                confined(root, entry.buffer.path()).and_then(|p| identity(&p)) == Some(target)
            })
        })
        .or_else(|| ses.find(rel))
}
