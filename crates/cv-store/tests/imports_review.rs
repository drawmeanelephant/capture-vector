//! Review-log observation and operation-scoped append regressions.

use cv_store::imports::{self as store, ReviewLog, ReviewState, Scan};
use std::fs::OpenOptions;
use std::io::Write;

#[test]
fn review_observation_never_creates_or_repairs_state() {
    let d = tempfile::tempdir().unwrap();
    let (_log, got, scan) = ReviewLog::open(d.path()).unwrap();
    assert!(got.is_empty());
    assert_eq!(scan, Scan::default());
    assert!(
        !d.path().join(".cv").exists(),
        "read-only review observation must not create .cv state"
    );

    let base = d.path().join(store::DIR);
    std::fs::create_dir_all(&base).unwrap();
    let path = base.join(store::REVIEW);
    std::fs::write(&path, b"{\"torn").unwrap();
    let before = std::fs::read(&path).unwrap();
    let (_log, got, scan) = ReviewLog::open(d.path()).unwrap();
    assert!(got.is_empty());
    assert!(scan.torn_bytes > 0);
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(
        !base.join(store::REVIEW_LOCK).exists(),
        "observation must not create even the review lock"
    );
}

#[test]
fn review_states_persist_last_write_wins_without_truncating_torn_bytes() {
    let d = tempfile::tempdir().unwrap();
    let (mut log, got, _) = ReviewLog::open(d.path()).unwrap();
    assert!(got.is_empty());
    log.append("r1", "a", ReviewState::Accepted).unwrap();
    log.append("r1", "b", ReviewState::Deferred).unwrap();
    log.append("r1", "a", ReviewState::Open).unwrap();
    drop(log);
    let path = d.path().join(store::DIR).join(store::REVIEW);
    let mut f = OpenOptions::new().append(true).open(&path).unwrap();
    write!(f, "{{\"torn").unwrap();
    drop(f);
    let before_append = std::fs::read(&path).unwrap();

    let (_, got, scan) = ReviewLog::open(d.path()).unwrap();
    assert!(scan.torn_bytes > 0);
    assert_eq!(std::fs::read(&path).unwrap(), before_append);
    assert_eq!(
        got,
        vec![
            (("r1".into(), "a".into()), ReviewState::Open),
            (("r1".into(), "b".into()), ReviewState::Deferred),
        ]
    );

    let (mut writer, _, _) = ReviewLog::open(d.path()).unwrap();
    writer.append("r1", "a", ReviewState::Deferred).unwrap();
    let after_append = std::fs::read(&path).unwrap();
    assert!(
        after_append.starts_with(&before_append),
        "append must not truncate or rewrite the torn tail"
    );
    assert!(after_append.len() > before_append.len());
    drop(writer);
    let (_, states, scan) = ReviewLog::open(d.path()).unwrap();
    assert_eq!(
        states,
        vec![
            (("r1".into(), "a".into()), ReviewState::Deferred),
            (("r1".into(), "b".into()), ReviewState::Deferred),
        ]
    );
    assert!(scan.undecodable > 0, "the torn bytes remain visible");
    assert!(store::scan(d.path()).unwrap().is_empty());
}

#[test]
fn independent_review_windows_use_operation_scoped_appends() {
    let d = tempfile::tempdir().unwrap();
    let (mut first, _, _) = ReviewLog::open(d.path()).unwrap();
    let (mut second, _, _) = ReviewLog::open(d.path()).unwrap();
    first.append("r1", "a", ReviewState::Accepted).unwrap();
    // Opening the second handle before/while the first exists does not
    // make it session-read-only: each append acquires and releases the
    // review lock for that operation only.
    second.append("r1", "a", ReviewState::Deferred).unwrap();
    drop(first);
    drop(second);
    let (_, states, _) = ReviewLog::open(d.path()).unwrap();
    assert_eq!(
        states,
        vec![(("r1".into(), "a".into()), ReviewState::Deferred)]
    );
}

#[test]
fn concurrent_review_appends_are_serialized_or_refused_never_unlocked() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().to_path_buf();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let mut threads = Vec::new();
    for (item, state) in [("a", ReviewState::Accepted), ("b", ReviewState::Deferred)] {
        let root = root.clone();
        let barrier = barrier.clone();
        threads.push(std::thread::spawn(move || {
            let (mut log, _, _) = ReviewLog::open(&root).unwrap();
            barrier.wait();
            log.append("r1", item, state)
        }));
    }
    let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    for result in &results {
        assert!(
            result.is_ok() || matches!(result, Err(store::Error::LockHeld(_))),
            "append must serialize or refuse, never write unlocked: {results:?}"
        );
    }
    let successes = results.iter().filter(|r| r.is_ok()).count();
    assert!(successes >= 1, "at least one append must win: {results:?}");
    let (_, states, scan) = ReviewLog::open(d.path()).unwrap();
    assert_eq!(scan.torn_bytes, 0);
    assert_eq!(states.len(), successes);
}
