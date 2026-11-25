//! Shared dialog scenes, one per kind: Confirm, Prompt and Form from the
//! Research consumers, Choose from the shell's recent workspaces, Secret
//! and Progress from specs a module would send.

use crate::research;
use cv_model::dialog::{ActionTone, DialogKind, DialogSpec, Progress, SecretField};
use cv_model::workbench::{KeyInput, KeyPress, Workbench};
use std::path::PathBuf;

fn type_text(wb: &mut Workbench, s: &str) {
    for c in s.chars() {
        wb.key(&KeyPress::new(KeyInput::Char(c)));
    }
}

pub fn confirm(wb: &mut Workbench) {
    research::notes(wb);
    wb.run_command("research.triage.archive");
}

pub fn prompt(wb: &mut Workbench) {
    research::notes(wb);
    wb.run_command("research.capture.idea");
    type_text(wb, "Mailboxes are saved filters");
}

pub fn form(wb: &mut Workbench) {
    research::notes(wb);
    wb.run_command("research.filter.build");
    // Tab to the value field and fill it, so the form shows a typed value.
    wb.key(&KeyPress::new(KeyInput::Tab));
    wb.key(&KeyPress::new(KeyInput::Tab));
    type_text(wb, "inbox");
}

pub fn choose(wb: &mut Workbench) {
    crate::boot(wb);
    wb.recent = ["field-notes", "handbook", "dogfood"]
        .iter()
        .map(|n| PathBuf::from("/Users/demo/sites").join(n))
        .collect();
    wb.run_command("shell.workspace.recent");
    type_text(wb, "no");
}

/// The fake secret typed here; the secret tests assert it reaches no
/// tree, layout, log or debug output.
pub const SECRET_SAMPLE: &str = "hunter2-app-password";

pub fn secret(wb: &mut Workbench) {
    crate::boot(wb);
    wb.dialogs.push(
        DialogSpec::new(
            "publish.credential",
            "Bluesky app password",
            DialogKind::Secret(SecretField {
                label: "App password".into(),
                name: "capture-vector/bsky-app-password".into(),
            }),
        )
        .body("Publishing to Bluesky needs an app password. It goes only to the secret store.")
        .cancel("Cancel")
        .action("store", "Store", ActionTone::Primary),
    );
    type_text(wb, SECRET_SAMPLE);
}

pub fn progress(wb: &mut Workbench) {
    crate::boot(wb);
    wb.dialogs.push(
        DialogSpec::new(
            "publish.export",
            "Exporting site",
            DialogKind::Progress(Progress {
                label: "42 of 100 pages".into(),
                permille: Some(420),
            }),
        )
        .cancel("Cancel export"),
    );
}
