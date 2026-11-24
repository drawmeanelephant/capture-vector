//! `.cv/research/` schema stamp: pre-stamp workspaces upgrade in place,
//! a newer schema fails honestly with the files untouched.

use cv_core::id::AccountId;
use cv_core::sys::{FixedClock, Timestamp};
use cv_notes::Capture;
use cv_notes::disk::{self, DiskError, SIDECAR};
use cv_store::sidecar::{Outcome, SidecarError, read_version, stamp_path};
use std::path::Path;

fn open(root: &Path) -> Result<(cv_notes::Research<disk::DiskPort>, disk::OpenReport), DiskError> {
    disk::open(
        root,
        AccountId("local".into()),
        Box::new(FixedClock(Timestamp(1_700_000_000_000))),
    )
}

fn seed(root: &Path) {
    let (mut r, rep) = open(root).unwrap();
    assert_eq!(rep.schema, Outcome::Fresh);
    r.capture(
        Capture::Idea {
            text: "graphs".into(),
        },
        Some("Graphs"),
    )
    .unwrap();
}

#[test]
fn unstamped_research_dir_is_upgraded_and_keeps_its_notes() {
    let ws = tempfile::tempdir().unwrap();
    seed(ws.path());
    // A workspace written before stamping existed.
    std::fs::remove_file(stamp_path(ws.path(), &SIDECAR)).unwrap();
    assert_eq!(read_version(ws.path(), &SIDECAR).unwrap(), Some(0));

    let (r, rep) = open(ws.path()).unwrap();
    assert_eq!(rep.schema, Outcome::Migrated { from: 0, to: 1 });
    assert_eq!(r.notebook.all().len(), 1);
    assert_eq!(read_version(ws.path(), &SIDECAR).unwrap(), Some(1));
    drop(r);
    assert_eq!(open(ws.path()).unwrap().1.schema, Outcome::Current);
}

#[test]
fn newer_research_schema_fails_without_touching_files() {
    let ws = tempfile::tempdir().unwrap();
    seed(ws.path());
    let stamp = stamp_path(ws.path(), &SIDECAR);
    std::fs::write(
        &stamp,
        br#"{"format":"cv-sidecar","namespace":"research","schema_version":2}"#,
    )
    .unwrap();
    let dir = ws.path().join(disk::DIR);
    let notes = std::fs::read(dir.join(disk::NOTES)).unwrap();
    let names = |d: &Path| {
        let mut v: Vec<_> = std::fs::read_dir(d)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        v.sort();
        v
    };
    let before = names(&dir);

    let Err(err) = open(ws.path()) else {
        panic!("a v2 research dir must not open");
    };
    assert!(matches!(
        err,
        DiskError::Schema(SidecarError::TooNew {
            namespace: "research",
            found: 2,
            supported: 1
        })
    ));
    assert_eq!(std::fs::read(dir.join(disk::NOTES)).unwrap(), notes);
    assert_eq!(names(&dir), before);
}
