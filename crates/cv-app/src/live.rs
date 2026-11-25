//! Real engine boot. On a worker thread: page snapshot from disk, then
//! `ProcessEngine::start` (locate `CV_BORIS_BIN` → bundled → PATH →
//! sibling, `--version`/`--build-info` gate, capability probe). A pass
//! puts the engine behind one `EngineScheduler` and starts the single
//! watch session; a failure is disclosed as a degraded state and the
//! workspace stays browsable from disk. After each watch build (bursts
//! coalesced), an IR projection runs beside the watch into `.cv/ir`, so
//! Pages shows the fields Boris reports. Nothing here spawns a process:
//! cv-engine does.

use crate::snapshot::workspace_snapshot;
use cv_core::engine::{
    Engine, EngineCaps, EngineInfo, EventSink, ExitClass, Fingerprint, PageDraft, Verb,
    WatchHandle, WriteOutcome,
};
use cv_core::events::engine::EngineEvent;
use cv_core::events::shell::DegradedReason;
use cv_core::events::{AppEvent, Envelope, SessionId};
use cv_core::rules::ArtifactSet;
use cv_engine::cursor::{CursorTracker, Observation};
use cv_engine::debounce::Debounced;
use cv_engine::probe::WatchMode;
use cv_engine::{EngineConfig, EngineError, EngineScheduler, ProcessEngine, SeatPolicy};
use cv_model::cascade::WANT_ENGINE;
use cv_model::modules::doctor::{EngineState, Feed};
use cv_model::workbench::{Inbound, Inbox};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::thread::JoinHandle;
use std::time::Duration;

pub type Scheduler = EngineScheduler<ProcessEngine>;

/// Watch builds inside this window share one IR projection.
const IR_WINDOW: Duration = Duration::from_millis(250);

/// The workspace's scheduler once the engine has passed its gate.
#[derive(Clone, Default)]
pub struct EngineSlot(Arc<OnceLock<Arc<Scheduler>>>);

impl EngineSlot {
    pub fn get(&self) -> Option<&Arc<Scheduler>> {
        self.0.get()
    }
}

fn absent() -> cv_core::Error {
    cv_core::Error::Engine("engine absent: Boris has not passed its version gate".into())
}

/// The `Engine` port over the slot (research promotion's writer). Every
/// call goes through the scheduler, so writes serialize with builds; it
/// refuses honestly while the engine is absent.
pub struct SlotEngine(pub EngineSlot);

impl Engine for SlotEngine {
    fn locate(&self) -> cv_core::Result<EngineInfo> {
        self.0.get().ok_or_else(absent)?.engine().locate()
    }
    fn capabilities(&self) -> EngineCaps {
        self.0
            .get()
            .map(|s| s.engine().capabilities())
            .unwrap_or_default()
    }
    fn snapshot(&self) -> cv_core::Result<ArtifactSet> {
        self.0.get().ok_or_else(absent)?.snapshot()
    }
    fn watch(&self, _: EventSink) -> cv_core::Result<Box<dyn WatchHandle>> {
        Err(cv_core::Error::Engine(
            "the workspace session owns the watch seat".into(),
        ))
    }
    fn run(&self, cmd: &Verb, sink: EventSink) -> cv_core::Result<ExitClass> {
        Ok(self.0.get().ok_or_else(absent)?.run(cmd, sink)?.exit)
    }
    fn write_content(
        &self,
        draft: &PageDraft,
        base: Option<Fingerprint>,
    ) -> cv_core::Result<WriteOutcome> {
        self.0.get().ok_or_else(absent)?.write_content(draft, base)
    }
}

/// The coalesced IR projection.
struct IrProjection {
    worker: Debounced,
}

impl IrProjection {
    fn request(&self) {
        self.worker.request();
    }
}

/// The workbench's handle on the scheduler-owned watch session.
struct SessionWatch {
    sched: Arc<Scheduler>,
    session: SessionId,
}

impl WatchHandle for SessionWatch {
    fn session(&self) -> &SessionId {
        &self.session
    }
    fn stop(&mut self) {
        self.sched.stop_watch();
    }
}

impl Drop for SessionWatch {
    fn drop(&mut self) {
        self.sched.stop_watch();
    }
}

/// Map a failed gate onto the disclosed degraded state.
pub fn degraded_reason(e: &EngineError) -> DegradedReason {
    match e {
        EngineError::VersionMismatch {
            found,
            found_vcs,
            expected,
            expected_vcs,
            ..
        } => {
            let v = found.rsplit('/').next().unwrap_or_default();
            let mut found = if v.is_empty() {
                "unknown".into()
            } else {
                v.to_string()
            };
            let mut want = WANT_ENGINE.to_string();
            // Same version, wrong build: name the revisions so the banner
            // doesn't read "found 0.9.0, want 0.9.0".
            if found == expected.rsplit('/').next().unwrap_or_default() {
                let vcs = found_vcs
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .unwrap_or("none");
                found = format!("{found} (vcs {vcs})");
                want = format!("{want} (vcs {expected_vcs})");
            }
            DegradedReason::EngineVersionMismatch { found, want }
        }
        EngineError::BuildInfoInvalid { .. } => DegradedReason::EngineVersionMismatch {
            found: "unknown (invalid --build-info)".into(),
            want: WANT_ENGINE.into(),
        },
        _ => DegradedReason::EngineAbsent,
    }
}

pub struct EngineBoot {
    pub cfg: EngineConfig,
    pub slot: EngineSlot,
    pub inbox: Inbox,
    /// A disclosure the boot outcome must not hide (research storage
    /// failed): kept in the status line and re-disclosed last.
    pub warning: Option<String>,
    /// A successful/failed maintenance result the boot outcome must not erase.
    pub notice: Option<String>,
    /// Doctor's facts: the gate and probe outcome land here before the
    /// status send that wakes the UI.
    pub doctor: Feed,
}

/// Final status + any degraded state; a carried warning stays visible.
fn finish(
    inbox: &Inbox,
    warning: &Option<String>,
    notice: &Option<String>,
    degraded: Option<DegradedReason>,
    status: String,
) {
    crate::logging::info(
        "engine_status",
        &[
            ("status", serde_json::json!(&status)),
            ("degraded", serde_json::json!(degraded.is_some())),
        ],
    );
    if let Some(r) = degraded {
        inbox.send(Inbound::Degraded(r));
    }
    let status = match notice {
        Some(n) => format!("{n} · {status}"),
        None => status,
    };
    match warning {
        Some(w) => {
            inbox.send(Inbound::Status(format!("{w} · {status}")));
            inbox.send(Inbound::Degraded(DegradedReason::StoreCorrupt));
        }
        None => {
            inbox.send(Inbound::Status(status));
        }
    }
}

/// Start the engine boot thread. `None` when the OS refuses one; the
/// failure is then disclosed like any engine that never started.
pub fn spawn(b: EngineBoot) -> Option<JoinHandle<()>> {
    let inbox = b.inbox.clone();
    let warning = b.warning.clone();
    let root = b.cfg.root.clone();
    let doctor = b.doctor.clone();
    match std::thread::Builder::new()
        .name("cv-engine-boot".into())
        .spawn(move || run(b))
    {
        Ok(handle) => Some(handle),
        Err(e) => {
            doctor.update(|f| {
                f.engine = EngineState::Failed {
                    error: format!("engine boot thread failed to start: {e}"),
                }
            });
            boot_thread_failed(&inbox, &warning, &root, &e);
            None
        }
    }
}

fn boot_thread_failed(
    inbox: &Inbox,
    warning: &Option<String>,
    root: &std::path::Path,
    e: &std::io::Error,
) {
    let status = format!(
        "{}: engine boot thread failed to start: {e}",
        root.display()
    );
    finish(
        inbox,
        warning,
        &None,
        Some(DegradedReason::EngineAbsent),
        status,
    );
}

#[derive(Clone)]
struct LiveDelivery {
    engine: Arc<ProcessEngine>,
    inbox: Inbox,
    epoch: Arc<AtomicU64>,
}

impl LiveDelivery {
    fn workspace_identity(&self) -> String {
        let id = self.engine.identity();
        format!(
            "{} · {} ({:?})",
            self.engine.root().display(),
            id.version,
            id.source
        )
    }

    /// Send the workbench a fresh snapshot of disk truth.
    fn resync(&self) {
        let _ = self.engine.disk_truth();
        let n = self.epoch.fetch_add(1, Ordering::SeqCst) + 1;
        self.inbox.send(Inbound::Snapshot(workspace_snapshot(
            self.engine.root(),
            self.engine.html_dir(),
            Some(self.engine.identity().version.clone()),
            n,
        )));
    }

    /// The coalescing worker for [`Self::project_ir`]. `None` (logged)
    /// when the OS refuses a thread: Pages then keeps its marked fallback.
    fn ir_projector(&self, sched: Weak<Scheduler>) -> Option<IrProjection> {
        let delivery = self.clone();
        Debounced::spawn("cv-ir-projection", IR_WINDOW, move || {
            delivery.project_ir(&sched)
        })
        .inspect_err(|e| {
            crate::logging::warn(
                "ir_projection_unavailable",
                &[("error", serde_json::json!(e.to_string()))],
            )
        })
        .ok()
        .map(|worker| IrProjection { worker })
    }

    /// Boris IR into `.cv/ir` beside the live watch, then a resync so
    /// Pages reads it. IR mode needs no layout, so it also runs after a
    /// failed HTML build. A failed projection is always disclosed in
    /// Activity, even when the watch build was already red: Pages keeping
    /// fallback or stale fields needs the projection's own cause.
    fn project_ir(&self, sched: &Weak<Scheduler>) {
        let Some(sched) = sched.upgrade() else {
            return;
        };
        let held = Arc::new(Mutex::new(Vec::new()));
        let sink = held.clone();
        let result = sched.run(
            &self.engine.ir_projection(),
            Box::new(move |env| sink.lock().unwrap_or_else(|e| e.into_inner()).push(env)),
        );
        drop(sched);
        if matches!(&result, Ok(r) if r.exit == ExitClass::Ok) {
            self.resync();
            return;
        }
        let why = match &result {
            Ok(r) => format!("{:?}", r.exit),
            Err(e) => e.to_string(),
        };
        crate::logging::warn("ir_projection_failed", &[("why", serde_json::json!(why))]);
        let mut deliver = self.inbox.sink();
        for env in std::mem::take(&mut *held.lock().unwrap_or_else(|e| e.into_inner())) {
            deliver(env);
        }
        if let Err(e) = result {
            self.inbox.send(Inbound::Status(format!(
                "Boris IR projection failed: {e} · Pages keeps its current fields · {}",
                self.workspace_identity()
            )));
        }
    }

    /// Refresh disk truth on a new session, a sequence gap or a build
    /// result, before forwarding the original envelope to the workbench
    /// and editor. Replay overlap is dropped before routing.
    fn resync_sink(
        &self,
        mut route: impl FnMut(&mut EventSink, Envelope) + Send + 'static,
    ) -> EventSink {
        let delivery = self.clone();
        let mut tracker = CursorTracker::new();
        let mut deliver = cv_widgets::editor::host::tee_builds(self.inbox.sink());
        Box::new(move |env| {
            let obs = tracker.observe(&env.cursor);
            if obs == Observation::Duplicate {
                return;
            }
            let built = matches!(
                env.payload,
                AppEvent::Engine(
                    EngineEvent::BuildFinished { .. } | EngineEvent::BuildFailed { .. }
                )
            );
            if matches!(obs, Observation::Resync(_)) || built {
                // Not via the scheduler: the start envelope can arrive
                // while `start_watch` holds its writer lock.
                delivery.resync();
            }
            route(&mut deliver, env);
        })
    }

    /// Boris's native watch can report a fatal failure with no diagnostics
    /// (printed only by `build`). Spend at most one attempt per workspace
    /// open, not one per event. Polling already runs one-shot builds on
    /// edits, so it never uses that attempt. Every other build result asks
    /// `ir` for a projection; the sink owns it, so it ends with the watch.
    fn watch_sink(
        &self,
        mode: WatchMode,
        ir: Option<IrProjection>,
        recover: impl FnOnce(SessionId) + Send + 'static,
    ) -> EventSink {
        let mut recover = Some(recover);
        let mut polling_failure = false;
        let inbox = self.inbox.clone();
        self.resync_sink(move |deliver, env| {
            let fatal = matches!(
                &env.payload,
                AppEvent::Engine(EngineEvent::BuildFailed { diagnostics, recoverable: false, .. })
                    if diagnostics.is_empty()
            );
            let build_result = match &env.payload {
                AppEvent::Engine(EngineEvent::BuildFinished { .. }) => Some(true),
                AppEvent::Engine(EngineEvent::BuildFailed { .. }) if !fatal => Some(false),
                _ => None,
            };
            if build_result == Some(true) {
                polling_failure = false;
            }
            if let (Some(ir), Some(_)) = (&ir, build_result) {
                ir.request();
            }
            let session = fatal.then(|| env.cursor.session.clone());
            deliver(env);
            if let Some(session) = session {
                match mode {
                    WatchMode::Native => {
                        if let Some(recover) = recover.take() {
                            recover(session);
                        }
                    }
                    WatchMode::OneShotFallback if !polling_failure => {
                        polling_failure = true;
                        inbox.send(Inbound::Degraded(DegradedReason::BuildFailed));
                    }
                    WatchMode::OneShotFallback => {}
                }
            }
        })
    }
}

fn recover_build(
    sched: &Scheduler,
    session: SessionId,
    delivery: &LiveDelivery,
    warning: &Option<String>,
    notice: &Option<String>,
) {
    // This runs off the pump thread: stopping the watch joins its pump.
    // Retire the handle and status before the diagnostic build takes the seat.
    if sched.stop_watch().as_ref() != Some(&session)
        || !delivery.inbox.send(Inbound::WatchStopped(session))
    {
        return;
    }
    let inbox = &delivery.inbox;
    let identity = delivery.workspace_identity();
    let trying = format!("Boris watch stopped · running one build for diagnostics… · {identity}");
    inbox.send(Inbound::Status(match warning {
        Some(w) => format!("{w} · {trying}"),
        None => trying,
    }));
    // Route output to the UI, never back through the watch recovery sink.
    let result = sched.run(
        &Verb::Build,
        delivery.resync_sink(|deliver, env| deliver(env)),
    );
    let (reason, status) = match result {
        Ok(r) if r.exit == ExitClass::Ok => (
            DegradedReason::WatchDead,
            "Boris one-shot build succeeded".into(),
        ),
        Ok(r) => (
            DegradedReason::EngineBuildFailing,
            format!("Boris one-shot build failed ({:?})", r.exit),
        ),
        Err(e) => (
            DegradedReason::EngineBuildFailing,
            format!("Boris one-shot build failed: {e}"),
        ),
    };
    // A successful diagnostic build doesn't mean the stopped watcher is live.
    // Reopening starts a new watch and rearms recovery.
    finish(
        inbox,
        warning,
        notice,
        Some(reason),
        format!("{status} · reopen workspace to resume watching · {identity}"),
    );
}

fn run(b: EngineBoot) {
    let EngineBoot {
        cfg,
        slot,
        inbox,
        warning,
        notice,
        doctor,
    } = b;
    let root: PathBuf = cfg.root.clone();
    let epoch = Arc::new(AtomicU64::new(1));
    // Browsable from disk whether or not the engine passes.
    if !inbox.send(Inbound::Snapshot(workspace_snapshot(
        &root,
        &cfg.html_dir,
        None,
        1,
    ))) {
        return;
    }
    let engine = match ProcessEngine::start(cfg) {
        Ok(e) => {
            doctor.update(|f| {
                f.engine = crate::doctor::located(e.identity());
                f.probe = Some(crate::doctor::probe(e.report()));
            });
            Arc::new(e)
        }
        Err(e) => {
            doctor.update(|f| f.engine = crate::doctor::engine_state(&e));
            let status = format!("{}: {e}", root.display());
            finish(&inbox, &warning, &notice, Some(degraded_reason(&e)), status);
            return;
        }
    };
    let sched = Arc::new(
        EngineScheduler::new(engine.clone(), SeatPolicy::StopWatch).with_seat(engine.html_dir()),
    );
    let _ = slot.0.set(sched.clone());
    start_workspace_watch(
        sched,
        LiveDelivery {
            engine,
            inbox,
            epoch,
        },
        warning,
        notice,
    );
}

fn start_workspace_watch(
    sched: Arc<Scheduler>,
    delivery: LiveDelivery,
    warning: Option<String>,
    notice: Option<String>,
) {
    let mode = delivery.engine.report().watch_mode;
    let mode_label = match mode {
        WatchMode::Native => "watch",
        WatchMode::OneShotFallback => "one-shot rebuilds",
    };
    let ready = Arc::new(OnceLock::new());
    let recover_ready = ready.clone();
    let recover_sched = Arc::downgrade(&sched);
    let recover_delivery = delivery.clone();
    let recover_warning = warning.clone();
    let recover_notice = notice.clone();
    let ir = delivery.ir_projector(Arc::downgrade(&sched));
    let sink = delivery.watch_sink(mode, ir, move |session| {
        std::thread::spawn(move || {
            // A fatal initial build can arrive before `start_watch`
            // returns. Publish the watch handle/status before retiring it.
            if *recover_ready.wait()
                && let Some(sched) = recover_sched.upgrade()
            {
                recover_build(
                    &sched,
                    session,
                    &recover_delivery,
                    &recover_warning,
                    &recover_notice,
                );
            }
        });
    });
    let inbox = &delivery.inbox;
    let published = match sched.start_watch(sink) {
        Ok(session) => {
            let published = inbox.send(Inbound::Watch(Box::new(SessionWatch { sched, session })));
            let status = format!("{} · {mode_label}", delivery.workspace_identity());
            finish(inbox, &warning, &notice, None, status);
            published
        }
        Err(e) => {
            let status = format!("{}: {e}", delivery.engine.root().display());
            finish(
                inbox,
                &warning,
                &notice,
                Some(DegradedReason::WatchDead),
                status,
            );
            false
        }
    };
    let _ = ready.set(published);
}

#[cfg(test)]
mod tests {
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
        LiveDelivery {
            engine: Arc::new(engine),
            inbox: wb.inbox(),
            epoch: Arc::new(AtomicU64::new(1)),
        }
    }

    fn build(seq: u64, payload: EngineEvent) -> Envelope {
        Envelope {
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
    fn native_watch_spends_only_one_attempt_after_forwarding_undocumented_failures() {
        let root = tempfile::tempdir().unwrap();
        let mut wb = Workbench::new(Registry::with_all());
        let delivery = test_delivery(root.path(), WatchMode::Native, &wb);
        let (tx, rx) = std::sync::mpsc::channel();
        let mut sink = delivery.watch_sink(WatchMode::Native, None, move |id| tx.send(id).unwrap());

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
        wb.drain();
        assert_eq!(wb.app.committed, 5);
        assert_eq!(wb.app.snapshot_epoch, 6);
    }

    #[test]
    fn polling_watch_discloses_failures_without_retrying_and_clears_on_success() {
        let root = tempfile::tempdir().unwrap();
        let mut wb = Workbench::new(Registry::with_all());
        let delivery = test_delivery(root.path(), WatchMode::OneShotFallback, &wb);
        let mut sink = delivery.watch_sink(WatchMode::OneShotFallback, None, |_| {
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
        assert_eq!(activity.list_len(Some("builds")), 2);
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
                message: "failed to load layout themes/boris/layouts/main.html: FileNotFound"
                    .into(),
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

    #[cfg(unix)]
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
        let ir = delivery.ir_projector(Arc::downgrade(&sched));
        let mut sink = delivery.watch_sink(WatchMode::Native, ir, |_| {
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
            "build --out .cv/ir\n"
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
        let ir = delivery.ir_projector(Arc::downgrade(&sched));
        let mut sink = delivery.watch_sink(WatchMode::Native, ir, |_| {
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
        let (_root, mut wb, delivery) = ir_workspace(Some(3));
        let sched = Arc::new(EngineScheduler::new(
            delivery.engine.clone(),
            SeatPolicy::StopWatch,
        ));
        let ir = delivery.ir_projector(Arc::downgrade(&sched));
        let mut sink = delivery.watch_sink(WatchMode::Native, ir, |_| {
            panic!("a failed projection is not a failed watch")
        });
        sink(layout_failed(0));
        let deadline = Instant::now() + Duration::from_secs(10);
        while !activity_rows(&wb).contains(&"ir finished · IoFailure".to_string()) {
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
        let ir = delivery.ir_projector(Arc::downgrade(&sched));
        let mut sink = delivery.watch_sink(WatchMode::Native, ir, |_| {
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
            "#!/bin/sh\necho \"$*\" >> \"$(dirname \"$0\")/argv.log\"\necho 'error: EIO: disk full' >&2\nexit 3\n",
        )
        .unwrap();
        sink(finished(1));
        let deadline = Instant::now() + Duration::from_secs(10);
        while !activity_rows(&wb).contains(&"ir finished · IoFailure".to_string()) {
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
        boot_thread_failed(&wb.inbox(), &None, Path::new("/ws"), &refused);
        wb.drain();
        assert_eq!(wb.app.degraded, Some(DegradedReason::EngineAbsent));
        assert!(
            wb.status
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
}
