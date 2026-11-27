//! Workspace-instance interference regressions. These probes synchronize
//! with the adoption's observable staged-write phase rather than assuming
//! that a journaling thread wins a scheduling race.

use cv_engine::adopt;
use cv_fixtures::lab::StubLab;
use cv_store::imports::{self as store, Finished, RecordedLab, RunRecord};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

fn seed_hugo_evidence(root: &Path, run_id: &str) {
    let root = &root.canonicalize().unwrap();
    let lock = store::claim_run(root, run_id).unwrap();
    let transcript = StubLab::new()
        .run("hugo", &lock.dir().join(store::OUT))
        .unwrap();
    let mut rec = RunRecord::started(
        run_id,
        "hugo",
        RecordedLab {
            tool_id: StubLab::new().tool_id(),
            source: "fixture".into(),
            path: "fixtures/captures/lab".into(),
        },
        1_764_100_000_000,
    );
    rec.argv = transcript.argv.clone();
    rec.finished = Some(Finished {
        exit_code: Some(0),
        exit: store::FinishedExit::Ok,
        outcome: "exited 0".into(),
        duration_ms: 0,
    });
    store::write(&lock, &rec).unwrap();
}

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

struct Fixture {
    _base: tempfile::TempDir,
    _source: tempfile::TempDir,
    root: PathBuf,
    src: adopt::AdoptionSource,
    expected: adopt::ExpectedWorkspaceInstance,
    run_id: String,
}

fn fixture() -> Fixture {
    let base = tempfile::tempdir().unwrap();
    let base_path = base.path().canonicalize().unwrap();
    let root = base_path.join("workspace");
    std::fs::create_dir_all(root.join("content")).unwrap();
    let source = tempfile::tempdir().unwrap();
    let run_id = "round17-hugo".to_string();
    seed_hugo_evidence(source.path(), &run_id);
    let dir = store::run_dir(source.path(), &run_id).unwrap();
    std::fs::write(
        dir.join(store::OUT).join("content/aa-large.bin"),
        vec![b'x'; 32 * 1024 * 1024],
    )
    .unwrap();
    let token = cv_store::workspace_instance::ensure(&root).unwrap();
    let expected = adopt::ExpectedWorkspaceInstance::new(
        token,
        cv_engine::file_identity::identity(&root),
        cv_store::workspace_mutation::current_identity(&root).unwrap(),
    );
    let src = adopt::source(source.path(), &run_id).unwrap();
    Fixture {
        _base: base,
        _source: source,
        root,
        src,
        expected,
        run_id,
    }
}

fn files_below(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                out.extend(files_below(&entry.path()));
            } else {
                out.push(entry.path());
            }
        }
    }
    out
}

fn staged_write_active(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|entries| {
        entries.flatten().any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".cv-write-")
        })
    })
}

fn wait_for_staged_write(dir: &Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !staged_write_active(dir) {
        assert!(
            Instant::now() < deadline,
            "interferer did not observe an active staged write at {}",
            dir.display()
        );
        std::thread::sleep(Duration::from_micros(100));
    }
}

fn transaction<'a>(
    root: &'a Path,
    src: &'a adopt::AdoptionSource,
    expected: &'a adopt::ExpectedWorkspaceInstance,
    gate: Arc<Barrier>,
) -> Result<adopt::TxReport, adopt::AdoptError> {
    adopt::transact_existing_with_identity(adopt::ExistingWorkspaceTransaction {
        target_root: root,
        src,
        content_input: "content",
        merge_subfolder: Some("imported"),
        scan_existing_ids: Box::new(move || {
            gate.wait();
            BTreeMap::new()
        }),
        at_ms: 42,
        expected_instance: expected,
    })
}

/// A post-revalidation rename plus byte-copy-back cannot be prevented by
/// POSIX directory locking. The transaction must nevertheless not report
/// success: partial output is disclosed and the recovery journal remains
/// with the surviving moved instance instead of being cleared.
#[cfg(unix)]
#[test]
fn post_write_identity_reverification_reports_partial_write_instead_of_false_success() {
    let fixture = fixture();
    let root = fixture.root;
    let moved = root.parent().unwrap().join("moved-survivor");
    let gate = Arc::new(Barrier::new(2));
    let interferer = std::thread::spawn({
        let root = root.clone();
        let moved = moved.clone();
        let gate = gate.clone();
        move || {
            gate.wait();
            wait_for_staged_write(&root.join("content/imported"));
            std::fs::rename(&root, &moved).unwrap();
            copy_tree_contents(&moved, &root);
        }
    });

    let err = transaction(&root, &fixture.src, &fixture.expected, gate)
        .expect_err("post-write identity mismatch must not report success");
    interferer.join().unwrap();
    let message = err.to_string();
    println!(
        "R17-POSTWRITE err={message:?} replacement={} survivor={} survivor_pending={}",
        root.join("content/imported").exists(),
        moved.join("content/imported").exists(),
        store::read_pending(&moved, &fixture.run_id)
            .unwrap()
            .is_some()
    );
    assert!(
        message.contains("may have been written"),
        "failure must disclose partial writes, not claim zero-write refusal: {message}"
    );
    assert!(
        store::read_pending(&moved, &fixture.run_id)
            .unwrap()
            .is_some(),
        "the surviving instance retains the recovery journal for inspection/retry"
    );
    let committed_writes = files_below(&moved.join("content/imported"))
        .into_iter()
        .filter(|path| {
            !path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with(".cv-write-"))
        })
        .collect::<Vec<_>>();
    assert!(
        committed_writes.is_empty(),
        "committed path-bound writes must not follow the moved survivor: {committed_writes:?}"
    );
}

/// Rename-away without a replacement is the other residual interference
/// shape: path-based writes can recreate a partial zombie root. That root is
/// not the captured instance, so the operation is an honestly diagnosed
/// partial-write failure rather than a raw OS error or false success.
#[cfg(unix)]
#[test]
fn rename_away_without_replacement_is_diagnosed_as_identity_loss_with_partial_state() {
    let fixture = fixture();
    let root = fixture.root;
    let moved = root.parent().unwrap().join("moved-survivor");
    let gate = Arc::new(Barrier::new(2));
    let interferer = std::thread::spawn({
        let root = root.clone();
        let moved = moved.clone();
        let gate = gate.clone();
        move || {
            gate.wait();
            wait_for_staged_write(&root.join("content/imported"));
            std::fs::rename(&root, &moved).unwrap();
        }
    });

    let err = transaction(&root, &fixture.src, &fixture.expected, gate)
        .expect_err("rename-away must not report success");
    interferer.join().unwrap();
    let message = err.to_string();
    println!(
        "R17-RENAME-AWAY err={message:?} zombie={} survivor={} survivor_pending={}",
        root.exists(),
        moved.join("content/imported").exists(),
        store::read_pending(&moved, &fixture.run_id)
            .unwrap()
            .is_some()
    );
    assert!(
        message.contains("workspace identity") && message.contains("may have been written"),
        "failure must name identity loss and partial-write truth: {message}"
    );
    assert!(
        store::read_pending(&moved, &fixture.run_id)
            .unwrap()
            .is_some(),
        "the surviving instance retains the recovery journal"
    );
    let committed_writes = files_below(&moved.join("content/imported"))
        .into_iter()
        .filter(|path| {
            !path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with(".cv-write-"))
        })
        .collect::<Vec<_>>();
    assert!(
        committed_writes.is_empty(),
        "committed path-bound writes must not follow the moved survivor: {committed_writes:?}"
    );
}

/// Replacing only `.cv` with a byte copy leaves the workspace root inode and
/// token bytes unchanged, but not the lock carrier inode. Verification must
/// bind the held carrier and therefore detect the substitution honestly.
#[cfg(unix)]
#[test]
fn cv_byte_copy_substitution_is_detected_by_lock_carrier_identity() {
    let fixture = fixture();
    let root = fixture.root;
    let cv_original = root.parent().unwrap().join("cv-original");
    let gate = Arc::new(Barrier::new(2));
    let interferer = std::thread::spawn({
        let root = root.clone();
        let cv_original = cv_original.clone();
        let gate = gate.clone();
        move || {
            gate.wait();
            wait_for_staged_write(&root.join("content/imported"));
            std::fs::rename(root.join(".cv"), &cv_original).unwrap();
            copy_tree_contents(&cv_original, &root.join(".cv"));
        }
    });

    let err = transaction(&root, &fixture.src, &fixture.expected, gate)
        .expect_err(".cv byte-copy substitution must not report success");
    interferer.join().unwrap();
    let message = err.to_string();
    println!(
        "R19-CV-BYTE-COPY err={message:?} root={} imported={}",
        root.exists(),
        root.join("content/imported").exists()
    );
    assert!(
        message.contains("workspace identity") && message.contains("may have been written"),
        "substitution must be diagnosed as identity loss with partial-write truth: {message}"
    );
}
