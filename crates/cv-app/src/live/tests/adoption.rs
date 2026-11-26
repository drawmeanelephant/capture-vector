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
    let transcript = cv_fixtures::lab::StubLab::new()
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
    rec.argv = transcript.argv.clone();
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
    let (batch_tx, batch_rx) = mpsc::channel();
    crate::migrate::adopt::arm_batch_entry_probe(batch_tx);
    let worker = crate::migrate::adopt::adopt(&mut wb, job, &crate::WorkspaceEnv::native())
        .expect("adoption worker starts");

    // The test-only probe fires immediately before the worker asks for the
    // batch writer lock. The colliding page is then written inside the
    // already-held serialized mutation, so any id snapshot taken before
    // that lock is necessarily stale.
    batch_rx
        .recv_timeout(Duration::from_secs(3))
        .expect("worker did not reach the batch");
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

/// Round-2 probe: stopping the watch before taking the batch writer lock
/// leaves a start-watch window. A queued watch start must never win that
/// window and observe the mutation batch as unpaused.
#[cfg(unix)]
#[test]
fn a_watch_start_cannot_enter_the_gap_between_pause_and_batch() {
    for attempt in 0..8 {
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
        let wb = Workbench::new(Registry::with_all());
        let delivery = test_delivery(&root, WatchMode::Native, &wb);
        let sched = Arc::new(EngineScheduler::new(
            delivery.engine.clone(),
            SeatPolicy::StopWatch,
        ));
        let slot = EngineSlot::default();
        let _ = slot.sched.set(sched.clone());
        slot.install_verbs(slot.verb_route(delivery.clone(), None));
        let initial = sched.start_watch(Box::new(|_| {})).unwrap();

        // Hold the writer lock. Queue the watch start first, then let the
        // adoption-shaped batch pause the live watch and queue behind it.
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let blocker_sched = sched.clone();
        let blocker = std::thread::spawn(move || {
            blocker_sched.mutation_batch(|| {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            });
        });
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();

        let racer_sched = sched.clone();
        let racer = std::thread::spawn(move || racer_sched.start_watch(Box::new(|_| {})));
        std::thread::sleep(Duration::from_millis(20));

        let batch_slot = slot.clone();
        let batch_sched = sched.clone();
        let batch = std::thread::spawn(move || {
            let mut observed_watch = false;
            crate::migrate::adopt::batch_watch_safe(&batch_slot, &batch_sched, || {
                for _ in 0..5 {
                    observed_watch |= batch_sched.watch_session().is_some();
                    std::thread::sleep(Duration::from_millis(5));
                }
            });
            observed_watch
        });
        std::thread::sleep(Duration::from_millis(20));
        release_tx.send(()).unwrap();
        blocker.join().unwrap();
        let observed_watch = batch.join().unwrap();
        let racer_result = racer.join().unwrap();

        assert!(
            !observed_watch,
            "WATCH-START-RACE: attempt {attempt}: a watch queued before the adoption batch started inside the supposedly paused batch; racer_result={racer_result:?}"
        );

        if let Some(w) = sched.watch_session() {
            sched.stop_watch_session(&w);
        }
        drop(initial);
        drop(wb);
    }
}

/// Engine replacement can stop the old watch before the adoption gate
/// records its snapshot. `None` then means "no session was visible yet",
/// not "the reopened workspace asked for no watch": restoration consults
/// current state and converges to a live session.
#[cfg(unix)]
#[test]
fn restore_watch_without_a_prerecorded_session_still_converges_live() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().unwrap();
    let bin = root.path().join("boris");
    std::fs::write(
        &bin,
        r#"#!/bin/sh
case "$1" in
watch) echo '{"event":"hello","watch_events_schema":1,"compiler":"boris/0.9.0"}' >&2; sleep 30;;
*) exit 2;;
esac
"#,
    )
    .unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();

    let wb = Workbench::new(Registry::with_all());
    let delivery = super::test_delivery(root.path(), WatchMode::Native, &wb);
    let sched = Arc::new(EngineScheduler::new(
        delivery.engine.clone(),
        SeatPolicy::StopWatch,
    ));
    let slot = EngineSlot::default();
    let _ = slot.sched.set(sched.clone());
    slot.install_verbs(slot.verb_route(delivery.clone(), None));

    slot.restore_watch(None, "adoption gate".into());
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while sched.watch_session().is_none() {
        assert!(
            std::time::Instant::now() < deadline,
            "restore converged to a live watch"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let live = sched.watch_session().unwrap();
    assert!(sched.stop_watch_session(&live));
    drop(wb);
}
