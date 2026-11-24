//! Envelope minting. One `Emitter` per engine owns the CV cursor
//! `(engine_session_id, seq)`: seq is gap-free within a session, and the
//! session rotates whenever a watch child starts, so any consumer that
//! sees a new session id knows to resync from disk.

use crate::ring::{EventRing, Resume};
use cv_contracts::watch::{Phase, WatchDiagnostic, WatchEvent};
use cv_core::diagnostic::{Diagnostic, Severity, Span};
use cv_core::engine::EventSink;
use cv_core::events::engine::{BuildPhase, EngineEvent};
use cv_core::events::{AppEvent, BuildId, Cursor, Envelope, EventId, SessionId, SnapshotEpoch};
use cv_core::id::WorkspaceId;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static SESSION_COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn new_session_id() -> SessionId {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let n = SESSION_COUNTER.fetch_add(1, Ordering::SeqCst);
    SessionId(format!("cv-{}-{nanos:x}-{n}", std::process::id()))
}

struct State {
    session: SessionId,
    seq: u64,
    epoch: u64,
}

pub struct Emitter {
    workspace: WorkspaceId,
    state: Mutex<State>,
    ring: EventRing,
}

impl Emitter {
    pub fn new(workspace: WorkspaceId, ring_capacity: usize) -> Self {
        Self {
            workspace,
            state: Mutex::new(State {
                session: new_session_id(),
                seq: 0,
                epoch: 0,
            }),
            ring: EventRing::new(ring_capacity),
        }
    }

    pub fn session(&self) -> SessionId {
        self.state.lock().unwrap().session.clone()
    }

    /// Start a new engine session (a new watch child): seq restarts at 0.
    pub fn rotate(&self) -> SessionId {
        let mut s = self.state.lock().unwrap();
        s.session = new_session_id();
        s.seq = 0;
        s.session.clone()
    }

    /// Mark a snapshot taken; later envelopes carry the new epoch.
    pub fn bump_epoch(&self) -> SnapshotEpoch {
        let mut s = self.state.lock().unwrap();
        s.epoch += 1;
        SnapshotEpoch(s.epoch)
    }

    pub fn cursor(&self) -> Cursor {
        let s = self.state.lock().unwrap();
        Cursor {
            session: s.session.clone(),
            seq: s.seq,
        }
    }

    pub fn ring(&self) -> &EventRing {
        &self.ring
    }

    pub fn resume(&self, seen: &Cursor) -> Resume {
        let s = self.state.lock().unwrap();
        self.ring.since(seen, &s.session)
    }

    /// Mint + record + deliver. When `only_in` is set and the session has
    /// rotated past it, the event belongs to a dead child: dropped.
    pub fn emit(
        &self,
        sink: &mut EventSink,
        only_in: Option<&SessionId>,
        build_id: Option<BuildId>,
        payload: EngineEvent,
    ) -> bool {
        let env = {
            let mut s = self.state.lock().unwrap();
            if only_in.is_some_and(|sid| *sid != s.session) {
                return false;
            }
            let seq = s.seq;
            s.seq += 1;
            let env = Envelope {
                workspace: self.workspace.clone(),
                snapshot_epoch: SnapshotEpoch(s.epoch),
                build_id,
                cursor: Cursor {
                    session: s.session.clone(),
                    seq,
                },
                event_id: EventId(format!("{}:{seq}", s.session.0)),
                payload: AppEvent::Engine(payload),
            };
            // Recorded under the lock so ring order == seq order.
            self.ring.record(&env);
            env
        };
        sink(env);
        true
    }
}

pub fn phase(p: Phase) -> BuildPhase {
    match p {
        Phase::Initial => BuildPhase::Initial,
        Phase::Rebuild => BuildPhase::Rebuild,
    }
}

fn severity(extra: &serde_json::Map<String, serde_json::Value>) -> Severity {
    match extra
        .get("severity")
        .or_else(|| extra.get("level"))
        .and_then(|v| v.as_str())
    {
        Some("warning") => Severity::Warning,
        Some("note") | Some("info") => Severity::Note,
        _ => Severity::Error,
    }
}

pub fn watch_diagnostic(d: &WatchDiagnostic, build: Option<&BuildId>) -> Diagnostic {
    let path = d.path.clone().or_else(|| {
        d.extra
            .get("sourcePath")
            .and_then(|v| v.as_str())
            .map(String::from)
    });
    Diagnostic {
        code: d.code.clone().unwrap_or_default(),
        severity: severity(&d.extra),
        span: path.map(|path| Span {
            path,
            start: 0,
            end: 0,
            line: d.line.unwrap_or(0),
            col: d
                .col
                .or_else(|| {
                    d.extra
                        .get("column")
                        .and_then(|v| v.as_u64())
                        .map(|c| c as u32)
                })
                .unwrap_or(0),
        }),
        message: d.message.clone().unwrap_or_default(),
        build_version: build.cloned(),
    }
}

/// Map one upstream watch record onto the engine vocabulary. `None` for
/// records with no engine-level meaning (build-started, watcher-started,
/// additive unknowns) — those are consumed, not lost: build-started
/// advances the build id.
pub fn map_watch_event(ev: &WatchEvent, build: Option<&BuildId>) -> Option<EngineEvent> {
    Some(match ev {
        WatchEvent::Hello {
            watch_events_schema,
            compiler,
        } => EngineEvent::WatchSessionStarted {
            schema: *watch_events_schema,
            compiler: compiler.clone(),
        },
        WatchEvent::BuildSucceeded {
            phase: p,
            pages_written,
            duration_ms,
            ..
        } => EngineEvent::BuildFinished {
            phase: phase(*p),
            pages_written: *pages_written,
            diagnostics: vec![],
            duration_ms: *duration_ms,
        },
        WatchEvent::BuildFailed {
            phase: p,
            diagnostics,
            recoverable,
            ..
        } => EngineEvent::BuildFailed {
            phase: phase(*p),
            diagnostics: diagnostics
                .iter()
                .map(|d| watch_diagnostic(d, build))
                .collect(),
            recoverable: *recoverable,
        },
        WatchEvent::BuildStarted { .. }
        | WatchEvent::WatcherStarted { .. }
        | WatchEvent::Unknown => {
            return None;
        }
    })
}
