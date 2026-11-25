use cv_core::id::StableId;
use cv_core::publish::{RunState, RunStatus};
use cv_core::sys::{Hash, Timestamp};
use cv_ledger::ingest;
use cv_ledger::journal::{self, Entry};
use cv_ledger::nouns::kind;
use cv_ledger::{Journal, LedgerIndex, RunPhase, runs};

fn run() -> StableId {
    StableId("run-1".into())
}

fn proof(name: &str) -> Vec<u8> {
    cv_fixtures::corpus("happy").file(&format!("dist/dist/_boris/proof/{name}"))
}

fn state(j: &mut Journal, s: RunState) {
    j.append(
        Timestamp(1),
        Entry::RunState {
            run: run(),
            state: s,
            note: None,
        },
    )
    .unwrap();
}

#[test]
fn happy_proof_pack_lifts_to_first_class_nouns() {
    let (a, c, l) = (
        proof("artifacts.json"),
        proof("checks.json"),
        proof("claims.json"),
    );
    let n = ingest::from_bytes(&run(), Some(&a), Some(&c), Some(&l)).unwrap();
    assert_eq!(n.artifacts.len(), 5);
    assert_eq!(n.checks.len(), 3);
    assert_eq!(n.claims.len(), 3);
    assert!(n.checks.iter().all(|c| c.status == "passed" && c.ran));
    assert!(n.claims.iter().all(|c| c.check_id.is_some()));
    assert!(
        n.artifacts.iter().all(|a| !a.id.0.contains('/')),
        "noun ids are never paths"
    );
    let inv = cv_fixtures::sha256_hex(&a);
    assert!(
        n.checks
            .iter()
            .all(|c| c.inventory_sha256.as_deref() == Some(inv.as_str()))
    );
}

#[test]
fn journal_is_truth_and_sqlite_rebuilds_from_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut j = Journal::open(dir.path()).unwrap();
    let (a, c, l) = (
        proof("artifacts.json"),
        proof("checks.json"),
        proof("claims.json"),
    );
    let n = ingest::from_bytes(&run(), Some(&a), Some(&c), Some(&l)).unwrap();
    for x in n.artifacts {
        j.append(Timestamp(0), Entry::Artifact(x)).unwrap();
    }
    for x in n.checks {
        j.append(Timestamp(0), Entry::Check(x)).unwrap();
    }
    for x in n.claims {
        j.append(Timestamp(0), Entry::Claim(x)).unwrap();
    }
    state(&mut j, RunState::Dispatching);
    state(&mut j, RunState::Complete(RunStatus::Partial));

    let idx = LedgerIndex::open(dir.path(), &j).unwrap();
    let before = (
        idx.nouns(kind::ARTIFACT, "run-1").unwrap(),
        idx.nouns(kind::CLAIM, "run-1").unwrap(),
        idx.runs().unwrap(),
    );
    assert_eq!(before.0.len(), 5);
    assert_eq!(idx.run_phase("run-1").unwrap().as_deref(), Some("partial"));
    let path = idx.path().to_path_buf();
    drop(idx);
    drop(j);

    // Index lost: rebuilt byte-for-byte from JSONL.
    std::fs::remove_file(&path).unwrap();
    let j = Journal::open(dir.path()).unwrap();
    let idx = LedgerIndex::open(dir.path(), &j).unwrap();
    let after = (
        idx.nouns(kind::ARTIFACT, "run-1").unwrap(),
        idx.nouns(kind::CLAIM, "run-1").unwrap(),
        idx.runs().unwrap(),
    );
    assert_eq!(before, after);
    drop(idx);

    // Index corrupt: discarded and rebuilt.
    std::fs::write(&path, b"not a database at all, just garbage bytes......").unwrap();
    let idx = LedgerIndex::open(dir.path(), &j).unwrap();
    assert_eq!(idx.runs().unwrap(), after.2);
}

#[test]
fn forced_ledger_index_rebuild_restores_rows_a_fresh_header_hides() {
    let dir = tempfile::tempdir().unwrap();
    let mut j = Journal::open(dir.path()).unwrap();
    state(&mut j, RunState::Dispatching);
    let idx = LedgerIndex::open(dir.path(), &j).unwrap();
    let path = idx.path().to_path_buf();
    assert_eq!(idx.runs().unwrap().len(), 1);
    drop(idx);

    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute("DELETE FROM runs", []).unwrap();
    drop(conn);
    let idx = LedgerIndex::open(dir.path(), &j).unwrap();
    assert!(
        idx.runs().unwrap().is_empty(),
        "test setup: fresh header hides loss"
    );
    drop(idx);
    drop(j);

    let records = LedgerIndex::rebuild_from_journal(dir.path()).unwrap();
    assert_eq!(records, 1);
    let j = Journal::open(dir.path()).unwrap();
    let idx = LedgerIndex::open(dir.path(), &j).unwrap();
    assert_eq!(idx.runs().unwrap().len(), 1);
}

/// Copy `src` over `dst` under a SQLite sidecar suffix layout: `""` is the
/// database itself, `-wal`/`-shm` its artifacts.
fn copy_sidecar_set(src: &std::path::Path, dst: &std::path::Path) {
    for suffix in ["", "-wal"] {
        let mut from = src.as_os_str().to_os_string();
        from.push(suffix);
        let mut to = dst.as_os_str().to_os_string();
        to.push(suffix);
        std::fs::copy(&from, &to).unwrap();
    }
}

#[test]
fn forced_ledger_rebuild_drops_stale_sqlite_sidecars() {
    let dir = tempfile::tempdir().unwrap();
    let mut j = Journal::open(dir.path()).unwrap();
    state(&mut j, RunState::Dispatching);
    let idx = LedgerIndex::open(dir.path(), &j).unwrap();
    let path = idx.path().to_path_buf();
    assert_eq!(idx.runs().unwrap().len(), 1);
    drop(idx);
    drop(j);

    // Shape a stale WAL pair at the index path: a foreign database whose
    // un-checkpointed WAL carries a `victim` schema, as a crashed writer
    // or a synced foreign artifact would leave behind. Copying while the
    // victim connection is open keeps the WAL un-checkpointed.
    let victim_dir = tempfile::tempdir().unwrap();
    let victim_path = victim_dir.path().join("victim.sqlite");
    let victim = rusqlite::Connection::open(&victim_path).unwrap();
    victim
        .execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE victim(x);
             INSERT INTO victim VALUES (1);",
        )
        .unwrap();
    copy_sidecar_set(&victim_path, &path);
    drop(victim);

    let records = LedgerIndex::rebuild_from_journal(dir.path()).unwrap();
    assert_eq!(records, 1);

    // The replacement must not expose the victim shape the stale WAL
    // describes, and the replayed journal rows must survive.
    let j = Journal::open(dir.path()).unwrap();
    let idx = LedgerIndex::open(dir.path(), &j).unwrap();
    assert_eq!(idx.runs().unwrap().len(), 1);
    let check = rusqlite::Connection::open(&path).unwrap();
    let mut victim_shape = check
        .prepare("SELECT name FROM sqlite_master WHERE name = 'victim'")
        .unwrap();
    assert!(
        !victim_shape.exists([]).unwrap(),
        "stale WAL must not contaminate the rebuilt index"
    );
}

#[test]
fn ordinary_ledger_open_rebuilds_through_a_clean_replacement_when_contaminated() {
    let dir = tempfile::tempdir().unwrap();
    let mut j = Journal::open(dir.path()).unwrap();
    state(&mut j, RunState::Dispatching);
    let idx = LedgerIndex::open(dir.path(), &j).unwrap();
    let path = idx.path().to_path_buf();
    assert_eq!(idx.runs().unwrap().len(), 1);
    drop(idx);

    // A foreign shape with un-checkpointed WAL at the index path:
    // `runs` with the wrong columns, so an in-place rebuild cannot even
    // replay into it, plus `victim` debris.
    let victim_dir = tempfile::tempdir().unwrap();
    let victim_path = victim_dir.path().join("victim.sqlite");
    let victim = rusqlite::Connection::open(&victim_path).unwrap();
    victim
        .execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE victim(x);
             INSERT INTO victim VALUES (1);
             CREATE TABLE runs(foreign_column TEXT);
             CREATE TABLE nouns(foreign_column TEXT);
             CREATE TABLE meta(foreign_column TEXT);",
        )
        .unwrap();
    copy_sidecar_set(&victim_path, &path);
    drop(victim);

    let idx = LedgerIndex::open(dir.path(), &j).unwrap();
    assert_eq!(
        idx.runs().unwrap().len(),
        1,
        "ordinary open must rebuild the journal rows"
    );
    let check = rusqlite::Connection::open(&path).unwrap();
    let mut victim_shape = check
        .prepare("SELECT name FROM sqlite_master WHERE name = 'victim'")
        .unwrap();
    assert!(
        !victim_shape.exists([]).unwrap(),
        "foreign tables must not survive an ordinary-open rebuild"
    );
}

#[test]
fn torn_append_is_dropped_not_guessed() {
    let dir = tempfile::tempdir().unwrap();
    let mut j = Journal::open(dir.path()).unwrap();
    state(&mut j, RunState::Dispatching);
    let path = j.path().to_path_buf();
    drop(j);
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    std::io::Write::write_all(&mut f, br#"{"seq":1,"at":2,"entry":{"type":"run_st"#).unwrap();
    drop(f);

    let j = Journal::open(dir.path()).unwrap();
    assert!(j.torn_tail().is_some());
    assert_eq!(j.records().len(), 1);
    let v = &runs(j.records())[&run()];
    assert_eq!(v.phase(), RunPhase::Dispatching);
    assert!(v.is_unresolved());
}

#[test]
fn corrupt_committed_line_fails_loudly() {
    let dir = tempfile::tempdir().unwrap();
    let path = Journal::open(dir.path()).unwrap().path().to_path_buf();
    std::fs::write(path, b"{garbage}\n").unwrap();
    assert!(Journal::open(dir.path()).is_err());
}

#[test]
fn multibyte_hash_in_committed_line_is_corrupt_not_a_panic() {
    let dir = tempfile::tempdir().unwrap();
    let mut j = Journal::open(dir.path()).unwrap();
    j.append(
        Timestamp(1),
        Entry::Approved {
            run: run(),
            by: "me".into(),
            grant_digest: Hash([0; 32]),
        },
    )
    .unwrap();
    let path = j.path().to_path_buf();
    drop(j);
    // 64 bytes with a two-byte char straddling the first hex pair.
    let tampered = format!("a\u{e9}{}", "a".repeat(61));
    assert_eq!(tampered.len(), 64);
    let line = std::fs::read_to_string(&path).unwrap();
    let line = line.replacen(&"0".repeat(64), &tampered, 1);
    std::fs::write(&path, line).unwrap();
    assert!(matches!(
        Journal::open(dir.path()),
        Err(cv_ledger::LedgerError::Corrupt { line: 1, .. })
    ));
}

#[test]
fn read_creates_nothing_when_the_project_has_no_ledger() {
    let dir = tempfile::tempdir().unwrap();
    assert!(journal::read(dir.path()).unwrap().is_none());
    assert!(!dir.path().join(".cv").exists(), "a reader writes nothing");
}

#[test]
fn read_sees_committed_records_beside_a_live_writer_and_keeps_a_torn_tail() {
    let dir = tempfile::tempdir().unwrap();
    let mut j = Journal::open(dir.path()).unwrap();
    state(&mut j, RunState::Dispatching);
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(j.path())
        .unwrap();
    std::io::Write::write_all(&mut f, br#"{"seq":1,"at":2"#).unwrap();
    let before = std::fs::read(j.path()).unwrap();
    let c = journal::read(dir.path()).unwrap().unwrap();
    assert_eq!(c.records, j.records());
    assert_eq!(c.torn_tail.as_deref(), Some(r#"{"seq":1,"at":2"#));
    assert_eq!(std::fs::read(j.path()).unwrap(), before, "left as found");
}

#[test]
fn read_refuses_corrupt_lines_and_newer_schemas() {
    let dir = tempfile::tempdir().unwrap();
    let path = Journal::open(dir.path()).unwrap().path().to_path_buf();
    std::fs::write(&path, b"{garbage}\n").unwrap();
    assert!(matches!(
        journal::read(dir.path()),
        Err(cv_ledger::LedgerError::Corrupt { line: 1, .. })
    ));
    std::fs::write(&path, b"").unwrap();
    let stamp = dir.path().join(".cv/ledger/schema.json");
    std::fs::write(
        &stamp,
        r#"{"format":"cv-sidecar","namespace":"ledger","schema_version":9}"#,
    )
    .unwrap();
    let err = journal::read(dir.path()).unwrap_err();
    assert!(err.to_string().contains("newer Capture Vector"), "{err}");
}

#[cfg(unix)]
#[test]
fn read_refuses_a_symlinked_ledger() {
    let dir = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    drop(Journal::open(elsewhere.path()).unwrap());
    std::os::unix::fs::symlink(elsewhere.path().join(".cv"), dir.path().join(".cv")).unwrap();
    assert!(journal::read(dir.path()).is_err());
}

#[test]
fn second_writer_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let j = Journal::open(dir.path()).unwrap();
    assert!(matches!(
        Journal::open(dir.path()),
        Err(cv_ledger::LedgerError::Locked(_))
    ));
    drop(j);
    assert!(Journal::open(dir.path()).is_ok());
}

#[test]
fn drifted_proof_schema_is_refused_not_lifted() {
    let mut a: serde_json::Value = serde_json::from_slice(&proof("artifacts.json")).unwrap();
    a["schema_version"] = serde_json::json!(2);
    let err = ingest::from_bytes(&run(), Some(a.to_string().as_bytes()), None, None).unwrap_err();
    assert!(
        err.to_string().contains("schema_version 2"),
        "unexpected error: {err}"
    );
}
