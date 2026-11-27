//! Round-2 adoption authority regressions: exact emit-mode admission,
//! zero-write target preflight, scheduler serialization, and durable
//! provenance across repeated adoptions.

use cv_core::engine::{Engine, EngineCaps, EngineSource, PageDraft};
use cv_engine::adopt;
use cv_engine::locate::{EngineIdentity, PINNED};
use cv_engine::probe::{CapabilityReport, WatchMode};
use cv_engine::{EngineConfig, ProcessEngine};
use cv_fixtures::lab::StubLab;
use cv_store::imports::{self as store, Finished, RecordedLab, RunRecord};
use std::collections::BTreeMap;
use std::path::Path;

fn seed_hugo_evidence(root: &Path, run_id: &str, mode: &str) {
    let root = &root.canonicalize().unwrap();
    let lock = store::claim_run(root, run_id).unwrap();
    let transcript = StubLab::new()
        .run("hugo", &lock.dir().join(store::OUT))
        .unwrap();
    let mut rec = RunRecord::started(
        run_id,
        mode,
        RecordedLab {
            tool_id: StubLab::new().tool_id(),
            source: "fixture".into(),
            path: "fixtures/captures/lab".into(),
        },
        1_764_100_000_000,
    );
    rec.argv = transcript
        .argv
        .iter()
        .map(|a| {
            if a == "--mode=hugo" {
                format!("--mode={mode}")
            } else {
                a.clone()
            }
        })
        .collect();
    rec.finished = Some(Finished {
        exit_code: Some(0),
        exit: store::FinishedExit::Ok,
        outcome: "exited 0".into(),
        duration_ms: 0,
    });
    store::write(&lock, &rec).unwrap();
}

fn walk_files(dir: &Path) -> Vec<std::path::PathBuf> {
    fn go(base: &Path, rel: &Path, out: &mut Vec<std::path::PathBuf>) {
        if let Ok(entries) = std::fs::read_dir(base.join(rel)) {
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
    go(dir, Path::new(""), &mut out);
    out
}

#[test]
fn non_emitting_or_unknown_modes_cannot_adopt_emit_shaped_evidence() {
    // Exact `-inventory` is now rejected, so attack the boundary of that
    // string check and the closed set of modes that can legitimately emit
    // content. Every one of these records is either not a canonical lab
    // mode or is a canonical non-emitting/cross-cutting mode.
    let modes = [
        "hugo-inventory ",
        "HUGO-INVENTORY",
        "hugo-inventory\t",
        "link-audit",
        "theme-archaeology",
        "not-a-registered-mode",
    ];
    let mut adopted = Vec::new();
    for (n, mode) in modes.iter().enumerate() {
        let source_ws = tempfile::tempdir().unwrap();
        let source_root = source_ws.path().canonicalize().unwrap();
        let run_id = format!("mode-probe-{n}");
        seed_hugo_evidence(&source_root, &run_id, mode);
        let target_ws = tempfile::tempdir().unwrap();
        let target_root = target_ws.path().canonicalize().unwrap();
        let result = adopt::source(&source_root, &run_id).and_then(|src| {
            adopt::transact(
                &target_root,
                &src,
                "content",
                Some("imported"),
                "existing",
                &BTreeMap::new(),
                7,
            )
        });
        if result.is_ok() && !walk_files(&target_root.join("content")).is_empty() {
            adopted.push((*mode, run_id));
        }
    }
    assert!(
        adopted.is_empty(),
        "MODE-FAIL-OPEN: non-emitting/unknown modes adopted emit-shaped evidence: {adopted:?}"
    );
}

#[test]
fn an_emit_mode_cannot_borrow_another_familys_report() {
    let source_ws = tempfile::tempdir().unwrap();
    let source_root = source_ws.path().canonicalize().unwrap();
    seed_hugo_evidence(&source_root, "wrong-family-jekyll", "jekyll");
    let err = adopt::source(&source_root, "wrong-family-jekyll").unwrap_err();
    assert!(
        err.to_string().contains("boris-hugo-migration-lab")
            && err.to_string().contains("mode jekyll"),
        "the report-family check itself must refuse once mode and argv agree: {err}"
    );
}

#[test]
fn undecodable_adoption_evidence_is_never_overwritten_after_partial_writes() {
    let ws = tempfile::tempdir().unwrap();
    let root = ws.path().canonicalize().unwrap();
    let run_id = "corrupt-adoption-hugo";
    seed_hugo_evidence(&root, run_id, "hugo");
    let sidecar = store::run_dir(&root, run_id).unwrap().join(store::ADOPTION);
    std::fs::write(&sidecar, b"not json\n").unwrap();
    let src = adopt::source(&root, run_id).unwrap();
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
    assert!(err.to_string().contains(store::ADOPTION), "{err}");
    assert!(!root.join("content/imported").exists());
    assert!(
        !store::run_dir(&root, run_id)
            .unwrap()
            .join(store::PENDING)
            .exists()
    );
    assert_eq!(std::fs::read(&sidecar).unwrap(), b"not json\n");
}

#[test]
fn mismatched_existing_provenance_is_a_zero_write_collision() {
    let ws = tempfile::tempdir().unwrap();
    let root = ws.path().canonicalize().unwrap();
    let run_id = "mismatched-provenance-hugo";
    seed_hugo_evidence(&root, run_id, "hugo");
    let src = adopt::source(&root, run_id).unwrap();
    adopt::transact(
        &root,
        &src,
        "content",
        Some("first"),
        "existing",
        &BTreeMap::new(),
        7,
    )
    .unwrap();
    let report = store::run_dir(&root, run_id).unwrap().join("report.json");
    std::fs::write(&report, b"corrupted provenance\n").unwrap();

    let err = adopt::transact(
        &root,
        &src,
        "content",
        Some("second"),
        "existing",
        &BTreeMap::new(),
        8,
    )
    .unwrap_err();
    assert!(
        matches!(err, adopt::AdoptError::Collisions(_)),
        "mismatched provenance must be classified as a collision: {err}"
    );
    assert!(!root.join("content/second").exists());
    assert!(
        !store::run_dir(&root, run_id)
            .unwrap()
            .join(store::PENDING)
            .exists()
    );
}

#[cfg(unix)]
#[test]
fn a_symlinked_provenance_target_is_not_a_post_copy_failure() {
    let ws = tempfile::tempdir().unwrap();
    let root = ws.path().canonicalize().unwrap();
    let run_id = "prov-symlink-hugo";
    seed_hugo_evidence(&root, run_id, "hugo");

    let outside = tempfile::tempdir().unwrap();
    let victim = outside.path().join("victim.json");
    std::fs::write(&victim, b"do not touch").unwrap();
    let run_dir = store::run_dir(&root, run_id).unwrap();
    std::os::unix::fs::symlink(&victim, run_dir.join("report.json")).unwrap();

    let src = adopt::source(&root, run_id).unwrap();
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
    let copied = walk_files(&root.join("content"));
    assert!(
        copied.is_empty(),
        "PROVENANCE-SYMLINK-PARTIAL-WRITE: error={err}; content files landed before the pre-existing provenance symlink refused the transaction: {copied:?}"
    );
    assert_eq!(std::fs::read(&victim).unwrap(), b"do not touch");
}

#[cfg(unix)]
#[test]
fn a_symlinked_adoption_sidecar_is_not_a_post_copy_failure() {
    let ws = tempfile::tempdir().unwrap();
    let root = ws.path().canonicalize().unwrap();
    let run_id = "sidecar-symlink-hugo";
    seed_hugo_evidence(&root, run_id, "hugo");

    let outside = tempfile::tempdir().unwrap();
    let victim = outside.path().join("victim-adoption.json");
    std::fs::write(&victim, b"do not touch").unwrap();
    let run_dir = store::run_dir(&root, run_id).unwrap();
    std::os::unix::fs::symlink(&victim, run_dir.join(store::ADOPTION)).unwrap();

    let src = adopt::source(&root, run_id).unwrap();
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
    let copied = walk_files(&root.join("content"));
    assert!(
        copied.is_empty(),
        "SIDECAR-SYMLINK-PARTIAL-WRITE: error={err}; content files landed before the pre-existing adoption-sidecar symlink refused the transaction: {copied:?}"
    );
    assert_eq!(std::fs::read(&victim).unwrap(), b"do not touch");
}

#[test]
fn two_adoptions_of_one_run_through_one_scheduler_do_not_deadlock_or_double_write() {
    use cv_engine::{EngineScheduler, SeatPolicy};
    use std::sync::{Arc, Barrier};

    let ws = tempfile::tempdir().unwrap();
    let root = ws.path().canonicalize().unwrap();
    let run_id = "two-adoptions-hugo";
    seed_hugo_evidence(&root, run_id, "hugo");
    let src = adopt::source(&root, run_id).unwrap();
    let sched = Arc::new(EngineScheduler::new(
        Arc::new(cv_fixtures::stub_engine::StubEngine::new("happy")),
        SeatPolicy::StopWatch,
    ));
    let barrier = Arc::new(Barrier::new(2));
    let mut threads = Vec::new();
    for _ in 0..2 {
        let sched = sched.clone();
        let barrier = barrier.clone();
        let root = root.clone();
        let src = src.clone();
        threads.push(std::thread::spawn(move || {
            barrier.wait();
            sched.mutation_batch(|| {
                adopt::transact(
                    &root,
                    &src,
                    "content",
                    Some("imported"),
                    "existing",
                    &BTreeMap::new(),
                    7,
                )
            })
        }));
    }
    let mut outcomes = Vec::new();
    for t in threads {
        outcomes.push(t.join().expect("adoption thread completed"));
    }
    let ok = outcomes.iter().filter(|r| r.is_ok()).count();
    let refused = outcomes.iter().filter(|r| r.is_err()).count();
    assert_eq!((ok, refused), (1, 1), "outcomes: {outcomes:?}");
    let mut files = walk_files(&root.join("content"));
    files.sort();
    let mut unique = files.clone();
    unique.dedup();
    assert_eq!(files, unique, "duplicate relative outputs: {files:?}");
}

#[test]
fn resume_after_the_sidecar_write_completes_one_event_not_two() {
    let ws = tempfile::tempdir().unwrap();
    let root = ws.path().canonicalize().unwrap();
    let run_id = "sidecar-crash-resume-hugo";
    seed_hugo_evidence(&root, run_id, "hugo");
    let src = adopt::source(&root, run_id).unwrap();
    adopt::transact(
        &root,
        &src,
        "content",
        Some("imported"),
        "existing",
        &BTreeMap::new(),
        7,
    )
    .unwrap();

    // Simulate the crash window after adoption.json landed but before the
    // recovery journal was retired: a faithful journal for exactly this
    // operation is still present.
    let planned = adopt::plan(&src, "content", Some("imported")).unwrap();
    let mut journal = store::PendingAdoption::new(run_id, "existing", "content");
    journal.content_subfolder = Some("imported".into());
    for p in &planned {
        let bytes = std::fs::read(&p.source).unwrap();
        journal.files.push(store::PendingFile {
            target: p.target.clone(),
            sha256: store::sha256_hex(&bytes),
            bytes: bytes.len() as u64,
        });
    }
    let lock = store::lock_run(&root, run_id).unwrap();
    store::write_pending(&lock, &journal).unwrap();
    drop(lock);

    adopt::transact(
        &root,
        &src,
        "content",
        Some("imported"),
        "existing",
        &BTreeMap::new(),
        8,
    )
    .unwrap();
    let rec = store::scan_adoptions(&root)
        .unwrap()
        .into_iter()
        .find(|r| r.run_id == run_id)
        .unwrap();
    assert_eq!(rec.events.len(), 1, "retry duplicated one adoption event");
    assert_eq!(rec.events[0].at_ms, 8);
    assert!(
        !store::run_dir(&root, run_id)
            .unwrap()
            .join(store::PENDING)
            .exists()
    );
}

#[test]
fn re_adoption_preserves_provenance_for_the_adoption_that_already_landed() {
    let ws = tempfile::tempdir().unwrap();
    let root = ws.path().canonicalize().unwrap();
    let run_id = "re-adoption-hugo";
    seed_hugo_evidence(&root, run_id, "hugo");
    let src = adopt::source(&root, run_id).unwrap();

    adopt::transact(
        &root,
        &src,
        "content",
        Some("first"),
        "existing",
        &BTreeMap::new(),
        7,
    )
    .unwrap();
    assert!(root.join("content/first/en.md").is_file());

    // Version-1 sidecars held the single adoption only in the top-level
    // fields. Downgrade the durable first record in place: the second
    // adoption must migrate that evidence forward instead of treating it
    // as absent or overwritten baggage.
    let sidecar = store::run_dir(&root, run_id).unwrap().join(store::ADOPTION);
    let mut v1: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&sidecar).unwrap()).unwrap();
    v1.as_object_mut().unwrap().remove("events");
    v1["schema_version"] = serde_json::json!(1);
    std::fs::write(&sidecar, serde_json::to_vec_pretty(&v1).unwrap()).unwrap();

    adopt::transact(
        &root,
        &src,
        "content",
        Some("second"),
        "existing",
        &BTreeMap::new(),
        8,
    )
    .unwrap();
    assert!(root.join("content/second/en.md").is_file());

    let records = store::scan_adoptions(&root).unwrap();
    let first_evidence = records.iter().any(|r| {
        r.pages
            .iter()
            .any(|p| p.path == "first/en.md" && p.entity_id == "en")
    });
    let second_evidence = records.iter().any(|r| {
        r.pages
            .iter()
            .any(|p| p.path == "second/en.md" && p.entity_id == "en")
    });
    assert!(
        first_evidence && second_evidence,
        "RE-ADOPTION-PROVENANCE-LOSS: both content trees remain, but adoption sidecars no longer prove both; first_evidence={first_evidence}, second_evidence={second_evidence}, records={records:?}"
    );
    let rec = records.iter().find(|r| r.run_id == run_id).unwrap();
    assert_eq!(rec.events.len(), 2, "events preserve both adoptions");
    assert_eq!(rec.events[0].content_subfolder.as_deref(), Some("first"));
    assert_eq!(rec.events[1].content_subfolder.as_deref(), Some("second"));
    assert_eq!(rec.events[0].at_ms, 7);
    assert_eq!(rec.events[1].at_ms, 8);
}

// Round-17 workspace-mutation locking, post-write identity verification,
// and residual filesystem-interference regressions.

fn copy_tree_contents(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree_contents(&entry.path(), &target);
        } else if entry.file_type().unwrap().is_file() {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn hugo_source() -> (tempfile::TempDir, String) {
    let source_ws = tempfile::tempdir().unwrap();
    let run_id = "round17-hugo".to_string();
    seed_hugo_evidence(source_ws.path(), &run_id, "hugo");
    (source_ws, run_id)
}
fn add_large_payload(root: &Path, run_id: &str) {
    let dir = store::run_dir(root, run_id).unwrap();
    std::fs::write(
        dir.join(store::OUT).join("content/aa-large.bin"),
        vec![b'x'; 32 * 1024 * 1024],
    )
    .unwrap();
}

fn cross_process_child() {
    if std::env::var_os("CV_ADOPT_NEW_CHILD").is_some() {
        let root = std::path::PathBuf::from(std::env::var("CV_ADOPT_ONCE_ROOT").unwrap());
        let source_root = std::path::PathBuf::from(std::env::var("CV_ADOPT_SOURCE_ROOT").unwrap());
        let run_id = std::env::var("CV_ADOPT_ONCE_RUN").unwrap();
        let src = adopt::source(&source_root, &run_id).unwrap();
        match adopt::transact(&root, &src, "content", None, "new", &BTreeMap::new(), 42) {
            Ok(report) => println!("new ok {report:?}"),
            Err(e) => {
                println!("new err {e}");
                std::process::exit(2);
            }
        }
        return;
    }
    if std::env::var_os("CV_ADOPT_ONCE_CHILD").is_none() {
        return;
    }
    let root = std::path::PathBuf::from(std::env::var("CV_ADOPT_ONCE_ROOT").unwrap());
    let run_id = std::env::var("CV_ADOPT_ONCE_RUN").unwrap();
    let subfolder = std::env::var("CV_ADOPT_ONCE_SUBFOLDER").unwrap();
    let token = cv_store::workspace_instance::read(&root).unwrap().unwrap();
    let expected = adopt::ExpectedWorkspaceInstance::new(
        token,
        cv_engine::file_identity::identity(&root),
        cv_store::workspace_mutation::current_identity(&root).unwrap(),
    );
    let src = adopt::source(&root, &run_id).unwrap();
    match adopt::transact_existing_with_identity(adopt::ExistingWorkspaceTransaction {
        target_root: &root,
        src: &src,
        content_input: "content",
        merge_subfolder: Some(&subfolder),
        scan_existing_ids: Box::new(BTreeMap::new),
        at_ms: 42,
        expected_instance: &expected,
    }) {
        Ok(report) => println!("{run_id} ok {report:?}"),
        Err(e) => {
            println!("{run_id} err {e}");
            std::process::exit(2);
        }
    }
}

#[test]
fn cross_process_adoption_child_entry() {
    cross_process_child();
}

#[test]
fn concurrent_cross_process_adoptions_cannot_both_commit_duplicate_page_ids() {
    if std::env::var_os("CV_ADOPT_ONCE_CHILD").is_some() {
        return;
    }
    let base_tmp = tempfile::tempdir().unwrap();
    let root = base_tmp.path().canonicalize().unwrap().join("workspace");
    std::fs::create_dir_all(root.join("content")).unwrap();
    let run_a = "run-a".to_string();
    let run_b = "run-b".to_string();
    seed_hugo_evidence(&root, &run_a, "hugo");
    seed_hugo_evidence(&root, &run_b, "hugo");
    add_large_payload(&root, &run_a);
    add_large_payload(&root, &run_b);
    cv_store::workspace_instance::ensure(&root).unwrap();

    let exe = std::env::current_exe().unwrap();
    let spawn = |run_id: &str, subfolder: &str| {
        std::process::Command::new(&exe)
            .arg("cross_process_adoption_child_entry")
            .arg("--exact")
            .arg("--nocapture")
            .env("CV_ADOPT_ONCE_CHILD", "1")
            .env("CV_ADOPT_ONCE_ROOT", &root)
            .env("CV_ADOPT_ONCE_RUN", run_id)
            .env("CV_ADOPT_ONCE_SUBFOLDER", subfolder)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap()
    };
    let a = spawn(&run_a, "merge-a");
    let b = spawn(&run_b, "merge-b");
    let out_a = a.wait_with_output().unwrap();
    let out_b = b.wait_with_output().unwrap();
    let successes = [out_a.status.success(), out_b.status.success()]
        .into_iter()
        .filter(|ok| *ok)
        .count();
    let merge_a = root.join("content/merge-a/en.md");
    let merge_b = root.join("content/merge-b/en.md");
    println!(
        "R17-CROSS-PROCESS successes={successes} a_status={} b_status={} merge_a={} merge_b={} a_out={} b_out={}",
        out_a.status,
        out_b.status,
        merge_a.exists(),
        merge_b.exists(),
        String::from_utf8_lossy(&out_a.stdout),
        String::from_utf8_lossy(&out_b.stdout)
    );
    assert!(
        successes <= 1,
        "two cross-process adoptions reported success in one workspace"
    );
    if merge_a.exists() && merge_b.exists() {
        let a = std::fs::read_to_string(&merge_a).unwrap_or_default();
        let b = std::fs::read_to_string(&merge_b).unwrap_or_default();
        assert!(
            !(a.contains("id: en") && b.contains("id: en")),
            "duplicate explicit Boris page ids committed in one workspace"
        );
    }
}

#[test]
fn cross_process_ordinary_content_write_cannot_interleave_with_new_workspace_adoption() {
    if std::env::var_os("CV_WRITE_ONCE_CHILD").is_some()
        || std::env::var_os("CV_ADOPT_NEW_CHILD").is_some()
    {
        return;
    }
    let base_tmp = tempfile::tempdir().unwrap();
    let base = base_tmp.path().canonicalize().unwrap();
    let root = base.join("workspace");
    let source_root = base.join("source");
    std::fs::create_dir_all(root.join("content")).unwrap();
    std::fs::create_dir_all(&source_root).unwrap();
    let run_id = "new-adoption-race".to_string();
    seed_hugo_evidence(&source_root, &run_id, "hugo");
    add_large_payload(&source_root, &run_id);
    let exe = std::env::current_exe().unwrap();
    let adopt_child = std::process::Command::new(&exe)
        .arg("cross_process_adoption_child_entry")
        .arg("--exact")
        .arg("--nocapture")
        .env("CV_ADOPT_NEW_CHILD", "1")
        .env("CV_ADOPT_ONCE_ROOT", &root)
        .env("CV_ADOPT_SOURCE_ROOT", &source_root)
        .env("CV_ADOPT_ONCE_RUN", &run_id)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let writer = std::process::Command::new(&exe)
        .arg("ordinary_writer_child_entry")
        .arg("--exact")
        .arg("--nocapture")
        .env("CV_WRITE_ONCE_CHILD", "1")
        .env("CV_WRITE_ONCE_ROOT", &root)
        .env("CV_WRITE_ONCE_RUN", &run_id)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let adopt_out = adopt_child.wait_with_output().unwrap();
    let write_out = writer.wait_with_output().unwrap();
    println!(
        "R21-NEW-ORDINARY adopt={} writer={} out={} err={}",
        adopt_out.status,
        write_out.status,
        String::from_utf8_lossy(&write_out.stdout),
        String::from_utf8_lossy(&write_out.stderr)
    );
    assert!(
        adopt_out.status.success(),
        "{}",
        String::from_utf8_lossy(&adopt_out.stdout)
    );
    assert_eq!(write_out.status.code(), Some(42));
    assert!(!root.join("content/ordinary.md").exists());
}

fn production_write_engine(root: &Path) -> ProcessEngine {
    ProcessEngine::with_report(
        EngineConfig::new(root),
        EngineIdentity {
            source: EngineSource::Path,
            path: root.join("boris"),
            version: PINNED.version.into(),
            vcs_revision: PINNED.vcs_revision.into(),
            build_info_raw: String::new(),
        },
        CapabilityReport {
            os: std::env::consts::OS.into(),
            caps: EngineCaps {
                writes: true,
                ..Default::default()
            },
            watch_mode: WatchMode::Native,
            evidence: vec![],
        },
    )
}

fn ordinary_writer_child() {
    if std::env::var_os("CV_WRITE_ONCE_CHILD").is_none() {
        return;
    }
    let root = std::path::PathBuf::from(std::env::var("CV_WRITE_ONCE_ROOT").unwrap());
    let run_id = std::env::var("CV_WRITE_ONCE_RUN").unwrap();
    let pending = root.join(".cv/imports").join(&run_id).join(store::PENDING);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !pending.exists() {
        if std::time::Instant::now() > deadline {
            eprintln!("ordinary writer never observed the adoption journal");
            std::process::exit(4);
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let engine = production_write_engine(&root);
    let outcome = engine.write_content(
        &PageDraft {
            path: "content/ordinary.md".into(),
            body: "---
id: en
title: Ordinary concurrent page
---
written by another Capture Vector process
"
            .into(),
            base: None,
        },
        None,
    );
    match outcome {
        Err(e) if e.to_string().contains("workspace is busy") => {
            println!("ordinary writer refused: {e}");
            std::process::exit(42);
        }
        Ok(_) => println!("ordinary writer wrote"),
        Err(e) => {
            eprintln!("ordinary writer unexpected error: {e}");
            std::process::exit(3);
        }
    }
}

#[test]
fn ordinary_writer_child_entry() {
    ordinary_writer_child();
}

#[test]
fn cross_process_ordinary_content_write_cannot_interleave_with_adoption() {
    if std::env::var_os("CV_WRITE_ONCE_CHILD").is_some() {
        return;
    }
    let base_tmp = tempfile::tempdir().unwrap();
    let root = base_tmp.path().canonicalize().unwrap().join("workspace");
    std::fs::create_dir_all(root.join("content")).unwrap();
    let run_id = "ordinary-race-run".to_string();
    seed_hugo_evidence(&root, &run_id, "hugo");
    add_large_payload(&root, &run_id);
    cv_store::workspace_instance::ensure(&root).unwrap();

    let exe = std::env::current_exe().unwrap();
    let adopt_child = std::process::Command::new(&exe)
        .arg("cross_process_adoption_child_entry")
        .arg("--exact")
        .arg("--nocapture")
        .env("CV_ADOPT_ONCE_CHILD", "1")
        .env("CV_ADOPT_ONCE_ROOT", &root)
        .env("CV_ADOPT_ONCE_RUN", &run_id)
        .env("CV_ADOPT_ONCE_SUBFOLDER", "imported")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let writer = std::process::Command::new(&exe)
        .arg("ordinary_writer_child_entry")
        .arg("--exact")
        .arg("--nocapture")
        .env("CV_WRITE_ONCE_CHILD", "1")
        .env("CV_WRITE_ONCE_ROOT", &root)
        .env("CV_WRITE_ONCE_RUN", &run_id)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let adopt_out = adopt_child.wait_with_output().unwrap();
    let write_out = writer.wait_with_output().unwrap();
    println!(
        "R19-ORDINARY-WRITE adopt_status={} writer_status={} writer_stdout={} writer_stderr={} ordinary_exists={}",
        adopt_out.status,
        write_out.status,
        String::from_utf8_lossy(&write_out.stdout),
        String::from_utf8_lossy(&write_out.stderr),
        root.join("content/ordinary.md").exists()
    );
    assert!(
        adopt_out.status.success(),
        "the adoption control should complete: {}",
        String::from_utf8_lossy(&adopt_out.stdout)
    );
    assert_eq!(
        write_out.status.code(),
        Some(42),
        "the concurrent ordinary write must be honestly refused while adoption holds the lock"
    );
    assert!(
        !root.join("content/ordinary.md").exists(),
        "the refused ordinary write must not commit duplicate explicit id content"
    );
}

/// Collision evidence is gathered only after the workspace mutation lock is
/// held. The probe attempts the same cross-process lock from inside the scan:
/// if the scan were still a pre-lock snapshot, that probe would acquire.
#[test]
fn existing_id_scan_runs_inside_the_workspace_mutation_lock() {
    let (source_ws, run_id) = hugo_source();
    let target = tempfile::tempdir().unwrap();
    let root = target.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("content")).unwrap();
    let token = cv_store::workspace_instance::ensure(&root).unwrap();
    let expected = adopt::ExpectedWorkspaceInstance::new(
        token,
        cv_engine::file_identity::identity(&root),
        cv_store::workspace_mutation::current_identity(&root).unwrap(),
    );
    let src = adopt::source(source_ws.path(), &run_id).expect("adoptable hugo run");
    let probe_root = root.to_path_buf();
    let tx = adopt::transact_existing_with_identity(adopt::ExistingWorkspaceTransaction {
        target_root: &root,
        src: &src,
        content_input: "content",
        merge_subfolder: Some("imported"),
        scan_existing_ids: Box::new(move || {
            match cv_store::workspace_mutation::lock(&probe_root) {
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => panic!("unexpected mutation-lock probe error: {e}"),
                Ok(_) => panic!("existing-id scan happened outside the mutation lock"),
            }
            BTreeMap::new()
        }),
        at_ms: 42,
        expected_instance: &expected,
    })
    .unwrap();
    assert!(tx.written > 0);
}

/// A same-process ordinary writer reaches the scheduler writer lock before
/// the workspace mutation lock. While adoption holds both, it waits behind
/// the scheduler batch rather than self-deadlocking on a lock the batch
/// already owns; after the batch releases, the write proceeds normally.
#[test]
fn same_process_scheduler_writer_serializes_behind_adoption_lock_without_deadlock() {
    let root_tmp = tempfile::tempdir().unwrap();
    let root = root_tmp.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("content")).unwrap();
    cv_store::workspace_instance::ensure(&root).unwrap();
    let engine = std::sync::Arc::new(production_write_engine(&root));
    let sched = std::sync::Arc::new(cv_engine::EngineScheduler::new(
        engine,
        cv_engine::SeatPolicy::StopWatch,
    ));
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let blocker_sched = sched.clone();
    let blocker = std::thread::spawn(move || {
        blocker_sched.mutation_batch(|| {
            let _guard = cv_store::workspace_mutation::lock(&root).unwrap();
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
    });
    entered_rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    let writer_sched = sched.clone();
    let writer = std::thread::spawn(move || {
        writer_sched.write_content(
            &PageDraft {
                path: "content/ordinary.md".into(),
                body: "---\nid: ordinary\ntitle: Ordinary\n---\nordinary\n".into(),
                base: None,
            },
            None,
        )
    });
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert!(
        !writer.is_finished(),
        "writer did not serialize behind the scheduler batch"
    );
    release_tx.send(()).unwrap();
    blocker.join().unwrap();
    let outcome = writer.join().unwrap().unwrap();
    assert!(matches!(
        outcome,
        cv_core::engine::WriteOutcome::Written { .. }
    ));
}

/// Unknown custom verbs are conservative content mutations: they cannot
/// bypass the workspace mutation lock merely because Capture Vector cannot
/// prove what the subcommand writes.
#[cfg(unix)]
#[test]
fn custom_verbs_participate_in_the_workspace_mutation_lock() {
    use std::os::unix::fs::PermissionsExt;
    let root_tmp = tempfile::tempdir().unwrap();
    let root = root_tmp.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("content")).unwrap();
    let boris = root.join("boris");
    std::fs::write(
        &boris,
        "#!/bin/sh\nif [ \"$1\" = write ]; then echo content > \"$PWD/content/scripted.md\"; exit 0; fi\nexit 0\n",
    )
    .unwrap();
    std::fs::set_permissions(&boris, std::fs::Permissions::from_mode(0o755)).unwrap();
    cv_store::workspace_instance::ensure(&root).unwrap();
    let engine = production_write_engine(&root);
    let _guard = cv_store::workspace_mutation::lock(&root).unwrap();
    let err = engine
        .run(
            &cv_core::engine::Verb::Custom {
                name: "write".into(),
                args: vec![],
            },
            Box::new(|_| {}),
        )
        .unwrap_err();
    assert!(
        err.to_string().contains("workspace is busy"),
        "custom verb did not participate in the mutation lock: {err}"
    );
    assert!(!root.join("content/scripted.md").exists());
}

/// Carrier identity is part of the evidence captured before the batch, not
/// merely a comparison between whatever lock adoption happened to acquire
/// and the path at that moment. A same-token byte copy substituted before
/// lock acquisition therefore fails closed before any write.
#[cfg(unix)]
#[test]
fn pre_lock_cv_byte_copy_substitution_fails_before_any_write() {
    let root_tmp = tempfile::tempdir().unwrap();
    let root = root_tmp.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("content")).unwrap();
    let (source_ws, run_id) = hugo_source();
    let token = cv_store::workspace_instance::ensure(&root).unwrap();
    let expected = adopt::ExpectedWorkspaceInstance::new(
        token,
        cv_engine::file_identity::identity(&root),
        cv_store::workspace_mutation::current_identity(&root).unwrap(),
    );
    let src = adopt::source(source_ws.path(), &run_id).unwrap();
    let original = root.join("cv-original");
    std::fs::rename(root.join(".cv"), &original).unwrap();
    copy_tree_contents(&original, &root.join(".cv"));
    let err = adopt::transact_existing_with_identity(adopt::ExistingWorkspaceTransaction {
        target_root: &root,
        src: &src,
        content_input: "content",
        merge_subfolder: Some("imported"),
        scan_existing_ids: Box::new(BTreeMap::new),
        at_ms: 42,
        expected_instance: &expected,
    })
    .expect_err("pre-lock substitution must fail before writes");
    assert!(err.to_string().contains("nothing was written"), "{err}");
    assert!(
        !root.join(".cv/imports").join(&run_id).exists(),
        "the substituted copied lock must not reach run-lock or journaling"
    );
}

/// A newly initialized workspace is path-visible to other Capture Vector
/// processes before this process has a live scheduler for it. New-workspace
/// adoption therefore establishes and holds the same mutation carrier.
#[test]
fn new_workspace_adoption_cannot_write_while_the_workspace_lock_is_held() {
    let root_tmp = tempfile::tempdir().unwrap();
    let root = root_tmp.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("content")).unwrap();
    cv_store::workspace_instance::ensure(&root).unwrap();
    let _held = cv_store::workspace_mutation::lock(&root).unwrap();
    let (source_ws, run_id) = hugo_source();
    let src = adopt::source(source_ws.path(), &run_id).unwrap();
    let err =
        adopt::transact(&root, &src, "content", None, "new", &BTreeMap::new(), 42).unwrap_err();
    assert!(err.to_string().contains("workspace is busy"), "{err}");
    assert!(!root.join("content/imported").exists());
}

/// Caller-selected derived-output redirection may not overlap source truth,
/// even though the verb is otherwise serialized and lock-participating.
#[test]
fn ir_output_overlapping_the_content_input_is_refused_before_any_write() {
    let root_tmp = tempfile::tempdir().unwrap();
    let root = root_tmp.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("content")).unwrap();
    cv_store::workspace_instance::ensure(&root).unwrap();
    let engine = production_write_engine(&root);
    for verb in [
        cv_core::engine::Verb::Ir {
            out: "content".into(),
            input: None,
        },
        cv_core::engine::Verb::Custom {
            name: "build".into(),
            args: vec!["--out".into(), "content/graph.json".into()],
        },
        cv_core::engine::Verb::Ir {
            out: ".cv".into(),
            input: None,
        },
        cv_core::engine::Verb::Custom {
            name: "build".into(),
            args: vec!["--out".into(), ".boris".into()],
        },
    ] {
        let err = engine.run(&verb, Box::new(|_| {})).unwrap_err();
        assert!(err.to_string().contains("overlaps"), "{verb:?}: {err}");
    }
    assert!(!root.join("content/graph.json").exists());
}
