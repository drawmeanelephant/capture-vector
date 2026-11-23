use cv_core::id::StableId;
use cv_core::publish::{RunState, RunStatus};
use cv_core::sys::Timestamp;
use cv_ledger::ingest;
use cv_ledger::journal::Entry;
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
    let j = Journal::open(dir.path()).unwrap();
    std::fs::write(j.path(), b"{garbage}\n").unwrap();
    drop(j);
    assert!(Journal::open(dir.path()).is_err());
}
