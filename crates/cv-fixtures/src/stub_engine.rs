//! `StubEngine` — replays captured fixtures through the `Engine` port.
//! `StubEngine ≡ ProcessEngine` conformance runs continuously once L1
//! lands; the suite is driven against this same API.

use cv_contracts::watch::{Phase, WatchDiagnostic, WatchEvent};
use cv_core::diagnostic::{Diagnostic, Severity, Span, StderrDiagnostic};
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

    /// Build id the next emitted envelope carries.
    fn next_build(&self) -> BuildId {
        BuildId(format!("stub-build-{}", self.seq.load(Ordering::SeqCst)))
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
                    let Some(payload) = map_event(ev, &self.next_build()) else {
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

/// Same mapping as `cv-engine`'s `watch_diagnostic`: the stub must hand
/// lanes the diagnostics the process adapter would.
fn watch_diagnostic(d: &WatchDiagnostic, build: &BuildId) -> Diagnostic {
    let extra = |k: &str| d.extra.get(k);
    let path = d.path.clone().or_else(|| {
        extra("sourcePath")
            .and_then(|v| v.as_str())
            .map(String::from)
    });
    Diagnostic {
        code: d.code.clone().unwrap_or_default(),
        severity: match extra("severity")
            .or_else(|| extra("level"))
            .and_then(|v| v.as_str())
        {
            Some("warning") => Severity::Warning,
            Some("note") | Some("info") => Severity::Note,
            _ => Severity::Error,
        },
        span: path.map(|path| Span {
            path,
            start: 0,
            end: 0,
            line: d.line.unwrap_or(0),
            col: d
                .col
                .or_else(|| extra("column").and_then(|v| v.as_u64()).map(|c| c as u32))
                .unwrap_or(0),
        }),
        message: d.message.clone().unwrap_or_default(),
        build_version: Some(build.clone()),
    }
}

fn map_event(ev: WatchEvent, build: &BuildId) -> Option<EngineEvent> {
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
            diagnostics,
            recoverable,
            ..
        } => EngineEvent::BuildFailed {
            phase: phase(p),
            diagnostics: diagnostics
                .iter()
                .map(|d| watch_diagnostic(d, build))
                .collect(),
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
        // Build verbs report the one-shot build in watch vocabulary, as
        // `ProcessEngine` does: diagnostics come from captured stderr.
        if verb_dir == "build" {
            let c = crate::corpus(&self.corpus);
            let (stdout, stderr) = (
                c.file_str("build/stdout.txt"),
                c.file_str("build/stderr.txt"),
            );
            let build = self.next_build();
            let diagnostics: Vec<Diagnostic> = stderr
                .lines()
                .filter_map(StderrDiagnostic::parse)
                .map(|d| Diagnostic {
                    build_version: Some(build.clone()),
                    ..Diagnostic::from(&d)
                })
                .collect();
            let ev = if exit == ExitClass::Ok {
                EngineEvent::BuildFinished {
                    phase: BuildPhase::Initial,
                    pages_written: stdout
                        .lines()
                        .chain(stderr.lines())
                        .filter(|l| l.starts_with("  wrote ") && l.ends_with(".html"))
                        .count() as u64,
                    diagnostics: diagnostics
                        .into_iter()
                        .filter(|d| d.severity != Severity::Error)
                        .collect(),
                    duration_ms: 0,
                }
            } else {
                EngineEvent::BuildFailed {
                    phase: BuildPhase::Initial,
                    diagnostics,
                    recoverable: exit == ExitClass::ContentFailure,
                }
            };
            self.emit(&mut sink, ev);
        }
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
