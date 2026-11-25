//! `ProcessEngine` — the `RawSubprocess` adapter behind the `Engine` port.

use crate::emit::Emitter;
use crate::error::{EngineError, Result};
use crate::fallback::{self, FallbackStart, build_event};
use crate::locate::{EngineIdentity, Locator, PINNED, Pin};
use crate::probe::{CapabilityReport, PROBE_TIMEOUT, WatchMode, probe};
use crate::proc::CancelToken;
use crate::ring::{DEFAULT_RING, Resume};
use crate::secrets::{SecretSource, SignOutput, nostr_sign};
use crate::snapshot::DiskTruth;
use crate::verbs::{IR_DIR, VerbOutput, argv, flag_value, run_argv, verb_name};
use crate::watch::WatchStart;
use cv_core::engine::{
    Engine, EngineCaps, EngineInfo, EventSink, ExitClass, Fingerprint, PageDraft, Verb,
    WatchHandle, WriteOutcome,
};
use cv_core::events::engine::{BuildPhase, EngineEvent};
use cv_core::events::{BuildId, Cursor};
use cv_core::id::WorkspaceId;
use cv_core::rules::ArtifactSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub root: PathBuf,
    pub workspace: WorkspaceId,
    pub locator: Locator,
    pub pin: Pin,
    /// Workspace-relative HTML output tree (Boris default `dist`).
    pub html_dir: String,
    /// Extra args for `boris watch --watch-json` (e.g. `--serve`).
    pub watch_args: Vec<String>,
    pub ring_capacity: usize,
    pub handshake_timeout: Duration,
    pub probe_timeout: Duration,
    pub poll_interval: Duration,
}

impl EngineConfig {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self {
            workspace: WorkspaceId(root.to_string_lossy().into_owned()),
            locator: Locator::from_env(&root),
            root,
            pin: PINNED,
            html_dir: "dist".into(),
            watch_args: vec![],
            ring_capacity: DEFAULT_RING,
            handshake_timeout: Duration::from_secs(10),
            probe_timeout: PROBE_TIMEOUT,
            poll_interval: Duration::from_millis(500),
        }
    }
}

pub struct ProcessEngine {
    cfg: EngineConfig,
    identity: EngineIdentity,
    report: CapabilityReport,
    emitter: Arc<Emitter>,
    watch_live: Arc<AtomicBool>,
    runs: AtomicU64,
}

impl ProcessEngine {
    /// Locate + gate + probe. Fails on any mismatch; never runs unpinned.
    pub fn start(cfg: EngineConfig) -> Result<Self> {
        let identity = cfg.locator.locate(&cfg.root, cfg.pin)?;
        let report = probe(&identity.path, &cfg.root, cfg.probe_timeout);
        Ok(Self::with_report(cfg, identity, report))
    }

    /// Skip probing with a known report (tests, or a cached probe).
    pub fn with_report(
        cfg: EngineConfig,
        identity: EngineIdentity,
        report: CapabilityReport,
    ) -> Self {
        Self {
            emitter: Arc::new(Emitter::new(cfg.workspace.clone(), cfg.ring_capacity)),
            cfg,
            identity,
            report,
            watch_live: Arc::new(AtomicBool::new(false)),
            runs: AtomicU64::new(0),
        }
    }

    pub fn identity(&self) -> &EngineIdentity {
        &self.identity
    }
    pub fn report(&self) -> &CapabilityReport {
        &self.report
    }
    pub fn root(&self) -> &Path {
        &self.cfg.root
    }
    /// The HTML output tree the watch writes: the scheduler's seat.
    pub fn html_dir(&self) -> &str {
        &self.cfg.html_dir
    }

    /// The IR projection the live session runs beside its watch: Boris
    /// IR mode into [`IR_DIR`], over the content root the watch was given.
    pub fn ir_projection(&self) -> Verb {
        Verb::Ir {
            out: IR_DIR.into(),
            input: flag_value(&self.cfg.watch_args, "--input").map(String::from),
        }
    }
    /// The next envelope will carry `(session, seq)` = this cursor.
    pub fn cursor(&self) -> Cursor {
        self.emitter.cursor()
    }
    /// Replay from the bounded ring, or `Resync`.
    pub fn resume(&self, seen: &Cursor) -> Resume {
        self.emitter.resume(seen)
    }
    pub fn ring_capacity(&self) -> usize {
        self.emitter.ring().capacity()
    }

    pub fn disk_truth(&self) -> DiskTruth {
        self.emitter.bump_epoch();
        crate::snapshot::read(&self.cfg.root, &self.cfg.html_dir)
    }

    /// Run a verb, returning verbatim stdout/stderr (Activity renders
    /// these as-is). Emits nothing.
    pub fn run_verb(&self, verb: &Verb, cancel: &CancelToken) -> Result<VerbOutput> {
        run_argv(&self.identity.path, &self.cfg.root, &argv(verb)?, cancel)
    }

    /// `Engine::run` with cancellation. Build verbs emit
    /// `BuildFinished`/`BuildFailed`; every completed verb ends with
    /// `VerbFinished`, while a failure before a Boris exit lands as
    /// `VerbFailed` so the stream never silently consumes cursor events.
    pub fn run_cancellable(
        &self,
        verb: &Verb,
        mut sink: EventSink,
        cancel: &CancelToken,
    ) -> Result<ExitClass> {
        match self.run_cancellable_inner(verb, &mut sink, cancel) {
            Ok(exit) => Ok(exit),
            Err(e) => {
                self.emitter.emit(
                    &mut sink,
                    None,
                    None,
                    EngineEvent::VerbFailed {
                        verb: verb_name(verb),
                        error: e.to_string(),
                    },
                );
                Err(e)
            }
        }
    }

    fn run_cancellable_inner(
        &self,
        verb: &Verb,
        sink: &mut EventSink,
        cancel: &CancelToken,
    ) -> Result<ExitClass> {
        let staged = crate::ir::staged_verb(verb)?;
        if staged.is_some() {
            crate::ir::prepare(&self.cfg.root)?;
        }
        let out = self.run_verb(staged.as_ref().unwrap_or(verb), cancel)?;
        let exit = out
            .outcome
            .class()
            .ok_or(EngineError::Abnormal(out.outcome))?;
        if let Verb::Ir { out, .. } = verb {
            if exit == ExitClass::Ok {
                if let Err(e) = crate::ir::publish(&self.cfg.root, out) {
                    crate::ir::cleanup(&self.cfg.root);
                    return Err(e);
                }
            } else {
                crate::ir::cleanup(&self.cfg.root);
            }
        }
        let n = self.runs.fetch_add(1, Ordering::SeqCst) + 1;
        let build = BuildId(format!("{}:r{n}", self.emitter.session().0));
        let is_build = matches!(argv(verb)?.first().map(String::as_str), Some("build"));
        if is_build {
            let ev = build_event(&out, BuildPhase::Initial, &build);
            self.emitter.emit(sink, None, Some(build.clone()), ev);
        }
        self.emitter.emit(
            sink,
            None,
            Some(build),
            EngineEvent::VerbFinished {
                verb: verb_name(verb),
                exit,
            },
        );
        Ok(exit)
    }

    pub fn sign_nostr(
        &self,
        plan: &Path,
        out: Option<&Path>,
        secrets: &dyn SecretSource,
        cancel: &CancelToken,
    ) -> Result<SignOutput> {
        nostr_sign(
            &self.identity.path,
            &self.cfg.root,
            plan,
            out,
            secrets,
            cancel,
        )
    }

    pub fn watch_live(&self) -> bool {
        self.watch_live.load(Ordering::SeqCst)
    }

    pub fn start_watch(&self, sink: EventSink) -> Result<Box<dyn WatchHandle>> {
        if self.watch_live.swap(true, Ordering::SeqCst) {
            return Err(EngineError::WatchAlreadyLive);
        }
        let r: Result<Box<dyn WatchHandle>> = match self.report.watch_mode {
            WatchMode::Native => crate::watch::start(
                WatchStart {
                    bin: &self.identity.path,
                    root: &self.cfg.root,
                    args: &self.cfg.watch_args,
                    handshake_timeout: self.cfg.handshake_timeout,
                    live: self.watch_live.clone(),
                },
                self.emitter.clone(),
                sink,
            )
            .map(|w| Box::new(w) as Box<dyn WatchHandle>),
            WatchMode::OneShotFallback => fallback::start(
                FallbackStart {
                    bin: self.identity.path.clone(),
                    root: self.cfg.root.clone(),
                    html_dir: self.cfg.html_dir.clone(),
                    build_args: vec![],
                    interval: self.cfg.poll_interval,
                    compiler: self.identity.version.clone(),
                    live: self.watch_live.clone(),
                },
                self.emitter.clone(),
                sink,
            )
            .map(|w| Box::new(w) as Box<dyn WatchHandle>),
        };
        if r.is_err() {
            self.watch_live.store(false, Ordering::SeqCst);
        }
        r
    }
}

impl Engine for ProcessEngine {
    fn locate(&self) -> cv_core::Result<EngineInfo> {
        Ok(EngineInfo {
            version: self.identity.version.clone(),
            vcs_revision: Some(self.identity.vcs_revision.clone()),
            caps: self.report.caps,
        })
    }

    fn capabilities(&self) -> EngineCaps {
        self.report.caps
    }

    fn snapshot(&self) -> cv_core::Result<ArtifactSet> {
        Ok(self.disk_truth().artifacts)
    }

    fn watch(&self, sink: EventSink) -> cv_core::Result<Box<dyn WatchHandle>> {
        Ok(self.start_watch(sink)?)
    }

    fn run(&self, cmd: &Verb, sink: EventSink) -> cv_core::Result<ExitClass> {
        Ok(self.run_cancellable(cmd, sink, &CancelToken::new())?)
    }

    fn write_content(
        &self,
        draft: &PageDraft,
        base: Option<Fingerprint>,
    ) -> cv_core::Result<WriteOutcome> {
        if !self.report.caps.writes {
            return Err(EngineError::Refused("workspace is not writable (probe)".into()).into());
        }
        Ok(crate::write::write_staged(
            &self.cfg.root,
            &draft.path,
            draft.body.as_bytes(),
            base.or(draft.base),
        )?)
    }
}
