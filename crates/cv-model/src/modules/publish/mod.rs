//! Publish — the publish ledger as mailboxes (L5). Runs, their sealed
//! grants and the evidence they left are folded from the `.cv/ledger/`
//! journal by an injected [`PublishPort`]; this module never touches
//! disk. Its verbs (preflight, approve, dispatch, reconcile, discard)
//! ask the port, which owns every ledger write, and then re-read the
//! ledger: nothing is shown that the journal did not record.
//!
//! Run state is the ledger's durable state, never optimism: a run whose
//! last record is `dispatching` has no recorded outcome and is shown as
//! unconfirmed, and an `unknown` run blocks its target until reconciled.

use crate::command::{CommandSpec, Intent};
use crate::dialog::DialogAnswer;
use crate::module::{
    Chip, InspectorModel, Link, Module, ModuleSpec, NavCache, NavItem, NavModel, ReadingLine,
    ReadingModel, Registry, RowModel, Slot, Tone,
};
use cv_core::events::Envelope;
use cv_core::id::{AccountId, AttrValue, NounAttrRow, NounId, NounKind, StableId, noun_kind};
use cv_core::publish::PreflightGrant;
use std::cell::RefCell;
use std::collections::BTreeMap;

pub const ID: &str = "publish";

/// A registered publish target, labelled as Boris labels it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetInfo {
    pub id: String,
    pub label: String,
    pub verified: bool,
    /// Keychain name of the secret its transport needs — never the value.
    pub secret: Option<String>,
    pub hosts_html: bool,
    /// How a new preflight for this target would deliver
    /// (`copy-to-folder → publish/static-dist`); `None` while the
    /// target has no delivery configured.
    pub delivery: Option<String>,
}

/// `cv_ledger::RunPhase`, mirrored so this crate stays free of the
/// ledger's storage stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    PreflightBlocked,
    Granted,
    Approved,
    Dispatching,
    Submitted,
    Unknown,
    Complete,
    Partial,
    Failed,
    Incomplete,
    Discarded,
}

impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Self::PreflightBlocked => "preflight-blocked",
            Self::Granted => "granted",
            Self::Approved => "approved",
            Self::Dispatching => "dispatching",
            Self::Submitted => "submitted",
            Self::Unknown => "unknown",
            Self::Complete => "complete",
            Self::Partial => "partial",
            Self::Failed => "failed",
            Self::Incomplete => "incomplete",
            Self::Discarded => "discarded",
        }
    }

    fn mailbox(self) -> &'static str {
        match self {
            Self::PreflightBlocked | Self::Granted | Self::Approved => PREFLIGHT,
            Self::Dispatching | Self::Submitted | Self::Unknown => UNRESOLVED,
            Self::Complete | Self::Partial | Self::Failed | Self::Incomplete | Self::Discarded => {
                COMPLETED
            }
        }
    }

    fn tone(self) -> Tone {
        match self {
            Self::Complete => Tone::Ok,
            Self::PreflightBlocked | Self::Failed => Tone::Error,
            Self::Dispatching | Self::Unknown | Self::Partial | Self::Incomplete => Tone::Warn,
            Self::Granted | Self::Approved | Self::Submitted => Tone::Normal,
            Self::Discarded => Tone::Dim,
        }
    }

    fn summary(self) -> &'static str {
        match self {
            Self::PreflightBlocked => "preflight blocked: nothing was sealed or delivered",
            Self::Granted => "sealed by preflight; awaiting approval",
            Self::Approved => "approved; not dispatched",
            Self::Dispatching => {
                "last durable state is dispatching and no outcome was recorded. Unless a \
                 publish is still running, it was interrupted: reconcile it before this \
                 target publishes again"
            }
            Self::Submitted => "accepted by the target; reconcile to confirm the outcome",
            Self::Unknown => {
                "the transport may have been touched before an outcome was recorded. \
                 Reconcile before this target publishes again; runs are never retried"
            }
            Self::Complete => "delivered",
            Self::Partial => "partly delivered",
            Self::Failed => "not delivered",
            Self::Incomplete => "delivery incomplete",
            Self::Discarded => "grant discarded before dispatch: nothing was delivered",
        }
    }
}

/// What the sealed plan ships, and where.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PlanSummary {
    pub adapter: String,
    pub destination: String,
    pub files: u64,
    pub bytes: u64,
}

/// The sealed target configuration's display fields.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TargetSettings {
    pub profile: String,
    pub base_url: String,
    pub verified: bool,
}

/// One publish run as the ledger records it.
#[derive(Debug, Clone, PartialEq)]
pub struct RunRecord {
    pub id: String,
    pub target: String,
    pub phase: Phase,
    pub grant: Option<PreflightGrant>,
    pub plan: Option<PlanSummary>,
    pub settings: Option<TargetSettings>,
    pub blocks: Vec<String>,
    pub approved_by: Option<String>,
    pub discarded_by: Option<String>,
    /// Non-blocking ruleset findings preflight saw on the sealed bytes.
    pub advisory: Vec<String>,
    /// Run states in record order (`dispatching`, `unknown`, …).
    pub history: Vec<String>,
    /// Notes recorded with those states and anything the fold could not
    /// decode.
    pub notes: Vec<String>,
    pub refusals: Vec<String>,
    pub reconciliations: Vec<String>,
    pub exports_checked: bool,
    /// Proof-pack artifacts lifted at preflight.
    pub artifacts: u64,
}

impl RunRecord {
    pub fn new(id: &str, target: &str, phase: Phase) -> Self {
        Self {
            id: id.into(),
            target: target.into(),
            phase,
            grant: None,
            plan: None,
            settings: None,
            blocks: Vec::new(),
            approved_by: None,
            discarded_by: None,
            advisory: Vec::new(),
            history: Vec::new(),
            notes: Vec::new(),
            refusals: Vec::new(),
            reconciliations: Vec::new(),
            exports_checked: false,
            artifacts: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceKind {
    /// A dispatch of the sealed plan, recorded before contact.
    Delivery,
    /// Ship-the-outputs read-back of one declared export.
    Receipt,
    /// A Boris proof-pack check, copied at preflight.
    Check,
    /// A Boris proof-pack claim, copied at preflight.
    Claim,
}

impl EvidenceKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Delivery => "delivery",
            Self::Receipt => "receipt",
            Self::Check => "check",
            Self::Claim => "claim",
        }
    }

    fn noun_kind(self) -> &'static str {
        match self {
            Self::Delivery => noun_kind::DELIVERY,
            Self::Receipt => noun_kind::RECEIPT,
            Self::Check => noun_kind::CHECK,
            Self::Claim => noun_kind::CLAIM,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceRecord {
    /// Stable noun id (the ledger's own digest id, or `rcpt:<run>:<path>`).
    pub id: String,
    pub run: String,
    pub kind: EvidenceKind,
    pub name: String,
    /// The recorded verdict, verbatim.
    pub status: String,
    pub detail: String,
}

/// The journal folded for display, in record order.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Ledger {
    pub runs: Vec<RunRecord>,
    pub evidence: Vec<EvidenceRecord>,
    /// Ledger-wide disclosures (an ignored torn tail, …).
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub enum LedgerRead {
    /// No workspace is open: there is no ledger to read.
    #[default]
    NoWorkspace,
    /// The workspace has not recorded a publish run yet.
    Absent,
    Read(Ledger),
    /// The journal exists but cannot be trusted; nothing is shown from it.
    Unreadable(String),
}

/// What a publish verb did, as the port reports it. The module re-reads
/// the ledger afterwards; this is only the status line and the run to
/// select.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Done {
    pub run: String,
    pub status: String,
}

/// What reconciling an unresolved run records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Observed {
    /// Ask the delivery adapter's read-only probe to look.
    Probe,
    Complete,
    Partial,
    NotDelivered,
}

/// IO boundary: the target registry, the ledger as read from disk, and
/// the publish verbs that write it. Verbs default to refusing, so a
/// read-only port stays read-only.
pub trait PublishPort {
    fn targets(&self) -> Vec<TargetInfo>;
    /// The ledger now. Read when the module is built and again after
    /// every verb.
    fn ledger(&mut self) -> LedgerRead;
    fn describe(&self) -> String;
    /// Delivery adapters a dispatch can go through.
    fn adapters(&self) -> Vec<String> {
        Vec::new()
    }
    fn preflight(&mut self, _target: &str) -> Result<Done, String> {
        Err(READ_ONLY.into())
    }
    fn approve(&mut self, _run: &str) -> Result<Done, String> {
        Err(READ_ONLY.into())
    }
    fn dispatch(&mut self, _run: &str) -> Result<Done, String> {
        Err(READ_ONLY.into())
    }
    fn reconcile(&mut self, _run: &str, _how: Observed) -> Result<Done, String> {
        Err(READ_ONLY.into())
    }
    fn discard(&mut self, _run: &str) -> Result<Done, String> {
        Err(READ_ONLY.into())
    }
}

const READ_ONLY: &str = "this publish ledger is read-only";

#[derive(Default)]
pub struct MemoryPort {
    pub targets: Vec<TargetInfo>,
    pub read: LedgerRead,
}

impl PublishPort for MemoryPort {
    fn targets(&self) -> Vec<TargetInfo> {
        self.targets.clone()
    }
    fn ledger(&mut self) -> LedgerRead {
        self.read.clone()
    }
    fn describe(&self) -> String {
        "in memory".into()
    }
}

thread_local! {
    static SEED: RefCell<Option<Box<dyn PublishPort>>> = const { RefCell::new(None) };
}

/// Composition-root hook: the next `register` on this thread uses `port`.
pub fn install(port: Box<dyn PublishPort>) {
    SEED.with(|s| *s.borrow_mut() = Some(port));
}

pub fn register(r: &mut Registry) {
    let port = SEED
        .with(|s| s.borrow_mut().take())
        .unwrap_or_else(|| Box::new(MemoryPort::default()));
    r.register(Box::new(PublishModule::new(port)))
        .expect("publish module id is unique");
}

const RUNS: &str = "runs";
const PREFLIGHT: &str = "preflight";
const UNRESOLVED: &str = "unresolved";
const COMPLETED: &str = "completed";
const TARGETS: &str = "targets";
const EVIDENCE: &str = "evidence";
const TARGET: &str = "target:";
/// Row key of the ledger-status row an empty `runs` mailbox shows.
pub const STATUS: &str = "publish-ledger";

const FIXED_VIEWS: &[(&str, &str)] = &[
    (RUNS, "All runs"),
    (PREFLIGHT, "Preflight"),
    (UNRESOLVED, "In flight / unknown"),
    (COMPLETED, "Completed"),
    (TARGETS, "Targets"),
    (EVIDENCE, "Evidence"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Item {
    Status,
    Target(usize),
    Run(usize),
    Evidence(usize),
}

enum Source {
    NoWorkspace,
    Absent,
    Read,
    Unreadable(String),
}

pub struct PublishModule {
    port: Box<dyn PublishPort>,
    adapters: Vec<String>,
    selected: Option<String>,
    targets: Vec<TargetInfo>,
    runs: Vec<RunRecord>,
    evidence: Vec<EvidenceRecord>,
    notes: Vec<String>,
    source: Source,
    describe: String,
    views: BTreeMap<String, Vec<Item>>,
    by_key: BTreeMap<String, Item>,
    /// Built once: the ledger is read when the module is.
    nav_cache: NavCache,
}

impl PublishModule {
    pub fn new(port: Box<dyn PublishPort>) -> Self {
        let mut m = Self {
            targets: port.targets(),
            describe: port.describe(),
            adapters: port.adapters(),
            port,
            selected: None,
            runs: Vec::new(),
            evidence: Vec::new(),
            notes: Vec::new(),
            source: Source::NoWorkspace,
            views: BTreeMap::new(),
            by_key: BTreeMap::new(),
            nav_cache: NavCache::default(),
        };
        m.reload();
        m
    }

    /// Re-read the ledger through the port and rebuild every view.
    fn reload(&mut self) {
        let (source, ledger) = match self.port.ledger() {
            LedgerRead::NoWorkspace => (Source::NoWorkspace, Ledger::default()),
            LedgerRead::Absent => (Source::Absent, Ledger::default()),
            LedgerRead::Read(l) => (Source::Read, l),
            LedgerRead::Unreadable(e) => (Source::Unreadable(e), Ledger::default()),
        };
        self.source = source;
        self.runs = ledger.runs;
        self.evidence = ledger.evidence;
        self.notes = ledger.notes;
        self.index();
    }

    /// Runs that block a new dispatch to their target until reconciled.
    pub fn unresolved_count(&self) -> usize {
        self.view(Some(UNRESOLVED)).len()
    }

    fn index(&mut self) {
        let mut v: BTreeMap<String, Vec<Item>> = FIXED_VIEWS
            .iter()
            .map(|(k, _)| ((*k).to_string(), Vec::new()))
            .collect();
        let mut push = |k: &str, item| v.entry(k.to_string()).or_default().push(item);
        for (i, r) in self.runs.iter().enumerate().rev() {
            push(RUNS, Item::Run(i));
            push(r.phase.mailbox(), Item::Run(i));
            push(&format!("{TARGET}{}", r.target), Item::Run(i));
        }
        if self.runs.is_empty() {
            push(RUNS, Item::Status);
        }
        for i in 0..self.targets.len() {
            push(TARGETS, Item::Target(i));
        }
        for i in 0..self.evidence.len() {
            push(EVIDENCE, Item::Evidence(i));
        }
        self.views = v;
        let mut keys = BTreeMap::new();
        for item in self.views.values().flatten() {
            keys.entry(self.key(*item)).or_insert(*item);
        }
        self.by_key = keys;
        self.nav_cache.invalidate();
    }

    fn view(&self, nav: Option<&str>) -> &[Item] {
        self.views
            .get(nav.unwrap_or(RUNS))
            .map_or(&[], Vec::as_slice)
    }

    fn key(&self, item: Item) -> String {
        match item {
            Item::Status => STATUS.into(),
            Item::Target(i) => format!("{TARGET}{}", self.targets[i].id),
            Item::Run(i) => self.runs[i].id.clone(),
            Item::Evidence(i) => self.evidence[i].id.clone(),
        }
    }

    fn label_of(&self, target: &str) -> String {
        self.targets
            .iter()
            .find(|t| t.id == target)
            .map_or_else(|| target.to_string(), |t| t.label.clone())
    }

    fn runs_of(&self, target: &str) -> &[Item] {
        self.view(Some(&format!("{TARGET}{target}")))
    }

    fn build_nav(&self) -> NavModel {
        let count = |k: &str| Some(self.view(Some(k)).len() as u64);
        let mut items: Vec<NavItem> = FIXED_VIEWS
            .iter()
            .map(|(k, l)| NavItem {
                key: (*k).into(),
                label: (*l).into(),
                count: count(k),
            })
            .collect();
        items[0].count = Some(self.runs.len() as u64);
        for k in self.views.keys().filter(|k| k.starts_with(TARGET)) {
            items.push(NavItem {
                key: k.clone(),
                label: format!("Target · {}", self.label_of(&k[TARGET.len()..])),
                count: count(k),
            });
        }
        NavModel { items }
    }

    fn status_title(&self) -> &'static str {
        match self.source {
            Source::NoWorkspace => "No workspace open",
            Source::Unreadable(_) => "Publish ledger unreadable",
            Source::Absent | Source::Read => "No publish runs yet",
        }
    }

    fn notes(&self) -> Vec<ReadingLine> {
        self.notes
            .iter()
            .map(|n| line("note", n.clone(), Tone::Warn, false))
            .collect()
    }

    fn status_reading(&self) -> ReadingModel {
        let lines = match &self.source {
            Source::NoWorkspace => vec![line(
                "",
                "Publish reads a workspace's .cv/ledger/ journal. Open a workspace to see its runs.",
                Tone::Dim,
                false,
            )],
            Source::Unreadable(e) => vec![
                line("error", e.clone(), Tone::Error, true),
                line(
                    "",
                    "No run is shown rather than a guessed one. The journal is left as found.",
                    Tone::Normal,
                    false,
                ),
            ],
            Source::Absent | Source::Read => vec![line(
                "",
                "Nothing has been preflighted in this workspace. Targets lists where it can publish.",
                Tone::Dim,
                false,
            )],
        };
        let mut lines = lines;
        lines.push(line("ledger", self.describe.clone(), Tone::Dim, true));
        lines.extend(self.notes());
        ReadingModel {
            title: self.status_title().into(),
            lines,
            ..Default::default()
        }
    }

    fn run_reading(&self, r: &RunRecord) -> ReadingModel {
        let mut lines = vec![
            line("target", self.label_of(&r.target), Tone::Normal, false),
            line("state", r.phase.label().to_string(), r.phase.tone(), false),
            line("", r.phase.summary().to_string(), r.phase.tone(), false),
        ];
        for b in &r.blocks {
            let text = match actions::audit_finding(b) {
                Some((code, subject)) => {
                    format!("{code} on {subject} · blocking ruleset finding, reveal it in Audit")
                }
                None => b.clone(),
            };
            lines.push(line("block", text, Tone::Error, true));
        }
        for a in &r.advisory {
            lines.push(line("advisory", a.clone(), Tone::Dim, false));
        }
        if let Some(by) = &r.approved_by {
            lines.push(line("approved", format!("by {by}"), Tone::Normal, false));
        }
        if let Some(by) = &r.discarded_by {
            lines.push(line("discarded", format!("by {by}"), Tone::Dim, false));
        }
        if let Some(p) = &r.plan {
            lines.push(line(
                "plan",
                format!("{} files · {} B → {}", p.files, p.bytes, p.destination),
                Tone::Normal,
                true,
            ));
        }
        if !r.history.is_empty() {
            lines.push(line("history", r.history.join(" → "), Tone::Normal, true));
        }
        for n in &r.notes {
            lines.push(line("note", n.clone(), Tone::Warn, false));
        }
        for x in &r.refusals {
            lines.push(line("refused", x.clone(), Tone::Warn, false));
        }
        for x in &r.reconciliations {
            lines.push(line("reconciled", x.clone(), Tone::Normal, false));
        }
        let (next, tone) = match actions::next_step(self, r) {
            Ok(s) => (s, Tone::Normal),
            Err(s) => (s, Tone::Dim),
        };
        lines.push(line("next", next, tone, false));
        let receipts: Vec<&EvidenceRecord> = self
            .evidence
            .iter()
            .filter(|e| e.run == r.id && e.kind == EvidenceKind::Receipt)
            .collect();
        if r.exports_checked {
            let mut by: BTreeMap<&str, usize> = BTreeMap::new();
            for e in &receipts {
                *by.entry(e.status.as_str()).or_default() += 1;
            }
            let s: Vec<String> = by.iter().map(|(k, n)| format!("{n} {k}")).collect();
            lines.push(line("exports", s.join(" · "), Tone::Normal, false));
        }
        lines.extend(self.notes());
        ReadingModel {
            title: format!("{} · {}", self.label_of(&r.target), r.phase.label()),
            lines,
            breadcrumbs: vec![self.label_of(&r.target), r.id.clone()],
            markdown: None,
        }
    }

    fn target_reading(&self, t: &TargetInfo) -> ReadingModel {
        let runs = self.runs_of(&t.id);
        let mut lines = vec![
            line("id", t.id.clone(), Tone::Normal, true),
            line("registry", verification(t.verified), Tone::Normal, false),
            line(
                "hosts",
                if t.hosts_html {
                    "the site tree and its machine exports"
                } else {
                    "records, not a site tree"
                },
                Tone::Normal,
                false,
            ),
            line(
                "secret",
                t.secret
                    .as_ref()
                    .map_or_else(|| "none".into(), |s| format!("keychain item {s}")),
                Tone::Normal,
                true,
            ),
            line("runs", runs.len().to_string(), Tone::Normal, false),
            match &t.delivery {
                Some(d) => line("delivery", d.clone(), Tone::Normal, true),
                None => line("delivery", actions::PENDING_ADAPTERS, Tone::Dim, false),
            },
        ];
        if let Some(Item::Run(i)) = runs.first() {
            let r = &self.runs[*i];
            lines.push(line(
                "latest",
                format!("{} · {}", r.id, r.phase.label()),
                r.phase.tone(),
                true,
            ));
        }
        ReadingModel {
            title: t.label.clone(),
            lines,
            ..Default::default()
        }
    }
}

fn line(label: &str, text: impl Into<String>, tone: Tone, mono: bool) -> ReadingLine {
    ReadingLine {
        label: (!label.is_empty()).then(|| label.to_string()),
        text: text.into(),
        tone,
        mono,
    }
}

fn verification(verified: bool) -> &'static str {
    if verified { "verified" } else { "unverified" }
}

/// Colour for a recorded verdict; the words themselves are shown as recorded.
fn status_tone(status: &str) -> Tone {
    match status {
        "passed" | "verified" | "present" => Tone::Ok,
        "failed" | "missing" | "stale" => Tone::Warn,
        _ => Tone::Normal,
    }
}

fn opt_hash(g: Option<&PreflightGrant>, f: impl Fn(&PreflightGrant) -> String) -> String {
    g.map_or_else(|| "—".into(), f)
}

impl Module for PublishModule {
    fn spec(&self) -> ModuleSpec {
        ModuleSpec {
            id: ID,
            title: "Publish",
            slot: Slot::Publish,
        }
    }

    fn ingest(&mut self, _env: &Envelope) -> bool {
        false
    }

    fn nav(&self) -> NavModel {
        self.nav_cache.get_or_build(|| self.build_nav())
    }

    fn list_len(&self, nav: Option<&str>) -> usize {
        self.view(nav).len()
    }

    fn row(&self, nav: Option<&str>, i: usize) -> RowModel {
        let Some(&item) = self.view(nav).get(i) else {
            return RowModel::default();
        };
        let key = self.key(item);
        match item {
            Item::Status => RowModel {
                key,
                title: self.status_title().into(),
                detail: self.describe.clone(),
                badge: None,
                tone: match self.source {
                    Source::Unreadable(_) => Tone::Error,
                    _ => Tone::Dim,
                },
                rail: None,
                path: false,
            },
            Item::Target(i) => {
                let t = &self.targets[i];
                let runs = self.runs_of(&t.id);
                let detail = match runs.first() {
                    Some(Item::Run(j)) => {
                        format!(
                            "{} run(s) · latest {}",
                            runs.len(),
                            self.runs[*j].phase.label()
                        )
                    }
                    _ => "no runs".into(),
                };
                RowModel {
                    key,
                    title: t.label.clone(),
                    detail,
                    badge: Some(Chip::new(
                        verification(t.verified),
                        status_tone(verification(t.verified)),
                    )),
                    tone: Tone::Normal,
                    rail: None,
                    path: false,
                }
            }
            Item::Run(i) => {
                let r = &self.runs[i];
                let detail = match (&r.plan, r.blocks.first()) {
                    (_, Some(b)) => b.clone(),
                    (Some(p), None) => format!("{} files · {} B", p.files, p.bytes),
                    (None, None) => String::new(),
                };
                RowModel {
                    key,
                    title: format!("{} · {}", self.label_of(&r.target), r.id),
                    detail,
                    badge: Some(Chip::new(r.phase.label(), r.phase.tone())),
                    tone: Tone::Normal,
                    rail: Some(r.phase.tone()),
                    path: false,
                }
            }
            Item::Evidence(i) => {
                let e = &self.evidence[i];
                RowModel {
                    key,
                    title: format!("{} · {}", e.kind.label(), e.name),
                    detail: e.run.clone(),
                    badge: Some(Chip::new(e.status.clone(), status_tone(&e.status))),
                    tone: Tone::Normal,
                    rail: Some(status_tone(&e.status)),
                    path: false,
                }
            }
        }
    }

    fn index_of_key(&self, nav: Option<&str>, key: &str) -> Option<usize> {
        let item = *self.by_key.get(key)?;
        self.view(nav).iter().position(|&x| x == item)
    }

    fn reading(&self, key: &str) -> ReadingModel {
        match self.by_key.get(key) {
            Some(Item::Run(i)) => self.run_reading(&self.runs[*i]),
            Some(Item::Target(i)) => self.target_reading(&self.targets[*i]),
            Some(Item::Evidence(i)) => {
                let e = &self.evidence[*i];
                let mut lines = vec![
                    line("kind", e.kind.label(), Tone::Normal, false),
                    line("status", e.status.clone(), status_tone(&e.status), false),
                    line("run", e.run.clone(), Tone::Normal, true),
                ];
                if !e.detail.is_empty() {
                    lines.push(line("detail", e.detail.clone(), Tone::Normal, false));
                }
                ReadingModel {
                    title: e.name.clone(),
                    lines,
                    breadcrumbs: vec![e.run.clone(), e.kind.label().into()],
                    markdown: None,
                }
            }
            Some(Item::Status) | None => self.status_reading(),
        }
    }

    fn inspector(&self, key: &str) -> InspectorModel {
        let field = |k: &str, v: String| (k.to_string(), v);
        match self.by_key.get(key) {
            Some(Item::Run(i)) => {
                let r = &self.runs[*i];
                let g = r.grant.as_ref();
                let p = r.plan.as_ref();
                let s = r.settings.as_ref();
                let dash = || "—".to_string();
                let evidence = |k: EvidenceKind| -> Vec<String> {
                    self.evidence
                        .iter()
                        .filter(|e| e.run == r.id && e.kind == k)
                        .map(|e| format!("{}: {}", e.name, e.status))
                        .collect()
                };
                let mut checks = evidence(EvidenceKind::Check);
                checks.extend(evidence(EvidenceKind::Claim));
                InspectorModel {
                    badges: vec![
                        Chip::new(r.phase.label(), r.phase.tone()),
                        Chip::new(
                            s.map_or("unconfigured", |s| verification(s.verified)),
                            s.map_or(Tone::Dim, |s| status_tone(verification(s.verified))),
                        ),
                    ],
                    fields: vec![
                        field("run", r.id.clone()),
                        field("target", r.target.clone()),
                        field(
                            "registry",
                            s.map_or_else(dash, |s| verification(s.verified).into()),
                        ),
                        field("state", r.phase.label().into()),
                        field("profile", s.map_or_else(dash, |s| s.profile.clone())),
                        field("base url", s.map_or_else(dash, |s| s.base_url.clone())),
                        field("adapter", p.map_or_else(dash, |p| p.adapter.clone())),
                        field(
                            "destination",
                            p.map_or_else(dash, |p| p.destination.clone()),
                        ),
                        field("files", p.map_or_else(dash, |p| p.files.to_string())),
                        field("bytes", p.map_or_else(dash, |p| p.bytes.to_string())),
                        field("artifacts", r.artifacts.to_string()),
                        field(
                            "artifact set",
                            opt_hash(g, |g| g.artifact_set_hash.to_hex()),
                        ),
                        field("graph", opt_hash(g, |g| g.graph_hash.to_hex())),
                        field("plan", opt_hash(g, |g| g.plan_hash.to_hex())),
                        field("target cfg", opt_hash(g, |g| g.target_cfg_hash.to_hex())),
                        field("ruleset", opt_hash(g, |g| g.ruleset_hash.to_hex())),
                        field("boris", opt_hash(g, |g| g.boris_version.clone())),
                        field("approved", r.approved_by.clone().unwrap_or_else(dash)),
                        field("ledger", self.describe.clone()),
                    ],
                    diagnostics: vec![],
                    lists: vec![
                        ("History".into(), r.history.clone()),
                        ("Exports".into(), evidence(EvidenceKind::Receipt)),
                        ("Deliveries".into(), evidence(EvidenceKind::Delivery)),
                        ("Checks & claims".into(), checks),
                    ],
                }
            }
            Some(Item::Target(i)) => {
                let t = &self.targets[*i];
                InspectorModel {
                    fields: vec![
                        field("id", t.id.clone()),
                        field("registry", verification(t.verified).into()),
                        field("hosts html", t.hosts_html.to_string()),
                        field("secret", t.secret.clone().unwrap_or_else(|| "none".into())),
                        field("runs", self.runs_of(&t.id).len().to_string()),
                    ],
                    ..Default::default()
                }
            }
            Some(Item::Evidence(i)) => {
                let e = &self.evidence[*i];
                InspectorModel {
                    fields: vec![
                        field("id", e.id.clone()),
                        field("kind", e.kind.label().into()),
                        field("run", e.run.clone()),
                        field("status", e.status.clone()),
                        field("ledger", self.describe.clone()),
                    ],
                    ..Default::default()
                }
            }
            Some(Item::Status) | None => InspectorModel {
                fields: vec![field("ledger", self.describe.clone())],
                ..Default::default()
            },
        }
    }

    fn noun(&self, key: &str) -> Option<NounId> {
        let kind = match self.by_key.get(key)? {
            Item::Run(_) => noun_kind::PLAN,
            Item::Evidence(i) => self.evidence[*i].kind.noun_kind(),
            Item::Target(_) | Item::Status => return None,
        };
        Some(NounId {
            account: AccountId("local".into()),
            kind: NounKind::register(kind),
            stable: StableId(key.into()),
        })
    }

    fn contains(&self, key: &str) -> bool {
        self.by_key.contains_key(key)
    }

    fn badge(&self) -> Option<u64> {
        matches!(self.source, Source::Read).then(|| self.unresolved_count() as u64)
    }

    fn select(&mut self, key: Option<&str>) {
        self.selected = key.map(str::to_string);
    }

    fn commands(&self) -> Vec<CommandSpec> {
        actions::commands(self)
    }

    fn run(&mut self, command: &str) -> Vec<Intent> {
        actions::run(self, command)
    }

    fn answer(&mut self, command: &str, answer: &DialogAnswer) -> Vec<Intent> {
        actions::answer(self, command, answer)
    }

    fn link(&self, key: &str) -> Option<Link> {
        let Some(Item::Run(i)) = self.by_key.get(key) else {
            return None;
        };
        let key = self.runs[*i]
            .blocks
            .iter()
            .find(|b| actions::audit_finding(b).is_some())?
            .strip_prefix(actions::RULESET_BLOCK)?;
        Some(Link {
            module: crate::modules::audit::ID,
            key: key.to_string(),
            label: "Reveal blocking finding in Audit".into(),
        })
    }

    fn attr_rows(&self) -> Vec<NounAttrRow> {
        let t = |s: &str| AttrValue::Text(s.into());
        let runs = self.runs.iter().map(|r| {
            let mut f = BTreeMap::new();
            f.insert("target".into(), t(&r.target));
            f.insert("state".into(), t(r.phase.label()));
            (r.id.as_str(), f)
        });
        let evidence = self.evidence.iter().map(|e| {
            let mut f = BTreeMap::new();
            f.insert("run".into(), t(&e.run));
            f.insert("name".into(), t(&e.name));
            f.insert("status".into(), t(&e.status));
            (e.id.as_str(), f)
        });
        runs.chain(evidence)
            .filter_map(|(key, fields)| {
                Some(NounAttrRow {
                    noun: self.noun(key)?,
                    fields,
                })
            })
            .collect()
    }
}

mod actions;

pub use actions::{APPROVE, DISCARD, DISPATCH, PREFLIGHT_PREFIX, RECONCILE};

#[cfg(test)]
mod tests;
