//! Research module through the shell: commands act on the current
//! selection, and a note leaving the list leaves the selection.

use cv_fixtures::stub_store::StubStore;
use cv_model::module::Registry;
use cv_model::workbench::Workbench;

fn wb() -> Workbench {
    let mut wb = Workbench::new(Registry::with_all()).with_store(Box::new(StubStore::new()));
    wb.run_command("shell.goto.research");
    wb.run_command("research.capture.idea");
    wb.run_command("research.capture.idea");
    wb
}

#[test]
fn archived_note_leaves_the_selection() {
    let mut wb = wb();
    wb.set_nav("inbox");
    wb.select_row(1);
    let key = wb.selected_key().unwrap().to_string();
    wb.run_command("research.triage.archive");
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
    wb.run_command("research.capture.idea");
    assert_eq!(
        wb.selected_key(),
        Some(key.as_str()),
        "key kept, index moved"
    );
    wb.clear_selection();
    wb.run_command("research.triage.archive");
    assert_eq!(wb.status, "research: no note selected");
}
