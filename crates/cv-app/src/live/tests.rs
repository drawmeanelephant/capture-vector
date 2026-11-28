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

mod adoption;
#[cfg(unix)]
mod cancellation;
mod generation;
#[cfg(unix)]
mod generation_attack;

/// Never write the executable inode in this process: a sibling fork could
/// inherit that fd and make Linux exec fail with ETXTBSY
/// (rust-lang/rust#114554). Unlike cv-engine's child-copy installer, this
/// stays inside cv-app's process firewall: write and chmod a sibling while
/// it is non-executable, then atomically rename it into place.
#[cfg(unix)]
fn install_executable(bin: &Path, contents: &str) {
    use std::os::unix::fs::PermissionsExt;

    let staging = bin.parent().unwrap().join("staging-contents");
    std::fs::write(&staging, contents).unwrap();
    std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::rename(&staging, bin).unwrap();
}

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
fn a_resync_requested_after_a_mutation_waits_behind_an_in_flight_scan() {
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let resyncer = Arc::new(Resyncer::spawn_with(move || {
        started_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    }));
    let root = tempfile::tempdir().unwrap();
    let wb = Workbench::new(Registry::with_all());
    let mut delivery = test_delivery(root.path(), WatchMode::Native, &wb);
    delivery.resyncer = resyncer.clone();
    let slot = EngineSlot::default();
    slot.install_verbs(slot.verb_route(delivery, None), slot.generation());

    resyncer.request();
    started_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("initial resync scan did not start");
    slot.request_resync();
    assert!(started_rx.try_recv().is_err());
    release_tx.send(()).unwrap();
    started_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("post-mutation resync request was dropped behind the in-flight scan");
    release_tx.send(()).unwrap();
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
    install_executable(
        &bin,
        &format!(
            "#!/bin/sh\ncase \"$1\" in\n\
         watch) {watch_setup} cat watch.ndjson >&2; exit 1;;\n\
         build) if [ -e attempts ]; then echo build >> attempts; exit 0; fi\n\
         echo build > attempts; exit {exit_code};;\n\
         *) exit 2;;\nesac\n"
        ),
    );
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
fn delayed_recovery_does_not_stop_a_replacement_watch() {
    let root = tempfile::tempdir().unwrap();
    let bin = root.path().join("boris");
    install_executable(
        &bin,
        r#"#!/bin/sh
case "$1" in
watch) echo '{"event":"hello","watch_events_schema":1,"compiler":"boris/0.9.0"}' >&2; sleep 30;;
build) echo build >> attempts; exit 0;;
*) exit 2;;
esac
"#,
    );
    let mut wb = Workbench::new(Registry::with_all());
    let delivery = test_delivery(root.path(), WatchMode::Native, &wb);
    let sched = Arc::new(EngineScheduler::new(
        delivery.engine.clone(),
        SeatPolicy::StopWatch,
    ));
    let obsolete = sched.start_watch(Box::new(|_| {})).unwrap();
    assert!(sched.stop_watch_session(&obsolete));
    let replacement = sched.start_watch(Box::new(|_| {})).unwrap();

    recover_build(
        &sched,
        obsolete,
        &EventId("fatal-watch-event".into()),
        &delivery,
        &None,
        &None,
    );

    assert_eq!(sched.watch_session(), Some(replacement.clone()));
    assert!(!root.path().join("attempts").exists());
    wb.drain();
    assert!(sched.stop_watch_session(&replacement));
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
    use std::time::{Duration, Instant};

    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("watch.ndjson"),
        cv_fixtures::corpus("content-only").file("watch/session.ndjson"),
    )
    .unwrap();
    let bin = root.path().join("boris");
    install_executable(
        &bin,
        "#!/bin/sh\ncase \"$1\" in\n\
         watch) cat watch.ndjson >&2; exit 3;;\n\
         build) [ \"$2\" = \"--out\" ] || echo \"$1\" >> attempts; exit 3;;\n\
         *) echo \"$1\" >> attempts; exit 3;;\nesac\n",
    );
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
    install_executable(
        &bin,
        &format!(
            "#!/bin/sh\necho \"$*\" >> \"$(dirname \"$0\")/argv.log\"\n\
             case \"$1 $2\" in\n\"build --out\") {run};;\n*) exit 2;;\nesac\n"
        ),
    );
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

#[cfg(unix)]
fn wait_for_ir_graph(root: &tempfile::TempDir, wb: &mut Workbench) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        wb.drain();
        let pages = wb
            .registry
            .get(wb.registry.index_of("pages").unwrap())
            .unwrap();
        let boris_backed = !pages
            .inspector("index.md")
            .fields
            .iter()
            .any(|(k, v)| k == "fields" && v.contains("no Boris IR"));
        if root.path().join(".cv/ir/graph.json").is_file() && boris_backed {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "{:?}",
            page_titles(wb)
        );
        std::thread::sleep(Duration::from_millis(10));
    }
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

    let argv = root.path().join("argv.log");
    wait_for_ir_graph(&root, &mut wb);
    assert!(page_titles(&wb).contains(&"Mini Site (English)".to_string()));
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
    wait_for_ir_graph(&_root, &mut wb);
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
    assert!(page_titles(&wb).contains(&"Mini Site (English)".to_string()));
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
    wait_for_ir_graph(&root, &mut wb);

    install_executable(
        &root.path().join("boris"),
        "#!/bin/sh\necho \"$*\" >> \"$(dirname \"$0\")/argv.log\"\nrm -f \"$3/graph.json\"\necho 'error: EFRONTMATTER: unknown key' >&2\nexit 1\n",
    );
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

/// Blocker-1 probe: the adoption gate's contract. A build through the
/// StopWatch scheduler stops the live watch; the gate must end with the
/// watch restored, never silently dead.
#[cfg(unix)]
#[test]
fn a_seat_taking_gate_restores_the_watch_it_stopped() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().unwrap();
    let bin = root.path().join("boris");
    std::fs::write(
        &bin,
        r#"#!/bin/sh
case "$1" in
watch) echo '{"event":"hello","watch_events_schema":1,"compiler":"boris/0.9.0"}' >&2; sleep 30;;
validate) exit 0;;
build) exit 0;;
*) exit 2;;
esac
"#,
    )
    .unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();

    let wb = Workbench::new(Registry::with_all());
    let delivery = test_delivery(root.path(), WatchMode::Native, &wb);
    let sched = Arc::new(EngineScheduler::new(
        delivery.engine.clone(),
        SeatPolicy::StopWatch,
    ));
    let slot = EngineSlot::default();
    let _ = slot.sched.set(sched.clone());
    slot.install_verbs(slot.verb_route(delivery.clone(), None), slot.generation());

    let session = sched.start_watch(Box::new(|_| {})).unwrap();
    let watching = sched.watch_session();
    assert_eq!(watching.as_ref(), Some(&session));

    // What the adoption worker's gate does: validate then build through
    // the scheduler. The build takes the seat and stops the watch.
    let cancel = cv_core::engine::CancelToken::new();
    let run =
        |verb: Verb, sink: EventSink| sched.run_reported(&verb, sink, &cancel).map(|r| r.exit);
    let sink = || Box::new(|_: Envelope| {}) as EventSink;
    let (validate, build) = cv_engine::adopt::validate_then_build(&run, &sink).expect("gate runs");
    assert_eq!(validate, ExitClass::Ok);
    assert_eq!(build, Some(ExitClass::Ok));
    assert_eq!(
        sched.watch_session(),
        None,
        "the build takes the watch's seat under StopWatch"
    );

    // The worker's completion path restores it.
    slot.restore_watch(watching, "adoption gate".into());
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while sched.watch_session().is_none() {
        assert!(
            std::time::Instant::now() < deadline,
            "the watch is live again after the gate"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let restarted = sched.watch_session().unwrap();
    assert_ne!(
        restarted,
        session.clone(),
        "a replacement session, not the corpse"
    );
    assert!(sched.stop_watch_session(&restarted));
    drop(wb);
}

/// A restore racing reopen can lose the writer-lock race to a healthy
/// replacement watch. `WatchAlreadyLive` is proof of restoration there,
/// never a dead watch: no later session-start event exists to clear a
/// false banner when the validate gate fails before build.
#[cfg(unix)]
#[test]
fn an_already_live_watch_is_not_disclosed_as_dead() {
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

    let mut wb = Workbench::new(Registry::with_all());
    let delivery = test_delivery(root.path(), WatchMode::Native, &wb);
    let sched = Arc::new(EngineScheduler::new(
        delivery.engine.clone(),
        SeatPolicy::StopWatch,
    ));
    start_workspace_watch(sched.clone(), delivery.clone(), None, None);
    let live = sched.watch_session().expect("first watch is live");

    // Exact losing-restoration endpoint: the replacement already owns the
    // single watch seat when another restore reaches start_watch.
    start_workspace_watch(
        sched.clone(),
        delivery.clone(),
        None,
        Some("stale restore · watch resumed".into()),
    );
    wb.drain();
    assert_ne!(
        wb.app.degraded,
        Some(DegradedReason::WatchDead),
        "a live healthy watch must never be disclosed as dead"
    );
    assert!(sched.stop_watch_session(&live));
    drop(wb);
}

/// The matching negative: a gate that leaves the watch untouched has
/// nothing to restore and must not spawn a duplicate session.
#[cfg(unix)]
#[test]
fn restore_watch_is_a_noop_when_the_watch_survived() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().unwrap();
    let bin = root.path().join("boris");
    std::fs::write(
        &bin,
        r#"#!/bin/sh
case "$1" in
watch) echo '{"event":"hello","watch_events_schema":1,"compiler":"boris/0.9.0"}' >&2; sleep 30;;
validate) exit 0;;
*) exit 2;;
esac
"#,
    )
    .unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();

    let wb = Workbench::new(Registry::with_all());
    let delivery = test_delivery(root.path(), WatchMode::Native, &wb);
    let sched = Arc::new(EngineScheduler::new(
        delivery.engine.clone(),
        SeatPolicy::StopWatch,
    ));
    let slot = EngineSlot::default();
    let _ = slot.sched.set(sched.clone());
    slot.install_verbs(slot.verb_route(delivery.clone(), None), slot.generation());

    let session = sched.start_watch(Box::new(|_| {})).unwrap();
    let watching = sched.watch_session();
    slot.restore_watch(watching, "no-op".into());
    assert_eq!(
        sched.watch_session(),
        Some(session.clone()),
        "validate never touched the seat; the same session must survive"
    );
    assert!(sched.stop_watch_session(&session));
    drop(wb);
}

/// Finding-3 probe: the adoption batch pauses the live watch for its
/// whole duration — no intermediate file state can be observed by a
/// watch build — and restores it afterwards in every outcome.
#[cfg(unix)]
#[test]
fn a_mutation_batch_runs_with_the_watch_paused_and_restored() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().unwrap();
    let bin = root.path().join("boris");
    std::fs::write(
        &bin,
        r#"#!/bin/sh
case "$1" in
watch) echo '{"event":"hello","watch_events_schema":1,"compiler":"boris/0.9.0"}' >&2; sleep 30;;
validate) exit 0;;
build) exit 0;;
*) exit 2;;
esac
"#,
    )
    .unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();

    let wb = Workbench::new(Registry::with_all());
    let delivery = test_delivery(root.path(), WatchMode::Native, &wb);
    let sched = Arc::new(EngineScheduler::new(
        delivery.engine.clone(),
        SeatPolicy::StopWatch,
    ));
    let slot = EngineSlot::default();
    let _ = slot.sched.set(sched.clone());
    slot.install_verbs(slot.verb_route(delivery.clone(), None), slot.generation());

    let session = sched.start_watch(Box::new(|_| {})).unwrap();

    // A slowed multi-file batch: several writes with real time between
    // them, exactly the shape a migration copy has.
    let sched_in_batch = sched.clone();
    let mut observed_none = 0;
    crate::migrate::adopt::batch_watch_safe(&slot, &sched, || {
        for i in 0..3 {
            std::thread::sleep(Duration::from_millis(40));
            assert!(
                sched_in_batch.watch_session().is_none(),
                "the watch is paused for the whole batch (write {i})"
            );
            observed_none += 1;
        }
        "batch-done"
    });
    assert_eq!(observed_none, 3);

    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while sched.watch_session().is_none() {
        assert!(
            std::time::Instant::now() < deadline,
            "the watch is restored after the batch"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let restored = sched.watch_session().unwrap();
    assert_ne!(
        restored,
        session.clone(),
        "a replacement session, not the stopped corpse"
    );
    assert!(sched.stop_watch_session(&restored));
    drop(wb);
}

/// The matching refusal path: a batch that fails still ends with the
/// watch restored — a collision refusal must not strand it either.
#[cfg(unix)]
#[test]
fn a_refused_mutation_batch_still_restores_the_watch() {
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
    let delivery = test_delivery(root.path(), WatchMode::Native, &wb);
    let sched = Arc::new(EngineScheduler::new(
        delivery.engine.clone(),
        SeatPolicy::StopWatch,
    ));
    let slot = EngineSlot::default();
    let _ = slot.sched.set(sched.clone());
    slot.install_verbs(slot.verb_route(delivery.clone(), None), slot.generation());

    let session = sched.start_watch(Box::new(|_| {})).unwrap();
    let result: Result<(), &str> = crate::migrate::adopt::batch_watch_safe(&slot, &sched, || {
        Err("collision refusal before any write")
    });
    assert!(result.is_err());

    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while sched.watch_session().is_none() {
        assert!(
            std::time::Instant::now() < deadline,
            "even a refused batch restores the watch"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let restored = sched.watch_session().unwrap();
    assert_ne!(restored, session.clone());
    assert!(sched.stop_watch_session(&restored));
    drop(wb);
}

/// A panicking batch must not strand the watch: restoration rides the
/// unwinding, and the scheduler stays usable because its locks tolerate
/// the poisoning a panic leaves behind.
#[cfg(unix)]
#[test]
fn a_panicking_batch_still_restores_the_watch_and_the_scheduler_survives() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().unwrap();
    let bin = root.path().join("boris");
    std::fs::write(
        &bin,
        r#"#!/bin/sh
case "$1" in
watch) echo '{"event":"hello","watch_events_schema":1,"compiler":"boris/0.9.0"}' >&2; sleep 30;;
validate) exit 0;;
*) exit 2;;
esac
"#,
    )
    .unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();

    let wb = Workbench::new(Registry::with_all());
    let delivery = test_delivery(root.path(), WatchMode::Native, &wb);
    let sched = Arc::new(EngineScheduler::new(
        delivery.engine.clone(),
        SeatPolicy::StopWatch,
    ));
    let slot = EngineSlot::default();
    let _ = slot.sched.set(sched.clone());
    slot.install_verbs(slot.verb_route(delivery.clone(), None), slot.generation());

    let session = sched.start_watch(Box::new(|_| {})).unwrap();
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::migrate::adopt::batch_watch_safe(&slot, &sched, || {
            panic!("a defect inside the transaction")
        })
    }));
    assert!(panicked.is_err(), "the panic itself still propagates");

    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while sched.watch_session().is_none() {
        assert!(
            std::time::Instant::now() < deadline,
            "restoration happens during unwinding, not only on normal return"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let restored = sched.watch_session().unwrap();
    assert_ne!(restored, session.clone());

    // The poisoned write lock did not brick the scheduler: a fresh batch
    // still serializes and completes.
    let after = sched.mutation_batch(|| "still usable");
    assert_eq!(after, "still usable");

    assert!(sched.stop_watch_session(&restored));
    drop(wb);
}
