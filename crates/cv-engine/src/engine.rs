//! `ProcessEngine` — the `RawSubprocess` adapter behind the `Engine` port.

use crate::emit::Emitter;
use crate::error::{EngineError, Result};
use crate::exit::ExitOutcome;
use crate::fallback::{self, FallbackStart, build_event};
use crate::locate::{EngineIdentity, Locator, PINNED, Pin};
use crate::paths::{
    canonical_parts as path_canonical_parts, canonicalize_with_missing, normalize_component,
};
use crate::probe::{CapabilityReport, PROBE_TIMEOUT, WatchMode, probe};
use crate::proc::CancelToken;
use crate::ring::{DEFAULT_RING, Resume};
use crate::secrets::{SecretSource, SignOutput, nostr_sign};
use crate::snapshot::DiskTruth;
use crate::verbs::{IR_DIR, VerbOutput, argv, flag_value, run_argv, verb_name};
use crate::watch::WatchStart;
use cv_core::engine::{
    Engine, EngineCaps, EngineInfo, EventSink, ExitClass, Fingerprint, MoveOutcome, PageDraft,
    PageMove, RemoveOutcome, Verb, WatchHandle, WriteOutcome,
};
use cv_core::events::engine::{BuildPhase, EngineEvent, VerbReport};
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
    /// Workspace-relative content input the write path stages under.
    pub content_input: PathBuf,
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
            content_input: PathBuf::from("content"),
            watch_args: vec![],
            ring_capacity: DEFAULT_RING,
            handshake_timeout: Duration::from_secs(10),
            probe_timeout: PROBE_TIMEOUT,
            poll_interval: Duration::from_millis(500),
        }
    }
}

fn relative_parts(raw: &str, what: &str) -> Result<Vec<String>> {
    let path = Path::new(raw);
    if path.is_absolute() || raw.trim().is_empty() || raw.contains('\\') {
        return Err(EngineError::Refused(format!(
            "{what} {raw:?} must be a workspace-relative path"
        )));
    }
    let parts = path
        .components()
        .filter(|c| *c != std::path::Component::CurDir)
        .map(|c| match c {
            std::path::Component::Normal(n) => Ok(n.to_string_lossy().into_owned()),
            _ => Err(EngineError::Refused(format!(
                "{what} {raw:?} must be a workspace-relative path"
            ))),
        })
        .collect::<Result<Vec<_>>>()?;
    if parts.is_empty() {
        return Err(EngineError::Refused(format!(
            "{what} {raw:?} must name a workspace-relative child, not the workspace root"
        )));
    }
    Ok(parts)
}

fn overlaps(a: &[String], b: &[String]) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

fn owned_cv_output(parts: &[String]) -> bool {
    [
        [".cv", "publish"],
        [".cv", "ir"],
        [".cv", "ir.staging"],
        [".cv", "out"],
    ]
    .iter()
    .map(|lane| lane.map(str::to_string))
    .any(|lane| parts.starts_with(&lane))
}

fn output_cfg(cfg: &EngineConfig, input: Option<&str>) -> Result<EngineConfig> {
    let mut cfg = cfg.clone();
    if let Some(input) = input {
        let absolute = Path::new(input).is_absolute();
        if input.trim().is_empty()
            || input != input.trim()
            || (!absolute && input != "." && cv_store::paths::portable_relative(input).is_err())
        {
            return Err(EngineError::Refused(format!(
                "content input {input:?} is not a usable workspace content authority"
            )));
        }
        cfg.content_input = input.into();
    }
    Ok(cfg)
}

fn path_from_parts(root: &Path, parts: &[String]) -> PathBuf {
    parts
        .iter()
        .fold(root.to_path_buf(), |p, part| p.join(part))
}

fn partition_parts(path: &Path, what: &str) -> Result<Vec<String>> {
    path_canonical_parts(path).map_err(|e| {
        EngineError::Refused(format!(
            "{what} cannot be canonicalized for partition validation: {e}"
        ))
    })
}

fn validate_output_path(cfg: &EngineConfig, raw: &str, what: &str) -> Result<()> {
    let raw_parts = relative_parts(raw, what)?;
    let lexical = raw_parts
        .iter()
        .map(|p| normalize_component(p))
        .collect::<Vec<_>>();
    let output_path = path_from_parts(&cfg.root, &raw_parts);
    let absolute = partition_parts(&output_path, what)?;
    let root_parts = partition_parts(&cfg.root, what)?;
    let Some(relative) = absolute.strip_prefix(root_parts.as_slice()) else {
        return Err(EngineError::Refused(format!(
            "{what} {raw:?} resolves outside the workspace"
        )));
    };

    let cv = [".cv".to_string()];
    let boris_meta = [".boris".to_string()];
    let profile = ["boris.json".to_string()];
    if overlaps(&lexical, &profile) || overlaps(relative, &profile) {
        return Err(EngineError::Refused(format!(
            "{what} {raw:?} overlaps workspace profile metadata"
        )));
    }
    let cv_violation = |parts: &[String]| parts.starts_with(&cv) && !owned_cv_output(parts);
    if cv_violation(&lexical) || cv_violation(relative) {
        return Err(EngineError::Refused(format!(
            "{what} {raw:?} overlaps Capture Vector metadata outside an owned output lane"
        )));
    }
    if (lexical.starts_with(&boris_meta) || relative.starts_with(&boris_meta)) && raw != ".boris" {
        return Err(EngineError::Refused(format!(
            "{what} {raw:?} overlaps Boris metadata"
        )));
    }

    let configured = &cfg.content_input;
    let input_path = if configured.is_absolute() {
        configured.clone()
    } else {
        cfg.root.join(configured)
    };
    let input_parts = partition_parts(&input_path, what)?;
    let input = configured.to_string_lossy();
    if input_parts == root_parts {
        let html = normalize_component(&cfg.html_dir);
        let derived = relative.starts_with(&cv)
            || relative.starts_with(&boris_meta)
            || relative.first().is_some_and(|p| *p == html);
        if !derived {
            return Err(EngineError::Refused(format!(
                "{what} {raw:?} overlaps the workspace content input, which is the workspace root"
            )));
        }
    } else if overlaps(&absolute, &input_parts) {
        return Err(EngineError::Refused(format!(
            "{what} {raw:?} overlaps the workspace content input {input:?}"
        )));
    }
    Ok(())
}

fn flag_value_at<'a>(args: &'a [String], i: usize, flag: &str) -> Option<&'a str> {
    let arg = &args[i];
    if arg == flag {
        args.get(i + 1).map(String::as_str)
    } else {
        arg.strip_prefix(flag)?.strip_prefix('=')
    }
}

fn validate_custom_outputs(cfg: &EngineConfig, args: &[String]) -> Result<()> {
    let effective = output_cfg(cfg, flag_value(args, "--input"))?;
    let cfg = &effective;
    const PATH_FLAGS: &[&str] = &[
        "--out",
        "--html-dir",
        "--dist",
        "--llms-path",
        "--rss-path",
        "--sitemap-path",
        "--rag-dir",
        "--context-dir",
        "--report",
        "--report-out",
        "--session-root",
        "--plan",
        "--bundle",
    ];
    for (i, _arg) in args.iter().enumerate() {
        for flag in PATH_FLAGS {
            if let Some(value) = flag_value_at(args, i, flag) {
                if *flag == "--out" && value.trim_start_matches("./") == ".boris" {
                    return Err(EngineError::Refused(format!(
                        "{flag} {value:?} overlaps Boris metadata"
                    )));
                }
                validate_output_path(cfg, value, flag)?;
            }
        }
        if let Some(value) = flag_value_at(args, i, "--target")
            && let Some((_, output)) = value.split_once('=')
        {
            validate_output_path(cfg, output, "--target")?;
        }
    }
    let implicit = [
        ("--rag", "rag"),
        ("--context", "context"),
        ("--llms", "llms.txt"),
        ("--rss", "rss.xml"),
        ("--no-rag", ".boris"),
    ];
    for (flag, output) in implicit {
        if args.iter().any(|a| a == flag) {
            validate_output_path(cfg, output, flag)?;
        }
    }
    Ok(())
}

fn validate_default_html_output(cfg: &EngineConfig, name: &str, args: &[String]) -> Result<()> {
    if !matches!(name, "build" | "watch") {
        return Ok(());
    }
    let effective = output_cfg(cfg, flag_value(args, "--input"))?;
    let cfg = &effective;
    let redirected = args.iter().any(|a| {
        [
            "--out",
            "--html-dir",
            "--target",
            "--rag",
            "--rag-dir",
            "--context",
            "--context-dir",
            "--llms",
            "--rss",
            "--no-rag",
        ]
        .iter()
        .any(|flag| a == flag || a.starts_with(&format!("{flag}=")))
    });
    if !redirected {
        validate_output_path(cfg, "dist", "default HTML output")?;
    }
    Ok(())
}

fn validate_watch_output_paths(cfg: &EngineConfig) -> Result<()> {
    validate_output_path(cfg, &cfg.html_dir, "watch HTML output")?;
    let verb = Verb::Custom {
        name: "watch".into(),
        args: cfg.watch_args.clone(),
    };
    validate_verb_output_paths(cfg, &verb)
}

fn validate_custom_init_targets(cfg: &EngineConfig, args: &[String]) -> Result<()> {
    // Boris init grammar is `[DIR] [--type NAME] [--quiet]`. Validate every
    // positional candidate rather than trusting position zero: the real CLI
    // also accepts the target after `--type NAME`. Unknown options are left
    // to Boris's usage error, while every candidate that could be DIR must
    // be partition-clean before spawn.
    let mut i = 0;
    let mut positional_only = false;
    let mut positional_count = 0;
    while i < args.len() {
        let arg = &args[i];
        if !positional_only && arg == "--" {
            positional_only = true;
        } else if !positional_only && arg == "--type" {
            i += 1;
        } else if !positional_only && (arg.starts_with("--type=") || arg == "--quiet") {
            // Flag or inline value only; no positional candidate here.
        } else if !positional_only && arg.starts_with('-') {
            // Unknown flag: Boris will refuse usage before writing. Its
            // possible value is still validated when it does not look like
            // a flag, which is deliberately conservative.
        } else {
            positional_count += 1;
            validate_output_path(cfg, arg, "init target")?;
        }
        i += 1;
    }
    // `[DIR]` is optional in Boris init; when it is absent the target is the
    // process working directory. Validate that implicit target too, except
    // for informational usage spellings that exit before writing.
    let informational = args
        .iter()
        .any(|a| matches!(a.as_str(), "-h" | "--help" | "-V" | "--version"));
    if positional_count == 0 && !informational {
        validate_output_path(cfg, ".", "implicit init target")?;
    }
    Ok(())
}

fn validate_verb_output_paths(cfg: &EngineConfig, verb: &Verb) -> Result<()> {
    if let Verb::Ir { out, input } = verb {
        let effective = output_cfg(cfg, input.as_deref())?;
        validate_output_path(&effective, out, "IR output")?;
    }
    if let Verb::Init { dir, .. } = verb {
        validate_output_path(cfg, dir, "init target")?;
    }
    if let Verb::Export { kind } = verb {
        use cv_core::engine::ExportKind;
        let output = match kind {
            ExportKind::Out => ".boris",
            ExportKind::Rag => "rag",
            ExportKind::Context => "context",
            ExportKind::Llms => "llms.txt",
            ExportKind::Rss => "rss.xml",
            ExportKind::Sitemap => "dist",
        };
        validate_output_path(cfg, output, "export output")?;
    }
    if let Verb::Custom { name, args } = verb {
        if name == "init" {
            validate_custom_init_targets(cfg, args)?;
        }
        validate_custom_outputs(cfg, args)?;
        validate_default_html_output(cfg, name, args)?;
    }
    if matches!(verb, Verb::Build) {
        validate_output_path(cfg, "dist", "build output")?;
    }
    Ok(())
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
        let report = probe(
            &identity.path,
            &cfg.root,
            &cfg.content_input,
            cfg.probe_timeout,
        );
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

    /// Ordinary content mutation participates in the same cross-process
    /// workspace critical section as adoption. Acquisition is bounded by
    /// cv-store's settling window, so contention is an honest retryable
    /// refusal rather than an indefinite wait or an unlocked write.
    fn workspace_mutation_lock(
        &self,
    ) -> Result<cv_store::workspace_mutation::WorkspaceMutationLock> {
        cv_store::workspace_mutation::lock_for_content_mutation(&self.cfg.root).map_err(|e| {
            if e.kind() == std::io::ErrorKind::WouldBlock {
                EngineError::Refused(
                    "workspace is busy with another mutation or migration adoption; try again when it completes"
                        .into(),
                )
            } else {
                EngineError::Io(e)
            }
        })
    }

    pub fn disk_truth(&self) -> DiskTruth {
        self.emitter.bump_epoch();
        crate::snapshot::read(&self.cfg.root, &self.cfg.html_dir)
    }

    /// Run a verb, returning verbatim stdout/stderr (Activity renders
    /// these as-is). Emits nothing.
    pub fn run_verb(&self, verb: &Verb, cancel: &CancelToken) -> Result<VerbOutput> {
        validate_verb_output_paths(&self.cfg, verb)?;
        let _content_mutation = if crate::verbs::may_mutate_content(verb) {
            Some(self.workspace_mutation_lock()?)
        } else {
            None
        };
        run_argv(&self.identity.path, &self.cfg.root, &argv(verb)?, cancel)
    }

    /// `Engine::run` with cancellation. Build verbs emit
    /// `BuildFinished`/`BuildFailed`; every completed verb ends with
    /// `VerbFinished`, while a failure before a Boris exit lands as
    /// `VerbFailed` so the stream never silently consumes cursor events.
    pub fn run_cancellable(
        &self,
        verb: &Verb,
        sink: EventSink,
        cancel: &CancelToken,
    ) -> Result<ExitClass> {
        self.run_cancellable_caused(verb, sink, cancel, None, false)
    }

    /// `Engine::run_reported` with cancellation: as `run_cancellable`,
    /// but the terminal envelope is `VerbReported`.
    pub fn run_reported_cancellable(
        &self,
        verb: &Verb,
        sink: EventSink,
        cancel: &CancelToken,
    ) -> Result<ExitClass> {
        self.run_cancellable_caused(verb, sink, cancel, None, true)
    }

    fn run_cancellable_caused(
        &self,
        verb: &Verb,
        mut sink: EventSink,
        cancel: &CancelToken,
        cause: Option<&cv_core::events::EventId>,
        report: bool,
    ) -> Result<ExitClass> {
        match self.run_cancellable_inner(verb, &mut sink, cancel, cause, report) {
            Ok(exit) => Ok(exit),
            Err(e) => {
                self.emitter.emit_caused(
                    &mut sink,
                    None,
                    None,
                    EngineEvent::VerbFailed {
                        verb: verb_name(verb),
                        error: e.to_string(),
                    },
                    cause,
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
        cause: Option<&cv_core::events::EventId>,
        report: bool,
    ) -> Result<ExitClass> {
        validate_verb_output_paths(&self.cfg, verb)?;
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
        let args = argv(verb)?;
        if args.first().map(String::as_str) == Some("build") {
            let ev = build_event(&out, BuildPhase::Initial, &build);
            self.emitter
                .emit_caused(sink, None, Some(build.clone()), ev, cause);
        }
        let terminal = if report {
            EngineEvent::VerbReported(VerbReport {
                verb: verb_name(verb),
                argv: args,
                exit,
                code: match out.outcome {
                    ExitOutcome::Exited { code } => Some(code),
                    _ => None,
                },
                duration_ms: out.duration_ms,
                stdout: VerbReport::bounded_stdout(&out.stdout),
                stderr: out.stderr,
            })
        } else {
            EngineEvent::VerbFinished {
                verb: verb_name(verb),
                exit,
            }
        };
        self.emitter
            .emit_caused(sink, None, Some(build), terminal, cause);
        Ok(exit)
    }

    pub fn sign_nostr(
        &self,
        plan: &Path,
        out: Option<&Path>,
        secrets: &dyn SecretSource,
        cancel: &CancelToken,
    ) -> Result<SignOutput> {
        if let Some(out) = out {
            let rel_owned;
            let rel = if out.is_absolute() {
                let root = canonicalize_with_missing(&self.cfg.root)?;
                let absolute = canonicalize_with_missing(out)?;
                rel_owned = absolute
                    .strip_prefix(&root)
                    .map_err(|_| {
                        EngineError::Refused(format!(
                            "sign output {:?} resolves outside the workspace",
                            out
                        ))
                    })?
                    .to_path_buf();
                &rel_owned
            } else {
                out
            };
            validate_output_path(&self.cfg, &rel.to_string_lossy(), "sign output")?;
        }
        let _content_mutation = self.workspace_mutation_lock()?;
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
        validate_watch_output_paths(&self.cfg)?;
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
        self.run_caused(cmd, sink, None)
    }

    fn run_caused(
        &self,
        cmd: &Verb,
        sink: EventSink,
        cause: Option<&cv_core::events::EventId>,
    ) -> cv_core::Result<ExitClass> {
        Ok(self.run_cancellable_caused(cmd, sink, &CancelToken::new(), cause, false)?)
    }

    fn run_reported(
        &self,
        cmd: &Verb,
        sink: EventSink,
        cancel: &cv_core::engine::CancelToken,
    ) -> cv_core::Result<ExitClass> {
        Ok(self.run_reported_cancellable(cmd, sink, cancel)?)
    }

    fn write_content(
        &self,
        draft: &PageDraft,
        base: Option<Fingerprint>,
    ) -> cv_core::Result<WriteOutcome> {
        if !self.report.caps.writes {
            return Err(EngineError::Refused("workspace is not writable (probe)".into()).into());
        }
        let _workspace_mutation = self.workspace_mutation_lock()?;
        Ok(crate::write::write_staged(
            &self.cfg.root,
            &draft.path,
            draft.body.as_bytes(),
            base.or(draft.base),
        )?)
    }

    fn remove_content(&self, path: &str, base: Fingerprint) -> cv_core::Result<RemoveOutcome> {
        if !self.report.caps.writes {
            return Err(EngineError::Refused("workspace is not writable (probe)".into()).into());
        }
        let _workspace_mutation = self.workspace_mutation_lock()?;
        Ok(crate::write::remove_staged(&self.cfg.root, path, base)?)
    }

    fn move_content(&self, mv: &PageMove) -> cv_core::Result<MoveOutcome> {
        if !self.report.caps.writes {
            return Err(EngineError::Refused("workspace is not writable (probe)".into()).into());
        }
        let _workspace_mutation = self.workspace_mutation_lock()?;
        Ok(crate::write::move_staged(&self.cfg.root, mv)?)
    }
}
