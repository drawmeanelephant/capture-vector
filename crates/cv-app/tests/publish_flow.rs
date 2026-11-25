//! The publish verbs through the shell, headless: a real `cv_publish`
//! ledger in a temp workspace, the captured `happy` build standing in
//! for Boris, copy-to-folder as the only delivery.

use cv_app::publish::{DiskPort, scene};
use cv_ledger::journal::{self, Entry};
use cv_model::modules::publish::ID;
use cv_model::workbench::Workbench;
use std::path::Path;

const PREFLIGHT: &str = "publish.preflight.static-dist";

fn workbench(root: &Path) -> Workbench {
    let mut wb = cv_app::init_with_publish(Box::new(DiskPort::writable(root, scene::writer(root))));
    wb.run_command("shell.goto.publish");
    wb
}

fn reading(wb: &Workbench) -> String {
    let key = wb.selected_key().expect("a run is selected").to_string();
    let i = wb.registry.index_of(ID).unwrap();
    wb.registry
        .get(i)
        .unwrap()
        .reading(&key)
        .lines
        .into_iter()
        .map(|l| format!("{}: {}\n", l.label.unwrap_or_default(), l.text))
        .collect()
}

fn approved(root: &Path) -> Workbench {
    let mut wb = workbench(root);
    wb.run_command(PREFLIGHT);
    wb.run_command("publish.approve");
    wb.dialog_act("approve");
    wb
}

/// Press the reconcile dialog's row for choice `id`.
fn choose(wb: &mut Workbench, id: &str) {
    let rows = wb.dialogs.open().expect("a dialog").visible_choices();
    let row = rows
        .iter()
        .position(|(c, _)| c == id)
        .unwrap_or_else(|| panic!("no choice {id} in {rows:?}"));
    wb.dialog_choose(row);
}

fn entries(root: &Path) -> Vec<Entry> {
    journal::read(root)
        .unwrap()
        .expect("a journal")
        .records
        .into_iter()
        .map(|r| r.entry)
        .collect()
}

fn site(root: &Path) -> std::path::PathBuf {
    root.join(".cv/publish/run-000000-0/site")
}

#[test]
fn preflight_approve_dispatch_copies_the_sealed_files_to_the_folder() {
    let d = tempfile::tempdir().unwrap();
    let mut wb = workbench(d.path());
    wb.run_command(PREFLIGHT);
    assert_eq!(
        wb.status,
        "preflight sealed run-000000-0: approve it to publish"
    );
    assert!(reading(&wb).contains("state: granted"));

    wb.run_command("publish.approve");
    let body = wb
        .dialogs
        .open()
        .expect("approval dialog")
        .spec
        .body
        .clone();
    let body = body.unwrap();
    assert!(body.contains("5 files"), "{body}");
    assert!(body.contains("plan: "), "{body}");
    assert!(body.contains("Advisory findings (7)"), "{body}");
    wb.dialog_act("approve");
    assert!(reading(&wb).contains("state: approved"));
    assert!(!d.path().join("publish/static-dist").exists());

    wb.run_command("publish.dispatch");
    assert_eq!(wb.status, "dispatched run-000000-0: complete");
    assert!(reading(&wb).contains("state: complete"));
    let shipped = std::fs::read(d.path().join("publish/static-dist/index.html")).unwrap();
    assert_eq!(
        shipped,
        std::fs::read(site(d.path()).join("index.html")).unwrap()
    );

    let e = entries(d.path());
    let at = |f: fn(&Entry) -> bool| e.iter().position(f).unwrap();
    assert!(
        at(|e| matches!(e, Entry::Approved { .. })) < at(|e| matches!(e, Entry::Delivery(_))),
        "approval is durable before dispatch"
    );
}

#[test]
fn heuristic_findings_are_advisory_and_never_block_preflight() {
    let d = tempfile::tempdir().unwrap();
    let mut wb = workbench(d.path());
    wb.run_command(PREFLIGHT);
    let r = reading(&wb);
    assert!(r.contains("state: granted"), "{r}");
    assert!(
        r.contains("advisory: SEO-META-DESCRIPTION on index · heuristic"),
        "{r}"
    );
    assert!(!r.contains("block: "), "{r}");
}

#[test]
fn a_page_changed_after_preflight_is_refused_and_the_refusal_is_shown() {
    let d = tempfile::tempdir().unwrap();
    let mut wb = approved(d.path());
    scene::tamper(d.path());
    wb.run_command("publish.dispatch");
    assert!(
        wb.status
            .starts_with("dispatch to Static dist refused: seal broken"),
        "{}",
        wb.status
    );
    let r = reading(&wb);
    assert!(r.contains("state: approved"), "{r}");
    assert!(r.contains("seal broken"), "{r}");
    assert!(!d.path().join("publish/static-dist").exists());
    assert!(
        !entries(d.path())
            .iter()
            .any(|e| matches!(e, Entry::Delivery(_))),
        "nothing was dispatched"
    );
}

#[test]
fn an_unknown_run_reconciles_by_probe_to_a_terminal_state() {
    let d = tempfile::tempdir().unwrap();
    scene::seed_unknown_copy(d.path());
    let mut wb = workbench(d.path());
    wb.set_nav("unresolved");
    wb.select_row(0);
    assert!(reading(&wb).contains("state: unknown"));
    wb.run_command("publish.reconcile");
    choose(&mut wb, "probe");
    assert!(wb.status.contains("by probe: failed"), "{}", wb.status);
    let r = reading(&wb);
    assert!(r.contains("state: failed"), "{r}");
    assert!(!r.contains("next: reconcile"), "{r}");
}

#[test]
fn an_unknown_run_reconciles_from_what_the_user_observed() {
    let d = tempfile::tempdir().unwrap();
    scene::seed_unknown_copy(d.path());
    let mut wb = workbench(d.path());
    wb.set_nav("unresolved");
    wb.select_row(0);
    wb.run_command("publish.reconcile");
    choose(&mut wb, "complete");
    assert!(wb.status.contains("by local: complete"), "{}", wb.status);
    assert!(reading(&wb).contains("state: complete"));
}

#[test]
fn a_dispatched_run_is_never_dispatched_again() {
    let d = tempfile::tempdir().unwrap();
    let mut wb = approved(d.path());
    wb.run_command("publish.dispatch");
    wb.run_command("publish.dispatch");
    assert!(wb.status.contains("only an approved run"), "{}", wb.status);
    let deliveries = entries(d.path())
        .iter()
        .filter(|e| matches!(e, Entry::Delivery(_)))
        .count();
    assert_eq!(deliveries, 1);
}

#[test]
fn targets_without_a_decided_adapter_are_disabled_pending_185() {
    let d = tempfile::tempdir().unwrap();
    let wb = workbench(d.path());
    let cmds = wb.commands();
    let why = |id: &str| {
        cmds.iter()
            .find(|c| c.id == id)
            .unwrap_or_else(|| panic!("no {id}"))
            .disabled
            .clone()
    };
    for t in ["github-pages", "standard-site", "nostr-nip23"] {
        let w = why(&format!("publish.preflight.{t}")).unwrap_or_default();
        assert!(w.contains("pending #185"), "{t}: {w}");
    }
    assert_eq!(why(PREFLIGHT), None);
}

#[test]
fn cancelling_the_approval_records_nothing() {
    let d = tempfile::tempdir().unwrap();
    let mut wb = workbench(d.path());
    wb.run_command(PREFLIGHT);
    wb.run_command("publish.approve");
    wb.dialog_cancel();
    assert!(reading(&wb).contains("state: granted"));
    assert!(
        !entries(d.path())
            .iter()
            .any(|e| matches!(e, Entry::Approved { .. }))
    );
}

#[test]
fn a_discarded_grant_cannot_be_approved() {
    let d = tempfile::tempdir().unwrap();
    let mut wb = workbench(d.path());
    wb.run_command(PREFLIGHT);
    wb.run_command("publish.discard-grant");
    wb.dialog_act("discard");
    assert!(reading(&wb).contains("state: discarded"));
    wb.run_command("publish.approve");
    assert!(wb.dialogs.open().is_none());
}
