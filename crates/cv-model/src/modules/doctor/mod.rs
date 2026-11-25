//! Doctor — one place that says which engine this window runs, how it
//! was found, what the capability probe saw on this OS, how the watch
//! session is doing, and the state of the `.cv/` sidecar. Off the rail:
//! the `shell.doctor` command and the status bar's engine chip open it.
//!
//! Two inputs: [`Facts`] the composition root fills through a [`Feed`]
//! (engine lookup, probe, store, app build), and the committed event
//! stream, which [`WatchLog`] folds into the watch session's state. The
//! support bundle ([`bundle`]) is built from the same two.

pub mod bundle;
pub mod facts;
#[cfg(test)]
mod tests;

pub use facts::{
    AppBuild, EngineState, Facts, Feed, Located, Lock, LockHolder, Namespace, Pin, Probe, Stamp,
    Store,
};

use crate::command::{CommandSpec, Intent};
use crate::module::{
    Chip, InspectorModel, Module, ModuleSpec, NavItem, NavModel, ReadingLine, ReadingModel,
    Registry, RowModel, Slot, Tone,
};
use cv_core::diagnostic::{Diagnostic, Severity};
use cv_core::engine::{EngineSource, ExitClass};
use cv_core::events::engine::{BuildPhase, EngineEvent, WatchEndReason};
use cv_core::events::shell::{DegradedReason, ShellEvent};
use cv_core::events::{AppEvent, Cursor, Envelope};
use std::cell::RefCell;

pub const ID: &str = "doctor";
pub const COPY_BUNDLE: &str = "doctor.copy-bundle";

thread_local! {
    static FEED: RefCell<Option<Feed>> = const { RefCell::new(None) };
}

/// Composition-root hook: Doctor modules registered on this thread from
/// now on read `feed`.
pub fn install(feed: Feed) {
    FEED.with(|f| *f.borrow_mut() = Some(feed));
}

/// The feed the next `register` on this thread uses (installing an empty
/// one when none was).
pub fn installed() -> Feed {
    FEED.with(|f| f.borrow_mut().get_or_insert_with(Feed::default).clone())
}

pub fn register(r: &mut Registry) {
    r.register(Box::new(DoctorModule::new(installed())))
        .expect("doctor module id is unique");
}

/// The last non-recoverable build failure and what it reported.
#[derive(Debug, Clone, PartialEq)]
pub struct Fatal {
    pub phase: BuildPhase,
    pub diagnostics: Vec<Diagnostic>,
    /// The failure itself carried none; these came from the next failed
    /// build (the one-shot build run for diagnostics).
    pub from_followup: bool,
}

/// The watch session as the committed event stream tells it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WatchLog {
    pub session: Option<String>,
    pub schema: Option<u32>,
    pub compiler: Option<String>,
    /// Cursor of the newest engine envelope.
    pub last_cursor: Option<Cursor>,
    pub engine_events: u64,
    pub ended: Option<WatchEndReason>,
    pub last_verb: Option<(String, ExitClass)>,
    pub fatal: Option<Fatal>,
    /// A watch-dead disclosure arrived after the newest session start.
    pub dead: bool,
    pub degraded: Option<DegradedReason>,
}

impl WatchLog {
    pub fn ingest(&mut self, env: &Envelope) -> bool {
        match &env.payload {
            AppEvent::Engine(ev) => {
                self.last_cursor = Some(env.cursor.clone());
                self.engine_events += 1;
                match ev {
                    EngineEvent::WatchSessionStarted { schema, compiler } => {
                        self.session = Some(env.cursor.session.0.clone());
                        self.schema = Some(*schema);
                        self.compiler = Some(compiler.clone());
                        self.ended = None;
                        self.dead = false;
                    }
                    EngineEvent::WatchSessionEnded { reason } => self.ended = Some(reason.clone()),
                    EngineEvent::VerbFinished { verb, exit } => {
                        self.last_verb = Some((verb.clone(), *exit));
                    }
                    EngineEvent::BuildFailed {
                        phase,
                        diagnostics,
                        recoverable: false,
                    } => {
                        self.fatal = Some(Fatal {
                            phase: *phase,
                            diagnostics: diagnostics.clone(),
                            from_followup: false,
                        });
                    }
                    EngineEvent::BuildFailed { diagnostics, .. } => {
                        if let Some(f) = &mut self.fatal
                            && f.diagnostics.is_empty()
                            && !diagnostics.is_empty()
                        {
                            f.diagnostics = diagnostics.clone();
                            f.from_followup = true;
                        }
                    }
                    _ => {}
                }
                true
            }
            AppEvent::Shell(ShellEvent::Degraded { reason }) => {
                if matches!(
                    reason,
                    DegradedReason::WatchDead | DegradedReason::EngineBuildFailing
                ) {
                    self.dead = true;
                }
                self.degraded = Some(reason.clone());
                true
            }
            _ => false,
        }
    }

    pub fn live(&self) -> bool {
        self.session.is_some() && self.ended.is_none() && !self.dead
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    Engine,
    Probe,
    Watch,
    Store,
    App,
}

const SECTIONS: [(Section, &str, &str); 5] = [
    (Section::Engine, "engine", "Engine"),
    (Section::Probe, "probe", "Capabilities"),
    (Section::Watch, "watch", "Watch session"),
    (Section::Store, "store", ".cv/ store"),
    (Section::App, "app", "App build"),
];

fn section(nav: Option<&str>) -> Section {
    SECTIONS
        .iter()
        .find(|(_, k, _)| Some(*k) == nav)
        .map_or(Section::Engine, |(s, _, _)| *s)
}

/// One fact: a row in its section and a reading pane of detail.
#[derive(Debug, Clone, Default)]
struct Item {
    key: String,
    label: String,
    value: String,
    tone: Tone,
    badge: Option<Chip>,
    /// `value` is a path: the row shows it as the title, cut from the start.
    path: bool,
    lines: Vec<ReadingLine>,
    diagnostics: Vec<Diagnostic>,
}

impl Item {
    fn new(key: impl Into<String>, label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            value: value.into(),
            ..Self::default()
        }
    }
    fn tone(mut self, t: Tone) -> Self {
        self.tone = t;
        self
    }
    fn badge(mut self, text: &str, t: Tone) -> Self {
        self.badge = Some(Chip::new(text, t));
        self
    }
    fn path(mut self) -> Self {
        self.path = true;
        self
    }
    fn say(mut self, text: impl Into<String>) -> Self {
        self.lines.push(note(text));
        self
    }
    fn line(mut self, label: &str, text: impl Into<String>) -> Self {
        self.lines.push(ReadingLine {
            label: Some(label.into()),
            text: text.into(),
            tone: Tone::Normal,
            mono: true,
        });
        self
    }
    fn problem(&self) -> bool {
        let bad = |t: Tone| matches!(t, Tone::Warn | Tone::Error);
        bad(self.tone) || self.badge.as_ref().is_some_and(|c| bad(c.tone))
    }
    fn severity(&self) -> Tone {
        let badge = self.badge.as_ref().map_or(Tone::Normal, |c| c.tone);
        if self.tone == Tone::Error || badge == Tone::Error {
            Tone::Error
        } else {
            Tone::Warn
        }
    }
}

fn note(text: impl Into<String>) -> ReadingLine {
    ReadingLine {
        text: text.into(),
        tone: Tone::Dim,
        ..ReadingLine::default()
    }
}

pub fn source_label(s: &EngineSource) -> &'static str {
    match s {
        EngineSource::EnvOverride => "CV_BORIS_BIN",
        EngineSource::Bundled => "bundled",
        EngineSource::Path => "PATH",
        EngineSource::SiblingCheckout => "sibling checkout",
    }
}

fn source_text(s: &EngineSource) -> &'static str {
    match s {
        EngineSource::EnvOverride => "the CV_BORIS_BIN override",
        EngineSource::Bundled => "the engines/ directory bundled with the app",
        EngineSource::Path => "the first boris on PATH",
        EngineSource::SiblingCheckout => "a sibling boris checkout (CV_BORIS_SIBLING=1)",
    }
}

/// Whether the engine this window runs is the pinned one: `None` when no
/// gate has run yet.
pub fn pin_matches(f: &Facts) -> Option<bool> {
    match &f.engine {
        EngineState::Located(_) => Some(true),
        EngineState::Fixture { version, .. } => Some(*version == f.pin.version),
        EngineState::VersionMismatch { .. } | EngineState::BuildInfoInvalid { .. } => Some(false),
        _ => None,
    }
}

const FIX_ABSENT: &str = "Put boris/0.9.0 on PATH or set CV_BORIS_BIN to it, then reopen the \
     workspace. The workspace stays browsable from disk meanwhile.";
const FIX_MISMATCH: &str = "Build the pinned tag (v0.9.0) or point CV_BORIS_BIN at it, then \
     reopen the workspace. The pin is deliberate: the contract mirrors are keyed to it.";

fn engine_items(f: &Facts) -> Vec<Item> {
    let pin = format!("{} @ {}", f.pin.version, f.pin.vcs_revision);
    let status = match &f.engine {
        EngineState::NotStarted => Item::new("engine.status", "Not located yet", "no workspace")
            .tone(Tone::Dim)
            .badge("idle", Tone::Dim)
            .say("No workspace is open, so nothing has looked for Boris. Open one with Open workspace… (mod+shift+O) or launch with --project <dir>."),
        EngineState::Locating => Item::new("engine.status", "Locating Boris…", "gate and probe running")
            .tone(Tone::Dim)
            .badge("locating", Tone::Dim)
            .say("The boot thread runs --version and --build-info against the pin, then the capability probe."),
        EngineState::Fixture { corpus, version, .. } => {
            Item::new("engine.status", version.clone(), "fixture replay")
                .badge("fixture", Tone::Dim)
                .say(format!("The fixture demo replays fixtures/captures/{corpus}; no engine binary runs. Open a workspace to run a real engine."))
        }
        EngineState::Located(l) => Item::new("engine.status", l.version.clone(), source_label(&l.source))
            .tone(Tone::Ok)
            .badge("OK", Tone::Ok)
            .line("binary", l.path.clone())
            .line("pin", pin.clone())
            .say("The binary passed the --version and --build-info gate against the pin."),
        EngineState::NotFound => Item::new(
            "engine.status",
            "Boris not found",
            format!("{} searched", f.candidates.len()),
        )
        .tone(Tone::Error)
        .badge("absent", Tone::Error)
        .say(FIX_ABSENT),
        EngineState::OverrideMissing { path } => {
            Item::new("engine.status", "CV_BORIS_BIN is not an executable", path.clone())
                .tone(Tone::Error)
                .badge("absent", Tone::Error)
                .say("An explicit override never falls through to another engine: fix or unset CV_BORIS_BIN, then reopen the workspace.")
        }
        EngineState::VersionMismatch { source, path, found, found_vcs } => Item::new(
            "engine.status",
            found.clone(),
            source_label(source),
        )
        .tone(Tone::Warn)
        .badge("mismatch", Tone::Warn)
        .line("found", format!("{found} @ {}", found_vcs.as_deref().unwrap_or("no revision")))
        .line("want", pin.clone())
        .line("binary", path.clone())
        .say(FIX_MISMATCH),
        EngineState::BuildInfoInvalid { path, .. } => {
            Item::new("engine.status", "Invalid --build-info", "not a Boris record")
                .tone(Tone::Warn)
                .badge("mismatch", Tone::Warn)
                .line("want", pin.clone())
                .line("binary", path.clone())
                .say(FIX_MISMATCH)
        }
        EngineState::Failed { error } => Item::new("engine.status", "Engine gate failed", error.clone())
            .tone(Tone::Error)
            .badge("failed", Tone::Error)
            .line("error", error.clone()),
    };
    let mut out = vec![status];
    if let Some(p) = f.engine.path() {
        out.push(Item::new("engine.path", "Binary", p).path());
    }
    if let Some(s) = f.engine.source() {
        out.push(
            Item::new("engine.source", "Located via", source_label(s))
                .say(format!("Found through {}.", source_text(s)))
                .say("Search order: CV_BORIS_BIN, then the app's bundled engines/, then PATH, then a sibling checkout (only with CV_BORIS_SIBLING=1). The first that exists is gated; a mismatch never falls through to the next."),
        );
    }
    match &f.engine {
        EngineState::Located(l) => {
            out.push(Item::new("engine.version", "--version", l.version.clone()));
            let mut bi = Item::new("engine.build-info", "--build-info", l.vcs_revision.clone());
            bi.lines.push(ReadingLine {
                label: Some("stdout".into()),
                text: l.build_info_raw.clone(),
                tone: Tone::Normal,
                mono: true,
            });
            out.push(bi);
        }
        EngineState::VersionMismatch {
            found, found_vcs, ..
        } => {
            out.push(Item::new("engine.version", "--version", found.clone()).tone(Tone::Warn));
            out.push(Item::new(
                "engine.build-info",
                "--build-info",
                found_vcs.clone().unwrap_or_else(|| "no revision".into()),
            ));
        }
        EngineState::BuildInfoInvalid { raw, .. } => {
            out.push(
                Item::new("engine.build-info", "--build-info", "not a Boris record")
                    .tone(Tone::Warn)
                    .line("stdout", raw.clone()),
            );
        }
        EngineState::Fixture {
            version,
            vcs_revision,
            ..
        } => {
            out.push(Item::new(
                "engine.version",
                "Captured compiler",
                version.clone(),
            ));
            if let Some(v) = vcs_revision {
                out.push(Item::new(
                    "engine.build-info",
                    "Captured revision",
                    v.clone(),
                ));
            }
        }
        _ => {}
    }
    let mut pin_item = Item::new("engine.pin", "Pin", f.pin.version.clone())
        .line("revision", f.pin.vcs_revision.clone())
        .say("The one engine this build accepts, as fixtures/MANIFEST.toml records it.");
    pin_item = match pin_matches(f) {
        Some(true) => pin_item.badge("match", Tone::Ok),
        Some(false) => pin_item.badge("differs", Tone::Warn),
        None => pin_item.badge("unchecked", Tone::Dim),
    };
    out.push(pin_item);
    let mut search = Item::new(
        "engine.search",
        "Search order",
        match f.candidates.len() {
            0 => "not searched".to_string(),
            1 => "1 candidate".to_string(),
            n => format!("{n} candidates"),
        },
    );
    let used = f.engine.path();
    for (src, p) in &f.candidates {
        let hit = used == Some(p.as_str());
        search.lines.push(ReadingLine {
            label: Some(if hit { "used" } else { source_label(src) }.into()),
            text: p.clone(),
            tone: if hit { Tone::Ok } else { Tone::Dim },
            mono: true,
        });
    }
    out.push(search);
    out
}

fn yes_no(key: &str, label: &str, on: bool, off_tone: Tone, why: &str) -> Item {
    let i = Item::new(key, label, "");
    if on {
        i.badge("yes", Tone::Ok)
            .say("The probe saw this work on this machine.")
    } else {
        i.badge("no", off_tone).say(why)
    }
}

fn probe_items(f: &Facts) -> Vec<Item> {
    let Some(p) = &f.probe else {
        let why = match &f.engine {
            EngineState::Fixture { .. } => "Fixture demo: there is no binary to probe.",
            EngineState::Locating => "The probe runs once the engine passes its gate.",
            EngineState::NotStarted => "Open a workspace to locate and probe an engine.",
            _ => "The engine did not pass its gate, so nothing was probed.",
        };
        return vec![
            Item::new("probe.none", "Not probed", why)
                .tone(Tone::Dim)
                .say(why),
        ];
    };
    let mut evidence = Item::new(
        "probe.evidence",
        "Probe evidence",
        format!("{} observations", p.evidence.len()),
    )
    .say("What each probe observed, verbatim. The watch and validate probes run in a scratch directory, never in the workspace.");
    for e in &p.evidence {
        evidence = evidence.line("observed", e.clone());
    }
    vec![
        Item::new("probe.os", "OS", p.os.clone()),
        Item::new(
            "probe.watch-mode",
            "Watch mode",
            if p.native_watch {
                "watch-json"
            } else {
                "one-shot"
            },
        )
        .badge(
            if p.native_watch { "native" } else { "fallback" },
            if p.native_watch { Tone::Ok } else { Tone::Warn },
        )
        .say(if p.native_watch {
            "Boris streams rebuilds over watch --watch-json."
        } else {
            "A file watcher runs one-shot builds; there is no watch session."
        }),
        yes_no(
            "probe.watch",
            "watch",
            p.caps.watch,
            Tone::Warn,
            "The engine did not complete the watch handshake here; rebuilds run one-shot instead.",
        ),
        yes_no(
            "probe.serve",
            "serve",
            p.caps.serve,
            Tone::Dim,
            "watch --serve was refused; previews are not served.",
        ),
        yes_no(
            "probe.authoring",
            "authoring (validate)",
            p.caps.authoring,
            Tone::Warn,
            "boris validate did not run as expected; authoring checks are unavailable.",
        ),
        yes_no(
            "probe.writes",
            "content writes",
            p.caps.writes,
            Tone::Error,
            "The workspace is not writable: saves and promotions refuse.",
        ),
        evidence,
    ]
}

fn exit_label(e: ExitClass) -> &'static str {
    match e {
        ExitClass::Ok => "0 (ok)",
        ExitClass::ContentFailure => "1 (content)",
        ExitClass::UsageError => "2 (usage)",
        ExitClass::IoFailure => "3 (I/O)",
    }
}

fn diagnostic_text(d: &Diagnostic) -> String {
    match &d.span {
        Some(s) => format!("{}: {}:{}:{}: {}", d.code, s.path, s.line, s.col, d.message),
        None => format!("{}: {}", d.code, d.message),
    }
}

fn watch_items(f: &Facts, w: &WatchLog) -> Vec<Item> {
    let mode = match &f.probe {
        Some(p) if !p.native_watch => "one-shot",
        _ => "watch-json",
    };
    let state = if w.live() {
        Item::new("watch.state", "Live", mode)
            .tone(Tone::Ok)
            .badge("live", Tone::Ok)
    } else if w.dead {
        Item::new("watch.state", "Watch session died", "last snapshot")
            .tone(Tone::Warn)
            .badge("dead", Tone::Warn)
            .say("Run Restart watch (reopen workspace) to start a new session. If it dies again, the last failure below says why.")
    } else if let Some(r) = &w.ended {
        Item::new("watch.state", "Ended", format!("{r:?}"))
            .tone(Tone::Dim)
            .badge("ended", Tone::Dim)
    } else {
        Item::new("watch.state", "Never started", "no handshake")
            .tone(Tone::Dim)
            .badge("none", Tone::Dim)
            .say("A watch starts once the engine passes its gate.")
    };
    let mut out = vec![state];
    if let Some(s) = &w.session {
        out.push(Item::new("watch.session", "Session", s.clone()));
        out.push(
            Item::new(
                "watch.handshake",
                "Handshake",
                w.compiler.clone().unwrap_or_default(),
            )
            .line("schema", w.schema.unwrap_or_default().to_string()),
        );
    }
    if let Some(c) = &w.last_cursor {
        out.push(
            Item::new("watch.cursor", "Last cursor", format!("#{}", c.seq))
                .line("session", c.session.0.clone())
                .line("engine events", w.engine_events.to_string()),
        );
    }
    if let Some(r) = &w.ended {
        out.push(Item::new("watch.ended", "Session end", format!("{r:?}")));
    }
    if let Some((verb, exit)) = &w.last_verb {
        let t = if *exit == ExitClass::Ok {
            Tone::Normal
        } else {
            Tone::Warn
        };
        out.push(
            Item::new("watch.exit", "Last exit", exit_label(*exit))
                .tone(t)
                .line("verb", verb.clone()),
        );
    }
    let fatal = match &w.fatal {
        None => Item::new("watch.fatal", "Last fatal failure", "none").tone(Tone::Dim),
        Some(fl) => {
            let n = fl.diagnostics.len();
            let mut i = Item::new(
                "watch.fatal",
                "Last fatal failure",
                format!("{n} diagnostic{}", if n == 1 { "" } else { "s" }),
            )
            .line("phase", format!("{:?} build", fl.phase).to_lowercase())
            .tone(Tone::Error)
            .badge("fatal", Tone::Error);
            if fl.from_followup {
                i = i.say("The failure carried no diagnostics; these come from the one-shot build that followed it.");
            } else if n == 0 {
                i = i.say("Boris reported the failure without diagnostics. Activity has the raw build output.");
            }
            for d in &fl.diagnostics {
                let tone = match d.severity {
                    Severity::Error => Tone::Error,
                    Severity::Warning => Tone::Warn,
                    Severity::Note => Tone::Dim,
                };
                i.lines.push(ReadingLine {
                    label: Some(format!("{:?}", d.severity).to_lowercase()),
                    text: diagnostic_text(d),
                    tone,
                    mono: true,
                });
            }
            i.diagnostics = fl.diagnostics.clone();
            i
        }
    };
    out.push(fatal);
    out
}

fn stamp_badge(ns: &Namespace) -> (String, Chip, Tone) {
    let s = ns.supported;
    match &ns.stamp {
        Stamp::Absent => (
            "not stamped".into(),
            Chip::new("fresh", Tone::Dim),
            Tone::Normal,
        ),
        Stamp::Unstamped => (
            "v0".into(),
            Chip::new("unstamped", Tone::Warn),
            Tone::Normal,
        ),
        Stamp::Version(v) if *v > s => (
            format!("v{v} > v{s}"),
            Chip::new("too new", Tone::Error),
            Tone::Error,
        ),
        Stamp::Version(v) if *v < s => (
            format!("v{v} → v{s}"),
            Chip::new("older", Tone::Warn),
            Tone::Normal,
        ),
        Stamp::Version(v) => (
            format!("schema v{v}"),
            Chip::new("current", Tone::Ok),
            Tone::Normal,
        ),
        Stamp::Unreadable(e) => (e.clone(), Chip::new("unreadable", Tone::Error), Tone::Error),
    }
}

fn store_items(f: &Facts) -> Vec<Item> {
    let Some(root) = &f.store.root else {
        return vec![
            Item::new("store.none", "No workspace open", "nothing kept in .cv/")
                .tone(Tone::Dim)
                .say("The fixture demo reads the checkout's captures and keeps no sidecar state."),
        ];
    };
    let mut out = vec![Item::new("store.root", "Workspace", root.clone()).path()];
    for ns in &f.store.namespaces {
        let (value, chip, tone) = stamp_badge(ns);
        let mut i = Item::new(format!("store.ns.{}", ns.name), ns.dir.clone(), value)
            .tone(tone)
            .line("stamp", format!("{}/schema.json", ns.dir))
            .line("reads", format!("up to v{}", ns.supported));
        i.badge = Some(chip);
        if let Some(e) = &ns.error {
            i = i
                .tone(Tone::Error)
                .badge("error", Tone::Error)
                .line("open error", e.clone());
        }
        out.push(i);
        if let Some(l) = &ns.lock {
            let lock = Item::new(format!("store.lock.{}", ns.name), l.path.clone(), "");
            out.push(match l.holder {
                LockHolder::ThisWindow => Item {
                    value: "this window".into(),
                    ..lock
                },
                LockHolder::OtherProcess => Item {
                    value: "another process".into(),
                    ..lock
                }
                .tone(Tone::Error)
                .badge("locked", Tone::Error)
                .say("Another Capture Vector has this workspace open. Close it (or end a stuck one), then reopen the workspace."),
            });
        }
        if let Some(ix) = &ns.index {
            out.push(
                Item::new(format!("store.index.{}", ns.name), format!("{} index", ns.name), ix.clone())
                    .path()
                    .say(format!(
                        "Rebuildable: the JSONL under {} is the truth. The index rebuilds on open whenever it is missing, corrupt or stale; to force a rebuild, quit, delete this file, and reopen the workspace.",
                        ns.dir
                    )),
            );
        }
    }
    out
}

fn app_items(f: &Facts) -> Vec<Item> {
    let a = &f.app;
    vec![
        Item::new("app.version", "Capture Vector", a.version.clone()),
        if a.vcs_revision.is_empty() {
            Item::new("app.vcs", "Commit", "unstamped")
                .say("A plain cargo build carries no commit; release builds stamp one.")
        } else {
            Item::new("app.vcs", "Commit", a.vcs_revision.clone())
        },
        Item::new("app.target", "Target", a.target.clone()).line("profile", a.profile.clone()),
        Item::new("app.toolchain", "Rust", a.toolchain.clone())
            .line("pinned by", "rust-toolchain.toml"),
        Item::new("app.logging", "Logging", a.logging.clone())
            .say("CV_LOG=<level> logs JSONL to stderr; CV_LOG_FILE=<path> writes it to a file instead."),
        Item::new("app.bundle", "Support bundle", "palette")
            .say("Run Copy Doctor support bundle from the command palette (mod+K).")
            .say("Copies a JSON report of every section here to the clipboard, for an issue or a support thread.")
            .say("Left out: page bodies, absolute paths of the engine and its search order (they come from PATH and CV_BORIS_BIN), environment values, and any field whose name looks like a secret. The workspace root and home directory are replaced by placeholders."),
    ]
}

pub struct DoctorModule {
    feed: Feed,
    watch: WatchLog,
}

impl DoctorModule {
    pub fn new(feed: Feed) -> Self {
        Self {
            feed,
            watch: WatchLog::default(),
        }
    }

    pub fn watch(&self) -> &WatchLog {
        &self.watch
    }

    fn items(&self, s: Section) -> Vec<Item> {
        let f = self.feed.get();
        match s {
            Section::Engine => engine_items(&f),
            Section::Probe => probe_items(&f),
            Section::Watch => watch_items(&f, &self.watch),
            Section::Store => store_items(&f),
            Section::App => app_items(&f),
        }
    }

    fn find(&self, key: &str) -> Option<Item> {
        let prefix = key.split('.').next()?;
        let (s, _, _) = SECTIONS.iter().find(|(_, k, _)| *k == prefix)?;
        self.items(*s).into_iter().find(|i| i.key == key)
    }
}

impl Module for DoctorModule {
    fn spec(&self) -> ModuleSpec {
        ModuleSpec {
            id: ID,
            title: "Doctor",
            slot: Slot::Utility,
        }
    }

    fn ingest(&mut self, env: &Envelope) -> bool {
        self.watch.ingest(env)
    }

    fn nav(&self) -> NavModel {
        NavModel {
            items: SECTIONS
                .iter()
                .map(|(s, key, label)| {
                    let problems = self.items(*s).iter().filter(|i| i.problem()).count();
                    NavItem {
                        key: (*key).into(),
                        label: (*label).into(),
                        count: (problems > 0).then_some(problems as u64),
                    }
                })
                .collect(),
        }
    }

    fn list_len(&self, nav: Option<&str>) -> usize {
        self.items(section(nav)).len()
    }

    fn row(&self, nav: Option<&str>, index: usize) -> RowModel {
        let Some(i) = self.items(section(nav)).into_iter().nth(index) else {
            return RowModel::default();
        };
        let rail = i.problem().then(|| i.severity());
        let (title, detail) = if i.path {
            (i.value, i.label)
        } else {
            (i.label, i.value)
        };
        RowModel {
            key: i.key,
            title,
            detail,
            badge: i.badge,
            tone: i.tone,
            rail,
            path: i.path,
        }
    }

    fn index_of_key(&self, nav: Option<&str>, key: &str) -> Option<usize> {
        self.items(section(nav)).iter().position(|i| i.key == key)
    }

    fn reading(&self, key: &str) -> ReadingModel {
        let Some(i) = self.find(key) else {
            return ReadingModel::default();
        };
        let crumb = SECTIONS
            .iter()
            .find(|(_, k, _)| key.starts_with(&format!("{k}.")))
            .map(|(_, _, l)| (*l).to_string());
        let mut lines = Vec::new();
        if !i.value.is_empty() {
            lines.push(ReadingLine {
                label: Some(if i.path { "path" } else { "value" }.into()),
                text: i.value.clone(),
                tone: i.tone,
                mono: i.path,
            });
        }
        lines.extend(i.lines);
        ReadingModel {
            title: i.label,
            lines,
            breadcrumbs: ["Doctor".to_string()].into_iter().chain(crumb).collect(),
            markdown: None,
        }
    }

    fn inspector(&self, key: &str) -> InspectorModel {
        let Some(i) = self.find(key) else {
            return InspectorModel::default();
        };
        InspectorModel {
            badges: i.badge.into_iter().collect(),
            fields: vec![("fact".into(), i.key)],
            diagnostics: i.diagnostics,
            lists: Vec::new(),
        }
    }

    fn commands(&self) -> Vec<CommandSpec> {
        vec![CommandSpec::new(COPY_BUNDLE, "Copy Doctor support bundle")]
    }

    fn run(&mut self, command: &str) -> Vec<Intent> {
        if command != COPY_BUNDLE {
            return Vec::new();
        }
        let text = bundle::render(&self.feed.get(), &self.watch);
        vec![
            Intent::Status(format!(
                "Support bundle copied ({} bytes). Page bodies, engine paths and environment values are left out.",
                text.len()
            )),
            Intent::Copy(text),
        ]
    }

    fn contains(&self, key: &str) -> bool {
        self.find(key).is_some()
    }
}
