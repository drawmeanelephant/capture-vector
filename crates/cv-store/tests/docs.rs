//! On-disk `cv_core::store::Store`: `.cv/store/docs.jsonl` is truth, the
//! app-data `index.sqlite` is a rebuildable projection, and the shared
//! `StubStore` semantics (put/get/query + `Predicate::matches_json`
//! parity) hold identically.

use cv_core::id::{AttrValue, StableId};
use cv_core::store::{Namespace, Op, Predicate, Store, Term};
use cv_fixtures::StubStore;
use cv_store::docs::{DIR, DOCS, DiskStore, Error, SCHEMA, WRITER_LOCK, index_path};
use cv_store::sidecar;
use serde_json::json;

fn ns(s: &str) -> Namespace {
    Namespace(s.into())
}

fn key(s: &str) -> StableId {
    StableId(s.into())
}

fn term(field: &str, op: Op, value: AttrValue) -> Term {
    Term {
        field: field.into(),
        op,
        value,
    }
}

/// The conformance scenario every `Store` must agree on, driven
/// verbatim against the stub oracle and the disk impl.
fn conformance(store: &dyn Store) {
    let research = ns("research.notes");
    store
        .put(
            &research,
            &key("n1"),
            &json!({"status": "open", "count": 3}),
        )
        .unwrap();
    store
        .put(
            &research,
            &key("n2"),
            &json!({"status": "closed", "count": 1}),
        )
        .unwrap();
    store
        .put(&ns("publish.runs"), &key("r1"), &json!({"status": "open"}))
        .unwrap();

    assert_eq!(
        store.get(&research, &key("n1")).unwrap(),
        Some(json!({"status": "open", "count": 3}))
    );
    assert_eq!(store.get(&research, &key("absent")).unwrap(), None);
    // A `get` in another namespace never sees the doc.
    assert_eq!(store.get(&ns("publish.runs"), &key("n1")).unwrap(), None);

    let open = Predicate {
        conj: vec![term("status", Op::Eq, AttrValue::Text("open".into()))],
    };
    assert_eq!(store.query(&research, &open).unwrap().len(), 1);
    let open_and_big = Predicate {
        conj: vec![
            term("status", Op::Eq, AttrValue::Text("open".into())),
            term("count", Op::Gt, AttrValue::Int(1)),
        ],
    };
    assert_eq!(
        store.query(&research, &open_and_big).unwrap(),
        vec![json!({"status": "open", "count": 3})]
    );
    // Missing field never matches; other namespaces are invisible.
    let no_field = Predicate {
        conj: vec![term("count", Op::Le, AttrValue::Int(5))],
    };
    assert_eq!(
        store.query(&ns("publish.runs"), &no_field).unwrap().len(),
        0
    );

    // Strict typing parity: JSON 3 is an Int, never f64-coerced.
    let float_eq = Predicate {
        conj: vec![term("count", Op::Eq, AttrValue::Float(3.0))],
    };
    assert_eq!(store.query(&research, &float_eq).unwrap().len(), 0);
    let contains = Predicate {
        conj: vec![term("status", Op::Contains, AttrValue::Text("pe".into()))],
    };
    assert_eq!(store.query(&research, &contains).unwrap().len(), 1);
    let ne = Predicate {
        conj: vec![term("status", Op::Ne, AttrValue::Text("open".into()))],
    };
    assert_eq!(store.query(&research, &ne).unwrap().len(), 1);
}

#[test]
fn disk_store_passes_the_stub_conformance() {
    conformance(&StubStore::new());
    let root = tempfile::tempdir().unwrap();
    let (store, _) = DiskStore::open(root.path()).unwrap();
    conformance(&store);
}

#[test]
fn disk_and_stub_queries_return_the_same_documents_in_the_same_order() {
    let root = tempfile::tempdir().unwrap();
    let (disk, _) = DiskStore::open(root.path()).unwrap();
    let stub = StubStore::new();
    let ns = ns("research.notes");
    for (id, doc) in [
        ("c", json!({"rank": 3, "kind": "note"})),
        ("a", json!({"rank": 1, "kind": "note"})),
        ("b", json!({"rank": 2, "kind": "note"})),
    ] {
        disk.put(&ns, &key(id), &doc).unwrap();
        stub.put(&ns, &key(id), &doc).unwrap();
    }
    let all = Predicate::default();
    assert_eq!(
        disk.query(&ns, &all).unwrap(),
        stub.query(&ns, &all).unwrap()
    );
    let ge = Predicate {
        conj: vec![term("rank", Op::Ge, AttrValue::Int(2))],
    };
    assert_eq!(disk.query(&ns, &ge).unwrap(), stub.query(&ns, &ge).unwrap());
}

#[test]
fn last_write_wins_and_persists_across_instances() {
    let root = tempfile::tempdir().unwrap();
    let ns = ns("shell.journal");
    {
        let (store, report) = DiskStore::open(root.path()).unwrap();
        assert!(report.index_rebuilt, "fresh open builds the index");
        assert_eq!(report.schema, sidecar::Outcome::Fresh);
        store.put(&ns, &key("e1"), &json!({"seq": 1})).unwrap();
        store.put(&ns, &key("e1"), &json!({"seq": 2})).unwrap();
    }
    let (store, report) = DiskStore::open(root.path()).unwrap();
    assert!(!report.index_rebuilt, "a fresh index is reused");
    assert_eq!(store.get(&ns, &key("e1")).unwrap(), Some(json!({"seq": 2})));
    assert_eq!(report.scan.lines, 2, "both writes stay on the trail");
    // Stamped exactly once, at the current schema.
    assert_eq!(
        sidecar::read_version(root.path(), &SCHEMA).unwrap(),
        Some(1)
    );
}

#[test]
fn a_same_size_journal_replacement_is_not_trusted_as_fresh() {
    let root = tempfile::tempdir().unwrap();
    let ns = ns("shell.journal");
    {
        let (store, _) = DiskStore::open(root.path()).unwrap();
        store.put(&ns, &key("e1"), &json!({"n": 1})).unwrap();
    }
    let journal = root.path().join(DIR).join(DOCS);
    let before = std::fs::read_to_string(&journal).unwrap();
    let after = before.replace("\"n\":1", "\"n\":2");
    assert_eq!(before.len(), after.len(), "same-size replacement");
    assert_eq!(before.lines().count(), after.lines().count());
    std::fs::write(&journal, after).unwrap();

    let (store, report) = DiskStore::open(root.path()).unwrap();
    assert!(
        report.index_rebuilt,
        "content freshness must reject equal-length foreign truth"
    );
    assert_eq!(store.get(&ns, &key("e1")).unwrap(), Some(json!({"n": 2})));
}

#[test]
fn a_stale_index_is_rebuilt_from_jsonl() {
    let root = tempfile::tempdir().unwrap();
    let ns = ns("shell.journal");
    {
        let (store, _) = DiskStore::open(root.path()).unwrap();
        store.put(&ns, &key("e1"), &json!({"n": 1})).unwrap();
    }
    // Direct journal append behind the index's back: journal_len is
    // stale, so the next open replays the trail.
    let journal = root.path().join(DIR).join(DOCS);
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(&journal)
        .unwrap();
    use std::io::Write;
    writeln!(
        f,
        "{}",
        json!({"schema_version": 1, "ns": "shell.journal", "key": "e2", "doc": {"n": 2}})
    )
    .unwrap();
    drop(f);
    let (store, report) = DiskStore::open(root.path()).unwrap();
    assert!(report.index_rebuilt);
    assert_eq!(store.get(&ns, &key("e2")).unwrap(), Some(json!({"n": 2})));
}

#[test]
fn a_corrupt_index_is_deleted_and_rebuilt() {
    let root = tempfile::tempdir().unwrap();
    let ns = ns("research.notes");
    {
        let (store, _) = DiskStore::open(root.path()).unwrap();
        store
            .put(&ns, &key("n1"), &json!({"status": "open"}))
            .unwrap();
    }
    let index = index_path(root.path()).unwrap();
    assert!(index.is_file());
    std::fs::write(&index, b"not a sqlite database").unwrap();
    let (store, report) = DiskStore::open(root.path()).unwrap();
    assert!(report.index_rebuilt, "corrupt index rebuilds");
    assert_eq!(
        store.get(&ns, &key("n1")).unwrap(),
        Some(json!({"status": "open"}))
    );
    // Deleting it outright takes the same recovery path.
    drop(store);
    std::fs::remove_file(&index).unwrap();
    let (store, report) = DiskStore::open(root.path()).unwrap();
    assert!(report.index_rebuilt);
    assert_eq!(
        store.get(&ns, &key("n1")).unwrap(),
        Some(json!({"status": "open"}))
    );
}

#[test]
fn a_foreign_index_shape_is_rebuilt() {
    let root = tempfile::tempdir().unwrap();
    {
        let (store, _) = DiskStore::open(root.path()).unwrap();
        store.put(&ns("a"), &key("k"), &json!({})).unwrap();
    }
    // A valid SQLite file whose contents are not this projection.
    let index = index_path(root.path()).unwrap();
    let conn = rusqlite::Connection::open(&index).unwrap();
    conn.execute_batch(
        "CREATE TABLE foreign_table (x TEXT); INSERT INTO foreign_table VALUES ('victim');",
    )
    .unwrap();
    conn.close().unwrap();
    let (store, report) = DiskStore::open(root.path()).unwrap();
    assert!(report.index_rebuilt);
    assert_eq!(store.get(&ns("a"), &key("k")).unwrap(), Some(json!({})));
    let conn = rusqlite::Connection::open(&index).unwrap();
    assert!(
        conn.prepare("SELECT x FROM foreign_table").is_err(),
        "debris tables do not survive the rebuild"
    );
}

#[test]
fn the_index_lives_outside_cv() {
    let root = tempfile::tempdir().unwrap();
    let index = index_path(root.path()).unwrap();
    let s = index.to_str().unwrap();
    assert!(!s.contains(".cv"), "{s}");
    assert!(!index.starts_with(root.path()), "{s}");
    let (_store, _) = DiskStore::open(root.path()).unwrap();
    assert!(!root.path().join(DIR).join("index.sqlite").exists());
}

#[test]
fn torn_tail_and_foreign_lines_are_skipped() {
    let root = tempfile::tempdir().unwrap();
    let ns = ns("shell.journal");
    {
        let (store, _) = DiskStore::open(root.path()).unwrap();
        store.put(&ns, &key("e1"), &json!({"n": 1})).unwrap();
    }
    let journal = root.path().join(DIR).join(DOCS);
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(&journal)
        .unwrap();
    use std::io::Write;
    writeln!(
        f,
        "{}",
        json!({"schema_version": 9, "ns": "shell.journal", "key": "e2", "doc": {}})
    )
    .unwrap();
    writeln!(f, "not json").unwrap();
    write!(f, "{{\"torn").unwrap();
    drop(f);
    let (store, report) = DiskStore::open(root.path()).unwrap();
    assert_eq!((report.scan.unsupported, report.scan.undecodable), (1, 1));
    assert!(report.scan.torn_bytes > 0);
    assert!(report.index_rebuilt, "the truncated tail moved the length");
    assert_eq!(
        store.query(&ns, &Predicate::default()).unwrap(),
        vec![json!({"n": 1})]
    );
    assert!(std::fs::read(&journal).unwrap().ends_with(b"\n"));
    // The store keeps working after the cleanup.
    store.put(&ns, &key("e3"), &json!({"n": 3})).unwrap();
    assert_eq!(store.get(&ns, &key("e3")).unwrap(), Some(json!({"n": 3})));
}

#[test]
fn a_second_writer_cannot_open_the_store() {
    let root = tempfile::tempdir().unwrap();
    let (store, _) = DiskStore::open(root.path()).unwrap();
    assert!(matches!(
        DiskStore::open(root.path()),
        Err(Error::Locked(_))
    ));
    assert!(root.path().join(DIR).join(WRITER_LOCK).is_file());
    drop(store);
    let (store, _) = DiskStore::open(root.path()).unwrap();
    store.put(&ns("a"), &key("k"), &json!({})).unwrap();
}

#[test]
fn an_unstamped_journal_is_adopted() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join(DIR);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(DOCS),
        "{\"schema_version\":1,\"ns\":\"a\",\"key\":\"k\",\"doc\":{\"v\":1}}\n",
    )
    .unwrap();
    let (store, report) = DiskStore::open(root.path()).unwrap();
    assert_eq!(report.schema, sidecar::Outcome::Migrated { from: 0, to: 1 });
    assert_eq!(
        store.get(&ns("a"), &key("k")).unwrap(),
        Some(json!({"v": 1}))
    );
    assert_eq!(
        sidecar::read_version(root.path(), &SCHEMA).unwrap(),
        Some(1)
    );
}

#[test]
fn a_newer_schema_is_refused_without_touching_the_journal() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join(DIR);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(DOCS), "future\n").unwrap();
    std::fs::write(
        dir.join("schema.json"),
        json!({"format": sidecar::FORMAT, "namespace": "store", "schema_version": 9}).to_string(),
    )
    .unwrap();
    assert!(matches!(
        DiskStore::open(root.path()),
        Err(Error::Schema(sidecar::SidecarError::TooNew { .. }))
    ));
    assert_eq!(std::fs::read_to_string(dir.join(DOCS)).unwrap(), "future\n");
}

#[cfg(unix)]
#[test]
fn planted_symlinks_are_refused() {
    let elsewhere = tempfile::tempdir().unwrap();
    let d = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(elsewhere.path(), d.path().join(".cv")).unwrap();
    assert!(matches!(DiskStore::open(d.path()), Err(Error::Symlink(_))));
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join(DIR)).unwrap();
    let target = elsewhere.path().join("victim");
    std::os::unix::fs::symlink(&target, d.path().join(DIR).join(DOCS)).unwrap();
    assert!(matches!(DiskStore::open(d.path()), Err(Error::Symlink(_))));
    assert!(!target.exists());

    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join(DIR)).unwrap();
    let target = elsewhere.path().join("lock-victim");
    std::os::unix::fs::symlink(&target, d.path().join(DIR).join(WRITER_LOCK)).unwrap();
    assert!(matches!(DiskStore::open(d.path()), Err(Error::Symlink(_))));
    assert!(!target.exists());
}

#[test]
fn ephemeral_store_passes_the_stub_conformance() {
    conformance(&cv_store::docs::EphemeralStore::new());
}

#[test]
fn put_after_index_write_failure_is_committed_and_the_index_is_rebuilt() {
    let root = tempfile::tempdir().unwrap();
    let (store, _) = DiskStore::open(root.path()).unwrap();
    // Break the index behind the open connection's back. The journal append
    // is still the commit point, so the index failure must not masquerade
    // as an uncommitted write.
    let index = index_path(root.path()).unwrap();
    rusqlite::Connection::open(&index)
        .unwrap()
        .execute_batch("DROP TABLE docs")
        .unwrap();
    store.put(&ns("a"), &key("k"), &json!({})).unwrap();
    assert_eq!(
        store.get(&ns("a"), &key("k")).unwrap(),
        Some(json!({})),
        "the live index is rebuilt rather than left stale"
    );
    let journal = std::fs::read_to_string(root.path().join(DIR).join(DOCS)).unwrap();
    assert_eq!(journal.lines().count(), 1, "no retry duplicate was needed");
    drop(store);

    let (store, report) = DiskStore::open(root.path()).unwrap();
    assert!(
        !report.index_rebuilt,
        "the in-session rebuild stamped the exact journal it projected"
    );
    assert_eq!(store.get(&ns("a"), &key("k")).unwrap(), Some(json!({})));
}

#[test]
fn fresh_journal_creation_reports_directory_durability_evidence() {
    let root = tempfile::tempdir().unwrap();
    let (_store, report) = DiskStore::open(root.path()).unwrap();
    assert!(report.journal_created);
    #[cfg(unix)]
    assert!(report.journal_dir_synced);
    #[cfg(not(unix))]
    assert!(!report.journal_dir_synced);
}
