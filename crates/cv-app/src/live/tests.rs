// Integration-focused unit tests for the live engine pipeline.

use super::*;
use cv_core::diagnostic::{Diagnostic, Severity};
use cv_core::engine::EngineSource;
use cv_core::events::engine::BuildPhase;
use cv_core::events::{BuildId, Cursor, EventId, SnapshotEpoch};
use cv_core::id::WorkspaceId;
use cv_engine::locate::{EngineIdentity, PINNED};
use cv_engine::probe::CapabilityReport;
use cv_model::module::Registry;
use cv_model::workbench::Workbench;
use std::path::Path;
use std::sync::mpsc;

fn test_delivery(root: &Path, mode: WatchMode, wb: &Workbench) -> LiveDelivery {
    let engine = ProcessEngine::with_report(
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
            caps: EngineCaps::default(),
            watch_mode: mode,
            evidence: vec![],
        },
    );
    let engine = Arc::new(engine);
    let inbox = wb.inbox();
    let epoch = Arc::new(AtomicU64::new(1));
    let resyncer = Arc::new(Resyncer::spawn(
        engine.clone(),
        inbox.clone(),
        epoch.clone(),
    ));
    LiveDelivery {
        engine,
        inbox,
        epoch,
        resyncer,
    }
}

fn build(seq: u64, payload: EngineEvent) -> Envelope {
    Envelope {
        cause: None,
        workspace: WorkspaceId("recovery-test".into()),
        snapshot_epoch: SnapshotEpoch(1),
        build_id: Some(BuildId(format!("build-{seq}"))),
        cursor: Cursor {
            session: SessionId("watch".into()),
            seq,
        },
        event_id: EventId(format!("event-{seq}")),
        payload: AppEvent::Engine(payload),
    }
}

#[test]
fn sink_delivery_does_not_wait_for_a_disk_scan() {
    let root = tempfile::tempdir().unwrap();
    let mut wb = Workbench::new(Registry::with_all());
    let mut delivery = test_delivery(root.path(), WatchMode::Native, &wb);
    let (scan_started, started) = mpsc::channel();
    let (release_scan, release) = mpsc::channel::<()>();
    delivery.resyncer = Arc::new(Resyncer::spawn_with(move || {
        scan_started.send(()).unwrap();
        release.recv().unwrap();
    }));
    let mut sink = delivery.resync_sink(|deliver, env| deliver(env));
    let (done_tx, done) = mpsc::channel();
    let delivered = std::thread::spawn(move || {
        sink(finished(1));
        done_tx.send(()).unwrap();
    });

    started.recv_timeout(Duration::from_secs(2)).unwrap();
    done.recv_timeout(Duration::from_secs(2))
        .expect("sink delivery must not wait for the filesystem scan");
    release_scan.send(()).unwrap();
    delivered.join().unwrap();

    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while wb.app.snapshot_epoch == 1 {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
        wb.drain();
    }
}

#[test]
fn native_watch_spends_only_one_attempt_after_forwarding_undocumented_failures() {
    let root = tempfile::tempdir().unwrap();
    let mut wb = Workbench::new(Registry::with_all());
    let delivery = test_delivery(root.path(), WatchMode::Native, &wb);
    let (tx, rx) = std::sync::mpsc::channel();
    let mut sink = delivery.watch_sink(WatchMode::Native, None, move |id, _cause| {
        tx.send(id).unwrap()
    });

    sink(build(
        1,
        EngineEvent::BuildFailed {
            phase: BuildPhase::Rebuild,
            diagnostics: vec![Diagnostic {
                code: "TEST".into(),
                severity: Severity::Error,
                span: None,
                message: "ordinary diagnostic".into(),
                build_version: Some(BuildId("build-1".into())),
            }],
            recoverable: false,
        },
    ));
    sink(build(
        2,
        EngineEvent::BuildFailed {
            phase: BuildPhase::Rebuild,
            diagnostics: vec![],
            recoverable: true,
        },
    ));
    assert!(rx.try_recv().is_err());
    wb.drain();
    assert!(wb.app.degraded.is_none());
    let activity = wb
        .registry
        .get(wb.registry.index_of("activity").unwrap())
        .unwrap();
    let row = activity.row(Some("diagnostics"), 0);
    assert_eq!(row.title, "TEST: ordinary diagnostic");
    assert!(
        activity
            .reading(&row.key)
            .lines
            .iter()
            .any(|l| l.text == "event-1 · build-1")
    );

    for seq in [3, 3, 4, 5] {
        sink(build(
            seq,
            EngineEvent::BuildFailed {
                phase: BuildPhase::Rebuild,
                diagnostics: vec![],
                recoverable: false,
            },
        ));
    }
    assert_eq!(
        rx.try_iter().collect::<Vec<_>>(),
        [SessionId("watch".into())]
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while wb.app.snapshot_epoch == 1 {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
        wb.drain();
    }
    wb.drain();
    assert_eq!(wb.app.committed, 5);
    assert!(wb.app.snapshot_epoch > 1);
}

#[test]
fn projection_and_watch_share_cursor_gap_duplicate_and_replay_rules() {
    let root = tempfile::tempdir().unwrap();
    let mut wb = Workbench::new(Registry::with_all());
    let delivery = test_delivery(root.path(), WatchMode::Native, &wb);
    let tracker = Arc::new(Mutex::new(CursorTracker::new()));
    let mut projection = delivery.projection_sink(tracker.clone());
    let mut watch = delivery.watch_sink_with_tracker(
        WatchMode::Native,
        None,
        |_, _| panic!("no fatal watch build in this stream"),
        tracker,
    );
    let result = |seq| {
        build(
            seq,
            EngineEvent::BuildFinished {
                phase: BuildPhase::Initial,
                pages_written: 0,
                diagnostics: vec![],
                duration_ms: 0,
            },
        )
    };

    projection(result(0));
    watch(build(
        2,
        EngineEvent::WatchSessionEnded {
            reason: cv_core::events::engine::WatchEndReason::CleanExit,
            exit: None,
        },
    ));
    wb.drain();
    assert_eq!(wb.app.committed, 2);

    // Replaying the sequence the gap skipped is overlap, not a new
    // committed event. The next unseen sequence still commits.
    projection(result(1));
    wb.drain();
    assert_eq!(wb.app.committed, 2);
    projection(result(3));
    wb.drain();
    assert_eq!(wb.app.committed, 3);
}

#[test]
fn polling_watch_discloses_failures_without_retrying_and_clears_on_success() {
    let root = tempfile::tempdir().unwrap();
    let mut wb = Workbench::new(Registry::with_all());
    let delivery = test_delivery(root.path(), WatchMode::OneShotFallback, &wb);
    let mut sink = delivery.watch_sink(WatchMode::OneShotFallback, None, |_, _| {
        panic!("polling already runs one-shot builds")
    });
    let mut hello = build(
        0,
        EngineEvent::WatchSessionStarted {
            schema: 1,
            compiler: format!("{} (cv one-shot fallback)", PINNED.version),
        },
    );
    hello.build_id = None;
    sink(hello);
    wb.drain();
    assert!(wb.app.degraded.is_none());

    let failed = |seq| {
        build(
            seq,
            EngineEvent::BuildFailed {
                phase: BuildPhase::Rebuild,
                diagnostics: vec![],
                recoverable: false,
            },
        )
    };
    sink(failed(1));
    sink(failed(2));
    wb.drain();
    assert_eq!(wb.app.degraded, Some(DegradedReason::BuildFailed));
    assert_eq!(wb.app.committed, 4); // hello, two results, one disclosure
    sink(build(
        3,
        EngineEvent::BuildFinished {
            phase: BuildPhase::Rebuild,
            pages_written: 0,
            diagnostics: vec![],
            duration_ms: 0,
        },
    ));
    wb.drain();
    assert!(wb.app.degraded.is_none());
    sink(failed(4));
    wb.drain();
    assert_eq!(wb.app.degraded, Some(DegradedReason::BuildFailed));
}

#[cfg(unix)]
fn run_native_recovery(exit: Option<i32>) -> Workbench {
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, Instant};

    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("watch.ndjson"),
        cv_fixtures::corpus("broken-frontmatter").file("watch/session.ndjson"),
    )
    .unwrap();
    let bin = root.path().join("boris");
    // No executable remains for the one-shot path in the spawn-error
    // case. Otherwise only the first attempt can fail, bounding the
    // harness even if recovery incorrectly feeds its own sink again.
    let watch_setup = if exit.is_none() { "rm \"$0\";" } else { "" };
    let exit_code = exit.unwrap_or_default();
    std::fs::write(
        &bin,
        format!(
            "#!/bin/sh\ncase \"$1\" in\n\
         watch) {watch_setup} cat watch.ndjson >&2; exit 1;;\n\
         build) if [ -e attempts ]; then echo build >> attempts; exit 0; fi\n\
         echo build > attempts; exit {exit_code};;\n\
         *) exit 2;;\nesac\n"
        ),
    )
    .unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut wb = Workbench::new(Registry::with_all());
    let delivery = test_delivery(root.path(), WatchMode::Native, &wb);
    let sched = Arc::new(EngineScheduler::new(
        delivery.engine.clone(),
        SeatPolicy::StopWatch,
    ));
    start_workspace_watch(sched.clone(), delivery, None, None);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        wb.drain();
        if wb.status.contains("reopen workspace") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "no recovery after watch failure: {}",
            wb.status
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let attempts = root.path().join("attempts");
    if exit.is_some() {
        assert_eq!(std::fs::read_to_string(attempts).unwrap(), "build\n");
    } else {
        assert!(!attempts.exists());
    }
    assert!(sched.watch_session().is_none());
    assert!(!sched.engine().watch_live());
    wb
}

#[cfg(unix)]
#[test]
fn failed_diagnostic_build_stops_once_and_discloses_the_failure() {
    let wb = run_native_recovery(Some(3));
    assert!(
        wb.status
            .contains("Boris one-shot build failed (IoFailure)")
    );
    assert_eq!(wb.app.degraded, Some(DegradedReason::EngineBuildFailing));
    let activity = wb
        .registry
        .get(wb.registry.index_of("activity").unwrap())
        .unwrap();
    assert_eq!(activity.list_len(Some("builds")), 1);
    assert_eq!(
        activity
            .row(Some("builds"), 0)
            .badge
            .map(|c| c.text)
            .as_deref(),
        Some("×2")
    );
    let reason = cv_widgets::shell::degraded_text(wb.app.degraded.as_ref().unwrap());
    let harness = cv_widgets::harness::Harness::new(wb);
    assert!(harness.rt.find("banner").is_some());
    assert!(harness.tree().to_string().contains(&reason));
}

#[cfg(unix)]
#[test]
fn successful_diagnostic_build_updates_activity_without_claiming_watch_is_live() {
    let wb = run_native_recovery(Some(0));
    assert!(wb.status.contains("Boris one-shot build succeeded"));
    assert_eq!(wb.app.degraded, Some(DegradedReason::WatchDead));
    let activity = wb
        .registry
        .get(wb.registry.index_of("activity").unwrap())
        .unwrap();
    assert_eq!(
        activity.row(Some("builds"), 0).title,
        "Build succeeded (initial)"
    );
}

#[cfg(unix)]
#[test]
fn diagnostic_build_error_is_disclosed_even_without_a_build_envelope() {
    let wb = run_native_recovery(None);
    assert!(wb.status.contains("Boris one-shot build failed:"));
    assert_eq!(wb.app.degraded, Some(DegradedReason::EngineBuildFailing));
}

#[cfg(unix)]
#[test]
fn fatal_initial_build_with_a_diagnostic_is_disclosed_without_a_recovery_build() {
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, Instant};

    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("watch.ndjson"),
        cv_fixtures::corpus("content-only").file("watch/session.ndjson"),
    )
    .unwrap();
    let bin = root.path().join("boris");
    std::fs::write(
        &bin,
        "#!/bin/sh\ncase \"$1\" in\n\
         watch) cat watch.ndjson >&2; exit 3;;\n\
         build) [ \"$2\" = \"--out\" ] || echo \"$1\" >> attempts; exit 3;;\n\
         *) echo \"$1\" >> attempts; exit 3;;\nesac\n",
    )
    .unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut wb = Workbench::new(Registry::with_all());
    let delivery = test_delivery(root.path(), WatchMode::Native, &wb);
    let sched = Arc::new(EngineScheduler::new(
        delivery.engine.clone(),
        SeatPolicy::StopWatch,
    ));
    start_workspace_watch(sched.clone(), delivery, None, None);
    let deadline = Instant::now() + Duration::from_secs(5);
    while sched.engine().watch_live() || wb.app.watch_live || wb.app.committed < 3 {
        assert!(
            Instant::now() < deadline,
            "watch never ended: {}",
            wb.status
        );
        std::thread::sleep(Duration::from_millis(5));
        wb.drain();
    }
    wb.drain();
    assert_eq!(
        wb.app.degraded,
        Some(DegradedReason::InitialBuildFailed {
            code: "ELAYOUT".into(),
            message: "failed to load layout themes/boris/layouts/main.html: FileNotFound".into(),
        })
    );
    assert!(!root.path().join("attempts").exists(), "no recovery build");
}

/// A workspace holding the captured `quoted-scalars` corpus, and a
/// fake boris whose `build --out DIR` logs its argv, then copies the
/// graph Boris captured for that corpus into DIR, or exits `fail`.
#[cfg(unix)]
fn ir_workspace(fail: Option<i32>) -> (tempfile::TempDir, Workbench, LiveDelivery) {
    use std::os::unix::fs::PermissionsExt;
    let editor = cv_fixtures::fixtures_root().join("editor");
    let manifest = cv_fixtures::Manifest::load(&editor.join("MANIFEST.toml"));
    let root = tempfile::tempdir().unwrap();
    for (key, _) in manifest.pinned_files() {
        if let Some(rel) = key.strip_prefix("corpus-src/quoted-scalars/") {
            let to = root.path().join(rel);
            std::fs::create_dir_all(to.parent().unwrap()).unwrap();
            std::fs::write(to, std::fs::read(editor.join(key)).unwrap()).unwrap();
        }
    }
    let graph = editor.join("captures/quoted-scalars/graph.json");
    manifest
        .verify(
            "captures/quoted-scalars/graph.json",
            &std::fs::read(&graph).unwrap(),
        )
        .unwrap();
    let run = match fail {
        Some(code) => format!("echo 'error: EIO: disk full' >&2; exit {code}"),
        None => format!(
            "mkdir -p \"$3\" && cp '{}' \"$3/graph.json\"",
            graph.display()
        ),
    };
    let bin = root.path().join("boris");
    std::fs::write(
        &bin,
        format!(
            "#!/bin/sh\necho \"$*\" >> \"$(dirname \"$0\")/argv.log\"\n\
             case \"$1 $2\" in\n\"build --out\") {run};;\n*) exit 2;;\nesac\n"
        ),
    )
    .unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let wb = Workbench::new(Registry::with_all());
    let delivery = test_delivery(root.path(), WatchMode::Native, &wb);
    (root, wb, delivery)
}

#[cfg(unix)]
fn page_titles(wb: &Workbench) -> Vec<String> {
    let pages = wb
        .registry
        .get(wb.registry.index_of("pages").unwrap())
        .unwrap();
    (0..pages.list_len(None))
        .map(|i| pages.row(None, i).title)
        .collect()
}

#[cfg(unix)]
fn activity_rows(wb: &Workbench) -> Vec<String> {
    let activity = wb
        .registry
        .get(wb.registry.index_of("activity").unwrap())
        .unwrap();
    (0..activity.list_len(None))
        .map(|i| {
            let r = activity.row(None, i);
            format!("{} · {}", r.title, r.detail)
        })
        .collect()
}

fn finished(seq: u64) -> Envelope {
    build(
        seq,
        EngineEvent::BuildFinished {
            phase: BuildPhase::Rebuild,
            pages_written: 2,
            diagnostics: vec![],
            duration_ms: 1,
        },
    )
}

#[cfg(unix)]
#[test]
fn a_burst_of_watch_builds_runs_one_ir_projection_and_pages_show_boris_fields() {
    use std::time::Instant;
    let (root, mut wb, delivery) = ir_workspace(None);
    let sched = Arc::new(EngineScheduler::new(
        delivery.engine.clone(),
        SeatPolicy::StopWatch,
    ));
    let mut sink = delivery.watch_pipeline(WatchMode::Native, Arc::downgrade(&sched), |_, _| {
        panic!("a green build never recovers")
    });
    for seq in 0..8 {
        sink(finished(seq));
    }
    wb.drain();
    assert!(page_titles(&wb).contains(&"\"Mini Site (English)\"".to_string()));

    let argv = root.path().join("argv.log");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !page_titles(&wb).contains(&"Mini Site (English)".to_string()) {
        assert!(Instant::now() < deadline, "{:?}", page_titles(&wb));
        std::thread::sleep(Duration::from_millis(10));
        wb.drain();
    }
    std::thread::sleep(IR_WINDOW * 3);
    assert_eq!(
        std::fs::read_to_string(&argv).unwrap(),
        format!("build --out {}\n", cv_engine::ir::IR_STAGING_DIR)
    );
    assert!(root.path().join(".cv/ir/graph.json").is_file());
    assert!(!root.path().join("dist").exists());
    // A later build is a new burst: one more projection.
    sink(finished(8));
    let deadline = Instant::now() + Duration::from_secs(10);
    while std::fs::read_to_string(&argv).unwrap().lines().count() < 2 {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// What a workspace without a theme gets from `boris watch`.
#[cfg(unix)]
fn layout_failed(seq: u64) -> Envelope {
    build(
        seq,
        EngineEvent::BuildFailed {
            phase: BuildPhase::Initial,
            diagnostics: vec![Diagnostic {
                code: "ELAYOUT".into(),
                severity: Severity::Error,
                span: None,
                message: "failed to load layout themes/boris/layouts/main.html".into(),
                build_version: Some(BuildId(format!("build-{seq}"))),
            }],
            recoverable: false,
        },
    )
}

#[cfg(unix)]
#[test]
fn a_failed_html_build_still_gets_boris_fields_from_the_ir_projection() {
    use std::time::Instant;
    let (_root, mut wb, delivery) = ir_workspace(None);
    let sched = Arc::new(EngineScheduler::new(
        delivery.engine.clone(),
        SeatPolicy::StopWatch,
    ));
    let mut sink = delivery.watch_pipeline(WatchMode::Native, Arc::downgrade(&sched), |_, _| {
        panic!("a located failure does not recover")
    });
    sink(layout_failed(0));
    // The watch child exits after a non-recoverable failure, dropping
    // its sink: the pending projection still runs.
    drop(sink);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !page_titles(&wb).contains(&"Mini Site (English)".to_string()) {
        assert!(Instant::now() < deadline, "{:?}", page_titles(&wb));
        std::thread::sleep(Duration::from_millis(10));
        wb.drain();
    }
}

#[cfg(unix)]
#[test]
fn a_failed_ir_projection_is_disclosed_after_a_red_watch_build_and_pages_keep_the_fallback() {
    use std::time::Instant;
    let (_root, mut wb, delivery) = ir_workspace(Some(1));
    let sched = Arc::new(EngineScheduler::new(
        delivery.engine.clone(),
        SeatPolicy::StopWatch,
    ));
    let mut sink = delivery.watch_pipeline(WatchMode::Native, Arc::downgrade(&sched), |_, _| {
        panic!("a failed projection is not a failed watch")
    });
    sink(layout_failed(0));
    let deadline = Instant::now() + Duration::from_secs(10);
    while !activity_rows(&wb).iter().any(|r| {
        r.starts_with("ir finished · ContentFailure") && r.contains("last published Boris IR")
    }) {
        assert!(Instant::now() < deadline, "{:?}", activity_rows(&wb));
        std::thread::sleep(Duration::from_millis(10));
        wb.drain();
    }
    assert!(page_titles(&wb).contains(&"\"Mini Site (English)\"".to_string()));
    assert_eq!(
        wb.app.degraded,
        Some(DegradedReason::InitialBuildFailed {
            code: "ELAYOUT".into(),
            message: "failed to load layout themes/boris/layouts/main.html".into(),
        })
    );
}

#[cfg(unix)]
#[test]
fn a_failed_ir_projection_keeps_the_previous_boris_graph_and_discloses_it() {
    use std::time::Instant;
    let (root, mut wb, delivery) = ir_workspace(None);
    let sched = Arc::new(EngineScheduler::new(
        delivery.engine.clone(),
        SeatPolicy::StopWatch,
    ));
    let mut sink = delivery.watch_pipeline(WatchMode::Native, Arc::downgrade(&sched), |_, _| {
        panic!("a green build never recovers")
    });
    sink(finished(0));
    let deadline = Instant::now() + Duration::from_secs(10);
    while !page_titles(&wb).contains(&"Mini Site (English)".to_string()) {
        assert!(Instant::now() < deadline, "{:?}", page_titles(&wb));
        std::thread::sleep(Duration::from_millis(10));
        wb.drain();
    }

    std::fs::write(
        root.path().join("boris"),
        "#!/bin/sh\necho \"$*\" >> \"$(dirname \"$0\")/argv.log\"\nrm -f \"$3/graph.json\"\necho 'error: EFRONTMATTER: unknown key' >&2\nexit 1\n",
    )
    .unwrap();
    sink(finished(1));
    let deadline = Instant::now() + Duration::from_secs(10);
    while !activity_rows(&wb).iter().any(|r| {
        r.starts_with("ir finished · ContentFailure") && r.contains("last published Boris IR")
    }) {
        assert!(Instant::now() < deadline, "{:?}", activity_rows(&wb));
        std::thread::sleep(Duration::from_millis(10));
        wb.drain();
    }
    // A later resync still reads the last successful Boris graph rather
    // than pretending the failed projection wrote fresh fields.
    delivery.resync();
    wb.drain();
    assert!(page_titles(&wb).contains(&"Mini Site (English)".to_string()));
    assert!(root.path().join(".cv/ir/graph.json").is_file());
    assert!(!root.path().join("dist").exists());
}

#[test]
fn gate_failures_map_to_disclosed_states() {
    let mismatch = EngineError::VersionMismatch {
        path: "b".into(),
        source_kind: EngineSource::Path,
        found: "boris/0.8.2".into(),
        found_vcs: None,
        expected: "boris/0.9.0",
        expected_vcs: "66efcfb",
    };
    assert_eq!(
        degraded_reason(&mismatch),
        DegradedReason::EngineVersionMismatch {
            found: "0.8.2".into(),
            want: "0.9.0".into()
        }
    );
    let wrong_build = EngineError::VersionMismatch {
        path: "b".into(),
        source_kind: EngineSource::Path,
        found: "boris/0.9.0".into(),
        found_vcs: Some(String::new()),
        expected: "boris/0.9.0",
        expected_vcs: "66efcfb",
    };
    assert_eq!(
        degraded_reason(&wrong_build),
        DegradedReason::EngineVersionMismatch {
            found: "0.9.0 (vcs none)".into(),
            want: "0.9.0 (vcs 66efcfb)".into()
        }
    );
    let missing = EngineError::NotFound { searched: vec![] };
    assert_eq!(degraded_reason(&missing), DegradedReason::EngineAbsent);
    assert_eq!(
        degraded_reason(&EngineError::OverrideMissing("x".into())),
        DegradedReason::EngineAbsent
    );
}

#[test]
fn boot_thread_failure_is_disclosed_as_engine_absent() {
    let mut wb = Workbench::new(Registry::with_all());
    let refused = std::io::Error::other("no threads left");
    boot_thread_failed(
        &wb.inbox(),
        &None,
        &Some("indexes rebuilt".into()),
        Path::new("/ws"),
        &refused,
    );
    wb.drain();
    assert_eq!(wb.app.degraded, Some(DegradedReason::EngineAbsent));
    assert!(
        wb.status.contains("indexes rebuilt")
            && wb
                .status
                .contains("engine boot thread failed to start: no threads left"),
        "{}",
        wb.status
    );
}

#[test]
fn slot_engine_refuses_while_absent() {
    let e = SlotEngine(EngineSlot::default());
    assert!(e.locate().is_err());
    assert!(e.run(&Verb::Build, Box::new(|_| {})).is_err());
    assert!(!e.capabilities().watch && !e.capabilities().writes);
}
