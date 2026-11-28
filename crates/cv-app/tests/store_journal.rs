//! The `Store` port binds to the opened workspace: journaled envelopes
//! land in `<root>/.cv/store/docs.jsonl`, Doctor reports the sidecar,
//! and fixture sessions never write into a real project's journal.

use cv_app::{WorkspaceEnv, init, open_workspace};
use cv_core::events::shell::DegradedReason;
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

fn journal_keys(root: &Path) -> Vec<String> {
    std::fs::read_to_string(root.join(".cv/store/docs.jsonl"))
        .unwrap()
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_slice::<serde_json::Value>(line.as_bytes())
                .unwrap()
                .get("key")
                .and_then(|k| k.as_str())
                .unwrap_or("")
                .to_string()
        })
        .collect()
}

#[test]
fn sequential_workbenches_resume_the_durable_shell_event_sequence() {
    let root = tempfile::tempdir().unwrap();
    let recent = tempfile::tempdir().unwrap();
    {
        let mut wb = init();
        open(&mut wb, root.path(), &absent_env(recent.path()));
        wb.disclose(DegradedReason::WatchDead);
    }
    let first = journal_keys(root.path());
    assert!(
        !first.is_empty(),
        "first workbench journaled shell evidence"
    );

    {
        let mut wb = init();
        open(&mut wb, root.path(), &absent_env(recent.path()));
        wb.disclose(DegradedReason::WatchDead);
    }
    let all = journal_keys(root.path());
    assert!(
        all.len() > first.len(),
        "second workbench appended evidence"
    );
    let unique: std::collections::BTreeSet<_> = all.iter().collect();
    assert_eq!(
        unique.len(),
        all.len(),
        "event keys must not collapse distinct durable evidence: {all:?}"
    );
    for key in &first {
        let first_count = first.iter().filter(|k| *k == key).count();
        let all_count = all.iter().filter(|k| *k == key).count();
        assert_eq!(
            first_count, all_count,
            "second workbench reused shell event key {key}"
        );
    }
}

#[test]
fn recoverable_shell_journal_corruption_is_disclosed_without_hiding_valid_records() {
    let root = tempfile::tempdir().unwrap();
    let recent = tempfile::tempdir().unwrap();
    let journal = root.path().join(".cv/store/docs.jsonl");
    {
        let (store, _) = cv_store::docs::DiskStore::open(root.path()).unwrap();
        store
            .put(
                &cv_core::store::Namespace("shell.journal".into()),
                &cv_core::id::StableId("preexisting".into()),
                &serde_json::json!({"valid": true}),
            )
            .unwrap();
    }
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&journal)
            .unwrap();
        writeln!(
            f,
            "{}",
            serde_json::json!({"schema_version": 9, "ns": "shell.journal", "key": "future", "doc": {}})
        )
        .unwrap();
        writeln!(f, "not json").unwrap();
    }

    let mut wb = init();
    open(&mut wb, root.path(), &absent_env(recent.path()));
    assert_eq!(wb.app.degraded, Some(DegradedReason::StoreCorrupt));
    assert!(
        wb.status
            .contains("shell journal skipped 2 undecodable or unsupported line(s)"),
        "{}",
        wb.status
    );
    drop(wb);

    let (store, _) = cv_store::docs::DiskStore::open(root.path()).unwrap();
    let docs = store
        .query(
            &cv_core::store::Namespace("shell.journal".into()),
            &cv_core::store::Predicate::default(),
        )
        .unwrap();
    assert!(
        docs.iter()
            .any(|d| d.get("valid") == Some(&serde_json::json!(true))),
        "valid records remain recoverable beside skipped evidence: {docs:?}"
    );
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
