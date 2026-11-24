//! Audit — the Findings mailbox (L4 §6, minimal slice). Every
//! `BuildFinished` re-evaluates the shipped rule packs (`cv-rules::eval`,
//! pure) over the build's artifact set; findings carry the lifecycle
//! `open → fixed | ignored (snoozed) | wontfix`, keyed by
//! `DedupKey = code + subject + hash(target)`, so a finding that re-emits
//! on the next build keeps its snooze. Lifecycle transitions persist
//! through an injected [`AuditPort`] (the `.cv/audit/` sidecar on disk).
//!
//! Heuristic-tier findings are advisory: they never count as blocking
//! (`AuditFinding::blocks_publish` → `AuthorityTier::may_block`).

use crate::command::{CommandSpec, Intent};
use crate::module::{
    InspectorModel, Link, Module, ModuleSpec, NavItem, NavModel, OutputRecord, ProjectSnapshot,
    ReadingLine, ReadingModel, Registry, RowModel, Slot, Tone,
};
use cv_core::events::engine::EngineEvent;
use cv_core::events::{AppEvent, Envelope};
use cv_core::id::{AccountId, NounId, NounKind, StableId, noun_kind};
use cv_core::rules::{
    ArtifactKind, AuthorityTier, CheckStatus, DedupKey, FindingLifecycle, Severity,
};
use cv_core::sys::Hash;
use cv_rules::{ArtifactSet, AuditFinding, Export, HtmlPage, IrGraph, Ruleset};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

pub const ID: &str = "audit";

/// Packs evaluated on every build: the full shipped v1 set minus
/// `targets`, whose canonical/parity checks need a deploy profile
/// (`base_url`) the build snapshot does not carry — without one every
/// `targets` rule would only report "not checked".
pub const DEFAULT_PACKS: &[&str] = &["a11y-content", "aeo-llm", "seo-core"];

pub fn default_ruleset() -> Ruleset {
    let all = Ruleset::v1();
    let mut out = all.clone();
    for p in all.packs() {
        if !DEFAULT_PACKS.contains(&p.meta.id.as_str()) {
            out = out.without_pack(&p.meta.id);
        }
    }
    out
}

/// IO boundary: artifact bytes in, lifecycle transitions out.
pub trait AuditPort {
    /// Bytes of `rel` (an artifact-index path) under the build's `root`.
    fn read(&self, root: &str, rel: &str) -> Result<Vec<u8>, String>;
    /// Persisted lifecycle per key (last transition wins).
    fn lifecycle(&mut self) -> Result<Vec<(DedupKey, FindingLifecycle)>, String>;
    fn record(&mut self, key: &DedupKey, to: FindingLifecycle) -> Result<(), String>;
    fn describe(&self) -> String;
}

/// In-memory port: `files` by artifact path (root ignored); transitions
/// land in `records`.
#[derive(Default)]
pub struct MemoryPort {
    pub files: BTreeMap<String, Vec<u8>>,
    pub records: Vec<(DedupKey, FindingLifecycle)>,
}

impl AuditPort for MemoryPort {
    fn read(&self, _root: &str, rel: &str) -> Result<Vec<u8>, String> {
        self.files
            .get(rel)
            .cloned()
            .ok_or_else(|| format!("{rel}: not available"))
    }
    fn lifecycle(&mut self) -> Result<Vec<(DedupKey, FindingLifecycle)>, String> {
        Ok(self.records.clone())
    }
    fn record(&mut self, key: &DedupKey, to: FindingLifecycle) -> Result<(), String> {
        self.records.push((key.clone(), to));
        Ok(())
    }
    fn describe(&self) -> String {
        "in memory".into()
    }
}

thread_local! {
    static SEED: RefCell<Option<Box<dyn AuditPort>>> = const { RefCell::new(None) };
}

/// Composition-root hook: the next `register` on this thread uses `port`.
pub fn install(port: Box<dyn AuditPort>) {
    SEED.with(|s| *s.borrow_mut() = Some(port));
}

pub fn register(r: &mut Registry) {
    let port = SEED
        .with(|s| s.borrow_mut().take())
        .unwrap_or_else(|| Box::new(MemoryPort::default()));
    r.register(Box::new(AuditModule::new(port, default_ruleset())))
        .expect("audit module id is unique");
}

/// Stable row key for a dedup key.
pub fn key_id(k: &DedupKey) -> String {
    format!(
        "{}|{}:{}|{}",
        k.code,
        k.subject.kind.0,
        k.subject.stable.0,
        k.target_hash.to_hex()
    )
}

pub fn lifecycle_label(l: FindingLifecycle) -> &'static str {
    match l {
        FindingLifecycle::Open => "open",
        FindingLifecycle::Fixed => "fixed",
        FindingLifecycle::Ignored => "snoozed",
        FindingLifecycle::Wontfix => "won't fix",
    }
}

fn severity_label(s: Severity) -> &'static str {
    match s {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Info => "info",
    }
}

fn tier_label(t: AuthorityTier) -> &'static str {
    match t {
        AuthorityTier::NormativeSpec => "normative spec",
        AuthorityTier::EngineDocumented => "engine documented",
        AuthorityTier::Heuristic => "heuristic (advisory)",
    }
}

pub struct Entry {
    pub id: String,
    pub af: AuditFinding,
    pub state: FindingLifecycle,
}

impl Entry {
    fn failed(&self) -> bool {
        self.af.finding.status == CheckStatus::Failed
    }

    fn blocks_publish(&self) -> bool {
        self.state != FindingLifecycle::Fixed && self.af.blocks_publish()
    }
}

/// Counts of the newest refresh (`AuditEvent::FindingsRefreshed` shape).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Refresh {
    pub opened: u32,
    pub fixed: u32,
    pub carried: u32,
}

pub struct AuditModule {
    port: Box<dyn AuditPort>,
    rules: Ruleset,
    snap: ProjectSnapshot,
    entries: Vec<Entry>,
    views: BTreeMap<String, Vec<usize>>,
    /// Persisted non-default lifecycle by key id.
    overrides: BTreeMap<String, FindingLifecycle>,
    hashes: Option<(Hash, Hash)>,
    refresh: Option<Refresh>,
    /// Artifacts that could not be read for the newest evaluation.
    unreadable: Vec<String>,
    /// The newest build failed: findings are from the last good one.
    stale: bool,
    store_error: Option<String>,
    selected: Option<String>,
}

const FIXED_VIEWS: &[(&str, &str)] = &[
    ("open", "Open"),
    ("blocking", "Blocking"),
    ("error", "Errors"),
    ("warning", "Warnings"),
    ("info", "Info"),
    ("snoozed", "Snoozed"),
    ("wontfix", "Won't fix"),
    ("fixed", "Fixed"),
    ("limited", "Not checked"),
];

impl AuditModule {
    pub fn new(mut port: Box<dyn AuditPort>, rules: Ruleset) -> Self {
        let mut overrides = BTreeMap::new();
        let store_error = match port.lifecycle() {
            Ok(v) => {
                for (k, l) in v {
                    if l == FindingLifecycle::Open {
                        overrides.remove(&key_id(&k));
                    } else {
                        overrides.insert(key_id(&k), l);
                    }
                }
                None
            }
            Err(e) => Some(e),
        };
        let mut m = Self {
            port,
            rules,
            snap: ProjectSnapshot::default(),
            entries: Vec::new(),
            views: BTreeMap::new(),
            overrides,
            hashes: None,
            refresh: None,
            unreadable: Vec::new(),
            stale: false,
            store_error,
            selected: None,
        };
        m.reindex();
        m
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn refresh(&self) -> Option<Refresh> {
        self.refresh
    }

    pub fn open_count(&self) -> usize {
        self.views.get("open").map_or(0, Vec::len)
    }

    /// Assemble the build's artifact set from the snapshot's artifact index.
    pub fn artifact_set(&self) -> (ArtifactSet, Vec<String>) {
        let mut b = ArtifactSet::builder().account(AccountId("local".into()));
        let mut errs = Vec::new();
        let root = self.snap.artifact_root.as_deref();
        let read = |o: &OutputRecord| match root {
            Some(r) => self.port.read(r, &o.path),
            None => Err(format!("{}: no artifact root", o.path)),
        };
        let pages: Vec<&OutputRecord> = self
            .snap
            .outputs
            .iter()
            .filter(|o| o.kind == "html-page")
            .collect();
        if pages.is_empty() {
            b = b.not_emitted(ArtifactKind::Html);
        }
        let mut html_missing = false;
        for o in pages {
            let entity = self
                .snap
                .pages
                .iter()
                .find(|p| p.output.as_deref() == Some(o.path.as_str()))
                .map(|p| p.id.as_str());
            match read(o).and_then(|bytes| {
                HtmlPage::parse(entity, &o.path, &bytes).map_err(|e| e.to_string())
            }) {
                Ok(p) => b = b.page(p),
                Err(e) => {
                    html_missing = true;
                    errs.push(e);
                }
            }
        }
        if html_missing {
            b = b.missing(ArtifactKind::Html);
        }
        type Parse = fn(&str, &[u8]) -> Result<Export, cv_rules::Error>;
        let exports: [(ArtifactKind, &str, &str, Parse); 6] = [
            (
                ArtifactKind::Sitemap,
                "sitemap",
                ".xml",
                Export::sitemap_xml,
            ),
            (ArtifactKind::Rss, "rss", ".xml", Export::rss_xml),
            (ArtifactKind::Llms, "llms", ".txt", Export::llms_txt),
            (
                ArtifactKind::Rag,
                "rag",
                ".jsonl",
                Export::rag_catalog_jsonl,
            ),
            (
                ArtifactKind::ContextBundle,
                "context",
                ".jsonl",
                Export::context_bundle_jsonl,
            ),
            (
                ArtifactKind::RobotsTxt,
                "robots",
                ".txt",
                Export::robots_txt,
            ),
        ];
        for (kind, class, ext, parse) in exports {
            let hit = self.snap.outputs.iter().find(|o| {
                o.path.ends_with(ext)
                    && if class == "robots" {
                        o.path == "robots.txt"
                    } else {
                        super::outputs::class(o) == class
                    }
            });
            let Some(o) = hit else {
                b = b.not_emitted(kind);
                continue;
            };
            match read(o).and_then(|bytes| parse(&o.path, &bytes).map_err(|e| e.to_string())) {
                Ok(e) => b = b.export(e),
                Err(e) => {
                    errs.push(e);
                    b = b.missing(kind);
                }
            }
        }
        match self
            .snap
            .outputs
            .iter()
            .find(|o| super::outputs::class(o) == "ir")
        {
            None => b = b.not_emitted(ArtifactKind::Ir),
            Some(o) => {
                match read(o).and_then(|bytes| {
                    IrGraph::from_json(&o.path, &bytes).map_err(|e| e.to_string())
                }) {
                    Ok(g) => b = b.ir(g),
                    Err(e) => {
                        errs.push(e);
                        b = b.missing(ArtifactKind::Ir);
                    }
                }
            }
        }
        (b.build(), errs)
    }

    fn evaluate(&mut self) {
        let (set, errs) = self.artifact_set();
        let report = cv_rules::eval(&set, &self.rules);
        let limited: BTreeSet<String> =
            report.limited().map(|af| af.finding.code.clone()).collect();
        self.unreadable = errs;
        self.stale = false;
        self.hashes = Some((report.artifact_set_hash, report.ruleset_hash));
        let mut prev: BTreeMap<String, Entry> = std::mem::take(&mut self.entries)
            .into_iter()
            .map(|e| (e.id.clone(), e))
            .collect();
        let mut r = Refresh::default();
        let mut next = Vec::with_capacity(report.findings.len());
        for af in report.findings {
            let id = key_id(&af.finding.key);
            let before = prev.remove(&id);
            let state = self
                .overrides
                .get(&id)
                .copied()
                .unwrap_or(FindingLifecycle::Open);
            if af.finding.status == CheckStatus::Failed {
                match before {
                    Some(b) if b.failed() && b.state != FindingLifecycle::Fixed => r.carried += 1,
                    _ => r.opened += 1,
                }
            }
            next.push(Entry { id, af, state });
        }
        for (_, mut e) in prev {
            if !e.failed() {
                continue;
            }
            if limited.contains(&e.af.finding.code) {
                self.stale = true;
                if e.state != FindingLifecycle::Fixed {
                    r.carried += 1;
                }
                next.push(e);
                continue;
            }
            if e.state != FindingLifecycle::Fixed {
                r.fixed += 1;
            }
            e.state = FindingLifecycle::Fixed;
            next.push(e);
        }
        next.sort_by(|a, b| {
            let k = |e: &Entry| {
                (
                    e.af.pack.clone(),
                    e.af.finding.code.clone(),
                    e.af.finding.subject.clone(),
                    e.af.finding.key.target_hash,
                )
            };
            k(a).cmp(&k(b))
        });
        self.entries = next;
        self.refresh = Some(r);
        self.reindex();
    }

    fn reindex(&mut self) {
        let mut v: BTreeMap<String, Vec<usize>> = FIXED_VIEWS
            .iter()
            .map(|(k, _)| ((*k).to_string(), Vec::new()))
            .collect();
        for p in self.rules.packs() {
            v.insert(format!("pack:{}", p.meta.id), Vec::new());
        }
        for (i, e) in self.entries.iter().enumerate() {
            let f = &e.af.finding;
            let mut push = |k: String| v.entry(k).or_default().push(i);
            if !e.failed() {
                push("limited".into());
                continue;
            }
            match e.state {
                FindingLifecycle::Open => {
                    push("open".into());
                    push(severity_label(e.af.finding.severity()).into());
                    if e.blocks_publish() {
                        push("blocking".into());
                    }
                }
                FindingLifecycle::Ignored => push("snoozed".into()),
                FindingLifecycle::Wontfix => push("wontfix".into()),
                FindingLifecycle::Fixed => {
                    push("fixed".into());
                    continue;
                }
            }
            push(format!("pack:{}", e.af.pack));
            push(format!("rule:{}", f.code));
            push(format!("noun:{}", f.subject.stable.0));
        }
        self.views = v;
    }

    fn view(&self, nav: Option<&str>) -> &[usize] {
        self.views
            .get(nav.unwrap_or("open"))
            .map_or(&[], Vec::as_slice)
    }

    fn entry(&self, key: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.id == key)
    }

    fn transition(&mut self, to: FindingLifecycle) -> Vec<Intent> {
        let Some(key) = self.selected.clone() else {
            return vec![Intent::Status("Select a finding first".into())];
        };
        let Some(i) = self.entries.iter().position(|e| e.id == key) else {
            return vec![Intent::Status("Select a finding first".into())];
        };
        let e = &self.entries[i];
        if !e.failed() || e.state == FindingLifecycle::Fixed {
            return vec![Intent::Status(format!(
                "{} is {} — nothing to change",
                e.af.finding.code,
                if e.failed() { "fixed" } else { "not checked" }
            ))];
        }
        if e.state == to {
            return vec![];
        }
        let dk = e.af.finding.key.clone();
        let code = e.af.finding.code.clone();
        self.entries[i].state = to;
        if to == FindingLifecycle::Open {
            self.overrides.remove(&key);
        } else {
            self.overrides.insert(key, to);
        }
        self.reindex();
        let msg = format!("{code} → {}", lifecycle_label(to));
        match self.port.record(&dk, to) {
            Ok(()) => {
                self.store_error = None;
                vec![Intent::Status(msg)]
            }
            Err(err) => {
                let s = format!("{msg} (not saved: {err})");
                self.store_error = Some(err);
                vec![Intent::Status(s)]
            }
        }
    }

    fn notes(&self) -> Vec<ReadingLine> {
        let warn = |t: String| ReadingLine {
            label: Some("note".into()),
            text: t,
            tone: Tone::Warn,
            mono: false,
        };
        let mut out = Vec::new();
        if self.stale {
            out.push(warn(
                "findings include results from the last good build".into(),
            ));
        }
        if !self.unreadable.is_empty() {
            out.push(warn(format!(
                "{} artifact(s) unreadable — their rules are not checked: {}",
                self.unreadable.len(),
                self.unreadable.join("; ")
            )));
        }
        if let Some(e) = &self.store_error {
            out.push(warn(format!("lifecycle sidecar unavailable: {e}")));
        }
        out
    }
}

impl Module for AuditModule {
    fn spec(&self) -> ModuleSpec {
        ModuleSpec {
            id: ID,
            title: "Audit",
            slot: Slot::Audit,
        }
    }

    fn ingest(&mut self, env: &Envelope) -> bool {
        match &env.payload {
            AppEvent::Engine(EngineEvent::BuildFinished { .. }) => {
                self.evaluate();
                true
            }
            AppEvent::Engine(EngineEvent::BuildFailed { .. }) => {
                let was = self.stale;
                self.stale = self.hashes.is_some();
                was != self.stale
            }
            _ => false,
        }
    }

    fn load(&mut self, snap: &ProjectSnapshot) -> bool {
        self.snap = snap.clone();
        false
    }

    fn nav(&self) -> NavModel {
        let count = |k: &str| Some(self.view(Some(k)).len() as u64);
        let mut items: Vec<NavItem> = FIXED_VIEWS
            .iter()
            .map(|(k, l)| NavItem {
                key: (*k).into(),
                label: (*l).into(),
                count: count(k),
            })
            .collect();
        for (prefix, label) in [("pack:", "Pack"), ("rule:", "Rule"), ("noun:", "Page")] {
            for k in self.views.keys().filter(|k| k.starts_with(prefix)) {
                items.push(NavItem {
                    key: k.clone(),
                    label: format!("{label} · {}", &k[prefix.len()..]),
                    count: count(k),
                });
            }
        }
        NavModel { items }
    }

    fn list_len(&self, nav: Option<&str>) -> usize {
        self.view(nav).len()
    }

    fn row(&self, nav: Option<&str>, i: usize) -> RowModel {
        let Some(e) = self.view(nav).get(i).map(|&j| &self.entries[j]) else {
            return RowModel::default();
        };
        let f = &e.af.finding;
        let failed = e.failed();
        RowModel {
            key: e.id.clone(),
            title: format!("{} · {}", f.code, f.subject.stable.0),
            detail: f.evidence.summary.clone(),
            badge: Some(if failed {
                lifecycle_label(e.state).into()
            } else {
                "not checked".into()
            }),
            tone: match e.state {
                _ if !failed => Tone::Dim,
                FindingLifecycle::Fixed => Tone::Ok,
                FindingLifecycle::Ignored | FindingLifecycle::Wontfix => Tone::Dim,
                FindingLifecycle::Open => Tone::Normal,
            },
            rail: Some(match e.af.finding.severity() {
                _ if !failed => Tone::Dim,
                Severity::Error => Tone::Error,
                Severity::Warning => Tone::Warn,
                Severity::Info => Tone::Normal,
            }),
        }
    }

    fn reading(&self, key: &str) -> ReadingModel {
        let Some(e) = self.entry(key) else {
            return ReadingModel {
                title: key.into(),
                lines: self.notes(),
                ..Default::default()
            };
        };
        let f = &e.af.finding;
        let line = |l: &str, t: String, mono| ReadingLine {
            label: Some(l.into()),
            text: t,
            tone: Tone::Normal,
            mono,
        };
        let mut lines = vec![
            line("subject", f.subject.stable.0.clone(), true),
            line("finding", f.evidence.summary.clone(), false),
            line(
                "lifecycle",
                if e.failed() {
                    lifecycle_label(e.state).into()
                } else {
                    "not checked".into()
                },
                false,
            ),
            line(
                "severity",
                severity_label(e.af.finding.severity()).into(),
                false,
            ),
            line("authority", tier_label(f.tier).into(), false),
            ReadingLine {
                label: Some("publish".into()),
                text: if e.blocks_publish() {
                    "blocks publishing".into()
                } else {
                    "advisory — does not block publishing".into()
                },
                tone: if e.blocks_publish() {
                    Tone::Error
                } else {
                    Tone::Normal
                },
                mono: false,
            },
        ];
        if let Some(fix) = &f.fix {
            lines.push(line("fix", fix.description.clone(), false));
        }
        if let Some((_, rule)) = self.rules.rule(&f.code) {
            lines.push(line("source", rule.citation.source.clone(), false));
            lines.push(line("citation", rule.citation.url.clone(), true));
        }
        lines.extend(self.notes());
        ReadingModel {
            title: f.code.clone(),
            lines,
            breadcrumbs: vec![e.af.pack.clone(), f.code.clone()],
            markdown: None,
        }
    }

    fn inspector(&self, key: &str) -> InspectorModel {
        let Some(e) = self.entry(key) else {
            return InspectorModel::default();
        };
        let f = &e.af.finding;
        let (set, rules) = self.hashes.map_or(("—".into(), "—".into()), |(a, r)| {
            (a.to_hex(), r.to_hex())
        });
        let mut evidence = Vec::new();
        if let Some(serde_json::Value::Object(m)) = &f.evidence.detail {
            for (k, v) in m {
                evidence.push(format!("{k}: {v}"));
            }
        }
        InspectorModel {
            fields: vec![
                ("code".into(), f.code.clone()),
                ("pack".into(), e.af.pack.clone()),
                ("subject".into(), f.subject.stable.0.clone()),
                ("target".into(), f.key.target_hash.to_hex()),
                ("status".into(), format!("{:?}", f.status).to_lowercase()),
                (
                    "lifecycle".into(),
                    if e.failed() {
                        lifecycle_label(e.state)
                    } else {
                        "not checked"
                    }
                    .into(),
                ),
                ("tier".into(), tier_label(f.tier).into()),
                ("blocks".into(), e.blocks_publish().to_string()),
                ("artifacts".into(), set),
                ("ruleset".into(), rules),
                ("packs".into(), DEFAULT_PACKS.join(", ")),
                ("sidecar".into(), self.port.describe()),
            ],
            diagnostics: vec![],
            lists: vec![("Evidence".into(), evidence)],
        }
    }

    fn noun(&self, key: &str) -> Option<NounId> {
        self.entry(key)?;
        Some(NounId {
            account: AccountId("local".into()),
            kind: NounKind(noun_kind::FINDING.into()),
            stable: StableId(key.into()),
        })
    }

    fn select(&mut self, key: Option<&str>) {
        self.selected = key.map(str::to_string);
    }

    fn commands(&self) -> Vec<CommandSpec> {
        vec![
            CommandSpec::new("audit.snooze", "Snooze finding (ignore)"),
            CommandSpec::new("audit.wontfix", "Mark finding won't fix"),
            CommandSpec::new("audit.reopen", "Reopen finding"),
        ]
    }

    fn run(&mut self, command: &str) -> Vec<Intent> {
        match command {
            "audit.snooze" => self.transition(FindingLifecycle::Ignored),
            "audit.wontfix" => self.transition(FindingLifecycle::Wontfix),
            "audit.reopen" => self.transition(FindingLifecycle::Open),
            _ => Vec::new(),
        }
    }

    fn contains(&self, key: &str) -> bool {
        self.entry(key).is_some()
    }

    fn link(&self, key: &str) -> Option<Link> {
        let s = &self.entry(key)?.af.finding.subject.stable.0;
        let page = self
            .snap
            .pages
            .iter()
            .find(|p| p.id == *s || p.output.as_deref() == Some(s.as_str()))?;
        Some(Link {
            module: super::pages::ID,
            key: page.path.clone(),
            label: "Open source page".into(),
        })
    }

    fn badge(&self) -> Option<u64> {
        self.hashes.map(|_| self.open_count() as u64)
    }
}

#[cfg(test)]
mod tests;
