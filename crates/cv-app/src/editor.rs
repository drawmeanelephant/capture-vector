//! Editor companions: load a page into the shared editor session and pin
//! it as a companion window (`cv_widgets::editor::MODULE`). Runs on a
//! command/boot path, never inside a frame.

use cv_model::windows::WindowId;
use cv_model::workbench::Workbench;
use cv_store::paths::confined_file;
use cv_ui_editor::completion::CompletionIndex;
use cv_ui_editor::save::{FsWriter, fingerprint_file};
use cv_widgets::editor::{self, host};
use std::path::Path;
use std::sync::Arc;

/// Open `rel` (repo-relative, under `root`) for editing. Saves stage
/// through `FsWriter` rooted at `root`; wikilink completion uses
/// `root/.boris/completion.json` when Boris has written one.
pub fn open_page(wb: &mut Workbench, root: &Path, rel: &str) -> std::io::Result<WindowId> {
    let ses = host::session();
    let path = confined_file(root, Path::new(rel))?;
    let base = fingerprint_file(&path)?;
    let found = ses.borrow().find(rel);
    match found {
        None => {
            let text = std::fs::read_to_string(&path)?;
            ses.borrow_mut().open(rel, &text, base);
        }
        // A clean buffer follows the disk; a dirty one keeps the user's
        // edits and its save reports the conflict.
        Some(id) if !ses.borrow().is_dirty(id) => {
            let mut s = ses.borrow_mut();
            if let Some(e) = s.get_mut(id)
                && e.buffer.base() != base
            {
                e.buffer.reload(&std::fs::read_to_string(&path)?, base);
            }
        }
        Some(_) => {}
    }
    if ses.borrow().completion.is_none()
        && let Ok(path) = confined_file(root, Path::new(".boris/completion.json"))
        && let Ok(json) = std::fs::read_to_string(path)
    {
        ses.borrow_mut().completion = CompletionIndex::parse(&json).ok();
    }
    host::set_writer(Arc::new(FsWriter::new(root)));
    Ok(wb.windows.open_companion(editor::MODULE, rel, rel))
}

/// `CV_EDIT=<page>` opens that page for editing at launch, relative to
/// the open workspace (else `CV_PROJECT`, else the working directory). A
/// failure is shown in the status line.
pub fn open_from_env(wb: &mut Workbench) {
    let Ok(rel) = std::env::var("CV_EDIT") else {
        return;
    };
    let root = wb
        .workspace
        .clone()
        .or_else(crate::workspace::project_env)
        .unwrap_or_else(|| ".".into());
    match open_page(wb, &root, &rel) {
        Ok(_) => crate::logging::info("editor_open", &[("page", serde_json::json!(&rel))]),
        Err(e) => {
            crate::logging::warn(
                "editor_open_failed",
                &[
                    ("page", serde_json::json!(&rel)),
                    ("error", serde_json::json!(e.to_string())),
                ],
            );
            wb.status = format!("cannot open {rel} for editing: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_page_requires_a_project_relative_path() {
        let dir = tempfile::tempdir().unwrap();
        let page = dir.path().join("p.md");
        std::fs::write(&page, "# Page").unwrap();
        let mut wb = crate::init();
        let err = open_page(&mut wb, dir.path(), page.to_str().unwrap()).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        assert!(wb.windows.companions.is_empty());
    }

    #[test]
    fn open_page_binds_a_companion_to_one_buffer() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("p.md"), "---\ntitle: P\n---\nbody\n").unwrap();
        let mut wb = crate::init();
        let a = open_page(&mut wb, dir.path(), "p.md").unwrap();
        let b = open_page(&mut wb, dir.path(), "p.md").unwrap();
        assert_eq!(a, b, "one window per buffer");
        let c = wb.windows.companions.iter().find(|c| c.id == a).unwrap();
        assert_eq!(
            (c.module.as_str(), c.key.as_str()),
            (editor::MODULE, "p.md")
        );
        assert!(host::session().borrow().find("p.md").is_some());
        assert!(open_page(&mut wb, dir.path(), "missing.md").is_err());

        std::fs::write(
            dir.path().join("p.md"),
            "---\ntitle: Q\n---\nchanged on disk\n",
        )
        .unwrap();
        open_page(&mut wb, dir.path(), "p.md").unwrap();
        let ses = host::session();
        let id = ses.borrow().find("p.md").unwrap();
        assert!(
            ses.borrow()
                .get(id)
                .unwrap()
                .buffer
                .text()
                .contains("changed on disk")
        );
    }
}
