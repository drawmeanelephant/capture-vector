//! Adoption scheduler-boundary regression tests for the live pipeline.

use super::*;

/// Existing-id preflight is part of the serialized adoption mutation, not
/// a stale snapshot taken before the batch waits for the scheduler lock.
#[cfg(unix)]
#[test]
fn adoption_existing_id_preflight_cannot_be_stale_across_the_mutation_batch() {
    let root_tmp = tempfile::tempdir().unwrap();
    let root = root_tmp.path().canonicalize().unwrap();
    install_executable(
        &root.join("boris"),
        r#"#!/bin/sh
case "$1" in
watch) echo '{"event":"hello","watch_events_schema":1,"compiler":"boris/0.9.0"}' >&2; sleep 30;;
*) exit 0;;
esac
"#,
    );
    std::fs::create_dir_all(root.join("content")).unwrap();

    // Finished, adoptable Hugo output mirrored in this workspace.
    let run_id = "preflight-race-hugo";
    let lock = cv_store::imports::claim_run(&root, run_id).unwrap();
    cv_fixtures::lab::StubLab::new()
        .run("hugo", &lock.dir().join(cv_store::imports::OUT))
        .unwrap();
    let mut rec = cv_store::imports::RunRecord::started(
        run_id,
        "hugo",
        cv_store::imports::RecordedLab {
            tool_id: "boris-migration-lab/0.8.1".into(),
            source: "fixture".into(),
            path: "fixtures/captures/lab".into(),
        },
        7,
    );
    rec.finished = Some(cv_store::imports::Finished {
        exit_code: Some(0),
        exit: cv_store::imports::FinishedExit::Ok,
        outcome: "exited 0".into(),
        duration_ms: 0,
    });
    cv_store::imports::write(&lock, &rec).unwrap();
    drop(lock);

    let mut wb = Workbench::new(Registry::with_all());
    let delivery = test_delivery(&root, WatchMode::Native, &wb);
    let sched = Arc::new(EngineScheduler::new(
        delivery.engine.clone(),
        SeatPolicy::StopWatch,
    ));
    let slot = EngineSlot::default();
    let _ = slot.sched.set(sched.clone());
    crate::slots::register_slot(&root, &slot);
    let session = sched.start_watch(Box::new(|_| {})).unwrap();

    // Hold the scheduler write lock. Only after the adoption worker has
    // snapshotted ids and queued behind this batch does the serialized
    // mutation create a page owning the incoming `en` id.
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let sched_mut = sched.clone();
    let mutation = std::thread::spawn(move || {
        sched_mut.mutation_batch(|| {
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            std::fs::write(
                sched_mut.engine().root().join("content/existing.md"),
                "---\nid: en\ntitle: Existing\n---\nAlready here\n",
            )
            .unwrap();
        });
    });
    entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();

    let job = cv_model::adoption::AdoptionJob {
        source_root: root.clone(),
        request: cv_model::adoption::AdoptionRequest {
            run_id: run_id.into(),
            target: cv_model::adoption::AdoptionTarget::ExistingWorkspace {
                subfolder: "imported".into(),
            },
        },
        cancel: cv_core::engine::CancelToken::new(),
    };
    let worker = crate::migrate::adopt::adopt(&mut wb, job, &crate::WorkspaceEnv::native())
        .expect("adoption worker starts");

    // Watch disappearance proves the worker passed the stale snapshot point
    // and is queued inside batch_watch_safe behind the held mutation batch.
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while sched.watch_session().is_some() {
        assert!(
            std::time::Instant::now() < deadline,
            "worker did not reach the batch"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    release_tx.send(()).unwrap();
    mutation.join().unwrap();
    worker.join().unwrap();

    assert!(root.join("content/existing.md").is_file());
    assert!(
        !root.join("content/imported/en.md").exists(),
        "adoption copied entity id 'en' even though a serialized mutation created that id before the batch began"
    );
    if let Some(w) = sched.watch_session() {
        sched.stop_watch_session(&w);
    }
    drop(session);
    drop(wb);
}
