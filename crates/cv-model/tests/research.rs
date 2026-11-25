//! Research module through the shell: commands act on the current
//! selection, and a note leaving the list leaves the selection.

use cv_fixtures::stub_store::StubStore;
use cv_model::module::Registry;
use cv_model::workbench::{KeyInput, KeyPress, Workbench};

/// Capture an idea through its prompt, typed as the user would.
fn capture(wb: &mut Workbench, text: &str) {
    wb.run_command("research.capture.idea");
    for c in text.chars() {
        wb.key(&KeyPress::new(KeyInput::Char(c)));
    }
    wb.key(&KeyPress::new(KeyInput::Enter));
}

fn wb() -> Workbench {
    let mut wb = Workbench::new(Registry::with_all()).with_store(Box::new(StubStore::new()));
    wb.run_command("shell.goto.research");
    capture(&mut wb, "Idea 1");
    capture(&mut wb, "Idea 2");
    wb
}

#[test]
fn archived_note_leaves_the_selection() {
    let mut wb = wb();
    wb.set_nav("inbox");
    wb.select_row(1);
    let key = wb.selected_key().unwrap().to_string();
    wb.run_command("research.triage.archive");
    assert!(wb.dialogs.is_open(), "archive asks first");
    assert_eq!(wb.list_len(), 2);
    wb.key(&KeyPress::new(KeyInput::Enter));
    assert_eq!(wb.status, format!("archived {key}"));
    assert_eq!(wb.list_len(), 1);
    assert_eq!(wb.selected_key(), None);
}

#[test]
fn moved_selection_follows_its_key_and_clearing_retargets_nothing() {
    let mut wb = wb();
    wb.set_nav("all");
    wb.select_row(1);
    let key = wb.selected_key().unwrap().to_string();
    capture(&mut wb, "Idea 3");
    assert_eq!(
        wb.selected_key(),
        Some(key.as_str()),
        "key kept, index moved"
    );
    wb.clear_selection();
    wb.run_command("research.triage.archive");
    assert_eq!(wb.status, "research: no note selected");
}

#[test]
fn escape_cancels_the_archive_and_the_note_stays() {
    let mut wb = wb();
    wb.set_nav("inbox");
    wb.select_row(0);
    wb.run_command("research.triage.archive");
    assert!(
        wb.key(&KeyPress::new(KeyInput::Char('j'))),
        "the dialog takes every key"
    );
    wb.key(&KeyPress::new(KeyInput::Escape));
    assert!(!wb.dialogs.is_open());
    assert_eq!(wb.list_len(), 2);
    assert!(
        wb.selected_key().is_some(),
        "j did not move the list under the dialog"
    );
}

#[test]
fn the_capture_prompt_stores_the_typed_text() {
    let mut wb = wb();
    wb.set_nav("all");
    let titles: Vec<String> = (0..wb.list_len()).map(|i| wb.row(i).title).collect();
    assert!(titles.contains(&"Idea 2".to_string()), "{titles:?}");
}
