#![cfg(feature = "disk")]

use cv_core::id::AccountId;
use cv_core::sys::{FixedClock, Timestamp};
use cv_notes::disk::{self, DiskError, DiskPort, OpenReport};
use cv_notes::{Capture, Research};
use std::fs;
use std::io::Write;
use std::path::Path;

fn open(root: &Path) -> Result<(Research<DiskPort>, OpenReport), DiskError> {
    disk::open(
        root,
        AccountId("local".into()),
        Box::new(FixedClock(Timestamp(1_700_000_000_000))),
    )
}

#[test]
fn second_writer_cannot_repair_journals_or_change_index_until_first_drops() {
    let ws = tempfile::tempdir().unwrap();
    let (mut writer, _) = open(ws.path()).unwrap();
    let note = writer
        .capture(
            Capture::Idea {
                text: "writer lifecycle".into(),
            },
            Some("Lock regression"),
        )
        .unwrap();
    assert_eq!(writer.search("lifecycle"), vec![note.clone()]);

    // Seed torn tails that a successful opener would truncate.
    let dir = ws.path().join(disk::DIR);
    let journals = [dir.join(disk::NOTES), dir.join(disk::PROVENANCE)];
    let tail = b"{\"incomplete\"";
    let committed = journals.each_ref().map(|path| fs::read(path).unwrap());
    for path in &journals {
        fs::OpenOptions::new()
            .append(true)
            .open(path)
            .unwrap()
            .write_all(tail)
            .unwrap();
    }
    // The FTS index lives in OS app-data keyed by project, not `.cv/`.
    let index = cv_store::paths::project_index_dir(ws.path(), "research")
        .unwrap()
        .join(disk::INDEX);
    let index_before = fs::read(&index).unwrap();
    let Err(DiskError::Locked(lock_path)) = open(ws.path()) else {
        panic!("second writer must be rejected with a lock error");
    };
    for (path, bytes) in journals.iter().zip(&committed) {
        assert_eq!(fs::read(path).unwrap(), [bytes.as_slice(), tail].concat());
    }
    assert_eq!(fs::read(index).unwrap(), index_before);

    drop(writer);
    assert!(lock_path.is_file(), "the stable lock file is not unlinked");
    let (reopened, report) = open(ws.path()).unwrap();
    assert_eq!(report.notes.torn_bytes, tail.len() as u64);
    assert_eq!(report.provenance.torn_bytes, tail.len() as u64);
    assert_eq!(journals.map(|path| fs::read(path).unwrap()), committed);
    assert!(!report.index_rebuilt);
    assert_eq!(reopened.search("lifecycle"), vec![note]);
}

#[test]
fn failed_journal_open_releases_the_writer_lock() {
    let ws = tempfile::tempdir().unwrap();
    let dir = ws.path().join(disk::DIR);
    let notes = dir.join(disk::NOTES);
    fs::create_dir_all(&notes).unwrap();

    assert!(matches!(open(ws.path()), Err(DiskError::Io(_))));
    assert!(!dir.join(disk::PROVENANCE).exists());
    assert!(!dir.join(disk::INDEX).exists());

    fs::remove_dir(notes).unwrap();
    assert!(open(ws.path()).is_ok());
}
