//! Adoption acceptance tests over the checked-in captured Hugo lab
//! output: adopt-to-new proves validate/build run and provenance lands in
//! `.cv/imports/<run>/`; the existing-workspace merge proves collisions
//! refuse before the first write; the crash-recovery probes prove an
//! interrupted attempt resumes instead of deadlocking on its own partial
//! output.
use cv_core::engine::{EventSink, ExitClass, Verb};
use cv_engine::adopt::{self, AdoptError, CollisionKind};
use cv_fixtures::lab::StubLab;
use cv_store::imports::{self as store, Finished, RecordedLab, RunRecord};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

fn test_lock(root: &Path, run_id: &str) -> store::RunLock {
    match store::claim_run(root, run_id) {
        Ok(lock) => lock,
        Err(store::Error::RunExists(_)) => store::lock_run(root, run_id).unwrap(),
        Err(e) => panic!("test run lock: {e}"),
    }
}

fn write_record(root: &Path, rec: &RunRecord) {
    let lock = test_lock(root, &rec.run_id);
    store::write(&lock, rec).expect("run record");
}

fn write_pending_for(root: &Path, journal: &store::PendingAdoption) {
    let lock = test_lock(root, &journal.run_id);
    store::write_pending(&lock, journal).unwrap();
}

fn seed_run(root: &Path, mode: &str, started_ms: i64) -> String {
    let id = format!("{started_ms}-{mode}");
    let root = &root.canonicalize().expect("run root");
    let dir = store::run_dir(root, &id).expect("run id");
    let t = StubLab::new()
        .run(mode, &dir.join(store::OUT))
        .expect("captured lab mode");
    let mut rec = RunRecord::started(
        &id,
        mode,
        RecordedLab {
            tool_id: StubLab::new().tool_id(),
            source: "fixture".into(),
            path: "fixtures/captures/lab".into(),
        },
        started_ms,
    );
    rec.inputs = t
        .argv
        .iter()
        .filter(|a| !a.starts_with("--mode=") && !a.starts_with("--out="))
        .filter_map(|a| a.split_once('='))
        .map(|(f, p)| store::input(f, p))
        .collect();
    rec.argv = t.argv.clone();
    rec.finished = Some(Finished {
        exit_code: Some(t.exit_code),
        exit: store::FinishedExit::Ok,
        outcome: format!("exited {}", t.exit_code),
        duration_ms: 0,
    });
    write_record(root, &rec);
    id
}

fn hugo_source() -> (tempfile::TempDir, String) {
    let source_ws = tempfile::tempdir().unwrap();
    let run_id = seed_run(source_ws.path(), "hugo", 1_764_000_000_000);
    (source_ws, run_id)
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        let ty = entry.file_type().unwrap();
        if ty.is_dir() {
            copy_tree(&entry.path(), &target);
        } else if ty.is_file() {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

#[test]
fn existing_workspace_transaction_revalidates_captured_instance_before_writing() {
    let (source_ws, run_id) = hugo_source();
    let base_tmp = tempfile::tempdir().unwrap();
    let base = base_tmp.path().canonicalize().unwrap();
    let root = base.join("workspace");
    std::fs::create_dir_all(root.join("content")).unwrap();
    let token = cv_store::workspace_instance::ensure(&root).unwrap();
    let expected = adopt::ExpectedWorkspaceInstance::new(
        token,
        cv_engine::file_identity::identity(&root),
        cv_store::workspace_mutation::current_identity(&root).unwrap(),
    );
    let src = adopt::source(source_ws.path(), &run_id).expect("adoptable hugo run");

    let moved = base.join("moved-survivor");
    std::fs::rename(&root, &moved).unwrap();
    copy_tree(&moved, &root);

    let err = adopt::transact_existing_with_identity(adopt::ExistingWorkspaceTransaction {
        target_root: &root,
        src: &src,
        content_input: "content",
        merge_subfolder: Some("imported"),
        scan_existing_ids: Box::new(BTreeMap::new),
        at_ms: 42,
        expected_instance: &expected,
    })
    .unwrap_err();
    let message = err.to_string();
    println!(
        "R15-ENGINE-IDENTITY error={message:?} replacement_imported={}",
        root.join("content/imported").exists()
    );
    assert!(
        message.contains("workspace identity"),
        "refusal must name the identity failure: {message}"
    );
    assert!(
        !root.join("content/imported").exists(),
        "no adoption byte may land in the byte-copy replacement"
    );
    assert!(
        !moved.join("content/imported").exists(),
        "fail-closed path-bound semantics do not follow the moved survivor"
    );
}

#[test]
fn adopt_to_new_copies_content_provenance_and_runs_validate_then_build() {
    let (source_ws, run_id) = hugo_source();
    let target = tempfile::tempdir().unwrap();
    let root = target.path().canonicalize().unwrap();

    let src = adopt::source(source_ws.path(), &run_id).expect("adoptable hugo run");
    assert!(
        !src.files.is_empty(),
        "hugo capture emits candidate content"
    );
    assert!(!src.pages.is_empty(), "hugo report names emitted pages");

    // The starter was just initialized; its content tree is replaced
    // by the transaction itself — inside the journaled recovery unit,
    // never before it.
    std::fs::create_dir_all(root.join("content")).unwrap();
    std::fs::write(root.join("content/index.md"), "starter\n").unwrap();

    let tx = adopt::transact(&root, &src, "content", None, "new", &BTreeMap::new(), 42).unwrap();
    let starter_survives = walk_files(&root.join("content"))
        .iter()
        .any(|f| std::fs::read(root.join("content").join(f)).unwrap_or_default() == b"starter\n");
    assert!(
        !starter_survives,
        "the starter tree is replaced inside the transaction"
    );
    assert_eq!(tx.written, src.files.len());
    assert_eq!(tx.resumed, 0);
    // Byte-for-byte copy, including binary assets.
    for f in &src.files {
        assert_eq!(
            std::fs::read(&f.source).unwrap(),
            std::fs::read(root.join("content").join(&f.relative)).unwrap(),
            "{}",
            f.relative
        );
    }
    assert!(tx.provenance_copied > 0);
    let imports = root.join(".cv/imports").join(&run_id);
    assert!(imports.join("report.json").is_file(), "report.json lands");
    assert!(
        imports.join("redirects.json").is_file(),
        "redirects.json lands"
    );
    assert!(
        imports.join("provenance").is_dir(),
        "per-page provenance lands"
    );
    assert!(imports.join("run.json").is_file(), "run record lands");
    assert!(
        !imports.join(store::PENDING).exists(),
        "the recovery journal is retired once the unit is durable"
    );

    let adoptions = store::scan_adoptions(&root).unwrap();
    assert_eq!(adoptions.len(), 1);
    assert_eq!(adoptions[0].run_id, run_id);
    assert_eq!(adoptions[0].mode, "hugo");
    assert!(!adoptions[0].pages.is_empty());
    let rec = store::scan(&root).unwrap();
    assert!(
        rec.iter()
            .any(|r| r.record.as_ref().is_ok_and(|r| r.adopted.is_some())),
        "run is marked adopted"
    );

    // The post-adoption gate: validate first, then build, in order.
    let verbs = Arc::new(Mutex::new(Vec::new()));
    let seen = verbs.clone();
    let run = move |verb: Verb, _sink: EventSink| {
        seen.lock().unwrap().push(verb);
        Ok(ExitClass::Ok)
    };
    let sink = || Box::new(|_: cv_core::events::Envelope| {}) as EventSink;
    let (validate, build) = adopt::validate_then_build(&run, &sink).unwrap();
    assert_eq!(validate, ExitClass::Ok);
    assert_eq!(build, Some(ExitClass::Ok));
    assert_eq!(
        *verbs.lock().unwrap(),
        vec![Verb::Validate, Verb::Build],
        "validate runs before build"
    );
}

#[test]
fn validate_failure_skips_build() {
    let run = |verb: Verb, _sink: EventSink| {
        Ok(match verb {
            Verb::Validate => ExitClass::ContentFailure,
            _ => panic!("build must not run after a failed validate"),
        })
    };
    let sink = || Box::new(|_: cv_core::events::Envelope| {}) as EventSink;
    let (validate, build) = adopt::validate_then_build(&run, &sink).unwrap();
    assert_eq!(validate, ExitClass::ContentFailure);
    assert_eq!(build, None);
}

#[test]
fn existing_workspace_merge_refuses_path_collisions_before_writing() {
    let (source_ws, run_id) = hugo_source();
    let target = tempfile::tempdir().unwrap();
    let root = target.path().canonicalize().unwrap();

    let src = adopt::source(source_ws.path(), &run_id).unwrap();
    let planned = adopt::plan(&src, "content", Some("imported/site")).unwrap();
    assert!(
        planned
            .iter()
            .all(|p| p.target.starts_with("content/imported/site/")),
        "merge is confined to the chosen subfolder"
    );

    // A byte-identical pre-existing file is still a collision on a fresh
    // attempt: nothing distinguishes it from the user's own content
    // until the recovery journal says adoption wrote it.
    let victim = planned[0].clone();
    std::fs::create_dir_all(root.join("content/imported/site")).unwrap();
    let victim_path = root.join(&victim.target);
    std::fs::create_dir_all(victim_path.parent().unwrap()).unwrap();
    std::fs::write(&victim_path, std::fs::read(&victim.source).unwrap()).unwrap();

    let err = adopt::transact(
        &root,
        &src,
        "content",
        Some("imported/site"),
        "existing",
        &BTreeMap::new(),
        1,
    )
    .unwrap_err();
    let AdoptError::Collisions(cs) = &err else {
        panic!("expected collisions, got {err}");
    };
    assert!(
        cs.iter()
            .any(|c| c.kind == CollisionKind::Path && c.incoming == victim.display),
        "path collision reported even for identical bytes: {cs:?}"
    );
    // Nothing was written and no journal claims otherwise.
    assert!(store::read_pending(&root, &run_id).unwrap().is_none());
    let written: Vec<_> = planned
        .iter()
        .filter(|p| p.target != victim.target && root.join(&p.target).exists())
        .collect();
    assert!(written.is_empty(), "preflight refused before any write");
}

#[test]
fn existing_workspace_merge_refuses_id_collisions_before_writing() {
    let (source_ws, run_id) = hugo_source();
    let target = tempfile::tempdir().unwrap();
    let root = target.path().canonicalize().unwrap();

    let src = adopt::source(source_ws.path(), &run_id).unwrap();
    let first = src.pages[0].entity_id.clone();
    let mut ids = BTreeMap::new();
    ids.insert(first.clone(), "posts/existing.md".to_string());

    let err = adopt::transact(
        &root,
        &src,
        "content",
        Some("imported"),
        "existing",
        &ids,
        1,
    )
    .unwrap_err();
    let AdoptError::Collisions(cs) = &err else {
        panic!("expected collisions, got {err}");
    };
    assert!(
        cs.iter()
            .any(|c| c.kind == CollisionKind::Id && c.incoming == first),
        "id collision reported: {cs:?}"
    );
    assert!(!root.join("content").exists(), "nothing was written");
    assert!(store::read_pending(&root, &run_id).unwrap().is_none());
}

/// The blocker-2 probe: an interrupted attempt (journal durable, some
/// files copied) must resume to completion instead of refusing its own
/// partial output forever.
#[test]
fn interrupted_adoption_resumes_byte_identical_files_and_converges() {
    let (source_ws, run_id) = hugo_source();
    let target = tempfile::tempdir().unwrap();
    let root = target.path().canonicalize().unwrap();
    let src = adopt::source(source_ws.path(), &run_id).unwrap();
    let planned = adopt::plan(&src, "content", Some("imported")).unwrap();

    // Simulate the crash: journal durable, the first two files written,
    // everything else missing — exactly what a mid-copy interruption
    // leaves behind, since staged writes are atomic per file.
    let mut journal = store::PendingAdoption::new(&run_id, "existing", "content");
    journal.content_subfolder = Some("imported".into());
    for p in &planned {
        let bytes = std::fs::read(&p.source).unwrap();
        journal.files.push(store::PendingFile {
            target: p.target.clone(),
            sha256: store::sha256_hex(&bytes),
            bytes: bytes.len() as u64,
        });
    }
    write_pending_for(&root, &journal);
    let n = 2.min(planned.len());
    let (pre_written, _) =
        adopt::copy_content(&root, &planned[..n], &[true; 2][..n.min(2)]).unwrap();
    assert_eq!(pre_written, n);

    // The retry of the same shape resumes: most files are written now,
    // the two already-landed ones are skipped rather than colliding.
    let tx = adopt::transact(
        &root,
        &src,
        "content",
        Some("imported"),
        "existing",
        &BTreeMap::new(),
        7,
    )
    .unwrap();
    assert_eq!(tx.resumed, n, "partial writes skip");
    assert_eq!(tx.written, planned.len() - n);
    assert!(tx.provenance_copied > 0, "provenance is inside the unit");
    for p in &planned {
        assert_eq!(
            std::fs::read(&p.source).unwrap(),
            std::fs::read(root.join(&p.target)).unwrap(),
            "{}",
            p.target
        );
    }
    assert!(store::read_pending(&root, &run_id).unwrap().is_none());
    assert_eq!(store::scan_adoptions(&root).unwrap().len(), 1);
}

/// The real-worker resume shape: after the crash, the workspace scan the
/// composition root passes as `existing_ids` already contains the copied
/// files' ids at their planned paths. The retry must converge anyway —
/// only ids owned *elsewhere* are genuine collisions.
#[test]
fn resume_tolerates_its_own_ids_in_the_workspace_scan() {
    let (source_ws, run_id) = hugo_source();
    let target = tempfile::tempdir().unwrap();
    let root = target.path().canonicalize().unwrap();
    let src = adopt::source(source_ws.path(), &run_id).unwrap();
    let planned = adopt::plan(&src, "content", Some("imported")).unwrap();

    let journal = pending_for(&root, &run_id, &src);
    write_pending_for(&root, &journal);
    let n = 2.min(planned.len());
    adopt::copy_content(&root, &planned[..n], &[true; 2][..n]).unwrap();

    // What the worker would compute from disk truth after the crash: the
    // already-landed pages' ids, at the paths the plan itself targets.
    let mut ids = BTreeMap::new();
    for page in &src.pages {
        let display = format!("imported/{}", page.path);
        if planned[..n].iter().any(|p| p.display == display) {
            ids.insert(page.entity_id.clone(), display);
        }
    }
    assert!(
        !ids.is_empty(),
        "the probe actually covers the self-collision"
    );
    let tx = adopt::transact(
        &root,
        &src,
        "content",
        Some("imported"),
        "existing",
        &ids,
        7,
    )
    .unwrap();
    assert_eq!(tx.resumed, n);
    assert!(store::read_pending(&root, &run_id).unwrap().is_none());
}

/// An id the scan reports at a path the plan does not target is a genuine
/// collision in resume mode too.
#[test]
fn resume_still_refuses_ids_owned_elsewhere() {
    let (source_ws, run_id) = hugo_source();
    let target = tempfile::tempdir().unwrap();
    let root = target.path().canonicalize().unwrap();
    let src = adopt::source(source_ws.path(), &run_id).unwrap();

    let journal = pending_for(&root, &run_id, &src);
    write_pending_for(&root, &journal);

    let first = src.pages[0].entity_id.clone();
    let mut ids = BTreeMap::new();
    ids.insert(first.clone(), "elsewhere/pre-existing.md".to_string());
    let err = adopt::transact(
        &root,
        &src,
        "content",
        Some("imported"),
        "existing",
        &ids,
        7,
    )
    .unwrap_err();
    assert!(
        matches!(err, AdoptError::Collisions(_)),
        "foreign id ownership refuses in resume mode: {err}"
    );
}

fn pending_for(
    root: &std::path::Path,
    run_id: &str,
    src: &adopt::AdoptionSource,
) -> store::PendingAdoption {
    let mut journal = store::PendingAdoption::new(run_id, "existing", "content");
    journal.content_subfolder = Some("imported".into());
    let planned = adopt::plan(src, "content", Some("imported")).unwrap();
    for p in &planned {
        let bytes = std::fs::read(&p.source).unwrap();
        journal.files.push(store::PendingFile {
            target: p.target.clone(),
            sha256: store::sha256_hex(&bytes),
            bytes: bytes.len() as u64,
        });
    }
    let _ = root;
    journal
}

/// Journal enforcement: an embedded run id that does not match the run
/// directory the journal lies in is foreign, never recovery input.
#[test]
fn a_journal_naming_a_different_run_is_rejected() {
    let (source_ws, run_id) = hugo_source();
    let target = tempfile::tempdir().unwrap();
    let root = target.path().canonicalize().unwrap();
    let src = adopt::source(source_ws.path(), &run_id).unwrap();
    let mut journal = pending_for(&root, &run_id, &src);
    write_pending_for(&root, &journal);
    // Rewrite the file with a foreign embedded id.
    journal.run_id = "somebody-else".into();
    let dir = store::run_dir(&root, &run_id).unwrap();
    std::fs::write(
        dir.join(store::PENDING),
        serde_json::to_vec_pretty(&journal).unwrap(),
    )
    .unwrap();

    let err = store::read_pending(&root, &run_id).unwrap_err();
    assert!(err.contains("somebody-else"), "{err}");
}

/// Journal enforcement: an entry target that is not a portable confined
/// path means the journal is tampered, and the retry refuses.
#[test]
fn a_journal_with_an_unportable_target_refuses_the_resume() {
    let (source_ws, run_id) = hugo_source();
    let target = tempfile::tempdir().unwrap();
    let root = target.path().canonicalize().unwrap();
    let src = adopt::source(source_ws.path(), &run_id).unwrap();
    let mut journal = pending_for(&root, &run_id, &src);
    journal.files[0].target = "../escape.md".into();
    write_pending_for(&root, &journal);

    let err = adopt::transact(
        &root,
        &src,
        "content",
        Some("imported"),
        "existing",
        &BTreeMap::new(),
        7,
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("portable confined"),
        "unportable entry refuses: {err}"
    );
}

/// Journal enforcement: a file set that differs from the operation's
/// plan is a different operation's journal.
#[test]
fn a_journal_with_a_different_file_set_refuses_the_resume() {
    let (source_ws, run_id) = hugo_source();
    let target = tempfile::tempdir().unwrap();
    let root = target.path().canonicalize().unwrap();
    let src = adopt::source(source_ws.path(), &run_id).unwrap();
    let mut journal = pending_for(&root, &run_id, &src);
    journal.files.pop();
    write_pending_for(&root, &journal);

    let err = adopt::transact(
        &root,
        &src,
        "content",
        Some("imported"),
        "existing",
        &BTreeMap::new(),
        7,
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("not this one"),
        "file-set mismatch refuses: {err}"
    );
}

/// Journal enforcement: the recorded sha256 must match the live source
/// bytes — hash entries are enforced integrity evidence, not decor.
#[test]
fn a_journal_whose_hashes_differ_from_the_source_refuses_the_resume() {
    let (source_ws, run_id) = hugo_source();
    let target = tempfile::tempdir().unwrap();
    let root = target.path().canonicalize().unwrap();
    let src = adopt::source(source_ws.path(), &run_id).unwrap();
    let mut journal = pending_for(&root, &run_id, &src);
    journal.files[0].sha256 = store::sha256_hex(b"not the real bytes");
    write_pending_for(&root, &journal);

    let err = adopt::transact(
        &root,
        &src,
        "content",
        Some("imported"),
        "existing",
        &BTreeMap::new(),
        7,
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("different bytes"),
        "hash mismatch refuses: {err}"
    );
}

/// And the positive control: a faithful journal still resumes.
#[test]
fn a_faithful_journal_still_resumes() {
    let (source_ws, run_id) = hugo_source();
    let target = tempfile::tempdir().unwrap();
    let root = target.path().canonicalize().unwrap();
    let src = adopt::source(source_ws.path(), &run_id).unwrap();
    let journal = pending_for(&root, &run_id, &src);
    write_pending_for(&root, &journal);
    // Land one byte-identical file as the interrupted attempt did.
    let planned = adopt::plan(&src, "content", Some("imported")).unwrap();
    adopt::copy_content(&root, &planned[..1], &[true]).unwrap();

    let tx = adopt::transact(
        &root,
        &src,
        "content",
        Some("imported"),
        "existing",
        &BTreeMap::new(),
        7,
    )
    .expect("a faithful journal resumes");
    assert_eq!(tx.resumed, 1);
}

/// Adopt-to-new crash window: the starter tree is already cleared, the
/// journal durable, the copy incomplete. Because deletion is inside the
/// recovery unit, the retry converges without any starter restoration
/// ritual — and the absent content dir does not confuse the re-clear.
#[test]
fn adopt_to_new_crash_after_clearing_resumes_to_convergence() {
    let (source_ws, run_id) = hugo_source();
    let target = tempfile::tempdir().unwrap();
    let root = target.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("content")).unwrap();
    std::fs::write(root.join("content/index.md"), "starter\n").unwrap();
    let src = adopt::source(source_ws.path(), &run_id).unwrap();

    // First attempt crashes after the clear but mid-copy: faithful
    // journal (starter manifest included), starter already deleted,
    // one incoming file written — then silence.
    let journal = new_workspace_journal(&root, &run_id, &src);
    write_pending_for(&root, &journal);
    std::fs::remove_dir_all(root.join("content")).unwrap();
    let planned = adopt::plan(&src, "content", None).unwrap();
    adopt::copy_content(&root, &planned[..1], &[true]).unwrap();

    // The retry must converge with no total re-clear: the one byte-
    // identical partial output is skipped, not rewritten, and the rest
    // of the plan lands.
    let tx = adopt::transact(&root, &src, "content", None, "new", &BTreeMap::new(), 42)
        .expect("crash after clearing is exactly what resume converges");
    assert_eq!(tx.resumed, 1, "byte-identical partial output is skipped");
    assert_eq!(tx.written, planned.len() - 1);
    for f in &src.files {
        let landed = root.join("content").join(&f.relative);
        assert_eq!(
            std::fs::read(&landed).unwrap(),
            std::fs::read(&f.source).unwrap(),
            "{}",
            f.relative
        );
    }
    assert!(store::read_pending(&root, &run_id).unwrap().is_none());
}

/// A faithful adopt-to-new journal: planned files plus the byte evidence
/// of whatever the starter tree currently holds.
fn new_workspace_journal(
    root: &std::path::Path,
    run_id: &str,
    src: &adopt::AdoptionSource,
) -> store::PendingAdoption {
    let mut journal = store::PendingAdoption::new(run_id, "new", "content");
    let planned = adopt::plan(src, "content", None).unwrap();
    for p in &planned {
        let bytes = std::fs::read(&p.source).unwrap();
        journal.files.push(store::PendingFile {
            target: p.target.clone(),
            sha256: store::sha256_hex(&bytes),
            bytes: bytes.len() as u64,
        });
    }
    if root.join("content").is_dir() {
        for f in walk_files(&root.join("content")) {
            let bytes = std::fs::read(root.join("content").join(&f)).unwrap();
            journal.starter.push(store::PendingFile {
                target: format!("content/{}", f.display()),
                sha256: store::sha256_hex(&bytes),
                bytes: bytes.len() as u64,
            });
        }
    }
    journal
}

/// Crash-interrupted state, then a foreign user file appears before the
/// retry. The resume must REFUSE and the file must survive untouched —
/// the exact data-loss probe from review.
#[test]
fn a_foreign_file_written_after_the_crash_survives_and_refuses_the_resume() {
    let (source_ws, run_id) = hugo_source();
    let target = tempfile::tempdir().unwrap();
    let root = target.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("content")).unwrap();
    std::fs::write(root.join("content/index.md"), "starter\n").unwrap();
    let src = adopt::source(source_ws.path(), &run_id).unwrap();

    let journal = new_workspace_journal(&root, &run_id, &src);
    write_pending_for(&root, &journal);
    std::fs::remove_dir_all(root.join("content")).unwrap();
    let planned = adopt::plan(&src, "content", None).unwrap();
    adopt::copy_content(&root, &planned[..1], &[true]).unwrap();

    // The user inspects the half-created workspace and writes a note.
    std::fs::write(root.join("content/user-note.md"), "mine\n").unwrap();

    let err =
        adopt::transact(&root, &src, "content", None, "new", &BTreeMap::new(), 42).unwrap_err();
    assert!(
        matches!(err, AdoptError::Foreign(_)),
        "foreign file refuses: {err}"
    );
    assert!(
        err.to_string().contains("user-note.md"),
        "the refusal names the foreign file: {err}"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("content/user-note.md")).unwrap(),
        "mine\n",
        "the foreign file survives byte-for-byte"
    );
    // And starter debris is untouched too: the refusal happened in
    // classification, before any deletion.
    assert!(
        root.join(&planned[0].target).exists(),
        "partial output is left in place for the eventual retry"
    );

    // Once the user moves their file aside, the retry converges.
    std::fs::remove_file(root.join("content/user-note.md")).unwrap();
    adopt::transact(&root, &src, "content", None, "new", &BTreeMap::new(), 42)
        .expect("after the user reclaims their file the retry converges");
}

/// The exact review probe: a file merely NAMED like staged-write debris
/// is user-controlled input, not provenance. It must survive byte-for-
/// byte and the resume must refuse — automated deletion by filename is
/// a data-loss capability this design does not have.
#[test]
fn a_file_named_like_write_debris_is_user_data_and_survives() {
    let (source_ws, run_id) = hugo_source();
    let target = tempfile::tempdir().unwrap();
    let root = target.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("content")).unwrap();
    std::fs::write(root.join("content/index.md"), "starter\n").unwrap();
    let src = adopt::source(source_ws.path(), &run_id).unwrap();

    let journal = new_workspace_journal(&root, &run_id, &src);
    write_pending_for(&root, &journal);
    std::fs::remove_dir_all(root.join("content")).unwrap();
    std::fs::create_dir_all(root.join("content")).unwrap();
    std::fs::write(
        root.join("content/.cv-write-user-notes.tmp"),
        "important user data",
    )
    .unwrap();

    let err =
        adopt::transact(&root, &src, "content", None, "new", &BTreeMap::new(), 42).unwrap_err();
    assert!(
        matches!(err, AdoptError::Foreign(_)),
        "temp-named user data refuses the resume: {err}"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("content/.cv-write-user-notes.tmp")).unwrap(),
        "important user data",
        "the file survives byte-for-byte"
    );
}

/// Genuine stranded debris gets the same treatment: safe refusal, never
/// deletion. The user recognizes it as litter, removes it explicitly,
/// and the retry converges.
#[test]
fn staged_write_debris_refuses_safely_until_the_user_cleans_it() {
    let (source_ws, run_id) = hugo_source();
    let target = tempfile::tempdir().unwrap();
    let root = target.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("content")).unwrap();
    std::fs::write(root.join("content/index.md"), "starter\n").unwrap();
    let src = adopt::source(source_ws.path(), &run_id).unwrap();

    let journal = new_workspace_journal(&root, &run_id, &src);
    write_pending_for(&root, &journal);
    std::fs::remove_dir_all(root.join("content")).unwrap();
    std::fs::create_dir_all(root.join("content")).unwrap();
    std::fs::write(root.join("content/.cv-write-x7f2.tmp"), "partial bytes").unwrap();

    let err =
        adopt::transact(&root, &src, "content", None, "new", &BTreeMap::new(), 42).unwrap_err();
    assert!(
        matches!(err, AdoptError::Foreign(_)),
        "even real debris is a safe refusal, never a deletion: {err}"
    );
    assert!(
        root.join("content/.cv-write-x7f2.tmp").exists(),
        "untouched"
    );

    // Explicit user cleanup, then convergence.
    std::fs::remove_file(root.join("content/.cv-write-x7f2.tmp")).unwrap();
    let tx = adopt::transact(&root, &src, "content", None, "new", &BTreeMap::new(), 42)
        .expect("after explicit cleanup the retry converges");
    assert!(tx.written > 0);
}

/// A foreign EDIT to a starter path: the bytes no longer match the
/// manifest, so the file is user data and must survive as a refusal.
#[test]
fn a_starter_file_edited_after_the_crash_is_foreign_and_survives() {
    let (source_ws, run_id) = hugo_source();
    let target = tempfile::tempdir().unwrap();
    let root = target.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("content")).unwrap();
    std::fs::write(root.join("content/index.md"), "starter\n").unwrap();
    let src = adopt::source(source_ws.path(), &run_id).unwrap();

    let journal = new_workspace_journal(&root, &run_id, &src);
    write_pending_for(&root, &journal);
    // Crash BEFORE the clear even ran: the starter tree is intact.
    // The user then edits the starter page.
    std::fs::write(
        root.join("content/index.md"),
        "starter, but edited by hand\n",
    )
    .unwrap();

    let err =
        adopt::transact(&root, &src, "content", None, "new", &BTreeMap::new(), 42).unwrap_err();
    assert!(
        matches!(err, AdoptError::Foreign(_)),
        "edited starter is foreign: {err}"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("content/index.md")).unwrap(),
        "starter, but edited by hand\n",
        "the edited file survives byte-for-byte"
    );
}

/// A foreign file squatting at a planned target path with different
/// bytes refuses; the byte-identical case is partial output and skips.
#[test]
fn foreign_bytes_at_a_planned_path_refuse_but_identical_output_skips() {
    let (source_ws, run_id) = hugo_source();
    let target = tempfile::tempdir().unwrap();
    let root = target.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("content")).unwrap();
    std::fs::write(root.join("content/index.md"), "starter\n").unwrap();
    let src = adopt::source(source_ws.path(), &run_id).unwrap();
    let planned = adopt::plan(&src, "content", None).unwrap();

    let journal = new_workspace_journal(&root, &run_id, &src);
    write_pending_for(&root, &journal);
    std::fs::remove_dir_all(root.join("content")).unwrap();

    // Foreign squat at planned[0]: bytes differ from the incoming source.
    let victim = root.join(&planned[0].target);
    std::fs::create_dir_all(victim.parent().unwrap()).unwrap();
    std::fs::write(&victim, b"not the migration bytes").unwrap();
    let err =
        adopt::transact(&root, &src, "content", None, "new", &BTreeMap::new(), 42).unwrap_err();
    assert!(
        matches!(err, AdoptError::Foreign(_)),
        "foreign bytes at a planned path refuse: {err}"
    );
    assert_eq!(
        std::fs::read(&victim).unwrap(),
        b"not the migration bytes",
        "the squatter is never overwritten or deleted"
    );

    // Byte-identical partial output at planned[1] skips instead.
    std::fs::remove_file(&victim).unwrap();
    adopt::copy_content(&root, &planned[..2], &[true, true]).unwrap();
    let tx = adopt::transact(&root, &src, "content", None, "new", &BTreeMap::new(), 42)
        .expect("identical partial output is fine");
    assert_eq!(tx.resumed, 2);
}

/// The reviewer probe: `content/imported/en` exists as a regular FILE
/// while the incoming tree needs it as a directory. Preflight must
/// refuse before the journal and before the first content byte — never
/// fail mid-copy over partial output.
#[test]
fn a_file_blocking_a_parent_directory_refuses_before_any_write() {
    let (source_ws, run_id) = hugo_source();
    let target = tempfile::tempdir().unwrap();
    let root = target.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("content/imported")).unwrap();
    std::fs::write(
        root.join("content/imported/en"),
        "i am a file, not a directory",
    )
    .unwrap();
    let src = adopt::source(source_ws.path(), &run_id).unwrap();
    assert!(
        src.files.iter().any(|f| f.relative.starts_with("en/")),
        "the hugo capture really does need en/ as a directory"
    );

    let err = adopt::transact(
        &root,
        &src,
        "content",
        Some("imported"),
        "existing",
        &BTreeMap::new(),
        7,
    )
    .unwrap_err();
    let AdoptError::Collisions(collisions) = &err else {
        panic!("parent-shape conflict must be a collision refusal: {err}");
    };
    assert!(
        collisions
            .iter()
            .any(|c| c.kind == adopt::CollisionKind::ParentShape),
        "the collision names the parent shape: {collisions:?}"
    );

    // Nothing was written: no partial content, no journal, no provenance.
    assert_eq!(
        std::fs::read_to_string(root.join("content/imported/en")).unwrap(),
        "i am a file, not a directory",
        "the blocking file is untouched"
    );
    assert!(
        !root.join("content/imported").join("en").is_dir(),
        "no directory was created over the file"
    );
    let content_files = walk_files(&root.join("content"));
    assert_eq!(
        content_files,
        vec![std::path::PathBuf::from("imported/en")],
        "only the user's own blocker file remains — no partial content"
    );
    assert!(
        store::read_pending(&root, &run_id).unwrap().is_none(),
        "no journal: there is nothing to resume because nothing was written"
    );

    // And the workspace is exactly as it was, so a retry after the user
    // removes the blocker converges without any cleanup ritual.
    std::fs::remove_file(root.join("content/imported/en")).unwrap();
    let tx = adopt::transact(
        &root,
        &src,
        "content",
        Some("imported"),
        "existing",
        &BTreeMap::new(),
        7,
    )
    .expect("after removing the blocker the merge converges");
    assert_eq!(tx.written, src.files.len());
}

/// A symlinked parent component is refused the same way.
#[cfg(unix)]
#[test]
fn a_symlink_blocking_a_parent_directory_refuses_before_any_write() {
    let (source_ws, run_id) = hugo_source();
    let target = tempfile::tempdir().unwrap();
    let root = target.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("content/imported")).unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(elsewhere.path(), root.join("content/imported/en")).unwrap();
    let src = adopt::source(source_ws.path(), &run_id).unwrap();

    let err = adopt::transact(
        &root,
        &src,
        "content",
        Some("imported"),
        "existing",
        &BTreeMap::new(),
        7,
    )
    .unwrap_err();
    assert!(
        matches!(err, AdoptError::Collisions(_)),
        "symlinked parent refuses pre-write: {err}"
    );
    assert!(store::read_pending(&root, &run_id).unwrap().is_none());
}

/// Every file below `dir`, as paths relative to `dir` itself.
fn walk_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    fn go(base: &std::path::Path, rel: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let abs = base.join(rel);
        if let Ok(entries) = std::fs::read_dir(&abs) {
            for e in entries.flatten() {
                let next = rel.join(e.file_name());
                if e.path().is_dir() {
                    go(base, &next, out);
                } else {
                    out.push(next);
                }
            }
        }
    }
    let mut out = Vec::new();
    go(dir, std::path::Path::new(""), &mut out);
    out
}

/// A crash victim whose bytes differ can only be foreign data — a user
/// edit after the interruption or corruption — and must still refuse.
#[test]
fn resume_refuses_targets_whose_bytes_differ() {
    let (source_ws, run_id) = hugo_source();
    let target = tempfile::tempdir().unwrap();
    let root = target.path().canonicalize().unwrap();
    let src = adopt::source(source_ws.path(), &run_id).unwrap();
    let planned = adopt::plan(&src, "content", Some("imported")).unwrap();

    let journal = pending_for(&root, &run_id, &src);
    write_pending_for(&root, &journal);
    let victim = root.join(&planned[0].target);
    std::fs::create_dir_all(victim.parent().unwrap()).unwrap();
    std::fs::write(&victim, b"user edited this after the crash").unwrap();

    let err = adopt::transact(
        &root,
        &src,
        "content",
        Some("imported"),
        "existing",
        &BTreeMap::new(),
        7,
    )
    .unwrap_err();
    assert!(
        matches!(err, AdoptError::Collisions(_)),
        "different bytes refuse even in resume mode: {err}"
    );
    assert_eq!(
        std::fs::read(&victim).unwrap(),
        b"user edited this after the crash",
        "user data is never overwritten"
    );
}

/// A retry naming a different target shape must not mix two operations'
/// partial state.
#[test]
fn mismatched_pending_journal_refuses_the_retry() {
    let (source_ws, run_id) = hugo_source();
    let target = tempfile::tempdir().unwrap();
    let root = target.path().canonicalize().unwrap();
    let src = adopt::source(source_ws.path(), &run_id).unwrap();

    let mut journal = store::PendingAdoption::new(&run_id, "existing", "content");
    journal.content_subfolder = Some("first-attempt".into());
    write_pending_for(&root, &journal);

    let err = adopt::transact(
        &root,
        &src,
        "content",
        Some("different-subfolder"),
        "existing",
        &BTreeMap::new(),
        7,
    )
    .unwrap_err();
    assert!(err.to_string().contains("interrupted adoption"), "{err}");
}

#[test]
fn inventory_only_runs_are_not_adoptable() {
    let ws = tempfile::tempdir().unwrap();
    let run_id = seed_run(ws.path(), "hugo-inventory", 1_764_000_000_001);
    let err = adopt::source(ws.path(), &run_id).unwrap_err();
    assert!(
        err.to_string().contains("inventory"),
        "inventory mode is rejected explicitly: {err}"
    );
}

#[test]
fn inventory_mode_cannot_adopt_even_with_emit_shaped_evidence() {
    let ws = tempfile::tempdir().unwrap();
    let root = ws.path().canonicalize().unwrap();
    let run_id = seed_run(&root, "hugo-inventory", 1_764_100_000_000);
    let dir = store::run_dir(&root, &run_id).unwrap();
    std::fs::remove_dir_all(dir.join(store::OUT)).unwrap();
    StubLab::new().run("hugo", &dir.join(store::OUT)).unwrap();

    let err = adopt::source(&root, &run_id).unwrap_err();
    assert!(
        err.to_string().contains("inventory"),
        "mode validation rejects the run before structural evidence can fail open: {err}"
    );
}

#[cfg(unix)]
#[test]
fn a_symlink_at_a_final_target_is_a_pre_write_collision_not_a_mid_copy_failure() {
    let (source_ws, run_id) = hugo_source();
    let source_root = source_ws.path().canonicalize().unwrap();
    let src = adopt::source(&source_root, &run_id).unwrap();
    let target = tempfile::tempdir().unwrap();
    let root = target.path().canonicalize().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("outside.md"), b"outside").unwrap();
    let victim = root.join("content/imported/fr/posts/bonjour.md");
    std::fs::create_dir_all(victim.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(outside.path().join("outside.md"), &victim).unwrap();

    let err = adopt::transact(
        &root,
        &src,
        "content",
        Some("imported"),
        "existing",
        &BTreeMap::new(),
        7,
    )
    .unwrap_err();
    assert!(
        matches!(err, AdoptError::Collisions(_)),
        "a final-target symlink is a deterministic pre-write collision: {err}"
    );
    let copied: Vec<_> = walk_files(&root.join("content"))
        .into_iter()
        .filter(|p| p != &std::path::PathBuf::from("imported/fr/posts/bonjour.md"))
        .collect();
    assert!(
        copied.is_empty(),
        "no earlier file may land when any final target is a symlink: {copied:?}"
    );
}

#[test]
fn a_held_run_lock_refuses_adoption_before_any_write() {
    let (source_ws, run_id) = hugo_source();
    let root = source_ws.path().canonicalize().unwrap();
    let src = adopt::source(&root, &run_id).unwrap();
    let held = store::lock_run(&root, &run_id).unwrap();
    let err = adopt::transact(
        &root,
        &src,
        "content",
        Some("imported"),
        "existing",
        &BTreeMap::new(),
        7,
    )
    .unwrap_err();
    assert!(
        matches!(err, AdoptError::Store(store::Error::LockHeld(_))),
        "the contested per-run lock refuses adoption: {err}"
    );
    drop(held);
    assert!(!root.join("content/imported").exists());
}

#[test]
fn unsafe_subfolders_are_refused() {
    for bad in [
        "",
        "..",
        "../escape",
        "a/../b",
        "/abs",
        "a//b",
        "a\\b",
        "CON",
    ] {
        assert!(adopt::subfolder(bad).is_err(), "{bad:?} must be refused");
    }
    assert_eq!(adopt::subfolder("imported/site").unwrap(), "imported/site");

    let absolute = adopt::subfolder("/abs").unwrap_err().to_string();
    assert!(absolute.contains("content input"), "{absolute}");
    assert!(!absolute.contains("content/"), "{absolute}");
    let empty = adopt::subfolder("").unwrap_err().to_string();
    assert!(empty.contains("content input"), "{empty}");
    assert!(!empty.contains("content/"), "{empty}");
}

#[test]
fn unfinished_runs_are_not_adoptable() {
    let ws = tempfile::tempdir().unwrap();
    let root = ws.path().canonicalize().unwrap();
    let rec = RunRecord::started(
        "r1",
        "hugo",
        RecordedLab {
            tool_id: "boris-migration-lab/0.8.1".into(),
            source: "fixture".into(),
            path: "x".into(),
        },
        0,
    );
    write_record(&root, &rec);
    assert!(adopt::source(&root, "r1").is_err());
}

#[cfg(unix)]
#[test]
fn symlinked_candidate_content_is_refused() {
    let (source_ws, run_id) = hugo_source();
    let root = source_ws.path().canonicalize().unwrap();
    let content = store::run_dir(&root, &run_id)
        .unwrap()
        .join(store::OUT)
        .join("content");
    let elsewhere = tempfile::tempdir().unwrap();
    let victim = content.join("en/posts/draft-note.md");
    assert!(victim.is_file(), "capture layout unchanged");
    std::fs::remove_file(&victim).unwrap();
    std::os::unix::fs::symlink(elsewhere.path().join("outside.md"), &victim).unwrap();
    let err = adopt::source(&root, &run_id).unwrap_err();
    assert!(err.to_string().contains("symlink"), "{err}");
}
