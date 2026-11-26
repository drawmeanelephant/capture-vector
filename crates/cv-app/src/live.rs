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
use cv_engine::{
    EngineConfig, EngineError, EngineScheduler, ProcessEngine, RecoveryBuild, SeatPolicy,
};
use cv_model::cascade::WANT_ENGINE;
use cv_model::modules::doctor::{EngineState, Feed};
use cv_model::modules::engine::{EnginePort, VerbRun};
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

/// The workspace's scheduler once the engine has passed its gate, and
/// the delivery palette verbs report through.
#[derive(Clone, Default)]
pub struct EngineSlot {
    /// Workspace mutation identity: set once, then reused for the process
    /// lifetime so reopen can never split one workspace across schedulers.
    sched: Arc<OnceLock<Arc<Scheduler>>>,
    /// The current shell generation's verb delivery. Unlike scheduler
    /// identity this is replaced on every successful open because an
    /// Inbox is generation-scoped and sends to a reset workbench correctly
    /// land nowhere.
    verbs: Arc<Mutex<Option<VerbRoute>>>,
    /// Monotonic shell-delivery generation. A restore may carry a cloned
    /// route while reopen replaces it; publication compares the clone's
    /// generation under the scheduler start lock rather than assuming an
    /// inbox send failure is the only stale-generation evidence.
    generations: Arc<AtomicU64>,
    /// Serializes boot attempts for this workspace identity. Without it,
    /// a switch-away/back during the first locate could create two
    /// successful engines before either OnceLock became visible.
    boot: Arc<Mutex<()>>,
}

impl EngineSlot {
    pub fn get(&self) -> Option<&Arc<Scheduler>> {
        self.sched.get()
    }

    /// Why the current serialized engine incarnation is unavailable.
    /// Readiness is scheduler-owned gate evidence, not a fresh probe.
    pub fn unready(&self) -> Option<String> {
        self.get().and_then(|s| s.readiness().err())
    }

    fn route(&self) -> Option<VerbRoute> {
        self.verbs.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Install the delivery for one shell generation. Scheduler identity is
    /// immutable, but delivery is not: workspace reopen resets the shell
    /// inbox, and reusing the prior generation's route would silence every
    /// event exactly as `reset_project` intends for stale workers.
    fn verb_route(&self, delivery: LiveDelivery, warning: Option<String>) -> VerbRoute {
        VerbRoute {
            delivery,
            warning,
            generation: 0,
            generations: self.generations.clone(),
        }
    }

    fn install_verbs(&self, mut route: VerbRoute) {
        route.generation = self.generations.fetch_add(1, Ordering::SeqCst) + 1;
        *self.verbs.lock().unwrap_or_else(|e| e.into_inner()) = Some(route);
    }

    /// Queue a convergent disk-truth scan when the workspace engine booted.
    /// A request made after a lifecycle mutation is guaranteed to run after
    /// that mutation even if an older scan is already in flight.
    pub fn request_resync(&self) {
        if self.unready().is_some() {
            return;
        }
        if let Some(route) = self.route() {
            route.delivery.resyncer.request();
        }
    }

    /// A sink wired like the live session's own: cursor-tracked,
    /// editor-tee'd, and queuing disk-truth resyncs on build results —
    /// never a raw inbox channel that would skip those invariants.
    /// `None` while the engine has not passed its gate.
    pub fn live_sink(&self) -> Option<EventSink> {
        if self.unready().is_some() {
            return None;
        }
        let route = self.route()?;
        Some(route.delivery.resync_sink(|deliver, env| deliver(env)))
    }

    /// Restart the workspace watch when the verb batch that ran after
    /// `before` stopped it — the same contract the palette's verb route
    /// honors (`SeatPolicy::StopWatch` means the caller restarts).
    /// Compares sessions rather than trusting a report, because an error
    /// path can lose `stopped_watch`. A no-op when the watch survived,
    /// was never live, or the engine slot is not booted.
    pub fn restore_watch(&self, before: Option<SessionId>, outcome: String) {
        let (Some(sched), Some(route)) = (self.sched.get().cloned(), self.route()) else {
            return;
        };
        if self.unready().is_some() || sched.watch_session().is_some() {
            return;
        }
        // `before` is only evidence for retiring the old UI handle. Whether
        // to restore is decided from current scheduler state: replacement
        // may have stopped the old session before this gate began and then
        // had its own fresh session stopped by the gate build. In every
        // such interleaving the open workspace's watch converges live.
        if let Some(stopped) = before {
            let _ = route.delivery.inbox.send(Inbound::WatchStopped(stopped));
        }
        let gate = route.clone();
        start_workspace_watch_gated(
            sched.clone(),
            route.delivery.clone(),
            route.warning.clone(),
            Some(format!("{outcome} · watch resumed")),
            move || gate.is_current(),
        );
    }

    /// Run a palette verb through the scheduler on a worker. Its events
    /// (the terminal `VerbReported`/`VerbFailed` included) reach the
    /// workbench like any engine envelope. A build that took the watch's
    /// seat restarts the watch afterwards, so the workspace stays live.
    pub fn start_verb(&self, run: VerbRun) -> Result<(), String> {
        let (Some(sched), Some(route)) = (self.sched.get().cloned(), self.route()) else {
            return Err(absent().to_string());
        };
        if let Err(e) = sched.readiness() {
            return Err(format!("the workspace engine is not ready: {e}"));
        }
        std::thread::Builder::new()
            .name("cv-engine-verb".into())
            .spawn(move || route.run(&sched, run))
            .map(|_| ())
            .map_err(|e| format!("could not start the verb worker: {e}"))
    }
}

#[derive(Clone)]
struct VerbRoute {
    delivery: LiveDelivery,
    warning: Option<String>,
    generation: u64,
    generations: Arc<AtomicU64>,
}

impl VerbRoute {
    fn is_current(&self) -> bool {
        self.generation != 0 && self.generations.load(Ordering::SeqCst) == self.generation
    }

    fn run(&self, sched: &Arc<Scheduler>, run: VerbRun) {
        let name = run.verb.name();
        let watching = sched.watch_session();
        let result = sched.run_reported(
            &run.verb,
            self.delivery.resync_sink(|deliver, env| deliver(env)),
            &run.cancel,
        );
        let outcome = match &result {
            Ok(r) if r.exit == ExitClass::Ok => format!("boris {name} finished"),
            Ok(_) => format!("boris {name} failed · see Activity › Errors"),
            Err(e) => format!("boris {name} did not finish: {e}"),
        };
        if result.is_err() {
            self.delivery.inbox.send(Inbound::Status(outcome.clone()));
        }
        // Err loses the report's `stopped_watch`, so compare sessions.
        let Some(stopped) = watching.filter(|_| sched.watch_session().is_none()) else {
            return;
        };
        if self.delivery.inbox.send(Inbound::WatchStopped(stopped)) {
            let gate = self.clone();
            start_workspace_watch_gated(
                sched.clone(),
                self.delivery.clone(),
                self.warning.clone(),
                Some(format!("{outcome} · watch resumed")),
                move || gate.is_current(),
            );
        }
    }
}

/// The palette's engine port over the workspace slot.
pub struct SlotVerbs(pub EngineSlot);

impl EnginePort for SlotVerbs {
    fn unavailable(&self) -> Option<String> {
        (self.0.get().is_none() || self.0.unready().is_some())
            .then(|| "Boris has not passed its version gate for this workspace (see Doctor)".into())
    }
    fn start(&mut self, run: VerbRun) -> Result<(), String> {
        self.0.start_verb(run)
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
    fn run_reported(
        &self,
        cmd: &Verb,
        sink: EventSink,
        cancel: &cv_core::engine::CancelToken,
    ) -> cv_core::Result<ExitClass> {
        Ok(self
            .0
            .get()
            .ok_or_else(absent)?
            .run_reported(cmd, sink, cancel)?
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
        self.sched.stop_watch_session(&self.session);
    }
}

impl Drop for SessionWatch {
    fn drop(&mut self) {
        self.sched.stop_watch_session(&self.session);
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
    // Stop only the fatal session atomically; a delayed recovery must not
    // strand a replacement watch that manual recovery already installed.
    if !sched.stop_watch_session(&session) || !delivery.inbox.send(Inbound::WatchStopped(session)) {
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
    let result = sched.run_recovery_build_caused(
        delivery.resync_sink(|deliver, env| deliver(env)),
        Some(cause),
    );
    let (reason, status) = match result {
        Ok(RecoveryBuild::WatchReplaced(_)) => {
            // Manual recovery already installed a live watch. Its own
            // status/watch events are authoritative; the stale diagnostic
            // must neither stop it nor overwrite its output ownership.
            return;
        }
        Ok(RecoveryBuild::Ran(ExitClass::Ok)) => (
            DegradedReason::WatchDead,
            "Boris one-shot build succeeded".into(),
        ),
        Ok(RecoveryBuild::Ran(exit)) => (
            DegradedReason::EngineBuildFailing,
            format!("Boris one-shot build failed ({exit:?})"),
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
    let _boot = slot.boot.lock().unwrap_or_else(|e| e.into_inner());
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
    if let Some(sched) = slot.get().cloned() {
        // Reopen is the documented engine restart path. Scheduler identity
        // stays stable so in-flight mutations cannot split-brain, but the
        // engine incarnation behind it must refresh from the config just
        // read from disk. The probe can create a write-capability temporary
        // in the content input, so it is serialized as workspace mutation
        // evidence rather than raced against an adoption batch. Replacement
        // then takes the same writer lock, stops the old-config watch, and
        // swaps engine + output-seat atomically against scheduler users.
        let engine = match sched.replace_engine_with(|| {
            ProcessEngine::start(cfg).map(|engine| {
                let html_dir = engine.html_dir().to_string();
                (Arc::new(engine), html_dir)
            })
        }) {
            Ok(engine) => engine,
            Err(e) => {
                doctor.update(|f| f.engine = crate::doctor::engine_state(&e));
                let status = format!("{}: {e}", root.display());
                finish(&inbox, &warning, &notice, Some(degraded_reason(&e)), status);
                return;
            }
        };
        doctor.update(|f| {
            f.engine = crate::doctor::located(engine.identity());
            f.probe = Some(crate::doctor::probe(engine.report()));
        });
        let html_dir = engine.html_dir().to_string();
        let version = engine.identity().version.clone();
        let resyncer = Arc::new(Resyncer::spawn(
            engine.clone(),
            inbox.clone(),
            epoch.clone(),
        ));
        let delivery = LiveDelivery {
            engine,
            inbox: inbox.clone(),
            epoch,
            resyncer,
        };
        slot.install_verbs(slot.verb_route(delivery.clone(), warning.clone()));
        let route = slot.route().expect("installed above");
        let _ = inbox.send(Inbound::Snapshot(workspace_snapshot(
            &root,
            &html_dir,
            Some(version),
            1,
        )));
        start_workspace_watch_gated(sched, delivery, warning, notice, move || route.is_current());
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
    let delivery = {
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
    };
    slot.install_verbs(slot.verb_route(delivery.clone(), warning.clone()));
    let route = slot.route().expect("installed above");
    let _ = slot.sched.set(sched.clone());
    start_workspace_watch_gated(sched, delivery, warning, notice, move || route.is_current());
}

#[cfg(test)]
fn start_workspace_watch(
    sched: Arc<Scheduler>,
    delivery: LiveDelivery,
    warning: Option<String>,
    notice: Option<String>,
) {
    start_workspace_watch_gated(sched, delivery, warning, notice, || true);
}

fn start_workspace_watch_gated(
    sched: Arc<Scheduler>,
    delivery: LiveDelivery,
    warning: Option<String>,
    notice: Option<String>,
    publishable: impl Fn() -> bool,
) {
    let mode = delivery.engine.report().watch_mode;
    let mode_label = match mode {
        WatchMode::Native => "watch",
        WatchMode::OneShotFallback => "one-shot rebuilds",
    };
    let ready = Arc::new(OnceLock::new());
    let inbox = &delivery.inbox;
    let mut attempts = 0;
    let started = loop {
        attempts += 1;
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
        let publish_inbox = delivery.inbox.clone();
        let publish_sched = sched.clone();
        let outcome = sched.start_watch_with_publication(sink, |session| {
            publishable()
                && publish_inbox.send(Inbound::Watch(Box::new(SessionWatch {
                    sched: publish_sched.clone(),
                    session: session.clone(),
                })))
        });
        // An already-live winner is normally a stable published session and
        // is reported below as already-restored. If it has already converged
        // away, though, it was transient stop/start state rather than a live
        // publication: retry bounded times instead of making that race a
        // terminal WatchDead. Atomic stop means this cannot spin on a
        // half-stopped child; each retry observes a converged empty slot.
        let retry_transient = matches!(&outcome, Err(e) if e.to_string().contains("already live"))
            && sched.watch_session().is_none()
            && attempts < 3;
        if !retry_transient {
            break outcome;
        }
    };
    let published = match started {
        Ok(Some(_session)) => {
            let status = format!("{} · {mode_label}", delivery.workspace_identity());
            finish(inbox, &warning, &notice, None, status);
            true
        }
        Ok(None) => {
            // The generation died before publication. The child was stopped
            // inside the scheduler start lock, and every send on this inbox
            // is intentionally dead; there is nothing useful to disclose there.
            false
        }
        Err(e) => {
            // A restore can lose the writer-lock race to reopen's healthy
            // replacement watch. That is already-restored, not dead: unlike
            // a real start failure there will be no later session-start
            // event to clear a false WatchDead banner.
            if sched.watch_session().is_some() {
                let status = format!("{} · watch already live", delivery.workspace_identity());
                finish(inbox, &warning, &notice, None, status);
            } else {
                let status = format!("{}: {e}", delivery.engine.root().display());
                finish(
                    inbox,
                    &warning,
                    &notice,
                    Some(DegradedReason::WatchDead),
                    status,
                );
            }
            false
        }
    };
    let _ = ready.set(published);
}

#[cfg(test)]
mod tests;
