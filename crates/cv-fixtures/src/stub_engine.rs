//! `StubEngine` — replays captured fixtures through the `Engine` port.
//! `StubEngine ≡ ProcessEngine` conformance runs continuously once L1
//! lands; the suite is driven against this same API.

use cv_contracts::watch::{Phase, WatchEvent};
use cv_core::engine::{
    Engine, EngineCaps, EngineInfo, EventSink, ExitClass, Fingerprint, PageDraft, Verb,
    WatchHandle, WriteOutcome,
};
use cv_core::events::{
    BuildId, Cursor, Envelope, EventId, SessionId, SnapshotEpoch,
    engine::{BuildPhase, EngineEvent},
};
use cv_core::id::WorkspaceId;
use cv_core::rules::ArtifactSet;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Replays one corpus's captured watch session + verb results.
pub struct StubEngine {
    pub corpus: String,
    seq: AtomicU64,
    sent: Mutex<Vec<EngineEvent>>,
}

struct StubWatch {
    session: SessionId,
    stopped: AtomicBool,
}

impl WatchHandle for StubWatch {
    fn session(&self) -> &SessionId {
        &self.session
    }
    fn stop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
    }
}

impl StubEngine {
    pub fn new(corpus: impl Into<String>) -> Self {
        Self {
            corpus: corpus.into(),
            seq: AtomicU64::new(0),
            sent: Mutex::new(Vec::new()),
        }
    }

    /// Events emitted so far — the conformance surface lanes assert on.
    pub fn sent(&self) -> Vec<EngineEvent> {
        self.sent.lock().unwrap().clone()
    }

    fn emit(&self, sink: &mut EventSink, payload: EngineEvent) {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst);
        self.sent.lock().unwrap().push(payload.clone());
        sink(Envelope {
            workspace: WorkspaceId("stub".into()),
            snapshot_epoch: SnapshotEpoch(0),
            build_id: Some(BuildId(format!("stub-build-{seq}"))),
            cursor: Cursor {
                session: SessionId("stub-session".into()),
                seq,
            },
            event_id: EventId(format!("stub-{seq}")),
            payload: cv_core::events::AppEvent::Engine(payload),
        });
    }

    /// Replay the corpus's recorded watch NDJSON session, mapping each
    /// upstream line onto engine events. Malformed lines surface as
    /// `MalformedLine`, never silently.
    pub fn replay_watch(&self, mut sink: EventSink) -> Box<dyn WatchHandle> {
        let ndjson = crate::corpus(&self.corpus).file_str("watch/session.ndjson");
        for line in ndjson.lines() {
            match cv_contracts::watch::decode_line(line) {
                Ok(Some(ev)) => {
                    let Some(payload) = map_event(ev) else {
                        continue;
                    };
                    self.emit(&mut sink, payload);
                }
                Ok(None) => continue,
                Err(_) => self.emit(
                    &mut sink,
                    EngineEvent::MalformedLine {
                        raw: line.to_string(),
                    },
                ),
            }
        }
        Box::new(StubWatch {
            session: SessionId("stub-session".into()),
            stopped: AtomicBool::new(false),
        })
    }
}

fn map_event(ev: WatchEvent) -> Option<EngineEvent> {
    let phase = |p: Phase| match p {
        Phase::Initial => BuildPhase::Initial,
        Phase::Rebuild => BuildPhase::Rebuild,
    };
    Some(match ev {
        WatchEvent::Hello {
            watch_events_schema,
            compiler,
        } => EngineEvent::WatchSessionStarted {
            schema: watch_events_schema,
            compiler,
        },
        // A start carries no diagnostics; the finished event lands next.
        WatchEvent::BuildStarted { .. } => return None,
        WatchEvent::BuildSucceeded {
            phase: p,
            pages_written,
            duration_ms,
            ..
        } => EngineEvent::BuildFinished {
            phase: phase(p),
            pages_written,
            diagnostics: vec![],
            duration_ms,
        },
        WatchEvent::BuildFailed {
            phase: p,
            recoverable,
            ..
        } => EngineEvent::BuildFailed {
            phase: phase(p),
            diagnostics: vec![],
            recoverable,
        },
        WatchEvent::WatcherStarted { .. } | WatchEvent::Unknown => return None,
    })
}

impl Engine for StubEngine {
    fn locate(&self) -> cv_core::Result<EngineInfo> {
        Ok(EngineInfo {
            version: "boris/0.9.0".into(),
            vcs_revision: Some("66efcfb".into()),
            caps: EngineCaps {
                watch: true,
                serve: true,
                authoring: true,
                writes: true,
            },
        })
    }

    fn capabilities(&self) -> EngineCaps {
        EngineCaps {
            watch: true,
            serve: true,
            authoring: true,
            writes: true,
        }
    }

    fn snapshot(&self) -> cv_core::Result<ArtifactSet> {
        Ok(ArtifactSet {
            ir: None,
            html: None,
            exports: vec![],
            deploy: None,
            missing: vec![],
        })
    }

    fn watch(&self, sink: EventSink) -> cv_core::Result<Box<dyn WatchHandle>> {
        Ok(self.replay_watch(sink))
    }

    fn run(&self, cmd: &Verb, mut sink: EventSink) -> cv_core::Result<ExitClass> {
        // Replay the corpus's captured exit code for the verb.
        let verb_dir = match cmd {
            Verb::Build => "build",
            Verb::Validate => "validate",
            Verb::Check => "check",
            _ => "build",
        };
        let code: i32 = crate::corpus(&self.corpus)
            .file_str(&format!("{verb_dir}/exit-code.txt"))
            .trim()
            .parse()
            .unwrap_or(3);
        let exit = ExitClass::from_code(code);
        self.emit(
            &mut sink,
            EngineEvent::VerbFinished {
                verb: format!("{cmd:?}"),
                exit,
            },
        );
        Ok(exit)
    }

    fn write_content(
        &self,
        _draft: &PageDraft,
        _base: Option<Fingerprint>,
    ) -> cv_core::Result<WriteOutcome> {
        Err(cv_core::Error::Engine("StubEngine does not write".into()))
    }
}
