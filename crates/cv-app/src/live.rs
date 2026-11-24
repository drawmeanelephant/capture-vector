//! Real engine boot. On a worker thread: page snapshot from disk, then
//! `ProcessEngine::start` (locate `CV_BORIS_BIN` → bundled → PATH →
//! sibling, `--version`/`--build-info` gate, capability probe). A pass
//! puts the engine behind one `EngineScheduler` and starts the single
//! watch session; a failure is disclosed as a degraded state and the
//! workspace stays browsable from disk. Nothing here spawns a process:
//! cv-engine does.

use crate::snapshot::workspace_snapshot;
use cv_core::engine::{
    Engine, EngineCaps, EngineInfo, EventSink, ExitClass, Fingerprint, PageDraft, Verb,
    WatchHandle, WriteOutcome,
};
use cv_core::events::engine::EngineEvent;
use cv_core::events::shell::DegradedReason;
use cv_core::events::{AppEvent, SessionId};
use cv_core::rules::ArtifactSet;
use cv_engine::cursor::{CursorTracker, Observation};
use cv_engine::probe::WatchMode;
use cv_engine::{EngineConfig, EngineError, EngineScheduler, ProcessEngine, SeatPolicy};
use cv_model::cascade::WANT_ENGINE;
use cv_model::workbench::{Inbound, Inbox};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread::JoinHandle;

pub type Scheduler = EngineScheduler<ProcessEngine>;

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
}

/// Final status + any degraded state; a carried warning stays visible.
fn finish(
    inbox: &Inbox,
    warning: &Option<String>,
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

pub fn spawn(b: EngineBoot) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("cv-engine-boot".into())
        .spawn(move || run(b))
        .expect("spawn engine boot thread")
}

#[derive(Clone)]
struct LiveDelivery {
    engine: Arc<ProcessEngine>,
    inbox: Inbox,
    epoch: Arc<AtomicU64>,
}

impl LiveDelivery {
    fn identity(&self) -> String {
        let id = self.engine.identity();
        format!(
            "{} · {} ({:?})",
            self.engine.root().display(),
            id.version,
            id.source
        )
    }

    /// A new session, cursor gap or build result refreshes disk truth
    /// before forwarding the envelope. Replay overlap is dropped.
    fn resync_sink(&self, mut on_fatal: impl FnMut() + Send + 'static) -> EventSink {
        let Self {
            engine,
            inbox,
            epoch,
        } = self.clone();
        let mut tracker = CursorTracker::new();
        // Build results also reach open editors' Problems rails.
        let mut deliver = cv_widgets::editor::host::tee_builds(inbox.sink());
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
                // Not via the scheduler: starting a watch emits its
                // first envelope while holding the writer lock.
                let _ = engine.disk_truth();
                let n = epoch.fetch_add(1, Ordering::SeqCst) + 1;
                inbox.send(Inbound::Snapshot(workspace_snapshot(
                    engine.root(),
                    "dist",
                    Some(engine.identity().version.clone()),
                    n,
                )));
            }
            let fatal = matches!(
                &env.payload,
                AppEvent::Engine(EngineEvent::BuildFailed { diagnostics, recoverable: false, .. })
                    if diagnostics.is_empty()
            );
            deliver(env);
            if fatal {
                on_fatal();
            }
        })
    }
}

fn recover_build(sched: &Scheduler, delivery: &LiveDelivery, warning: &Option<String>) {
    // Join the watch before the diagnostic build, so its session-end
    // event cannot overwrite the disclosure about the build outcome.
    if sched.stop_watch().is_none() {
        return;
    }
    let inbox = &delivery.inbox;
    if !inbox.send(Inbound::Status(
        "Boris watch stopped · running one build for diagnostics…".into(),
    )) {
        return;
    }
    // A diagnostic build is terminal for this watch. Its own failure
    // never schedules another retry.
    let (reason, status) = match sched.run(&Verb::Build, delivery.resync_sink(|| {})) {
        Ok(r) if r.exit == ExitClass::Ok => (
            DegradedReason::WatchDead,
            "Boris diagnostic build succeeded".into(),
        ),
        Ok(r) => (
            DegradedReason::EngineBuildFailing,
            format!("Boris diagnostic build failed ({:?})", r.exit),
        ),
        Err(e) => (
            DegradedReason::EngineBuildFailing,
            format!("Boris diagnostic build failed: {e}"),
        ),
    };
    finish(
        inbox,
        warning,
        Some(reason),
        format!(
            "{status} · reopen workspace to resume watching · {}",
            delivery.identity()
        ),
    );
}

fn run(b: EngineBoot) {
    let EngineBoot {
        cfg,
        slot,
        inbox,
        warning,
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
        Ok(e) => Arc::new(e),
        Err(e) => {
            let status = format!("{}: {e}", root.display());
            finish(&inbox, &warning, Some(degraded_reason(&e)), status);
            return;
        }
    };
    let sched = Arc::new(EngineScheduler::new(engine.clone(), SeatPolicy::StopWatch));
    let _ = slot.0.set(sched.clone());
    start_watch(
        sched,
        LiveDelivery {
            engine,
            inbox,
            epoch,
        },
        warning,
    );
}

fn start_watch(sched: Arc<Scheduler>, delivery: LiveDelivery, warning: Option<String>) {
    let mode = delivery.engine.report().watch_mode;
    let label = match mode {
        WatchMode::Native => "watch",
        WatchMode::OneShotFallback => "one-shot rebuilds",
    };
    let ready = Arc::new(OnceLock::new());
    let published = ready.clone();
    let recovery_sched = Arc::downgrade(&sched);
    let recovery_delivery = delivery.clone();
    let recovery_warning = warning.clone();
    // The native watcher may report a fatal failure with diagnostics
    // only available from a one-shot build. Allow one attempt per open.
    // Polling already runs one-shot builds on edits.
    let mut recover = (mode == WatchMode::Native).then_some(move || {
        // Wait off the pump thread. An initial failure can arrive
        // before start_watch publishes the handle and boot status.
        std::thread::spawn(move || {
            if *published.wait()
                && let Some(sched) = recovery_sched.upgrade()
            {
                recover_build(&sched, &recovery_delivery, &recovery_warning);
            }
        });
    });
    let sink = delivery.resync_sink(move || {
        if let Some(recover) = recover.take() {
            recover();
        }
    });
    let inbox = &delivery.inbox;
    let started = match sched.start_watch(sink) {
        Ok(session) => {
            let sent = inbox.send(Inbound::Watch(Box::new(SessionWatch { sched, session })));
            finish(
                inbox,
                &warning,
                None,
                format!("{} · {label}", delivery.identity()),
            );
            sent
        }
        Err(e) => {
            let status = format!("{}: {e}", delivery.engine.root().display());
            finish(inbox, &warning, Some(DegradedReason::WatchDead), status);
            false
        }
    };
    let _ = ready.set(started);
}

#[cfg(test)]
mod tests {
    use super::*;
    use cv_core::engine::EngineSource;

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
    fn slot_engine_refuses_while_absent() {
        let e = SlotEngine(EngineSlot::default());
        assert!(e.locate().is_err());
        assert!(e.run(&Verb::Build, Box::new(|_| {})).is_err());
        assert!(!e.capabilities().watch && !e.capabilities().writes);
    }
}
