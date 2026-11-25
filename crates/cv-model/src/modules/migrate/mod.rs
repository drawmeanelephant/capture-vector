//! Migrate — `boris-migration-lab` runs kept under `.cv/imports/`, the
//! reports they wrote, and the lab's review items. The first module in
//! the rail's Utilities group.
//!
//! Pure: the composition root reads the run directories, decodes the
//! lab's documents through the `cv-contracts` mirrors, and hands this
//! module an [`Imports`] through a [`MigratePort`]. Every disposition,
//! count and reason here is the lab's own; nothing is re-derived. Review
//! items are triaged (open, accepted, deferred) like Audit findings and
//! never block anything.

#[cfg(test)]
mod tests;

use crate::command::{CommandSpec, Intent};
use crate::module::{
    Chip, InspectorModel, Module, ModuleSpec, NavItem, NavModel, ReadingLine, ReadingModel,
    Registry, RowModel, Slot, Tone,
};
use cv_core::events::Envelope;
use cv_core::id::{AccountId, AttrValue, NounAttrRow, NounId, NounKind, StableId, noun_kind};
use std::cell::RefCell;
use std::collections::BTreeMap;

pub const ID: &str = "migrate";
pub const NEW: &str = "migrate.new";
pub const REFRESH: &str = "migrate.refresh";
pub const ADOPT: &str = "migrate.adopt";
pub const ACCEPT: &str = "migrate.review.accept";
pub const DEFER: &str = "migrate.review.defer";
pub const REOPEN: &str = "migrate.review.reopen";

const NEW_UNAVAILABLE: &str = "starting an import from Capture Vector is not in this build yet";
const ADOPT_UNAVAILABLE: &str = "adopting a run into a workspace is not in this build yet";

/// Which lab binary this window found, as the `--version` gate saw it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum LabState {
    /// No workspace is open, so nothing looked for the lab.
    #[default]
    NoWorkspace,
    /// Runs come from the checked-in captures (`StubLab`).
    Fixture {
        tool_id: String,
    },
    Located {
        tool_id: String,
        source: String,
        path: String,
    },
    Absent {
        searched: Vec<String>,
    },
    OverrideMissing {
        path: String,
    },
    Mismatch {
        found: String,
        want: String,
        path: String,
    },
    Failed {
        error: String,
    },
}

/// A mode family: one mailbox of runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Family {
    pub key: String,
    pub label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
    Running,
    Ok,
    Refused,
    Failed,
    Adopted,
}

impl RunStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Ok => "ok",
            Self::Refused => "refused",
            Self::Failed => "failed",
            Self::Adopted => "adopted",
        }
    }
    fn tone(self) -> Tone {
        match self {
            Self::Running => Tone::Dim,
            Self::Ok | Self::Adopted => Tone::Ok,
            Self::Refused => Tone::Warn,
            Self::Failed => Tone::Error,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReviewState {
    #[default]
    Open,
    Accepted,
    Deferred,
}

impl ReviewState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Accepted => "accepted",
            Self::Deferred => "deferred",
        }
    }
}

/// One lab `human_review` / `unsupported` entry.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReviewItem {
    /// Stable within its run, path-free.
    pub id: String,
    /// The lab's class, verbatim (`human_review`, `unsupported`).
    pub class: String,
    /// The source path or entity the entry names.
    pub source: String,
    pub construct: String,
    pub reason: String,
    pub state: ReviewState,
}

/// One run directory under `.cv/imports/`, as read from disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportRun {
    pub id: String,
    pub mode: String,
    pub family: Option<String>,
    /// Display name of the input (its last path segment).
    pub source: String,
    pub status: RunStatus,
    pub started_ms: i64,
    /// `format` and `schema_version` of the run's main lab document.
    pub format: Option<(String, u32)>,
    /// `tool_version` the main document reports.
    pub tool_version: Option<String>,
    /// The lab binary's `--version` when the run started.
    pub lab: Option<String>,
    pub boris_pin: Option<String>,
    pub argv: Vec<String>,
    pub exit_code: Option<i32>,
    pub outcome: Option<String>,
    pub duration_ms: Option<u64>,
    /// The lab's `--out`.
    pub out: String,
    pub provenance: Option<String>,
    /// Count per disposition, in the order the lab reports them.
    pub counts: Vec<(String, u64)>,
    /// Other totals the main document states (pages emitted, …).
    pub totals: Vec<(String, u64)>,
    /// `(file name, body)` of the run's Markdown report.
    pub report: Option<(String, String)>,
    /// Why the run record or report could not be read.
    pub problem: Option<String>,
    /// Why the adapter refused the run before it started.
    pub refused: Option<String>,
    pub review: Vec<ReviewItem>,
}

impl ImportRun {
    pub fn new(id: impl Into<String>, status: RunStatus) -> Self {
        Self {
            id: id.into(),
            mode: String::new(),
            family: None,
            source: String::new(),
            status,
            started_ms: 0,
            format: None,
            tool_version: None,
            lab: None,
            boris_pin: None,
            argv: Vec::new(),
            exit_code: None,
            outcome: None,
            duration_ms: None,
            out: String::new(),
            provenance: None,
            counts: Vec::new(),
            totals: Vec::new(),
            report: None,
            problem: None,
            refused: None,
            review: Vec::new(),
        }
    }
}

/// Everything the module shows.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Imports {
    pub lab: LabState,
    pub families: Vec<Family>,
    pub runs: Vec<ImportRun>,
    /// `.cv/imports/` itself could not be read.
    pub error: Option<String>,
    /// Where review triage persists.
    pub store: String,
}

/// IO boundary: the composition root reads `.cv/imports/`.
pub trait MigratePort {
    fn load(&mut self) -> Imports;
    fn record(&mut self, run: &str, item: &str, to: ReviewState) -> Result<(), String>;
}

/// In-memory port: serves `imports`; transitions land in `records`.
#[derive(Default)]
pub struct MemoryPort {
    pub imports: Imports,
    pub records: Vec<(String, String, ReviewState)>,
}

impl MigratePort for MemoryPort {
    fn load(&mut self) -> Imports {
        let mut i = self.imports.clone();
        for (run, item, to) in &self.records {
            if let Some(it) = i
                .runs
                .iter_mut()
                .filter(|r| &r.id == run)
                .flat_map(|r| r.review.iter_mut())
                .find(|it| &it.id == item)
            {
                it.state = *to;
            }
        }
        i
    }
    fn record(&mut self, run: &str, item: &str, to: ReviewState) -> Result<(), String> {
        self.records.push((run.into(), item.into(), to));
        Ok(())
    }
}

thread_local! {
    static SEED: RefCell<Option<Box<dyn MigratePort>>> = const { RefCell::new(None) };
}

/// Composition-root hook: the next `register` on this thread uses `port`.
pub fn install(port: Box<dyn MigratePort>) {
    SEED.with(|s| *s.borrow_mut() = Some(port));
}

pub fn register(r: &mut Registry) {
    let port = SEED
        .with(|s| s.borrow_mut().take())
        .unwrap_or_else(|| Box::new(MemoryPort::default()));
    r.register(Box::new(MigrateModule::new(port)))
        .expect("migrate module id is unique");
}

const LAB_KEY: &str = "lab";
const NEW_KEY: &str = "new";

fn run_key(id: &str) -> String {
    format!("run:{id}")
}

fn item_key(run: &str, item: &str) -> String {
    format!("review:{run}:{item}")
}

enum Target<'a> {
    Lab,
    New,
    Run(&'a ImportRun),
    Item(&'a ImportRun, &'a ReviewItem),
}

pub struct MigrateModule {
    port: Box<dyn MigratePort>,
    imports: Imports,
    selected: Option<String>,
}

fn line(label: &str, text: impl Into<String>) -> ReadingLine {
    ReadingLine {
        label: Some(label.into()),
        text: text.into(),
        tone: Tone::Normal,
        mono: false,
    }
}

fn mono(label: &str, text: impl Into<String>) -> ReadingLine {
    ReadingLine {
        mono: true,
        ..line(label, text)
    }
}

fn note(text: impl Into<String>) -> ReadingLine {
    ReadingLine {
        text: text.into(),
        tone: Tone::Dim,
        ..ReadingLine::default()
    }
}

fn class_tone(class: &str) -> Tone {
    match class {
        "human_review" => Tone::Warn,
        _ => Tone::Dim,
    }
}

fn plural(n: u64, one: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {one}s")
    }
}

const FIX_LAB: &str = "Build the pinned boris-migration-lab (scripts/fetch-migration-lab.sh) and put it on PATH or point CV_MIGRATION_LAB_BIN at it, then reopen the workspace. Runs already under .cv/imports/ stay readable without it.";

impl MigrateModule {
    pub fn new(mut port: Box<dyn MigratePort>) -> Self {
        let imports = port.load();
        Self {
            port,
            imports,
            selected: None,
        }
    }

    pub fn imports(&self) -> &Imports {
        &self.imports
    }

    fn runs_sorted(&self) -> Vec<&ImportRun> {
        let mut v: Vec<&ImportRun> = self.imports.runs.iter().collect();
        v.sort_by(|a, b| b.started_ms.cmp(&a.started_ms).then(b.id.cmp(&a.id)));
        v
    }

    fn items(&self) -> Vec<(&ImportRun, &ReviewItem)> {
        let mut v: Vec<(&ImportRun, &ReviewItem)> = self
            .runs_sorted()
            .into_iter()
            .flat_map(|r| r.review.iter().map(move |i| (r, i)))
            .collect();
        let rank = |s: ReviewState| match s {
            ReviewState::Open => 0,
            ReviewState::Deferred => 1,
            ReviewState::Accepted => 2,
        };
        v.sort_by_key(|(_, i)| rank(i.state));
        v
    }

    fn open_items(&self) -> usize {
        self.imports
            .runs
            .iter()
            .flat_map(|r| &r.review)
            .filter(|i| i.state == ReviewState::Open)
            .count()
    }

    fn lab_problem(&self) -> bool {
        !matches!(
            self.imports.lab,
            LabState::Located { .. } | LabState::Fixture { .. } | LabState::NoWorkspace
        )
    }

    fn keys(&self, nav: Option<&str>) -> Vec<String> {
        match nav.unwrap_or("runs") {
            "new" => vec![NEW_KEY.into(), LAB_KEY.into()],
            "review" => self
                .items()
                .into_iter()
                .map(|(r, i)| item_key(&r.id, &i.id))
                .collect(),
            n => {
                let family = n.strip_prefix("family:");
                self.runs_sorted()
                    .into_iter()
                    .filter(|r| family.is_none() || r.family.as_deref() == family)
                    .map(|r| run_key(&r.id))
                    .collect()
            }
        }
    }

    fn target(&self, key: &str) -> Option<Target<'_>> {
        match key {
            LAB_KEY => return Some(Target::Lab),
            NEW_KEY => return Some(Target::New),
            _ => {}
        }
        if let Some(id) = key.strip_prefix("run:") {
            return self
                .imports
                .runs
                .iter()
                .find(|r| r.id == id)
                .map(Target::Run);
        }
        let rest = key.strip_prefix("review:")?;
        let (run, item) = rest.rsplit_once(':')?;
        let r = self.imports.runs.iter().find(|r| r.id == run)?;
        let i = r.review.iter().find(|i| i.id == item)?;
        Some(Target::Item(r, i))
    }

    fn lab_row(&self) -> RowModel {
        let (title, detail, chip) = match &self.imports.lab {
            LabState::NoWorkspace => (
                "boris-migration-lab".to_string(),
                "not checked: no workspace open".to_string(),
                Chip::new("idle", Tone::Dim),
            ),
            LabState::Fixture { tool_id } => (
                tool_id.clone(),
                "fixture captures".into(),
                Chip::new("fixture", Tone::Dim),
            ),
            LabState::Located {
                tool_id, source, ..
            } => (tool_id.clone(), source.clone(), Chip::new("OK", Tone::Ok)),
            LabState::Absent { searched } => (
                "boris-migration-lab not found".into(),
                format!("{} searched", searched.len()),
                Chip::new("absent", Tone::Error),
            ),
            LabState::OverrideMissing { path } => (
                "CV_MIGRATION_LAB_BIN is not an executable".into(),
                path.clone(),
                Chip::new("absent", Tone::Error),
            ),
            LabState::Mismatch { found, want, .. } => (
                found.clone(),
                format!("pinned {want}"),
                Chip::new("mismatch", Tone::Warn),
            ),
            LabState::Failed { error } => (
                "Lab gate failed".into(),
                error.clone(),
                Chip::new("failed", Tone::Error),
            ),
        };
        let rail = self.lab_problem().then_some(chip.tone);
        RowModel {
            key: LAB_KEY.into(),
            title,
            detail,
            tone: Tone::Normal,
            rail,
            badge: Some(chip),
            path: false,
        }
    }

    fn summary(r: &ImportRun) -> String {
        let mut parts = vec![r.mode.clone()];
        if let Some(p) = &r.problem {
            parts.push(p.clone());
        } else if let Some(why) = &r.refused {
            parts.push(why.clone());
        } else {
            for (k, n) in &r.counts {
                if *n > 0 {
                    parts.push(format!("{n} {k}"));
                }
            }
        }
        let open = r
            .review
            .iter()
            .filter(|i| i.state == ReviewState::Open)
            .count() as u64;
        if open > 0 {
            parts.push(format!("{} to review", open));
        }
        parts.join(" · ")
    }

    fn run_row(r: &ImportRun) -> RowModel {
        let rail = match (r.problem.is_some(), r.status) {
            (true, _) => Some(Tone::Warn),
            (_, RunStatus::Failed) => Some(Tone::Error),
            (_, RunStatus::Refused) => Some(Tone::Warn),
            _ => None,
        };
        let badge = if r.problem.is_some() && r.status == RunStatus::Ok {
            Chip::new("unreadable", Tone::Warn)
        } else {
            Chip::new(r.status.label(), r.status.tone())
        };
        RowModel {
            key: run_key(&r.id),
            title: if r.source.is_empty() {
                r.id.clone()
            } else {
                r.source.clone()
            },
            detail: Self::summary(r),
            badge: Some(badge),
            tone: if r.status == RunStatus::Running {
                Tone::Dim
            } else {
                Tone::Normal
            },
            rail,
            path: false,
        }
    }

    fn item_row(r: &ImportRun, i: &ReviewItem) -> RowModel {
        let open = i.state == ReviewState::Open;
        let detail = [i.construct.as_str(), i.reason.as_str()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" · ");
        RowModel {
            key: item_key(&r.id, &i.id),
            title: i.source.clone(),
            detail,
            badge: Some(if open {
                Chip::new(i.class.clone(), class_tone(&i.class))
            } else {
                Chip::new(i.state.label(), Tone::Dim)
            }),
            tone: if open { Tone::Normal } else { Tone::Dim },
            rail: open.then(|| class_tone(&i.class)),
            path: i.source.contains('/'),
        }
    }

    fn lab_reading(&self) -> ReadingModel {
        let mut lines = Vec::new();
        match &self.imports.lab {
            LabState::NoWorkspace => lines.push(note(
                "No workspace is open, so nothing has looked for boris-migration-lab.",
            )),
            LabState::Fixture { tool_id } => {
                lines.push(line("captured", tool_id.clone()));
                lines.push(note(
                    "The fixture demo shows runs replayed from fixtures/captures/lab; no lab binary runs.",
                ));
            }
            LabState::Located {
                tool_id,
                source,
                path,
            } => {
                lines.push(line("version", tool_id.clone()));
                lines.push(line("located via", source.clone()));
                lines.push(mono("binary", path.clone()));
                lines.push(note(
                    "The binary passed the --version gate against the pin.",
                ));
            }
            LabState::Absent { searched } => {
                for p in searched {
                    lines.push(mono("searched", p.clone()));
                }
                lines.push(note(FIX_LAB));
            }
            LabState::OverrideMissing { path } => {
                lines.push(mono("CV_MIGRATION_LAB_BIN", path.clone()));
                lines.push(note(
                    "An explicit override never falls through to another binary: fix or unset CV_MIGRATION_LAB_BIN, then reopen the workspace.",
                ));
            }
            LabState::Mismatch { found, want, path } => {
                lines.push(line("found", found.clone()));
                lines.push(line("want", want.clone()));
                lines.push(mono("binary", path.clone()));
                lines.push(note(FIX_LAB));
            }
            LabState::Failed { error } => {
                lines.push(line("error", error.clone()));
                lines.push(note(FIX_LAB));
            }
        }
        ReadingModel {
            title: self.lab_row().title,
            lines,
            breadcrumbs: vec!["Migrate".into(), "New import".into()],
            markdown: None,
        }
    }

    fn run_reading(&self, r: &ImportRun) -> ReadingModel {
        let mut lines = vec![ReadingLine {
            label: Some("status".into()),
            text: r.status.label().into(),
            tone: r.status.tone(),
            mono: false,
        }];
        if let Some(p) = &r.problem {
            lines.push(ReadingLine {
                label: Some("unreadable".into()),
                text: p.clone(),
                tone: Tone::Warn,
                mono: false,
            });
            lines.push(note(
                "The lab's output is left as it is; the report could not be decoded by this build's mirrors.",
            ));
        }
        if let Some(why) = &r.refused {
            lines.push(ReadingLine {
                label: Some("refused".into()),
                text: why.clone(),
                tone: Tone::Warn,
                mono: false,
            });
        }
        if r.status == RunStatus::Running {
            lines.push(note(
                "No exit was recorded yet. A run whose app quit before the lab ended stays running.",
            ));
        }
        for (k, n) in &r.totals {
            lines.push(line(k, n.to_string()));
        }
        for (k, n) in &r.counts {
            lines.push(line(k, n.to_string()));
        }
        let open = r
            .review
            .iter()
            .filter(|i| i.state == ReviewState::Open)
            .count() as u64;
        if !r.review.is_empty() {
            lines.push(line(
                "review",
                format!("{} open of {}", open, plural(r.review.len() as u64, "item")),
            ));
        }
        if let Some((name, _)) = &r.report {
            lines.push(note(format!(
                "{name} below is the lab's report, drawn as a draft sketch."
            )));
        }
        ReadingModel {
            title: format!("{}: {}", r.mode, r.source),
            lines,
            breadcrumbs: vec!["Migrate".into(), "Runs".into()],
            markdown: r.report.as_ref().map(|(_, b)| b.clone()),
        }
    }

    fn item_reading(r: &ImportRun, i: &ReviewItem) -> ReadingModel {
        let mut lines = vec![
            ReadingLine {
                label: Some("class".into()),
                text: i.class.clone(),
                tone: class_tone(&i.class),
                mono: false,
            },
            mono("source", i.source.clone()),
        ];
        if !i.construct.is_empty() {
            lines.push(line("construct", i.construct.clone()));
        }
        if !i.reason.is_empty() {
            lines.push(line("reason", i.reason.clone()));
        }
        lines.push(line("triage", i.state.label()));
        lines.push(line("run", format!("{} ({})", r.id, r.mode)));
        lines.push(note(
            "Review items never block publishing or adoption. Accept or defer one from the command palette (mod+K).",
        ));
        ReadingModel {
            title: if i.construct.is_empty() {
                i.source.clone()
            } else {
                i.construct.clone()
            },
            lines,
            breadcrumbs: vec!["Migrate".into(), "Needs review".into(), r.id.clone()],
            markdown: None,
        }
    }

    fn transition(&mut self, to: ReviewState) -> Vec<Intent> {
        let Some(key) = self.selected.clone() else {
            return vec![Intent::Status("select a review item first".into())];
        };
        let Some(Target::Item(r, i)) = self.target(&key) else {
            return vec![Intent::Status("select a review item first".into())];
        };
        let (run, item) = (r.id.clone(), i.id.clone());
        if let Err(e) = self.port.record(&run, &item, to) {
            return vec![Intent::Status(format!("review state not saved: {e}"))];
        }
        if let Some(it) = self
            .imports
            .runs
            .iter_mut()
            .filter(|r| r.id == run)
            .flat_map(|r| r.review.iter_mut())
            .find(|i| i.id == item)
        {
            it.state = to;
        }
        vec![Intent::Status(format!("review item {}", to.label()))]
    }

    fn noun_of(&self, key: &str) -> Option<NounId> {
        let (kind, stable) = match self.target(key)? {
            Target::Run(r) => (noun_kind::IMPORT_RUN, r.id.clone()),
            Target::Item(r, i) => (noun_kind::REVIEW_ITEM, format!("{}:{}", r.id, i.id)),
            Target::Lab | Target::New => return None,
        };
        Some(NounId {
            account: AccountId("local".into()),
            kind: NounKind::register(kind),
            stable: StableId(stable),
        })
    }
}

impl Module for MigrateModule {
    fn spec(&self) -> ModuleSpec {
        ModuleSpec {
            id: ID,
            title: "Migrate",
            slot: Slot::Utilities,
        }
    }

    fn ingest(&mut self, _env: &Envelope) -> bool {
        false
    }

    fn nav(&self) -> NavModel {
        let n = |key: &str, label: &str, count: usize| NavItem {
            key: key.into(),
            label: label.into(),
            count: (count > 0).then_some(count as u64),
        };
        let mut items = vec![
            n("new", "New import", usize::from(self.lab_problem())),
            n("runs", "Runs", self.imports.runs.len()),
            n("review", "Needs review", self.open_items()),
        ];
        for f in &self.imports.families {
            let c = self
                .imports
                .runs
                .iter()
                .filter(|r| r.family.as_deref() == Some(f.key.as_str()))
                .count();
            items.push(n(&format!("family:{}", f.key), &f.label, c));
        }
        // Runs first: it is the mailbox the module opens on.
        items.swap(0, 1);
        NavModel { items }
    }

    fn list_len(&self, nav: Option<&str>) -> usize {
        self.keys(nav).len()
    }

    fn row(&self, nav: Option<&str>, index: usize) -> RowModel {
        let Some(key) = self.keys(nav).into_iter().nth(index) else {
            return RowModel::default();
        };
        match self.target(&key) {
            Some(Target::Lab) => self.lab_row(),
            Some(Target::New) => RowModel {
                key,
                title: "New import…".into(),
                detail: NEW_UNAVAILABLE.into(),
                tone: Tone::Dim,
                badge: Some(Chip::new("unavailable", Tone::Dim)),
                ..RowModel::default()
            },
            Some(Target::Run(r)) => Self::run_row(r),
            Some(Target::Item(r, i)) => Self::item_row(r, i),
            None => RowModel::default(),
        }
    }

    fn index_of_key(&self, nav: Option<&str>, key: &str) -> Option<usize> {
        self.keys(nav).iter().position(|k| k == key)
    }

    fn reading(&self, key: &str) -> ReadingModel {
        match self.target(key) {
            Some(Target::Lab) => self.lab_reading(),
            Some(Target::New) => ReadingModel {
                title: "New import".into(),
                lines: vec![
                    note(format!("Not available: {NEW_UNAVAILABLE}.")),
                    note(
                        "Runs written under .cv/imports/<run-id>/ (a run.json beside the lab's out/) appear in Runs. Run Reload import runs (mod+K) after adding one.",
                    ),
                ],
                breadcrumbs: vec!["Migrate".into()],
                markdown: None,
            },
            Some(Target::Run(r)) => self.run_reading(r),
            Some(Target::Item(r, i)) => Self::item_reading(r, i),
            None => {
                let mut lines = Vec::new();
                if let Some(e) = &self.imports.error {
                    lines.push(ReadingLine {
                        label: Some(".cv/imports".into()),
                        text: e.clone(),
                        tone: Tone::Error,
                        mono: false,
                    });
                }
                ReadingModel {
                    title: key.into(),
                    lines,
                    ..ReadingModel::default()
                }
            }
        }
    }

    fn inspector(&self, key: &str) -> InspectorModel {
        match self.target(key) {
            Some(Target::Run(r)) => {
                let mut fields = vec![
                    ("run".into(), r.id.clone()),
                    ("mode".into(), r.mode.clone()),
                ];
                let mut push = |k: &str, v: Option<String>| {
                    fields.push((k.into(), v.unwrap_or_else(|| "—".into())));
                };
                push("format", r.format.as_ref().map(|(f, _)| f.clone()));
                push("schema", r.format.as_ref().map(|(_, v)| v.to_string()));
                push("lab version", r.tool_version.clone());
                push("lab binary", r.lab.clone());
                push("Boris pin", r.boris_pin.clone());
                push("argv", (!r.argv.is_empty()).then(|| r.argv.join(" ")));
                push("exit code", r.exit_code.map(|c| c.to_string()));
                push("ended", r.outcome.clone());
                push("duration", r.duration_ms.map(|d| format!("{d} ms")));
                push("output", Some(r.out.clone()));
                push("provenance", r.provenance.clone());
                let mut badges = vec![Chip::new(r.status.label(), r.status.tone())];
                if r.problem.is_some() {
                    badges.push(Chip::new("report unreadable", Tone::Warn));
                }
                InspectorModel {
                    badges,
                    fields,
                    diagnostics: Vec::new(),
                    lists: vec![(
                        "Counts".into(),
                        r.counts.iter().map(|(k, n)| format!("{k}: {n}")).collect(),
                    )],
                }
            }
            Some(Target::Item(r, i)) => InspectorModel {
                badges: vec![
                    Chip::new(i.class.clone(), class_tone(&i.class)),
                    Chip::new("does not block", Tone::Dim),
                ],
                fields: vec![
                    ("run".into(), r.id.clone()),
                    ("item".into(), i.id.clone()),
                    ("triage".into(), i.state.label().into()),
                    ("sidecar".into(), self.imports.store.clone()),
                ],
                diagnostics: Vec::new(),
                lists: Vec::new(),
            },
            Some(Target::Lab) => InspectorModel {
                badges: self.lab_row().badge.into_iter().collect(),
                fields: vec![("tool".into(), "boris-migration-lab".into())],
                diagnostics: Vec::new(),
                lists: Vec::new(),
            },
            _ => InspectorModel::default(),
        }
    }

    fn noun(&self, key: &str) -> Option<NounId> {
        self.noun_of(key)
    }

    fn select(&mut self, key: Option<&str>) {
        self.selected = key.map(str::to_string);
    }

    fn commands(&self) -> Vec<CommandSpec> {
        vec![
            CommandSpec::new(NEW, "New import…").disabled(Some(NEW_UNAVAILABLE.into())),
            CommandSpec::new(REFRESH, "Reload import runs"),
            CommandSpec::new(ACCEPT, "Accept review item"),
            CommandSpec::new(DEFER, "Defer review item"),
            CommandSpec::new(REOPEN, "Reopen review item"),
            CommandSpec::new(ADOPT, "Adopt run…").disabled(Some(ADOPT_UNAVAILABLE.into())),
        ]
    }

    fn run(&mut self, command: &str) -> Vec<Intent> {
        match command {
            ACCEPT => self.transition(ReviewState::Accepted),
            DEFER => self.transition(ReviewState::Deferred),
            REOPEN => self.transition(ReviewState::Open),
            REFRESH => {
                self.imports = self.port.load();
                vec![Intent::Status(format!(
                    "{} import run(s) loaded",
                    self.imports.runs.len()
                ))]
            }
            NEW => vec![Intent::Status(format!("New import: {NEW_UNAVAILABLE}"))],
            ADOPT => vec![Intent::Status(format!("Adopt: {ADOPT_UNAVAILABLE}"))],
            _ => Vec::new(),
        }
    }

    fn contains(&self, key: &str) -> bool {
        self.target(key).is_some()
    }

    fn badge(&self) -> Option<u64> {
        let n = self.open_items() as u64;
        (n > 0).then_some(n)
    }

    fn attr_rows(&self) -> Vec<NounAttrRow> {
        let t = |s: &str| AttrValue::Text(s.into());
        let mut out = Vec::new();
        for r in &self.imports.runs {
            let mut f = BTreeMap::new();
            f.insert("mode".into(), t(&r.mode));
            f.insert("status".into(), t(r.status.label()));
            f.insert("source".into(), t(&r.source));
            f.insert("review_items".into(), AttrValue::Int(r.review.len() as i64));
            f.insert("readable".into(), AttrValue::Bool(r.problem.is_none()));
            if let Some(n) = self.noun_of(&run_key(&r.id)) {
                out.push(NounAttrRow { noun: n, fields: f });
            }
            for i in &r.review {
                let mut f = BTreeMap::new();
                f.insert("run".into(), t(&r.id));
                f.insert("class".into(), t(&i.class));
                f.insert("triage".into(), t(i.state.label()));
                f.insert("construct".into(), t(&i.construct));
                if let Some(n) = self.noun_of(&item_key(&r.id, &i.id)) {
                    out.push(NounAttrRow { noun: n, fields: f });
                }
            }
        }
        out
    }
}
