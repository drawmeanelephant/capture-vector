//! `StubEngine` — replays captured fixtures through the `Engine` port.
//! `StubEngine ≡ ProcessEngine` conformance runs continuously once L1
//! lands; the suite is driven against this same API.

use cv_contracts::watch::{Phase, WatchDiagnostic, WatchEvent};
use cv_core::diagnostic::{Diagnostic, Severity, Span, StderrDiagnostic};
use cv_core::engine::{
    CancelToken, Engine, EngineCaps, EngineInfo, EventSink, ExitClass, ExportKind, Fingerprint,
    PageDraft, Verb, WatchHandle, WriteOutcome,
};
use cv_core::events::{
    BuildId, Cursor, Envelope, EventId, SessionId, SnapshotEpoch,
    engine::{BuildPhase, EngineEvent, VerbReport, WatchEndReason, WatchExit},
};
use cv_core::id::WorkspaceId;
use cv_core::rules::ArtifactSet;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Replays one corpus's captured watch session + verb results.
pub struct StubEngine {
    pub corpus: String,
    emitter: Arc<StubEmitter>,
}

/// Cursor semantics match `cv-engine`'s emitter: `(session, seq)` with a
/// gap-free seq per session; one-shot verbs before any watch run in
/// `stub-session`, and every watch start opens a fresh session
/// (`stub-session-2`, `-3`, …). One watch at a time, as `ProcessEngine`.
/// Event and build ids stay globally unique across sessions.
struct StubEmitter {
    minted: AtomicU64,
    watches: AtomicU64,
    live: AtomicBool,
    cursor: Mutex<(SessionId, u64)>,
    sent: Mutex<Vec<EngineEvent>>,
}

/// The `--out` the IR captures were taken with (`cv_engine::verbs::IR_DIR`).
pub const IR_CAPTURE_OUT: &str = ".cv/ir";

/// `cv_engine::EngineError::WatchAlreadyLive` as it crosses the port.
pub const WATCH_ALREADY_LIVE: &str = "a watch session is already live for this workspace";

impl StubEmitter {
    fn next_build(&self) -> BuildId {
        BuildId(format!("stub-build-{}", self.minted.load(Ordering::SeqCst)))
    }

    fn rotate(&self) -> SessionId {
        let n = self.watches.fetch_add(1, Ordering::SeqCst);
        let session = SessionId(format!("stub-session-{}", n + 2));
        *self.cursor.lock().unwrap_or_else(|e| e.into_inner()) = (session.clone(), 0);
        session
    }

    fn emit(&self, sink: &mut EventSink, payload: EngineEvent) {
        self.emit_in(sink, None, payload);
    }

    fn emit_caused(&self, sink: &mut EventSink, payload: EngineEvent, cause: Option<&EventId>) {
        self.emit_in_caused(sink, None, payload, cause);
    }

    /// With `only_in`, drop the event once the session has rotated away.
    fn emit_in(&self, sink: &mut EventSink, only_in: Option<&SessionId>, payload: EngineEvent) {
        self.emit_in_caused(sink, only_in, payload, None);
    }

    fn emit_in_caused(
        &self,
        sink: &mut EventSink,
        only_in: Option<&SessionId>,
        payload: EngineEvent,
        cause: Option<&EventId>,
    ) {
        let (cursor, id) = {
            let mut c = self.cursor.lock().unwrap_or_else(|e| e.into_inner());
            if only_in.is_some_and(|s| *s != c.0) {
                return;
            }
            let cursor = Cursor {
                session: c.0.clone(),
                seq: c.1,
            };
            c.1 += 1;
            self.sent
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(payload.clone());
            (cursor, self.minted.fetch_add(1, Ordering::SeqCst))
        };
        sink(Envelope {
            cause: cause.cloned(),
            workspace: WorkspaceId("stub".into()),
            snapshot_epoch: SnapshotEpoch(0),
            build_id: Some(BuildId(format!("stub-build-{id}"))),
            cursor,
            event_id: EventId(format!("stub-{id}")),
            payload: cv_core::events::AppEvent::Engine(payload),
        });
    }
}

/// The replayed session stays open (the recording never ends) until
/// stopped or dropped, which ends it `Killed` — as `ProcessWatch` does.
struct StubWatch {
    session: SessionId,
    sink: Option<EventSink>,
    emitter: Arc<StubEmitter>,
}

impl WatchHandle for StubWatch {
    fn session(&self) -> &SessionId {
        &self.session
    }
    fn stop(&mut self) {
        if let Some(mut sink) = self.sink.take() {
            self.emitter.emit_in(
                &mut sink,
                Some(&self.session),
                EngineEvent::WatchSessionEnded {
                    reason: WatchEndReason::Killed,
                    exit: Some(WatchExit::Killed),
                },
            );
            self.emitter.live.store(false, Ordering::SeqCst);
        }
    }
}

impl Drop for StubWatch {
    fn drop(&mut self) {
        self.stop();
    }
}

impl StubEngine {
    pub fn new(corpus: impl Into<String>) -> Self {
        Self {
            corpus: corpus.into(),
            emitter: Arc::new(StubEmitter {
                minted: AtomicU64::new(0),
                watches: AtomicU64::new(0),
                live: AtomicBool::new(false),
                cursor: Mutex::new((SessionId("stub-session".into()), 0)),
                sent: Mutex::new(Vec::new()),
            }),
        }
    }

    /// Events emitted so far — the conformance surface lanes assert on.
    pub fn sent(&self) -> Vec<EngineEvent> {
        self.emitter
            .sent
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn next_build(&self) -> BuildId {
        self.emitter.next_build()
    }

    fn emit(&self, sink: &mut EventSink, payload: EngineEvent) {
        self.emitter.emit(sink, payload)
    }

    /// Replay the corpus's recorded watch NDJSON session, mapping each
    /// upstream line onto engine events. Malformed lines surface as
    /// `MalformedLine`, never silently. Panics if a watch is already live
    /// (`Engine::watch` returns the error instead).
    pub fn replay_watch(&self, sink: EventSink) -> Box<dyn WatchHandle> {
        self.watch(sink).expect(WATCH_ALREADY_LIVE)
    }

    fn start_replay(&self, mut sink: EventSink) -> Box<dyn WatchHandle> {
        let session = self.emitter.rotate();
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
            session,
            sink: Some(sink),
            emitter: self.emitter.clone(),
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
                .or_else(|| {
                    extra("column")
                        .and_then(|v| v.as_u64())
                        .map(|c| u32::try_from(c).unwrap_or(u32::MAX))
                })
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
        WatchEvent::WatcherStarted { .. } | WatchEvent::Unknown { .. } => return None,
    })
}

impl StubEngine {
    /// `boris init --help` as captured (Boris prints it on stderr).
    pub fn init_help() -> String {
        let c = crate::corpus(crate::INIT);
        format!(
            "{}{}",
            c.file_str("help/stdout.txt"),
            c.file_str("help/stderr.txt")
        )
    }

    /// The captured report of the last `Verb::Init` outcome for
    /// `archetype` (`refuse`: a target that was not empty), stdout then
    /// stderr, as `ProcessEngine::run_verb` would hand it back.
    pub fn init_report(capture: &str) -> String {
        let c = crate::corpus(crate::INIT);
        format!(
            "{}{}",
            c.file_str(&format!("{capture}/stdout.txt")),
            c.file_str(&format!("{capture}/stderr.txt"))
        )
    }
}

/// The `verbs/<corpus>/` capture directory a reported verb replays.
pub fn verb_capture_key(verb: &Verb) -> Option<String> {
    Some(match verb {
        Verb::Build => "build".into(),
        Verb::Validate => "validate".into(),
        Verb::Check => "check".into(),
        Verb::Plan => "plan".into(),
        Verb::Graph => "graph".into(),
        Verb::Impact { id } => format!("impact/{id}"),
        Verb::Export { kind } => format!(
            "export-{}",
            match kind {
                ExportKind::Out => "out",
                ExportKind::Rag => "rag",
                ExportKind::Context => "context",
                ExportKind::Llms => "llms",
                ExportKind::Rss => "rss",
                ExportKind::Sitemap => "sitemap",
            }
        ),
        _ => return None,
    })
}

/// The one-shot build in watch vocabulary, as `ProcessEngine` reports
/// it: diagnostics come from captured stderr.
fn one_shot_build(exit: ExitClass, stdout: &str, stderr: &str, build: &BuildId) -> EngineEvent {
    let diagnostics: Vec<Diagnostic> = stderr
        .lines()
        .filter_map(StderrDiagnostic::parse)
        .map(|d| Diagnostic {
            build_version: Some(build.clone()),
            ..Diagnostic::from(&d)
        })
        .collect();
    if exit == ExitClass::Ok {
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
    }
}

/// Replay `boris init <dir> --type <archetype>`: a target that is not
/// empty gets the captured refusal; otherwise the captured starter tree is
/// written into `dir` (used as given — the stub has no workspace).
fn init(dir: &std::path::Path, archetype: &str) -> cv_core::Result<ExitClass> {
    let c = crate::corpus(crate::INIT);
    let occupied = dir.exists()
        && (!dir.is_dir() || std::fs::read_dir(dir).is_ok_and(|mut d| d.next().is_some()));
    let capture = if occupied { "refuse" } else { archetype };
    if c.pinned_under(&format!("{capture}/")).is_empty() || capture == "help" {
        return Err(cv_core::Error::Engine(format!(
            "stub engine has no capture for init --type {archetype}"
        )));
    }
    let code: i32 = c
        .file_str(&format!("{capture}/exit-code.txt"))
        .trim()
        .parse()
        .unwrap_or(3);
    let exit = ExitClass::from_code(code);
    if !occupied && exit == ExitClass::Ok {
        let tree = format!("{archetype}/tree/");
        for rel in c.pinned_under(&tree) {
            let dst = dir.join(&rel[tree.len()..]);
            let io = |e: std::io::Error| cv_core::Error::Engine(format!("{}: {e}", dst.display()));
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent).map_err(io)?;
            }
            std::fs::write(&dst, c.file(&rel)).map_err(io)?;
        }
    }
    Ok(exit)
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
        if self.emitter.live.swap(true, Ordering::SeqCst) {
            return Err(cv_core::Error::Engine(WATCH_ALREADY_LIVE.into()));
        }
        Ok(self.start_replay(sink))
    }

    fn run(&self, cmd: &Verb, sink: EventSink) -> cv_core::Result<ExitClass> {
        self.run_caused(cmd, sink, None)
    }

    fn run_caused(
        &self,
        cmd: &Verb,
        mut sink: EventSink,
        cause: Option<&EventId>,
    ) -> cv_core::Result<ExitClass> {
        if let Verb::Init { dir, archetype } = cmd {
            let exit = init(std::path::Path::new(dir), archetype)?;
            self.emitter.emit_caused(
                &mut sink,
                EngineEvent::VerbFinished {
                    verb: cmd.name(),
                    exit,
                },
                cause,
            );
            return Ok(exit);
        }
        // Replay the corpus's captured exit code for the verb.
        let (c, dir, is_build) = match cmd {
            Verb::Build => (crate::corpus(&self.corpus), "build/", true),
            Verb::Validate => (crate::corpus(&self.corpus), "validate/", false),
            Verb::Check => (crate::corpus(&self.corpus), "check/", false),
            // Captured with the output dir the live session uses; the
            // input only names the root the capture already built.
            Verb::Ir { out, .. } if out == IR_CAPTURE_OUT => {
                (crate::ir_capture(&self.corpus), "", true)
            }
            // No captured verb: the oracle must not replay a build's
            // bytes for a verb it never saw — it can't answer.
            other => {
                return Err(cv_core::Error::Engine(format!(
                    "stub engine has no capture for verb {other:?}"
                )));
            }
        };
        let code: i32 = c
            .file_str(&format!("{dir}exit-code.txt"))
            .trim()
            .parse()
            .unwrap_or(3);
        let exit = ExitClass::from_code(code);
        if is_build {
            let ev = one_shot_build(
                exit,
                &c.file_str(&format!("{dir}stdout.txt")),
                &c.file_str(&format!("{dir}stderr.txt")),
                &self.next_build(),
            );
            self.emitter.emit_caused(&mut sink, ev, cause);
        }
        self.emitter.emit_caused(
            &mut sink,
            EngineEvent::VerbFinished {
                verb: cmd.name(),
                exit,
            },
            cause,
        );
        Ok(exit)
    }

    /// Replays `verbs/<corpus>/<key>/`: the argv it was captured with, its
    /// exit code and output. No wall clock: `duration_ms` is 0.
    fn run_reported(
        &self,
        cmd: &Verb,
        mut sink: EventSink,
        _cancel: &CancelToken,
    ) -> cv_core::Result<ExitClass> {
        let c = crate::corpus(crate::VERBS);
        let dir = verb_capture_key(cmd)
            .map(|k| format!("{}/{k}/", self.corpus))
            .filter(|d| !c.pinned_under(d).is_empty())
            .ok_or_else(|| {
                cv_core::Error::Engine(format!(
                    "stub engine has no {} capture of verb {cmd:?}",
                    self.corpus
                ))
            })?;
        let file = |f: &str| c.file_str(&format!("{dir}{f}"));
        let argv: Vec<String> = file("argv.txt").lines().map(String::from).collect();
        let code: i32 = file("exit-code.txt").trim().parse().unwrap_or(3);
        let exit = ExitClass::from_code(code);
        let (stdout, stderr) = (file("stdout.txt"), file("stderr.txt"));
        if argv.first().map(String::as_str) == Some("build") {
            let ev = one_shot_build(exit, &stdout, &stderr, &self.next_build());
            self.emit(&mut sink, ev);
        }
        self.emit(
            &mut sink,
            EngineEvent::VerbReported(VerbReport {
                verb: cmd.name(),
                argv,
                exit,
                code: Some(code),
                duration_ms: 0,
                stdout: VerbReport::bounded_stdout(&stdout),
                stderr,
            }),
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
