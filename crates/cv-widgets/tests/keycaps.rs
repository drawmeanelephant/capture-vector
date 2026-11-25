//! Command tooltips follow the pinned keycap platform, not the build
//! target. A separate test binary: the pin is per process and the first
//! one wins, so this pins Linux before any runtime pins macOS.

use cv_model::theme::Theme;
use cv_model::windows::WindowId;
use cv_ui_editor::EditorSession;
use cv_widgets::editor::{self, EditorWindow};
use std::cell::RefCell;
use std::rc::Rc;

#[test]
fn editor_command_tooltips_name_ctrl_when_keycaps_are_not_macos() {
    cv_widgets::keys::pin_keycaps(rune::KeyPlatform::Linux);
    let ses = Rc::new(RefCell::new(EditorSession::new()));
    let buf = ses.borrow_mut().open("p.md", "one\n", None);
    let rt = editor::runtime(EditorWindow::new(WindowId(1), ses, buf, Theme::dark()));
    let tree = cv_widgets::tree::element_tree(&rt);
    let help = |key: &str| {
        tree["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["key"] == key)
            .unwrap_or_else(|| panic!("no element {key}"))["help"]
            .as_str()
            .map(str::to_owned)
    };
    assert_eq!(help("editor-undo").as_deref(), Some("Undo  Ctrl+Z"));
    assert_eq!(help("editor-redo").as_deref(), Some("Redo  Ctrl+Shift+Z"));
    assert_eq!(help("editor-save").as_deref(), Some("Save  Ctrl+S"));
}
