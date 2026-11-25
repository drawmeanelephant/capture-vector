//! Adoption acceptance tests over the checked-in captured Hugo lab
//! output: adopt-to-new proves validate/build run and provenance lands in
//! `.cv/imports/<run>/`; the existing-workspace merge proves collisions
//! refuse before the first write and nothing is silently overwritten.

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

#[test]
fn adopt_to_new_copies_content_provenance_and_runs_validate_then_build() {
    let source_ws = tempfile::tempdir().unwrap();
    let run_id = seed_run(source_ws.path(), "hugo", 1_764_000_000_000);
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

    let planned = adopt::plan(&src, "content", None).unwrap();
    let written = adopt::copy_content(&root, &planned).unwrap();
    assert_eq!(written, src.files.len());
    // Byte-for-byte copy, including binary assets.
    for f in &src.files {
        let dest = planned
            .iter()
            .find(|p| p.display == f.relative)
            .expect("planned");
        assert_eq!(
            std::fs::read(&f.source).unwrap(),
            std::fs::read(root.join(&dest.target)).unwrap(),
            "{}",
            dest.target
        );
    }
    let provenance = adopt::copy_provenance(&src, &root).unwrap();
    assert!(provenance > 0);
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

    adopt::record_adoption(&src, &root, 42, "new", None, &planned).unwrap();
    let adoptions = store::scan_adoptions(&root).unwrap();
    assert_eq!(adoptions.len(), 1);
    assert_eq!(adoptions[0].run_id, run_id);
    assert_eq!(adoptions[0].mode, "hugo");
    assert!(!adoptions[0].pages.is_empty());
    // Provenance stays in the sidecar: no adopted page gained frontmatter
    // keys it did not already have — verified byte-for-byte above.
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
    let source_ws = tempfile::tempdir().unwrap();
    let run_id = seed_run(source_ws.path(), "hugo", 1_764_000_000_000);
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

    // Pre-create one colliding path and fill the rest of the merge first
    // to prove the preflight refuses before ANY write.
    let victim = planned[0].clone();
    std::fs::create_dir_all(root.join("content/imported/site")).unwrap();
    let victim_path = root.join(&victim.target);
    std::fs::create_dir_all(victim_path.parent().unwrap()).unwrap();
    std::fs::write(&victim_path, b"user data").unwrap();

    let err = adopt::collisions(&root, &src, &planned, &BTreeMap::new()).unwrap_err();
    let AdoptError::Collisions(cs) = &err else {
        panic!("expected collisions, got {err}");
    };
    assert!(
        cs.iter()
            .any(|c| c.kind == CollisionKind::Path && c.incoming == victim.display),
        "path collision reported: {cs:?}"
    );
    // Nothing was written beyond the pre-existing victim file, and the
    // victim's bytes are untouched.
    assert_eq!(std::fs::read(&victim_path).unwrap(), b"user data");
    assert_eq!(
        std::fs::read_dir(root.join("content/imported/site"))
            .unwrap()
            .count(),
        victim_path
            .parent()
            .unwrap()
            .strip_prefix(root.join("content/imported/site"))
            .map(|_| 1)
            .unwrap_or(0),
        "no other merge files appeared"
    );
}

#[test]
fn existing_workspace_merge_refuses_id_collisions_before_writing() {
    let source_ws = tempfile::tempdir().unwrap();
    let run_id = seed_run(source_ws.path(), "hugo", 1_764_000_000_000);
    let target = tempfile::tempdir().unwrap();
    let root = target.path().canonicalize().unwrap();

    let src = adopt::source(source_ws.path(), &run_id).unwrap();
    let planned = adopt::plan(&src, "content", Some("imported")).unwrap();
    let first = src.pages[0].entity_id.clone();
    let mut ids = BTreeMap::new();
    ids.insert(first.clone(), "posts/existing.md".to_string());

    let err = adopt::collisions(&root, &src, &planned, &ids).unwrap_err();
    let AdoptError::Collisions(cs) = &err else {
        panic!("expected collisions, got {err}");
    };
    assert!(
        cs.iter()
            .any(|c| c.kind == CollisionKind::Id && c.incoming == first),
        "id collision reported: {cs:?}"
    );
    assert!(!root.join("content").exists(), "nothing was written");
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
    let source_ws = tempfile::tempdir().unwrap();
    let run_id = seed_run(source_ws.path(), "hugo", 1_764_000_000_000);
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
