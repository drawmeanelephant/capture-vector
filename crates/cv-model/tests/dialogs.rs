//! The shell's side of path fields: Browse asks the windowing layer for a
//! native picker, and its answer fills the field of the dialog that asked.

use cv_model::dialog::{DialogKind, DialogSpec, Field, FileFilter, Pick, PickPurpose, Value};
use cv_model::module::Registry;
use cv_model::workbench::{ShellRequest, Workbench};
use std::path::PathBuf;

fn import_form(wb: &mut Workbench) {
    wb.dialogs.push(
        DialogSpec::new(
            "research.import.enex",
            "Import notes",
            DialogKind::Form(vec![Field::path(
                "file",
                "Export file",
                "",
                Pick::File(vec![FileFilter::new("Evernote export", &["enex"])]),
            )]),
        )
        .cancel("Cancel"),
    );
}

fn field_value(wb: &Workbench) -> Value {
    wb.dialogs.open().unwrap().fields()[0].value()
}

#[test]
fn browse_asks_for_a_file_picker_with_the_fields_filters() {
    let mut wb = Workbench::new(Registry::with_all());
    import_form(&mut wb);
    wb.dialog_browse(0);
    assert_eq!(
        wb.requests,
        vec![ShellRequest::PickFile {
            filters: vec![FileFilter::new("Evernote export", &["enex"])],
            purpose: PickPurpose::DialogField {
                dialog: "research.import.enex".into(),
                field: 0,
            },
        }]
    );
}

#[test]
fn the_picked_path_fills_the_field_and_a_dismissed_picker_leaves_it() {
    let mut wb = Workbench::new(Registry::with_all());
    import_form(&mut wb);
    wb.dialog_browse(0);
    let Some(ShellRequest::PickFile { purpose, .. }) = wb.requests.pop() else {
        panic!("no picker request");
    };
    wb.picked(purpose.clone(), None);
    assert_eq!(field_value(&wb), Value::Path(String::new()));
    wb.picked(purpose, Some(PathBuf::from("/notes/export.enex")));
    assert_eq!(field_value(&wb), Value::Path("/notes/export.enex".into()));
}

#[test]
fn a_late_picker_answer_does_not_fill_a_different_dialog() {
    let mut wb = Workbench::new(Registry::with_all());
    import_form(&mut wb);
    wb.dialog_browse(0);
    let Some(ShellRequest::PickFile { purpose, .. }) = wb.requests.pop() else {
        panic!("no picker request");
    };
    wb.dialog_cancel();
    wb.dialogs.push(
        DialogSpec::new(
            "research.export",
            "Export notes",
            DialogKind::Form(vec![Field::path("file", "Folder", "", Pick::Folder)]),
        )
        .cancel("Cancel"),
    );
    wb.picked(purpose, Some(PathBuf::from("/notes/export.enex")));
    assert_eq!(field_value(&wb), Value::Path(String::new()));
}
