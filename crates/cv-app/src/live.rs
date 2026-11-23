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
use std::sync::{Arc, OnceLock, Weak};
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
        EngineError::VersionMismatch { found, .. } => {
            let v = found.rsplit('/').next().unwrap_or_default();
            DegradedReason::EngineVersionMismatch {
                found: if v.is_empty() {
                    "unknown".into()
                } else {
                    v.into()
                },
                want: WANT_ENGINE.into(),
            }
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
}

pub fn spawn(b: EngineBoot) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("cv-engine-boot".into())
        .spawn(move || run(b))
        .expect("spawn engine boot thread")
}

/// Envelopes pass through a `CursorTracker`: a new session or a seq gap
/// rereads disk truth and lands a fresh snapshot before the envelope;
/// replay overlap is dropped. A finished build also refreshes the page
/// list (files may have appeared), still ahead of its own envelope.
///
/// Boris's watch reports a fatal initial build with no diagnostics (they
/// only reach the one-shot build's stderr), so that failure triggers one
/// `build` through the scheduler, as the fixture path does.
fn resync_sink(
    engine: Arc<ProcessEngine>,
    sched: Weak<Scheduler>,
    inbox: Inbox,
    epoch: Arc<AtomicU64>,
    compiler: String,
) -> EventSink {
    let mut tracker = CursorTracker::new();
    let (root, html) = (engine.root().to_path_buf(), html_dir());
    Box::new(move |env| {
        let obs = tracker.observe(&env.cursor);
        if obs == Observation::Duplicate {
            return;
        }
        let built = matches!(
            env.payload,
            AppEvent::Engine(EngineEvent::BuildFinished { .. } | EngineEvent::BuildFailed { .. })
        );
        if matches!(obs, Observation::Resync(_)) || built {
            // Disk truth through the engine (bumps its snapshot epoch).
            // Not via the scheduler: the session-start envelope arrives
            // while `start_watch` holds its writer lock.
            let _ = engine.disk_truth();
            let n = epoch.fetch_add(1, Ordering::SeqCst) + 1;
            inbox.send(Inbound::Snapshot(workspace_snapshot(
                &root,
                &html,
                Some(compiler.clone()),
                n,
            )));
        }
        let recover = matches!(
            &env.payload,
            AppEvent::Engine(EngineEvent::BuildFailed { diagnostics, recoverable: false, .. })
                if diagnostics.is_empty()
        );
        inbox.send(Inbound::Envelope(env));
        if recover && let Some(s) = sched.upgrade() {
            let sink = resync_sink(
                engine.clone(),
                Weak::new(),
                inbox.clone(),
                epoch.clone(),
                compiler.clone(),
            );
            // Off the pump thread: the build stops the watch, which joins it.
            std::thread::spawn(move || {
                let _ = s.run(&Verb::Build, sink);
            });
        }
    })
}

fn html_dir() -> String {
    "dist".into()
}

fn run(b: EngineBoot) {
    let EngineBoot { cfg, slot, inbox } = b;
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
            inbox.send(Inbound::Degraded(degraded_reason(&e)));
            inbox.send(Inbound::Status(format!("{}: {e}", root.display())));
            return;
        }
    };
    let id = engine.identity().clone();
    let mode = match engine.report().watch_mode {
        WatchMode::Native => "watch",
        WatchMode::OneShotFallback => "one-shot rebuilds",
    };
    let sched = Arc::new(EngineScheduler::new(engine.clone(), SeatPolicy::StopWatch));
    let _ = slot.0.set(sched.clone());
    let sink = resync_sink(
        engine,
        Arc::downgrade(&sched),
        inbox.clone(),
        epoch,
        id.version.clone(),
    );
    match sched.start_watch(sink) {
        Ok(session) => {
            inbox.send(Inbound::Watch(Box::new(SessionWatch { sched, session })));
            inbox.send(Inbound::Status(format!(
                "{} · {} ({:?}) · {mode}",
                root.display(),
                id.version,
                id.source
            )));
        }
        Err(e) => {
            inbox.send(Inbound::Degraded(DegradedReason::WatchDead));
            inbox.send(Inbound::Status(format!("{}: {e}", root.display())));
        }
    }
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
