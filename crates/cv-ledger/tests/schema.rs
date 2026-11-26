//! `.cv/ledger/` schema stamp, versioned independently of other
//! namespaces: pre-stamp journals upgrade, newer ones fail untouched.

use cv_core::id::StableId;
use cv_core::publish::RunState;
use cv_core::sys::Timestamp;
use cv_ledger::journal::{Entry, SIDECAR};
use cv_ledger::{Journal, LedgerError};
use cv_store::sidecar::{SidecarError, read_version, stamp_path};

fn seed(root: &std::path::Path) {
    let mut j = Journal::open(root).unwrap();
    j.append(
        Timestamp(1),
        Entry::RunState {
            run: StableId("run-1".into()),
            state: RunState::Dispatching,
            note: None,
        },
    )
    .unwrap();
}

#[test]
fn fresh_journal_is_stamped() {
    let ws = tempfile::tempdir().unwrap();
    drop(Journal::open(ws.path()).unwrap());
    assert_eq!(read_version(ws.path(), &SIDECAR).unwrap(), Some(2));
}

#[test]
fn unstamped_journal_is_upgraded_and_keeps_its_records() {
    let ws = tempfile::tempdir().unwrap();
    seed(ws.path());
    std::fs::remove_file(stamp_path(ws.path(), &SIDECAR)).unwrap();
    assert_eq!(read_version(ws.path(), &SIDECAR).unwrap(), Some(0));
    let j = Journal::open(ws.path()).unwrap();
    assert_eq!(j.records().len(), 1);
    assert_eq!(read_version(ws.path(), &SIDECAR).unwrap(), Some(2));
}

#[test]
fn newer_ledger_schema_fails_without_touching_the_journal() {
    let ws = tempfile::tempdir().unwrap();
    seed(ws.path());
    std::fs::write(
        stamp_path(ws.path(), &SIDECAR),
        br#"{"format":"cv-sidecar","namespace":"ledger","schema_version":5}"#,
    )
    .unwrap();
    let path = Journal::path_for(ws.path());
    let before = std::fs::read(&path).unwrap();
    let Err(err) = Journal::open(ws.path()) else {
        panic!("a v5 ledger must not open");
    };
    assert!(matches!(
        err,
        LedgerError::Schema(SidecarError::TooNew {
            namespace: "ledger",
            found: 5,
            supported: 2
        })
    ));
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn a_newer_research_schema_does_not_block_the_ledger() {
    let ws = tempfile::tempdir().unwrap();
    let research = ws.path().join(".cv/research");
    std::fs::create_dir_all(&research).unwrap();
    std::fs::write(
        research.join("schema.json"),
        br#"{"format":"cv-sidecar","namespace":"research","schema_version":99}"#,
    )
    .unwrap();
    seed(ws.path());
    assert_eq!(Journal::open(ws.path()).unwrap().records().len(), 1);
}
