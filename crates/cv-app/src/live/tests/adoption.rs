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
        operation: cv_model::status::StatusOperation::mint(),
    };
    let (batch_tx, batch_rx) = mpsc::channel();
    crate::migrate::adopt::arm_batch_entry_probe(root.clone(), batch_tx);
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

/// Round-15 probe: identity captured before the adoption batch must be
/// revalidated inside that batch. Renaming the workspace away and copying
/// it back at the captured path creates a byte-copy replacement with the
/// same `.cv` token but a different containing-directory identity. The
/// batch must fail closed rather than write the adoption into that foreign
/// replacement.
#[cfg(unix)]
#[test]
fn adoption_revalidates_workspace_identity_inside_the_mutation_batch() {
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

    let root_tmp = tempfile::tempdir().unwrap();
    let base = root_tmp.path().canonicalize().unwrap();
    let root = base.join("workspace");
    std::fs::create_dir_all(&root).unwrap();
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

    let run_id = "identity-toctou-hugo";
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

    let job = cv_model::adoption::AdoptionJob {
        source_root: root.clone(),
        request: cv_model::adoption::AdoptionRequest {
            run_id: run_id.into(),
            target: cv_model::adoption::AdoptionTarget::ExistingWorkspace {
                subfolder: "imported".into(),
            },
        },
        cancel: cv_core::engine::CancelToken::new(),
        operation: cv_model::status::StatusOperation::mint(),
    };
    // Hold the scheduler write lock until the worker has captured identity
    // and reached the batch boundary. The replacement then happens in the
    // exact capture-to-batch window this regression models.
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

    let (batch_tx, batch_rx) = mpsc::channel();
    crate::migrate::adopt::arm_batch_entry_probe(root.clone(), batch_tx);
    let worker = crate::migrate::adopt::adopt(&mut wb, job, &crate::WorkspaceEnv::native())
        .expect("adoption worker starts");

    batch_rx
        .recv_timeout(Duration::from_secs(3))
        .expect("worker did not reach the batch");
    let moved = base.join("moved-survivor");
    std::fs::rename(&root, &moved).unwrap();
    copy_tree(&moved, &root);
    release_tx.send(()).unwrap();
    blocker.join().unwrap();

    worker.join().unwrap();
    // The refusal is a shell-level terminal adoption outcome, not a
    // generation-scoped engine stream event: it must still surface when the
    // workspace generation retires before the UI drains it.
    wb.reset_project(Registry::with_all());
    wb.drain();
    let replacement_written = root.join("content/imported/en.md").exists();
    let survivor_written = moved.join("content/imported/en.md").exists();
    println!(
        "R15-IDENTITY-TOCTOU replacement_written={replacement_written} survivor_written={survivor_written} status={:?}",
        wb.status
    );
    assert!(
        !replacement_written,
        "adoption wrote into a foreign byte-copy replacement at the captured path"
    );
    assert!(
        !survivor_written,
        "fail-closed semantics: the path-bound batch must not follow the moved workspace"
    );
    assert!(
        wb.status.contains("adoption") && wb.status.contains("workspace identity"),
        "refusal must be terminally disclosed, got {:?}",
        wb.status
    );

    if let Some(w) = sched.watch_session() {
        sched.stop_watch_session(&w);
    }
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
        slot.install_verbs(slot.verb_route(delivery.clone(), None), slot.generation());
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
    slot.install_verbs(slot.verb_route(delivery.clone(), None), slot.generation());

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

/// Round-16 adversarial probe: corruption that appears after the workspace
/// has opened must still use the persistent degraded identity channel and
/// must not be misreported as a missing engine slot.
#[cfg(unix)]
#[test]
fn r16_malformed_identity_after_open_is_disclosed_as_store_corrupt() {
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

    let run_id = "malformed-after-open-hugo";
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
    // Registration mints/uses valid evidence, exactly like the initial open.
    crate::slots::register_slot(&root, &slot);

    // External corruption after that open. The next adoption lookup computes
    // a degraded error key rather than the registered valid-token key.
    std::fs::write(
        root.join(cv_store::workspace_instance::FILE),
        "foreign-corruption\n",
    )
    .unwrap();

    let job = cv_model::adoption::AdoptionJob {
        source_root: root.clone(),
        request: cv_model::adoption::AdoptionRequest {
            run_id: run_id.into(),
            target: cv_model::adoption::AdoptionTarget::ExistingWorkspace {
                subfolder: "imported".into(),
            },
        },
        cancel: cv_core::engine::CancelToken::new(),
        operation: cv_model::status::StatusOperation::mint(),
    };
    let worker = crate::migrate::adopt::adopt(&mut wb, job, &crate::WorkspaceEnv::native())
        .expect("adoption worker starts");
    worker.join().unwrap();
    wb.drain();
    println!(
        "R16-MALFORMED-AFTER-OPEN degraded={:?} status={:?} imported={}",
        wb.app.degraded,
        wb.status,
        root.join("content/imported/en.md").exists()
    );
    assert!(!root.join("content/imported/en.md").exists());
    assert_eq!(
        wb.app.degraded,
        Some(cv_core::events::shell::DegradedReason::StoreCorrupt),
        "malformed workspace-instance evidence must use the persistent degraded channel"
    );
    assert!(
        wb.status.contains("workspace identity") || wb.status.contains("instance evidence"),
        "the refusal must name identity evidence, not a misleading missing slot: {:?}",
        wb.status
    );
}
