//! Headless editor scenes: `cv-app --editor --scene <name> --tree t.json
//! --snap s.png`. The PNG gate (`scripts/check-ui-png.sh`) snaps them on
//! the canonical rasterizer; `cv-widgets`' editor tests hold the trees.

use cv_core::diagnostic::{Diagnostic, StderrDiagnostic};
use cv_core::engine::{Engine, Verb};
use cv_fixtures::stub_engine::StubEngine;
use cv_model::theme::Theme;
use cv_model::windows::WindowId;
use cv_ui_editor::EditorSession;
use cv_widgets::editor::host::Scene;
use cv_widgets::editor::{Closing, EditorWindow};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

/// Named editor scenes, each a fresh session in its own window.
pub const SCENES: &[Scene] = &[("broken-wikilink", broken_wikilink), ("close", close)];

fn window(path: &str, text: &str) -> EditorWindow {
    let ses = Rc::new(RefCell::new(EditorSession::new()));
    let buf = ses.borrow_mut().open(path, text, None);
    EditorWindow::new(WindowId(1), ses, buf, Theme::dark())
}

/// The captured `broken-wikilink` page with its build's diagnostics: one
/// fresh `EREFERENCEMISSING` problem on line 6.
fn broken_wikilink(w: &mut EditorWindow) {
    let corpus = "broken-wikilink";
    let page = cv_fixtures::fixtures_root().join("corpus-src/broken-wikilink/page.md");
    *w = window(
        "page.md",
        &std::fs::read_to_string(page).unwrap_or_default(),
    );
    let engine = StubEngine::new(corpus);
    let seen = Arc::new(Mutex::new(None));
    let s = seen.clone();
    let ran = engine.run(
        &Verb::Build,
        Box::new(move |env| *s.lock().unwrap_or_else(|e| e.into_inner()) = env.build_id),
    );
    let build = seen.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let (Ok(_), Some(build)) = (ran, build) else {
        return;
    };
    let diags: Vec<Diagnostic> = cv_fixtures::corpus(corpus)
        .file_str("build/stderr.txt")
        .lines()
        .filter_map(StderrDiagnostic::parse)
        .map(|d| Diagnostic {
            build_version: Some(build.clone()),
            ..Diagnostic::from(&d)
        })
        .collect();
    w.session.borrow_mut().ingest_build(build, &diags);
    w.refresh();
}

/// A window closed over unsaved edits, reopened asking what to do.
fn close(w: &mut EditorWindow) {
    *w = window("content/guide.md", "---\ntitle: Guide\n---\nFirst draft.\n");
    w.body.set_caret(w.body.text.len());
    w.body.insert("Unsaved line.\n");
    w.body_changed();
    w.closing = Closing::Confirm { stuck: false };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_scenes_build_the_states_they_name() {
        let mut w = window("x.md", "");
        broken_wikilink(&mut w);
        assert_eq!(w.problems().len(), 1);
        close(&mut w);
        assert!(w.is_dirty());
        assert_eq!(w.closing, Closing::Confirm { stuck: false });
    }
}
