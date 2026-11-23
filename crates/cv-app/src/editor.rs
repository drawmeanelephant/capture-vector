//! Editor companions: load a page into the shared editor session and pin
//! it as a companion window (`cv_widgets::editor::MODULE`). Runs on a
//! command/boot path, never inside a frame.

use cv_model::windows::WindowId;
use cv_model::workbench::Workbench;
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
    if ses.borrow().find(rel).is_none() {
        let path = root.join(rel);
        let text = std::fs::read_to_string(&path)?;
        let base = fingerprint_file(&path)?;
        ses.borrow_mut().open(rel, &text, base);
    }
    if ses.borrow().completion.is_none()
        && let Ok(json) = std::fs::read_to_string(root.join(".boris/completion.json"))
    {
        ses.borrow_mut().completion = CompletionIndex::parse(&json).ok();
    }
    host::set_writer(Arc::new(FsWriter::new(root)));
    Ok(wb.windows.open_companion(editor::MODULE, rel, rel))
}

#[cfg(test)]
mod tests {
    use super::*;

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
    }
}
