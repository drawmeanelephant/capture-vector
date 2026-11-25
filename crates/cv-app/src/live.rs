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
    Engine, EngineCaps, EngineInfo, EventSink, ExitClass, Fingerprint, MoveOutcome, PageDraft,
    PageMove, RemoveOutcome, Verb, WatchHandle, WriteOutcome,
};
use cv_core::events::engine::EngineEvent;
use cv_core::events::shell::DegradedReason;
use cv_core::events::{AppEvent, Envelope, EventId, SessionId};
use cv_core::rules::ArtifactSet;
use cv_engine::cursor::{CursorTracker, Observation};
use cv_engine::probe::WatchMode;
use cv_engine::{EngineConfig, EngineError, EngineScheduler, ProcessEngine, SeatPolicy};
use cv_model::cascade::WANT_ENGINE;
use cv_model::modules::doctor::{EngineState, Feed};
use cv_model::workbench::{Inbound, Inbox};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, Weak};
use std::thread::JoinHandle;
use std::time::Duration;

pub type Scheduler = EngineScheduler<ProcessEngine>;

/// Watch builds inside this window share one IR projection.
const IR_WINDOW: Duration = Duration::from_millis(250);

mod projection;
use projection::IrProjection;

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
        self.run_caused(cmd, sink, None)
    }

    fn run_caused(
        &self,
        cmd: &Verb,
        sink: EventSink,
        cause: Option<&cv_core::events::EventId>,
    ) -> cv_core::Result<ExitClass> {
        Ok(self
            .0
            .get()
            .ok_or_else(absent)?
            .run_caused(cmd, sink, cause)?
            .exit)
    }
    fn write_content(
        &self,
        draft: &PageDraft,
        base: Option<Fingerprint>,
    ) -> cv_core::Result<WriteOutcome> {
        self.0.get().ok_or_else(absent)?.write_content(draft, base)
    }
    fn remove_content(&self, path: &str, base: Fingerprint) -> cv_core::Result<RemoveOutcome> {
        self.0.get().ok_or_else(absent)?.remove_content(path, base)
    }
    fn move_content(&self, mv: &PageMove) -> cv_core::Result<MoveOutcome> {
        self.0.get().ok_or_else(absent)?.move_content(mv)
    }
}

/// The coalesced IR projection.
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
    let notice = b.notice.clone();
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
            boot_thread_failed(&inbox, &warning, &notice, &root, &e);
            None
        }
    }
}

fn boot_thread_failed(
    inbox: &Inbox,
    warning: &Option<String>,
    notice: &Option<String>,
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
        notice,
        Some(DegradedReason::EngineAbsent),
        status,
    );
}

#[derive(Default)]
struct ResyncState {
    pending: bool,
    closed: bool,
}

struct ResyncShared {
    state: Mutex<ResyncState>,
    wake: Condvar,
}

/// Filesystem snapshot scans are cheap to request but potentially slow to
/// perform. Keep them off engine sink delivery: the emitter serializes
/// cursor delivery, so scanning there would stall every unrelated engine
/// producer behind disk latency. Requests coalesce because each scan reads
/// current disk truth.
struct Resyncer {
    shared: Arc<ResyncShared>,
}

fn scan_disk_truth(engine: &ProcessEngine, inbox: &Inbox, epoch: &AtomicU64) {
    let _ = engine.disk_truth();
    let n = epoch.fetch_add(1, Ordering::SeqCst) + 1;
    inbox.send(Inbound::Snapshot(workspace_snapshot(
        engine.root(),
        engine.html_dir(),
        Some(engine.identity().version.clone()),
        n,
    )));
}

impl Resyncer {
    fn spawn(engine: Arc<ProcessEngine>, inbox: Inbox, epoch: Arc<AtomicU64>) -> Self {
        Self::spawn_with(move || scan_disk_truth(&engine, &inbox, &epoch))
    }

    fn spawn_with(mut scan: impl FnMut() + Send + 'static) -> Self {
        let shared = Arc::new(ResyncShared {
            state: Mutex::new(ResyncState::default()),
            wake: Condvar::new(),
        });
        let worker = shared.clone();
        std::thread::Builder::new()
            .name("cv-resync".into())
            .spawn(move || {
                loop {
                    let mut s = worker.state.lock().unwrap_or_else(|e| e.into_inner());
                    while !s.pending && !s.closed {
                        s = worker.wake.wait(s).unwrap_or_else(|e| e.into_inner());
                    }
                    if s.closed {
                        return;
                    }
                    s.pending = false;
                    drop(s);
                    scan();
                }
            })
            .expect("cv-resync thread");
        Self { shared }
    }

    fn request(&self) {
        let mut s = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        s.pending = true;
        self.shared.wake.notify_one();
    }
}

impl Drop for Resyncer {
    fn drop(&mut self) {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .closed = true;
        self.shared.wake.notify_one();
    }
}

#[derive(Clone)]
struct LiveDelivery {
    engine: Arc<ProcessEngine>,
    inbox: Inbox,
    epoch: Arc<AtomicU64>,
    resyncer: Arc<Resyncer>,
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

    /// Send the workbench a fresh snapshot of disk truth. Call this only
    /// when already outside engine event delivery; sink paths use
    /// `request_resync` so filesystem latency cannot stall other producers.
    fn resync(&self) {
        scan_disk_truth(&self.engine, &self.inbox, &self.epoch);
    }

    fn request_resync(&self) {
        self.resyncer.request();
    }

    /// Queue disk-truth refresh on a new session, a sequence gap or a
    /// build result, then forward the original envelope without blocking
    /// engine delivery on filesystem latency. Replay overlap is dropped
    /// before routing. Snapshots are convergent state, so coalescing them
    /// preserves the final disk truth while Activity ordering stays exact.
    fn resync_sink(
        &self,
        route: impl FnMut(&mut EventSink, Envelope) + Send + 'static,
    ) -> EventSink {
        self.resync_sink_with_tracker(Arc::new(Mutex::new(CursorTracker::new())), route)
    }

    fn resync_sink_with_tracker(
        &self,
        tracker: Arc<Mutex<CursorTracker>>,
        mut route: impl FnMut(&mut EventSink, Envelope) + Send + 'static,
    ) -> EventSink {
        let delivery = self.clone();
        let mut deliver = cv_widgets::editor::host::tee_builds(self.inbox.sink());
        Box::new(move |env| {
            let obs = tracker
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .observe(&env.cursor);
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
                delivery.request_resync();
            }
            route(&mut deliver, env);
        })
    }

    /// Boris's native watch can report a fatal failure with no diagnostics
    /// (printed only by `build`). Spend at most one attempt per workspace
    /// open, not one per event. Polling already runs one-shot builds on
    /// edits, so it never uses that attempt. Every other build result asks
    /// `ir` for a projection; the sink owns it, so it ends with the watch.
    #[cfg(test)]
    fn watch_sink(
        &self,
        mode: WatchMode,
        ir: Option<IrProjection>,
        recovery: impl FnOnce(SessionId, EventId) + Send + 'static,
    ) -> EventSink {
        self.watch_sink_with_tracker(
            mode,
            ir,
            recovery,
            Arc::new(Mutex::new(CursorTracker::new())),
        )
    }

    fn watch_sink_with_tracker(
        &self,
        mode: WatchMode,
        ir: Option<IrProjection>,
        recovery: impl FnOnce(SessionId, EventId) + Send + 'static,
        tracker: Arc<Mutex<CursorTracker>>,
    ) -> EventSink {
        let mut recovery = Some(recovery);
        let mut polling_failure = false;
        let inbox = self.inbox.clone();
        self.resync_sink_with_tracker(tracker, move |deliver, env| {
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
            let recovery_cause = fatal.then(|| (env.cursor.session.clone(), env.event_id.clone()));
            deliver(env);
            if let Some((session, cause)) = recovery_cause {
                match mode {
                    WatchMode::Native => {
                        if let Some(recover) = recovery.take() {
                            recover(session, cause);
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

    /// Build the single ordered route shared by the watch child and IR
    /// projection. The outer sink only forwards; the inner sink owns the
    /// cursor tracker, while the projector holds the route weakly so the
    /// pipeline drops cleanly with the watch.
    fn watch_pipeline(
        &self,
        mode: WatchMode,
        sched: Weak<Scheduler>,
        recovery: impl FnOnce(SessionId, EventId) + Send + 'static,
    ) -> EventSink {
        let tracker = Arc::new(Mutex::new(CursorTracker::new()));
        let ir = self.ir_projector(sched, tracker.clone());
        self.watch_sink_with_tracker(mode, ir, recovery, tracker)
    }
}

fn recover_build(
    sched: &Scheduler,
    session: SessionId,
    cause: &EventId,
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
    let result = sched.run_caused(
        &Verb::Build,
        delivery.resync_sink(|deliver, env| deliver(env)),
        Some(cause),
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
        {
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
    let sink = delivery.watch_pipeline(mode, Arc::downgrade(&sched), move |session, cause| {
        std::thread::spawn(move || {
            // A fatal initial build can arrive before `start_watch`
            // returns. Publish the watch handle/status before retiring it.
            if *recover_ready.wait()
                && let Some(sched) = recover_sched.upgrade()
            {
                recover_build(
                    &sched,
                    session,
                    &cause,
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
mod tests;
