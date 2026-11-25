//! Editor companions: load a page into the shared editor session and pin
//! it as a companion window (`cv_widgets::editor::MODULE`). Runs on a
//! command/boot path, never inside a frame.

use cv_model::windows::WindowId;
use cv_model::workbench::Workbench;
use cv_store::paths::confined_file;
use cv_ui_editor::completion::CompletionIndex;
use cv_ui_editor::save::{FsWriter, fingerprint_file};
use cv_widgets::editor::{self, host};
use std::path::{Component, Path};
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

/// Open a Pages row for editing: `page` is relative to the workspace's
/// content input, as Pages keys are.
pub fn edit_page(wb: &mut Workbench, page: &str) {
    let Some(root) = wb.workspace.clone() else {
        wb.status = format!("open a workspace to edit {page}");
        return;
    };
    let input = crate::snapshot::content_input(&root);
    open_for_editing(wb, &under_input(&input, page));
}

/// `input/page` without `.` segments, so a page opened from Pages and
/// the same page named by `CV_EDIT` share one buffer.
fn under_input(input: &str, page: &str) -> String {
    Path::new(input)
        .join(page)
        .components()
        .filter(|c| *c != Component::CurDir)
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// `CV_EDIT=<page>` opens that page for editing at launch, relative to
/// the open workspace (else `CV_PROJECT`, else the working directory). A
/// failure is shown in the status line.
pub fn open_from_env(wb: &mut Workbench) {
    let Ok(rel) = std::env::var("CV_EDIT") else {
        return;
    };
    open_for_editing(wb, &rel);
}

fn open_for_editing(wb: &mut Workbench, rel: &str) {
    let root = wb
        .workspace
        .clone()
        .or_else(crate::workspace::project_env)
        .unwrap_or_else(|| ".".into());
    match open_page(wb, &root, rel) {
        Ok(_) => crate::logging::info("editor_open", &[("page", serde_json::json!(rel))]),
        Err(e) => {
            crate::logging::warn(
                "editor_open_failed",
                &[
                    ("page", serde_json::json!(rel)),
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

    /// A workspace `root` beside a file holding text that must never
    /// reach an editor buffer.
    fn root_beside_a_secret() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("ws");
        std::fs::create_dir_all(root.join("content")).unwrap();
        std::fs::write(dir.path().join("secret.md"), "outside the workspace").unwrap();
        (dir, root)
    }

    fn assert_refused(wb: &Workbench, err: std::io::Error) {
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        assert!(wb.windows.companions.is_empty());
    }

    /// Directory links only: a Windows directory symlink or junction and a
    /// Unix symlink all carry the reparse bit `is_link` checks. std has no
    /// cross-platform link API, so the tests share one helper.
    #[cfg(unix)]
    fn link_dir(target: &std::path::Path, link: &std::path::Path) -> std::io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }

    #[cfg(windows)]
    fn link_dir(target: &std::path::Path, link: &std::path::Path) -> std::io::Result<()> {
        std::os::windows::fs::symlink_dir(target, link)
    }

    #[cfg(unix)]
    fn link_file(target: &std::path::Path, link: &std::path::Path) -> std::io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }

    #[cfg(windows)]
    fn link_file(target: &std::path::Path, link: &std::path::Path) -> std::io::Result<()> {
        std::os::windows::fs::symlink_file(target, link)
    }

    #[test]
    fn open_page_refuses_parent_directory_traversal() {
        let (_dir, root) = root_beside_a_secret();
        let mut wb = crate::init();
        for rel in ["../secret.md", "content/../../secret.md"] {
            let err = open_page(&mut wb, &root, rel).unwrap_err();
            assert_refused(&wb, err);
            assert!(host::session().borrow().find(rel).is_none(), "{rel}");
        }
    }

    #[test]
    fn open_page_refuses_a_linked_page_that_escapes_the_root() {
        let (dir, root) = root_beside_a_secret();
        link_file(&dir.path().join("secret.md"), &root.join("content/p.md")).unwrap();
        let mut wb = crate::init();
        let err = open_page(&mut wb, &root, "content/p.md").unwrap_err();
        assert_refused(&wb, err);
        assert!(host::session().borrow().find("content/p.md").is_none());
    }

    #[test]
    fn open_page_refuses_a_page_under_a_linked_directory() {
        let (dir, root) = root_beside_a_secret();
        link_dir(dir.path(), &root.join("content/out")).unwrap();
        let mut wb = crate::init();
        let err = open_page(&mut wb, &root, "content/out/secret.md").unwrap_err();
        assert_refused(&wb, err);
        assert!(
            host::session()
                .borrow()
                .find("content/out/secret.md")
                .is_none()
        );
    }

    #[test]
    fn a_refused_page_is_reported_in_the_status_line() {
        let (_dir, root) = root_beside_a_secret();
        let mut wb = crate::init();
        wb.workspace = Some(root);
        open_for_editing(&mut wb, "../secret.md");
        assert_eq!(
            wb.status,
            "cannot open ../secret.md for editing: not a confined project-relative path"
        );
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
