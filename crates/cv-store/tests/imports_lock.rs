//! Focused integration regressions for per-run lock acquisition.

use cv_store::imports::{self as store, Error, RunProbe};
use std::path::{Path, PathBuf};

fn legacy_run(root: &Path, id: &str) -> PathBuf {
    let dir = store::run_dir(root, id).unwrap();
    std::fs::create_dir_all(dir.join(store::OUT)).unwrap();
    let rec = store::RunRecord::started(
        id,
        "hugo",
        store::RecordedLab {
            tool_id: "lab".into(),
            source: "test".into(),
            path: "/lab".into(),
        },
        7,
    );
    let mut bytes = serde_json::to_vec_pretty(&rec).unwrap();
    bytes.push(b'\n');
    std::fs::write(dir.join(store::RECORD), bytes).unwrap();
    dir
}

#[test]
fn failed_lock_acquisition_refuses_without_touching_run_bytes() {
    let d = tempfile::tempdir().unwrap();
    let dir = legacy_run(d.path(), "held");
    let first = store::lock_run(d.path(), "held").unwrap();
    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    entries.sort();
    let record_before = std::fs::read(dir.join(store::RECORD)).unwrap();
    let lock_before = std::fs::read(dir.join(store::RUN_LOCK)).unwrap();

    assert!(matches!(
        store::lock_run(d.path(), "held"),
        Err(Error::LockHeld(_))
    ));
    assert_eq!(store::probe_run(d.path(), "held").unwrap(), RunProbe::Held);
    assert_eq!(
        std::fs::read(dir.join(store::RECORD)).unwrap(),
        record_before
    );
    assert_eq!(
        std::fs::read(dir.join(store::RUN_LOCK)).unwrap(),
        lock_before
    );
    let mut after: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    after.sort();
    assert_eq!(after, entries, "a refused acquisition creates nothing");
    drop(first);
}
