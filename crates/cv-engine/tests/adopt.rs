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
        exit: "ok".into(),
        outcome: format!("exited {}", t.exit_code),
        duration_ms: 0,
    });
    store::write(root, &rec).expect("run record");
    id
}

fn hugo_source() -> (tempfile::TempDir, String) {
    let source_ws = tempfile::tempdir().unwrap();
    let run_id = seed_run(source_ws.path(), "hugo", 1_764_000_000_000);
    (source_ws, run_id)
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

    // The starter was just initialized; its content tree is replaced.
    std::fs::create_dir_all(root.join("content")).unwrap();
    std::fs::write(root.join("content/index.md"), "starter\n").unwrap();
    adopt::clear_new_workspace_content(&root, "content").unwrap();
    assert!(!root.join("content").exists());

    let tx = adopt::transact(&root, &src, "content", None, "new", &BTreeMap::new(), 42).unwrap();
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
    store::write_pending(&root, &journal).unwrap();
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

    let mut journal = store::PendingAdoption::new(&run_id, "existing", "content");
    journal.content_subfolder = Some("imported".into());
    store::write_pending(&root, &journal).unwrap();
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

    let mut journal = store::PendingAdoption::new(&run_id, "existing", "content");
    journal.content_subfolder = Some("imported".into());
    store::write_pending(&root, &journal).unwrap();

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

/// A crash victim whose bytes differ can only be foreign data — a user
/// edit after the interruption or corruption — and must still refuse.
#[test]
fn resume_refuses_targets_whose_bytes_differ() {
    let (source_ws, run_id) = hugo_source();
    let target = tempfile::tempdir().unwrap();
    let root = target.path().canonicalize().unwrap();
    let src = adopt::source(source_ws.path(), &run_id).unwrap();
    let planned = adopt::plan(&src, "content", Some("imported")).unwrap();

    let journal = {
        let mut j = store::PendingAdoption::new(&run_id, "existing", "content");
        j.content_subfolder = Some("imported".into());
        j
    };
    store::write_pending(&root, &journal).unwrap();
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
    store::write_pending(&root, &journal).unwrap();

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
    store::write(&root, &rec).unwrap();
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
