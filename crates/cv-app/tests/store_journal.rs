//! The `Store` port binds to the opened workspace: journaled envelopes
//! land in `<root>/.cv/store/docs.jsonl`, Doctor reports the sidecar,
//! and fixture sessions never write into a real project's journal.

use cv_app::{WorkspaceEnv, init, open_workspace};
use cv_core::store::Store;
use cv_engine::locate::Locator;
use cv_model::modules::doctor::{self, Stamp};
use cv_model::workbench::Workbench;
use std::path::Path;

fn absent_env(recent: &Path) -> WorkspaceEnv {
    WorkspaceEnv {
        locator: Some(Locator::default()),
        lab_locator: Some(Locator::default()),
        recent_file: Some(recent.join("recent.json")),
    }
}

fn open(wb: &mut Workbench, root: &Path, env: &WorkspaceEnv) {
    open_workspace(wb, root.to_path_buf(), env)
        .expect("workspace opens")
        .join()
        .unwrap();
    wb.drain();
}

#[test]
fn open_workspace_journals_envelopes_to_the_cv_sidecar() {
    let root = tempfile::tempdir().unwrap();
    let recent = tempfile::tempdir().unwrap();
    let mut wb = init();
    open(&mut wb, root.path(), &absent_env(recent.path()));

    let journal = root.path().join(".cv/store/docs.jsonl");
    let body = std::fs::read_to_string(&journal).expect("the journal exists");
    assert!(body.contains("\"shell.journal\""), "{body}");
    assert!(
        root.path().join(".cv/store/schema.json").is_file(),
        "the sidecar is stamped"
    );

    // Doctor's store evidence covers the journal namespace.
    let facts = doctor::installed().get();
    let row = facts
        .store
        .namespaces
        .iter()
        .find(|n| n.name == "store")
        .expect("store namespace reported");
    assert_eq!(row.dir, ".cv/store");
    assert_eq!(row.stamp, Stamp::Version(1));

    // The journaled evidence reads back once the writer lock is dropped.
    drop(wb);
    let (store, _) = cv_store::docs::DiskStore::open(root.path()).unwrap();
    let journaled = store
        .query(
            &cv_core::store::Namespace("shell.journal".into()),
            &cv_core::store::Predicate::default(),
        )
        .unwrap();
    assert!(!journaled.is_empty());
}

#[test]
fn fixture_sessions_never_write_into_a_projects_journal() {
    let root = tempfile::tempdir().unwrap();
    let recent = tempfile::tempdir().unwrap();
    let mut wb = init();
    open(&mut wb, root.path(), &absent_env(recent.path()));
    let journal = root.path().join(".cv/store/docs.jsonl");
    let before = std::fs::read_to_string(&journal).unwrap().lines().count();

    cv_app::open_corpus(&mut wb, "happy");
    let after = std::fs::read_to_string(&journal).unwrap().lines().count();
    assert_eq!(
        before, after,
        "fixture envelopes stay out of the workspace journal"
    );
}
