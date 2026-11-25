//! File-identity editor lookup. Case-only spellings can name the same
//! file on case-insensitive volumes, while remaining distinct files on
//! case-sensitive volumes. Device+file-index identity separates those two
//! facts instead of lowercasing paths globally.

use cv_store::paths::confined_file;
use cv_ui_editor::BufferId;
use cv_widgets::editor::host;
use std::path::{Path, PathBuf};

fn confined(root: &Path, rel: &str) -> Option<PathBuf> {
    confined_file(root, Path::new(rel)).ok()
}

/// Find an editor buffer for `rel` by real file identity. Falling back to
/// the session's normalized string lookup keeps non-file/test buffers
/// usable on platforms that expose no file index.
pub fn buffer_for(root: &Path, rel: &str) -> Option<BufferId> {
    let ses = host::session();
    let ses = ses.borrow();
    let Some(target) = confined(root, rel).and_then(|p| cv_engine::file_identity::identity(&p))
    else {
        return ses.find(rel);
    };
    ses.ids()
        .find(|id| {
            ses.get(*id).is_some_and(|entry| {
                confined(root, entry.buffer.path())
                    .and_then(|p| cv_engine::file_identity::identity(&p))
                    == Some(target)
            })
        })
        .or_else(|| ses.find(rel))
}
