//! Workspace-reopen adoption regression: the composition root must keep
//! one serialized scheduler identity per workspace across switch-away/back.

mod common;

use cv_app::{WorkspaceEnv, open_workspace};
use cv_core::engine::CancelToken;
use cv_engine::locate::Locator;
use cv_fixtures::lab::StubLab;
use cv_model::adoption::{AdoptionJob, AdoptionRequest, AdoptionTarget};
use cv_store::imports::{self as store, Finished, RecordedLab, RunRecord};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

const BORIS_SHIM: &str = include_str!("../../cv-engine/tests/fake-boris/boris");

fn write(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

#[cfg(unix)]
fn fake_boris() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("boris");
    common::install_executable(&bin, BORIS_SHIM);
    let raw =
        std::fs::read_to_string(cv_fixtures::fixtures_root().join("engine-version.txt")).unwrap();
    let build_info = raw
        .lines()
        .find(|l| l.trim_start().starts_with('{'))
        .unwrap()
        .to_string();
    let v: serde_json::Value = serde_json::from_str(&build_info).unwrap();
    write(
        &dir.path().join("version.txt"),
        format!("{}\n", v["version"].as_str().unwrap()).as_bytes(),
    );
    write(&dir.path().join("build-info.txt"), build_info.as_bytes());
    write(&dir.path().join("verbs/validate/exit-code.txt"), b"0");
    write(&dir.path().join("verbs/build/exit-code.txt"), b"0");
    write(
        &dir.path().join("watch.ndjson"),
        b"{\"event\":\"hello\",\"watch_events_schema\":1,\"compiler\":\"boris/0.9.0\"}\n",
    );
    write(&dir.path().join("watch-hold"), b"");
    (dir, bin)
}

fn env_for(boris: &Path, recent_file: &Path) -> WorkspaceEnv {
    WorkspaceEnv {
        locator: Some(Locator {
            env_override: Some(boris.to_path_buf()),
            bundled_dirs: Vec::new(),
            path_var: None,
            sibling_dirs: Vec::new(),
        }),
        recent_file: Some(recent_file.to_path_buf()),
    }
}

fn seed_long_run(root: &Path, run_id: &str, extra_files: usize) {
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
    // Small but numerous files make the journaled copy phase long enough
    // for a workspace switch-away/switch-back sequence without a large write.
    for i in 0..extra_files {
        write(
            &lock
                .dir()
                .join(store::OUT)
                .join(format!("content/zz/file-{i:04}.md")),
            format!("file {i}\n").as_bytes(),
        );
    }
    drop(lock);
}

#[cfg(unix)]
#[test]
fn adoption_cannot_run_inside_a_stale_scheduler_after_workspace_reopen() {
    let (_boris_dir, boris) = fake_boris();
    let ws_tmp = tempfile::tempdir().unwrap();
    let ws = ws_tmp.path().canonicalize().unwrap();
    write(
        &ws.join("content/index.md"),
        b"---\ntitle: Home\n---\nHome\n",
    );
    let other_tmp = tempfile::tempdir().unwrap();
    let other = other_tmp.path().canonicalize().unwrap();
    write(
        &other.join("content/index.md"),
        b"---\ntitle: Other\n---\nOther\n",
    );
    let recent_tmp = tempfile::tempdir().unwrap();
    let env = env_for(&boris, &recent_tmp.path().join("recent.json"));

    let run_id = "reopen-long-hugo";
    seed_long_run(&ws, run_id, 1500);

    let mut wb = cv_app::init();
    open_workspace(&mut wb, ws.clone(), &env)
        .expect("first boot starts")
        .join()
        .unwrap();
    let first = cv_app::slots::scheduler_for(&ws).expect("first scheduler booted");

    let job = AdoptionJob {
        source_root: ws.clone(),
        request: AdoptionRequest {
            run_id: run_id.into(),
            target: AdoptionTarget::ExistingWorkspace {
                subfolder: "imported".into(),
            },
        },
        cancel: CancelToken::new(),
    };
    let worker = cv_app::migrate::adopt::adopt(&mut wb, job, &env).expect("adoption starts");
    let pending = ws.join(".cv/imports").join(run_id).join(store::PENDING);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !pending.exists() {
        assert!(
            Instant::now() < deadline,
            "adoption did not reach its journal"
        );
        std::thread::sleep(Duration::from_millis(2));
    }

    // Exact user shape: switch away, then switch back while the adoption
    // copy is still inside its first scheduler batch.
    open_workspace(&mut wb, other.clone(), &env)
        .expect("switch-away boot starts")
        .join()
        .unwrap();
    open_workspace(&mut wb, ws.clone(), &env)
        .expect("switch-back boot starts")
        .join()
        .unwrap();
    let second = cv_app::slots::scheduler_for(&ws).expect("second scheduler booted");

    let pending_before = pending.exists();
    second.mutation_batch(|| {
        write(
            &ws.join("content/reopened-scheduler-write.md"),
            b"write from the reopened workspace scheduler\n",
        );
    });
    let pending_after = pending.exists();
    let same_scheduler = Arc::as_ptr(&first) == Arc::as_ptr(&second);
    assert!(
        !(pending_before && pending_after && !same_scheduler),
        "SCHEDULER-SPLIT: adoption was still inside the first scheduler batch while a mutation batch completed on the post-reopen scheduler (same_scheduler={same_scheduler}, pending_before={pending_before}, pending_after={pending_after})"
    );

    worker.join().unwrap();
    if let Some(w) = second.watch_session() {
        second.stop_watch_session(&w);
    }
}
