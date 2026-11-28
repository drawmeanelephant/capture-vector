//! Engine verbs — Boris's own verbs (build, validate, check, plan,
//! graph, impact, exports) as palette commands. The module only asks:
//! the composition root's [`EnginePort`] runs each verb through the
//! workspace's `EngineScheduler` on a worker, so writer-seat rules hold,
//! and the run comes back as a `VerbReported` envelope that Activity
//! files as a run noun. Long verbs (build, exports) hold a cancellable
//! progress dialog; short ones report on the status line. Off the rail:
//! an impact result opens here as a reading view.

use crate::command::{CommandSpec, Intent};
use crate::dialog::{DialogAnswer, DialogKind, DialogSpec, Progress};
use crate::module::{
    InspectorModel, Link, Module, ModuleSpec, NavItem, NavModel, ProjectSnapshot, ReadingLine,
    ReadingModel, Registry, RowModel, Slot, Tone,
};
use cv_core::engine::{CancelToken, ExitClass, ExportKind, Verb};
use cv_core::events::engine::{EngineEvent, VerbReport};
use cv_core::events::{AppEvent, Envelope};
use std::cell::RefCell;

pub const ID: &str = "engine";
/// The progress dialog a long verb holds open.
pub const RUNNING: &str = "engine.running";
/// Why verbs wait before a composition root installed a port.
pub const NO_ENGINE: &str = "no Boris engine is attached";

const IMPACT: &str = "impact";

/// One palette run: the verb and the token that cancels it. The token is
/// shared verbatim with cv-engine, so Cancel kills the child process tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerbRun {
    pub verb: Verb,
    pub cancel: CancelToken,
}

/// IO boundary. `start` returns once the run is queued; the run ends with
/// one `VerbReported` or `VerbFailed` envelope in the workbench inbox.
pub trait EnginePort {
    /// Why no verb can run now (`None`: they can).
    fn unavailable(&self) -> Option<String>;
    fn start(&mut self, run: VerbRun) -> Result<(), String>;
}

struct Unattached;

impl EnginePort for Unattached {
    fn unavailable(&self) -> Option<String> {
        Some(NO_ENGINE.into())
    }
    fn start(&mut self, _: VerbRun) -> Result<(), String> {
        Err(NO_ENGINE.into())
    }
}

thread_local! {
    static PORT: RefCell<Box<dyn EnginePort>> = RefCell::new(Box::new(Unattached));
}

/// Composition-root hook: engine modules on this thread run verbs
/// through `port` from now on, including ones already registered (a
/// fixture corpus or workspace swaps the engine, not the registry).
pub fn install(port: Box<dyn EnginePort>) {
    PORT.with(|p| *p.borrow_mut() = port);
}

/// No engine: every verb is listed disabled with [`NO_ENGINE`].
pub fn detach() {
    install(Box::new(Unattached));
}

fn unavailable() -> Option<String> {
    PORT.with(|p| p.borrow().unavailable())
}

fn start(run: VerbRun) -> Result<(), String> {
    PORT.with(|p| p.borrow_mut().start(run))
}

pub fn register(r: &mut Registry) {
    r.register(Box::new(EngineModule::default()))
        .expect("engine module id is unique");
}

/// The palette verbs: `(command suffix, title, verb)`.
fn verbs() -> Vec<(&'static str, &'static str, Option<Verb>)> {
    let export = |kind| Some(Verb::Export { kind });
    vec![
        ("build", "Boris: Build site", Some(Verb::Build)),
        ("validate", "Boris: Validate", Some(Verb::Validate)),
        (
            "check",
            "Boris: Check (findings to Audit)",
            Some(Verb::Check),
        ),
        ("plan", "Boris: Plan publication", Some(Verb::Plan)),
        ("graph", "Boris: Graph", Some(Verb::Graph)),
        // The page comes from the selection when the command runs.
        (IMPACT, "Boris: Impact of selected page", None),
        (
            "export.out",
            "Boris: Export IR (.boris)",
            export(ExportKind::Out),
        ),
        (
            "export.rag",
            "Boris: Export RAG catalog",
            export(ExportKind::Rag),
        ),
        (
            "export.context",
            "Boris: Export context bundle",
            export(ExportKind::Context),
        ),
        (
            "export.llms",
            "Boris: Export llms.txt",
            export(ExportKind::Llms),
        ),
        (
            "export.rss",
            "Boris: Export RSS feed",
            export(ExportKind::Rss),
        ),
        (
            "export.sitemap",
            "Boris: Export sitemap",
            export(ExportKind::Sitemap),
        ),
    ]
}

/// Verbs that hold the progress dialog: they write output trees.
fn is_long(verb: &Verb) -> bool {
    matches!(verb, Verb::Build | Verb::Export { .. })
}

/// One impact run's answer, as Boris reported it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Impact {
    target: String,
    /// `(type, value)` pairs, in Boris's order.
    impacted: Vec<(String, String)>,
    command: String,
}

impl Impact {
    fn key(&self) -> String {
        format!("{IMPACT}:{}", self.target)
    }
}

/// `boris impact <id> --format json` prints its report on stderr.
fn parse_impact(target: &str, r: &VerbReport) -> Option<Impact> {
    let v: serde_json::Value = serde_json::from_str(&r.stderr).ok()?;
    let impacted = v
        .get("impact")?
        .as_array()?
        .iter()
        .map(|n| {
            let s = |k: &str| n.get(k).and_then(|x| x.as_str()).unwrap_or_default();
            (s("type").to_string(), s("value").to_string())
        })
        .collect();
    Some(Impact {
        target: target.into(),
        impacted,
        command: r.command(),
    })
}

#[derive(Default)]
pub struct EngineModule {
    /// `(source path, graph id)` of every page in the newest snapshot.
    pages: Vec<(String, String)>,
    focus: Option<String>,
    /// Started runs not yet answered, oldest first: `(verb name, run)`.
    running: Vec<(String, VerbRun)>,
    /// Newest first, one per target page.
    impacts: Vec<Impact>,
    /// Intents owed after an ingest (close progress, status, reveal).
    owed: Vec<Intent>,
}

impl EngineModule {
    fn page_id(&self, key: &str) -> Option<&str> {
        self.pages
            .iter()
            .find(|(path, id)| path == key || id == key)
            .map(|(_, id)| id.as_str())
    }

    fn long_running(&self) -> Option<&str> {
        self.running
            .iter()
            .find(|(_, r)| is_long(&r.verb))
            .map(|(n, _)| n.as_str())
    }

    fn launch(&mut self, verb: Verb) -> Vec<Intent> {
        let name = verb.name();
        let label = product_label(&name);
        let progress = progress_copy(&name);
        let long = is_long(&verb);
        if long && let Some(other) = self.long_running() {
            let other = lowercase_first(product_label(other));
            return vec![Intent::outcome(format!(
                "{other} is still running; wait for it or cancel it first"
            ))];
        }
        let run = VerbRun {
            verb,
            cancel: CancelToken::new(),
        };
        if let Err(e) = start(run.clone()) {
            return vec![Intent::outcome(format!("{label} could not start: {e}"))];
        }
        self.running.push((name, run));
        let mut out = vec![Intent::progress(progress.clone())];
        if long {
            out.push(Intent::dialog(
                DialogSpec::new(
                    RUNNING,
                    label,
                    DialogKind::Progress(Progress {
                        label: progress,
                        permille: None,
                    }),
                )
                .body("The run is recorded in Activity when it finishes.")
                .cancel("Cancel"),
            ));
        }
        out
    }

    /// The oldest started run of `verb`, removed; `None` for runs this
    /// module did not start.
    fn finish(&mut self, verb: &str) -> Option<VerbRun> {
        let i = self.running.iter().position(|(n, _)| n == verb)?;
        let (_, run) = self.running.remove(i);
        if is_long(&run.verb) {
            self.owed.push(Intent::CloseDialog(RUNNING.into()));
        }
        Some(run)
    }

    fn reported(&mut self, r: &VerbReport) -> bool {
        let Some(run) = self.finish(&r.verb) else {
            return false;
        };
        let label = product_label(&r.verb);
        let status = match r.exit {
            ExitClass::Ok => format!("{label} {}", success_word(&r.verb)),
            _ => format!("{label} failed · see Activity"),
        };
        self.owed.push(Intent::outcome(status));
        if let (Verb::Impact { id }, ExitClass::Ok) = (&run.verb, r.exit)
            && let Some(impact) = parse_impact(id, r)
        {
            let key = impact.key();
            self.impacts.retain(|i| i.target != impact.target);
            self.impacts.insert(0, impact);
            self.owed.push(Intent::Reveal {
                module: ID.into(),
                key,
                nav: None,
            });
        }
        true
    }

    fn failed(&mut self, verb: &str, _error: &str) -> bool {
        let Some(run) = self.finish(verb) else {
            return false;
        };
        let label = product_label(verb);
        let status = if run.cancel.is_cancelled() {
            format!("{label} cancelled")
        } else {
            format!("{label} failed · see Activity")
        };
        self.owed.push(Intent::outcome(status));
        true
    }

    fn impact(&self, key: &str) -> Option<&Impact> {
        self.impacts.iter().find(|i| i.key() == key)
    }
}

fn lowercase_first(mut s: String) -> String {
    if let Some(first) = s.get(..1) {
        s.replace_range(..1, &first.to_lowercase());
    }
    s
}

fn progress_copy(verb: &str) -> String {
    match verb {
        "build" => "Building the site…".into(),
        "validate" => "Validating the site…".into(),
        "check" => "Checking the site…".into(),
        "plan" => "Planning publication…".into(),
        "graph" => "Reading the page graph…".into(),
        "impact" => "Reviewing impact…".into(),
        "export:out" => "Exporting IR…".into(),
        "export:rag" => "Exporting the RAG catalog…".into(),
        "export:context" => "Exporting the context bundle…".into(),
        "export:llms" => "Exporting llms.txt…".into(),
        "export:rss" => "Exporting the RSS feed…".into(),
        "export:sitemap" => "Exporting the sitemap…".into(),
        other => format!("Running Boris {other}…"),
    }
}

fn success_word(verb: &str) -> &'static str {
    match verb {
        "validate" => "passed",
        "check" => "completed",
        "plan" => "is ready",
        _ => "finished",
    }
}

/// The user-facing product result name for one Boris verb. Activity keeps
/// the exact argv, exit class and duration; the status names what the user
/// asked Capture Vector to do.
pub fn product_label(verb: &str) -> String {
    match verb {
        "build" => "Site build".into(),
        "validate" => "Validation".into(),
        "check" => "Check".into(),
        "plan" => "Publication plan".into(),
        "graph" => "Graph".into(),
        "impact" => "Impact review".into(),
        "export:out" => "IR export".into(),
        "export:rag" => "RAG catalog export".into(),
        "export:context" => "Context bundle export".into(),
        "export:llms" => "llms.txt export".into(),
        "export:rss" => "RSS feed export".into(),
        "export:sitemap" => "Sitemap export".into(),
        other => format!("Boris {other}"),
    }
}

impl Module for EngineModule {
    fn spec(&self) -> ModuleSpec {
        ModuleSpec {
            id: ID,
            title: "Engine",
            slot: Slot::Utility,
        }
    }

    fn ingest(&mut self, env: &Envelope) -> bool {
        match &env.payload {
            AppEvent::Engine(EngineEvent::VerbReported(r)) => self.reported(r),
            AppEvent::Engine(EngineEvent::VerbFailed { verb, error }) => self.failed(verb, error),
            _ => false,
        }
    }

    fn take_owed(&mut self) -> Vec<Intent> {
        std::mem::take(&mut self.owed)
    }

    fn load(&mut self, snap: &ProjectSnapshot) -> bool {
        self.pages = snap
            .pages
            .iter()
            .map(|p| (p.path.clone(), p.id.clone()))
            .collect();
        false
    }

    fn nav(&self) -> NavModel {
        NavModel {
            items: vec![NavItem {
                key: IMPACT.into(),
                label: "Impact".into(),
                count: Some(self.impacts.len() as u64),
            }],
        }
    }

    fn list_len(&self, _nav: Option<&str>) -> usize {
        self.impacts.len()
    }

    fn row(&self, _nav: Option<&str>, i: usize) -> RowModel {
        let Some(m) = self.impacts.get(i) else {
            return RowModel::default();
        };
        RowModel {
            key: m.key(),
            title: format!("Impact of {}", m.target),
            detail: format!("{} impacted · {}", m.impacted.len(), m.command),
            ..Default::default()
        }
    }

    fn reading(&self, key: &str) -> ReadingModel {
        let Some(m) = self.impact(key) else {
            return ReadingModel {
                title: key.into(),
                ..Default::default()
            };
        };
        let line = |label: &str, text: String, mono| ReadingLine {
            label: Some(label.into()),
            text,
            tone: Tone::Normal,
            mono,
        };
        let mut lines = vec![
            line("page", m.target.clone(), true),
            line("command", m.command.clone(), true),
            line(
                "impacted",
                match m.impacted.len() {
                    0 => "nothing else depends on this page".into(),
                    n => format!("{n} transitively, as Boris reports them"),
                },
                false,
            ),
        ];
        lines.extend(
            m.impacted
                .iter()
                .map(|(kind, value)| line(kind, value.clone(), true)),
        );
        ReadingModel {
            title: format!("Impact of {}", m.target),
            lines,
            breadcrumbs: vec!["Impact".into(), m.target.clone()],
            markdown: None,
        }
    }

    fn inspector(&self, key: &str) -> InspectorModel {
        let Some(m) = self.impact(key) else {
            return InspectorModel::default();
        };
        InspectorModel {
            fields: vec![
                ("page".into(), m.target.clone()),
                ("command".into(), m.command.clone()),
            ],
            lists: vec![(
                "Impacted".into(),
                m.impacted.iter().map(|(k, v)| format!("{k} {v}")).collect(),
            )],
            ..Default::default()
        }
    }

    fn contains(&self, key: &str) -> bool {
        self.impact(key).is_some()
    }

    fn link(&self, key: &str) -> Option<Link> {
        let m = self.impact(key)?;
        let path = self.pages.iter().find(|(_, id)| *id == m.target)?.0.clone();
        Some(Link {
            module: super::pages::ID,
            key: path,
            label: "Open page".into(),
        })
    }

    fn select(&mut self, key: Option<&str>) {
        self.focus = key.map(str::to_string);
    }

    fn commands(&self) -> Vec<CommandSpec> {
        let why = unavailable();
        verbs()
            .into_iter()
            .map(|(id, title, _)| {
                CommandSpec::new(format!("{ID}.{id}"), title).disabled(why.clone())
            })
            .collect()
    }

    fn run(&mut self, command: &str) -> Vec<Intent> {
        let Some(suffix) = command.strip_prefix("engine.") else {
            return vec![];
        };
        if let Some(why) = unavailable() {
            return vec![Intent::outcome(format!(
                "Boris verbs are unavailable: {why}"
            ))];
        }
        if suffix == IMPACT {
            let id = self.focus.as_deref().and_then(|k| self.page_id(k));
            return match id {
                Some(id) => self.launch(Verb::Impact { id: id.into() }),
                None => vec![Intent::outcome(
                    "Select a page in Pages first; impact runs on the selected page.",
                )],
            };
        }
        match verbs().into_iter().find(|(id, _, _)| *id == suffix) {
            Some((_, _, Some(verb))) => self.launch(verb),
            _ => vec![Intent::outcome(format!("unknown command {command}"))],
        }
    }

    fn answer(&mut self, command: &str, answer: &DialogAnswer) -> Vec<Intent> {
        if command != RUNNING || answer.action.is_some() {
            return vec![];
        }
        let Some((name, run)) = self.running.iter().find(|(_, r)| is_long(&r.verb)) else {
            return vec![];
        };
        run.cancel.cancel();
        let label = lowercase_first(product_label(name));
        vec![Intent::progress(format!("Cancelling {label}…"))]
    }
}

#[cfg(test)]
mod tests;
