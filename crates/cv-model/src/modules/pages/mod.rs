//! Pages — the Trunk/Satellite graph as a mailbox tree. Trunks are pages
//! without a parent; every page with satellites is a nested mailbox whose
//! list shows its children. Diagnostics attach to pages by the source path
//! Boris cites and are the same `Diagnostic` values Activity shows.

mod edit;
mod lifecycle;
pub mod port;

use super::activity::severity_tone;
use crate::command::{CommandSpec, Intent};
use crate::dialog::DialogAnswer;
use crate::module::{
    Chip, FieldSource, InspectorModel, Link, Module, ModuleSpec, NavCache, NavItem, NavModel,
    PageRecord, ProjectSnapshot, ReadingLine, ReadingModel, Registry, RowModel, Slot, Tone,
};
use cv_core::diagnostic::{Diagnostic, Severity};
use cv_core::engine::{MoveOutcome, PageDraft, PageMove, RemoveOutcome, WriteOutcome};
use cv_core::events::engine::EngineEvent;
use cv_core::events::pages::PagesEvent;
use cv_core::events::{AppEvent, Envelope};
use cv_core::id::{AccountId, AttrValue, NounAttrRow, NounId, NounKind, StableId, noun_kind};
use port::PagesPort;
use std::cell::RefCell;
use std::collections::BTreeMap;

pub const ID: &str = "pages";
/// Row key for diagnostics that cite no page (project-level failures).
pub const PROJECT_KEY: &str = "(project)";

const ALL: &str = "all";
const TRUNKS: &str = "trunks";
const ISSUES: &str = "issues";
const ORPHANS: &str = "orphans";
const TREE: &str = "tree:";
const NO_PARENT: &str = "<trunk>";

type Seed = (Box<dyn PagesPort>, String);
thread_local! {
    static SEED: RefCell<Option<Seed>> = const { RefCell::new(None) };
}

/// Composition-root hook: lifecycle verbs write through `port`, and
/// input-relative page paths are interpreted under `content_dir`.
pub fn install_in(port: Box<dyn PagesPort>, content_dir: &str) {
    SEED.with(|s| *s.borrow_mut() = Some((port, content_dir.into())));
}

pub fn register(r: &mut Registry) {
    let (port, dir) = SEED
        .with(|s| s.borrow_mut().take())
        .unwrap_or_else(|| (Box::new(UnavailablePort), "content".into()));
    let mut m = PagesModule::new(port);
    m.content_dir = dir;
    r.register(Box::new(m)).expect("pages module id is unique");
}

struct UnavailablePort;
impl PagesPort for UnavailablePort {
    fn read(&self, _: &str) -> Result<port::Source, String> {
        Err("page lifecycle is unavailable without a writable workspace".into())
    }
    fn write(&mut self, _: &PageDraft) -> Result<WriteOutcome, String> {
        Err("page lifecycle is unavailable without a writable workspace".into())
    }
    fn remove(
        &mut self,
        _: &str,
        _: cv_core::engine::Fingerprint,
    ) -> Result<RemoveOutcome, String> {
        Err("page lifecycle is unavailable without a writable workspace".into())
    }
    fn rename(&mut self, _: &PageMove) -> Result<MoveOutcome, String> {
        Err("page lifecycle is unavailable without a writable workspace".into())
    }
}

pub struct PagesModule {
    pages: Vec<PageRecord>,
    by_path: BTreeMap<String, usize>,
    by_id: BTreeMap<String, usize>,
    children: BTreeMap<String, Vec<usize>>,
    /// Depth-first tree order (trunks, then their satellites).
    order: Vec<usize>,
    /// Each page's position in `order`.
    order_pos: Vec<usize>,
    depth: Vec<usize>,
    trunks: Vec<usize>,
    orphans: Vec<usize>,
    issues: Vec<usize>,
    failed: bool,
    /// The latest build's diagnostics, verbatim.
    diagnostics: Vec<Diagnostic>,
    build: Option<String>,
    /// Invalidated by `load` and `ingest`, the only mutators.
    nav_cache: NavCache,
    focus: Option<String>,
    /// Workspace-relative content input (`content`, or another profile input).
    content_dir: String,
    port: Box<dyn PagesPort>,
}

impl Default for PagesModule {
    fn default() -> Self {
        Self::new(Box::new(UnavailablePort))
    }
}

impl PagesModule {
    pub fn pages(&self) -> &[PageRecord] {
        &self.pages
    }

    pub fn diagnostics_for(&self, path: &str) -> Vec<&Diagnostic> {
        self.diagnostics
            .iter()
            .filter(|d| match (&d.span, path) {
                (Some(s), p) => s.path == p,
                (None, p) => p == PROJECT_KEY,
            })
            .chain(self.diagnostics.iter().filter(|d| {
                path == PROJECT_KEY
                    && d.span
                        .as_ref()
                        .is_some_and(|s| !self.by_path.contains_key(&s.path))
            }))
            .collect()
    }

    /// A failed build with nothing located on a page still gets a row:
    /// a failure is never invisible in Pages.
    fn unattached(&self) -> bool {
        self.failed || !self.diagnostics_for(PROJECT_KEY).is_empty()
    }

    /// Diagnostics computed against an older build render dimmed.
    pub fn is_stale(&self, d: &Diagnostic) -> bool {
        match (&d.build_version, &self.build) {
            (Some(b), Some(cur)) => &b.0 != cur,
            _ => false,
        }
    }

    fn rebuild(&mut self) {
        self.by_path.clear();
        self.by_id.clear();
        self.children.clear();
        self.trunks.clear();
        self.orphans.clear();
        for (i, p) in self.pages.iter().enumerate() {
            self.by_path.insert(p.path.clone(), i);
            self.by_id.entry(p.id.clone()).or_insert(i);
        }
        for (i, p) in self.pages.iter().enumerate() {
            match &p.parent {
                None => self.trunks.push(i),
                Some(par) if self.by_id.contains_key(par) && par != &p.id => {
                    self.children.entry(par.clone()).or_default().push(i)
                }
                Some(_) => self.orphans.push(i),
            }
        }
        let title = |i: &usize| self.pages[*i].title.to_lowercase();
        self.trunks.sort_by_key(title);
        self.orphans.sort_by_key(title);
        for v in self.children.values_mut() {
            v.sort_by_key(|i| self.pages[*i].title.to_lowercase());
        }
        self.order.clear();
        self.depth = vec![0; self.pages.len()];
        let mut seen = vec![false; self.pages.len()];
        let mut stack: Vec<(usize, usize)> = self
            .trunks
            .iter()
            .chain(&self.orphans)
            .rev()
            .map(|&i| (i, 0))
            .collect();
        while let Some((i, d)) = stack.pop() {
            if std::mem::replace(&mut seen[i], true) {
                continue;
            }
            self.order.push(i);
            self.depth[i] = d;
            if let Some(kids) = self.children.get(&self.pages[i].id) {
                stack.extend(kids.iter().rev().map(|&k| (k, d + 1)));
            }
        }
        // Parent cycles reach no trunk; list them rather than drop them.
        for (i, _) in seen.iter().enumerate().filter(|(_, s)| !**s) {
            self.orphans.push(i);
            self.order.push(i);
        }
        self.order_pos = vec![0; self.pages.len()];
        for (pos, &i) in self.order.iter().enumerate() {
            self.order_pos[i] = pos;
        }
    }

    fn view(&self, nav: Option<&str>) -> &[usize] {
        match nav.unwrap_or(ALL) {
            ALL => &self.order,
            TRUNKS => &self.trunks,
            ORPHANS => &self.orphans,
            ISSUES => &self.issues,
            k => k
                .strip_prefix(TREE)
                .and_then(|id| self.children.get(id))
                .map_or(&[], Vec::as_slice),
        }
    }

    /// O(pages + diagnostics): runs on every build event, so a rebuild
    /// burst over a large project must not pay pages × diagnostics.
    fn reindex_issues(&mut self) {
        let cited: std::collections::BTreeSet<&str> = self
            .diagnostics
            .iter()
            .filter_map(|d| d.span.as_ref().map(|s| s.path.as_str()))
            .collect();
        self.issues = self
            .order
            .iter()
            .copied()
            .filter(|&i| {
                let path = self.pages[i].path.as_str();
                if path == PROJECT_KEY {
                    !self.diagnostics_for(path).is_empty()
                } else {
                    cited.contains(path)
                }
            })
            .collect();
    }

    fn build_nav(&self) -> NavModel {
        let n = |v: usize| Some(v as u64);
        let mut items = vec![
            NavItem {
                key: ALL.into(),
                label: "All pages".into(),
                count: n(self.order.len()),
            },
            NavItem {
                key: TRUNKS.into(),
                label: "Trunks".into(),
                count: n(self.trunks.len()),
            },
            NavItem {
                key: ISSUES.into(),
                label: "With diagnostics".into(),
                count: n(self.list_len(Some(ISSUES))),
            },
            NavItem {
                key: ORPHANS.into(),
                label: "Missing parent".into(),
                count: n(self.orphans.len()),
            },
        ];
        for &i in &self.order {
            let p = &self.pages[i];
            if let Some(kids) = self.children.get(&p.id) {
                items.push(NavItem {
                    key: format!("{TREE}{}", p.id),
                    label: format!("{}{}", "  ".repeat(self.depth[i]), p.title),
                    count: n(kids.len()),
                });
            }
        }
        NavModel { items }
    }

    fn ancestors(&self, i: usize) -> Vec<usize> {
        let mut out = vec![];
        let mut cur = self.pages[i]
            .parent
            .as_ref()
            .and_then(|p| self.by_id.get(p))
            .copied();
        while let Some(c) = cur {
            if out.contains(&c) || c == i {
                break;
            }
            out.push(c);
            cur = self.pages[c]
                .parent
                .as_ref()
                .and_then(|p| self.by_id.get(p))
                .copied();
        }
        out.reverse();
        out
    }

    fn title_of(&self, id: &str) -> String {
        self.by_id.get(id).map_or_else(
            || format!("{id} (missing)"),
            |&i| self.pages[i].title.clone(),
        )
    }

    fn worst(&self, path: &str) -> Option<Severity> {
        let rank = |s: &Severity| match s {
            Severity::Error => 0,
            Severity::Warning => 1,
            Severity::Note => 2,
        };
        self.diagnostics_for(path)
            .iter()
            .map(|d| d.severity)
            .min_by_key(rank)
    }

    fn new(port: Box<dyn PagesPort>) -> Self {
        Self {
            pages: Vec::new(),
            by_path: BTreeMap::new(),
            by_id: BTreeMap::new(),
            children: BTreeMap::new(),
            order: Vec::new(),
            order_pos: Vec::new(),
            depth: Vec::new(),
            trunks: Vec::new(),
            orphans: Vec::new(),
            issues: Vec::new(),
            failed: false,
            diagnostics: Vec::new(),
            build: None,
            nav_cache: NavCache::default(),
            focus: None,
            content_dir: "content".into(),
            port,
        }
    }

    fn workspace_path(&self, input_rel: &str) -> String {
        match self.content_dir.as_str() {
            "" | "." => input_rel.into(),
            d => format!("{}/{}", d.trim_end_matches('/'), input_rel),
        }
    }

    fn focused(&mut self) -> Option<usize> {
        let key = self.focus.take()?;
        self.by_path.get(&key).copied()
    }

    fn descendants(&self, i: usize) -> std::collections::BTreeSet<String> {
        let mut out = std::collections::BTreeSet::new();
        let mut stack: Vec<usize> = self
            .children
            .get(&self.pages[i].id)
            .cloned()
            .unwrap_or_default();
        while let Some(k) = stack.pop() {
            if out.insert(self.pages[k].id.clone()) {
                stack.extend(
                    self.children
                        .get(&self.pages[k].id)
                        .into_iter()
                        .flatten()
                        .copied(),
                );
            }
        }
        out
    }

    fn inbound_links(&self, id: &str) -> Vec<String> {
        self.pages
            .iter()
            .filter(|p| p.wikilinks.iter().any(|t| t == id))
            .map(|p| format!("{} ({})", p.title, p.id))
            .collect()
    }

    fn outcome(&self, ev: PagesEvent) -> Vec<Intent> {
        let status = ev.summary();
        vec![Intent::Pages(ev), Intent::Outcome(status)]
    }

    fn failed(&self, verb: &str, path: &str, message: String) -> Vec<Intent> {
        self.outcome(PagesEvent::Failed {
            verb: verb.into(),
            path: path.into(),
            message,
        })
    }

    fn parent_options(&self) -> Vec<String> {
        let mut out = vec![NO_PARENT.to_string()];
        out.extend(self.order.iter().map(|&i| self.pages[i].id.clone()));
        out
    }

    fn rebuild_for_lifecycle(&mut self) {
        self.rebuild();
        self.reindex_issues();
        self.nav_cache.invalidate();
    }

    fn pages_mut(&mut self) -> &mut Vec<PageRecord> {
        &mut self.pages
    }

    fn has_page_id(&self, id: &str) -> bool {
        self.by_id.contains_key(id)
    }

    fn page_id_title(&self, path: &str) -> Option<(String, String)> {
        let p = &self.pages[*self.by_path.get(path)?];
        Some((p.id.clone(), p.title.clone()))
    }

    fn page_id_title_parent(&self, path: &str) -> Option<(String, String, Option<String>)> {
        let p = &self.pages[*self.by_path.get(path)?];
        Some((p.id.clone(), p.title.clone(), p.parent.clone()))
    }

    fn move_page_path(&mut self, from: &str, to: &str) {
        if let Some(&i) = self.by_path.get(from) {
            self.pages[i].path = to.into();
        }
    }

    fn set_page_parent(&mut self, path: &str, parent: Option<&str>) {
        if let Some(&i) = self.by_path.get(path) {
            self.pages[i].parent = parent.map(str::to_string);
        }
    }

    fn remove_page(&mut self, path: &str) {
        if let Some(i) = self.by_path.get(path).copied() {
            self.pages.remove(i);
        }
    }

    /// Existing ids a page may point at: itself and its descendants are
    /// excluded so the dialog cannot offer an immediate parent cycle.
    fn reparent_targets(&self, path: &str) -> Vec<(String, String)> {
        let Some(&i) = self.by_path.get(path) else {
            return Vec::new();
        };
        let mut excluded = self.descendants(i);
        excluded.insert(self.pages[i].id.clone());
        self.order
            .iter()
            .map(|&k| &self.pages[k])
            .filter(|p| !excluded.contains(&p.id))
            .map(|p| (p.id.clone(), format!("{} ({})", p.title, p.id)))
            .collect()
    }

    /// Inbound references and direct child satellites the latest Boris IR
    /// reported. Source-only pages are new to that IR, so their impact is
    /// disclosed as unknown rather than mistaken for an empty impact.
    fn delete_impact(&self, path: &str) -> String {
        let Some(&i) = self.by_path.get(path) else {
            return "Boris IR no longer lists this page.".into();
        };
        let p = &self.pages[i];
        if p.fields == FieldSource::Source {
            return "Boris IR does not cover this page yet; inbound [[links]] and child satellites are unknown.".into();
        }
        let links = self.inbound_links(&p.id);
        let kids: Vec<String> = self
            .children
            .get(&p.id)
            .into_iter()
            .flatten()
            .filter(|&&k| self.pages[k].fields == FieldSource::Boris)
            .map(|&k| format!("{} ({})", self.pages[k].title, self.pages[k].id))
            .collect();
        let incomplete = self.pages.iter().any(|p| p.fields == FieldSource::Source);
        let mut out = String::new();
        if links.is_empty() && kids.is_empty() {
            out.push_str(
                "The latest Boris IR reports no inbound [[links]] and no child satellites.",
            );
        } else {
            if !links.is_empty() {
                out.push_str("Inbound [[links]]:\n");
                for l in links {
                    out.push_str(&format!("• {l}\n"));
                }
            }
            if !kids.is_empty() {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str("Child satellites:\n");
                for k in kids {
                    out.push_str(&format!("• {k}\n"));
                }
            }
        }
        if incomplete {
            out.push_str(
                "\n\nSome pages are not covered by that IR yet; this impact may be incomplete.",
            );
        }
        out.trim_end().to_string()
    }

    fn referenced_by(&self, id: &str) -> Vec<String> {
        self.pages
            .iter()
            .flat_map(|p| {
                p.relations
                    .iter()
                    .filter(move |(_, t)| t == id)
                    .map(move |(r, _)| format!("{} ({r})", p.title))
            })
            .collect()
    }
}

impl Module for PagesModule {
    fn spec(&self) -> ModuleSpec {
        ModuleSpec {
            id: ID,
            title: "Pages",
            slot: Slot::Pages,
        }
    }

    fn ingest(&mut self, env: &Envelope) -> bool {
        let AppEvent::Engine(ev) = &env.payload else {
            return false;
        };
        let (diags, failed) = match ev {
            EngineEvent::BuildFinished { diagnostics, .. } => (diagnostics, false),
            EngineEvent::BuildFailed { diagnostics, .. } => (diagnostics, true),
            _ => return false,
        };
        self.failed = failed && diags.iter().all(|d| d.span.is_none());
        self.diagnostics = diags.clone();
        self.build = env.build_id.as_ref().map(|b| b.0.clone());
        self.reindex_issues();
        self.nav_cache.invalidate();
        true
    }

    fn load(&mut self, snap: &ProjectSnapshot) -> bool {
        self.pages = snap.pages.clone();
        self.rebuild();
        self.reindex_issues();
        self.nav_cache.invalidate();
        true
    }

    fn nav(&self) -> NavModel {
        self.nav_cache.get_or_build(|| self.build_nav())
    }

    fn list_len(&self, nav: Option<&str>) -> usize {
        let extra = nav == Some(ISSUES) && self.unattached();
        self.view(nav).len() + usize::from(extra)
    }

    fn row(&self, nav: Option<&str>, i: usize) -> RowModel {
        let v = self.view(nav);
        let Some(&pi) = v.get(i) else {
            let v = v.len();
            if nav == Some(ISSUES) && i == v && self.unattached() {
                let n = self.diagnostics_for(PROJECT_KEY).len();
                return RowModel {
                    key: PROJECT_KEY.into(),
                    title: "Project-level diagnostics".into(),
                    detail: if n == 0 {
                        "build failed — no located diagnostic".into()
                    } else {
                        "not attached to a page".into()
                    },
                    badge: Some(Chip::new(
                        format!("{n} diag"),
                        self.worst(PROJECT_KEY).map_or(Tone::Error, severity_tone),
                    )),
                    tone: self.worst(PROJECT_KEY).map_or(Tone::Error, severity_tone),
                    rail: Some(self.worst(PROJECT_KEY).map_or(Tone::Error, severity_tone)),
                    path: false,
                };
            }
            return RowModel::default();
        };
        let p = &self.pages[pi];
        let diags = self.diagnostics_for(&p.path);
        let kids = self.children.get(&p.id).map_or(0, Vec::len);
        let indent = if nav.unwrap_or(ALL) == ALL {
            "  ".repeat(self.depth[pi])
        } else {
            String::new()
        };
        let stale = !diags.is_empty() && diags.iter().all(|d| self.is_stale(d));
        let tone = match self.worst(&p.path) {
            _ if stale => Tone::Dim,
            Some(s) => severity_tone(s),
            None => Tone::Normal,
        };
        RowModel {
            key: p.path.clone(),
            title: format!("{indent}{}", p.title),
            detail: if kids > 0 {
                format!("{} · {kids} satellites", p.path)
            } else {
                p.path.clone()
            },
            badge: (!diags.is_empty()).then(|| Chip::new(format!("{} diag", diags.len()), tone)),
            tone,
            rail: self.worst(&p.path).map(severity_tone),
            path: false,
        }
    }

    fn index_of_key(&self, nav: Option<&str>, key: &str) -> Option<usize> {
        let v = self.view(nav);
        let page = match self.by_path.get(key) {
            // `by_path` keeps the last page per path, so it names the only
            // candidate just when paths are unique.
            Some(&pi) if self.by_path.len() == self.pages.len() => match nav.unwrap_or(ALL) {
                ALL => self.order_pos.get(pi).copied(),
                _ => v.iter().position(|&x| x == pi),
            },
            Some(_) => v.iter().position(|&x| self.pages[x].path == key),
            None => None,
        };
        let project = key == PROJECT_KEY && nav == Some(ISSUES) && self.unattached();
        page.or(project.then_some(v.len()))
    }

    fn reading(&self, key: &str) -> ReadingModel {
        if key == PROJECT_KEY {
            return ReadingModel {
                title: "Project-level diagnostics".into(),
                lines: self
                    .failed
                    .then(|| ReadingLine {
                        label: Some("build".into()),
                        text: "the latest build failed; see Activity for the full event".into(),
                        tone: Tone::Error,
                        mono: false,
                    })
                    .into_iter()
                    .chain(
                        self.diagnostics_for(PROJECT_KEY)
                            .into_iter()
                            .map(|d| ReadingLine {
                                label: Some(d.code.clone()),
                                text: d.message.clone(),
                                tone: severity_tone(d.severity),
                                mono: false,
                            }),
                    )
                    .collect(),
                ..Default::default()
            };
        }
        let Some(&i) = self.by_path.get(key) else {
            return ReadingModel {
                title: key.into(),
                ..Default::default()
            };
        };
        let p = &self.pages[i];
        let line = |l: &str, t: String| ReadingLine {
            label: Some(l.into()),
            text: t,
            tone: Tone::Normal,
            mono: false,
        };
        let mut lines = vec![];
        for &k in self.children.get(&p.id).into_iter().flatten() {
            lines.push(line("satellite", self.pages[k].title.clone()));
        }
        for (rel, target) in &p.relations {
            lines.push(line(rel, self.title_of(target)));
        }
        if !p.tags.is_empty() {
            lines.push(line("tags", p.tags.join(", ")));
        }
        let mut crumbs: Vec<String> = self
            .ancestors(i)
            .into_iter()
            .map(|a| self.pages[a].title.clone())
            .collect();
        if let Some(par) = p
            .parent
            .as_ref()
            .filter(|par| !self.by_id.contains_key(*par))
        {
            crumbs.push(format!("{par} (missing)"));
        }
        crumbs.push(p.title.clone());
        ReadingModel {
            title: p.title.clone(),
            lines,
            breadcrumbs: crumbs,
            markdown: Some(p.body.clone()),
        }
    }

    fn inspector(&self, key: &str) -> InspectorModel {
        let diagnostics: Vec<Diagnostic> = self.diagnostics_for(key).into_iter().cloned().collect();
        let Some(&i) = self.by_path.get(key) else {
            return InspectorModel {
                diagnostics,
                ..Default::default()
            };
        };
        let p = &self.pages[i];
        let mut fields = vec![
            ("id".to_string(), p.id.clone()),
            (
                "role".into(),
                if p.parent.is_none() {
                    "trunk"
                } else {
                    "satellite"
                }
                .into(),
            ),
        ];
        if p.fields == FieldSource::Source {
            fields.push((
                "fields".into(),
                "from source frontmatter, no Boris IR yet".into(),
            ));
        }
        if let Some((mode, source)) = &p.imported_from {
            fields.push(("imported from".into(), format!("{mode} · {source}")));
        }
        fields.extend(p.frontmatter.iter().filter(|(k, _)| k != "id").cloned());
        if let Some(b) = &self.build {
            fields.push(("build".into(), b.clone()));
        }
        let mut descendants = vec![];
        let mut stack: Vec<usize> = self.children.get(&p.id).cloned().unwrap_or_default();
        while let Some(k) = stack.pop() {
            if descendants.len() > self.pages.len() {
                break;
            }
            descendants.push(self.pages[k].title.clone());
            stack.extend(self.children.get(&self.pages[k].id).into_iter().flatten());
        }
        InspectorModel {
            badges: vec![],
            fields,
            diagnostics,
            lists: vec![
                ("Satellites (nav, breadcrumbs)".into(), descendants),
                ("Referenced by".into(), self.referenced_by(&p.id)),
                ("Outputs".into(), p.output.iter().cloned().collect()),
            ],
        }
    }

    fn noun(&self, key: &str) -> Option<NounId> {
        let p = &self.pages[*self.by_path.get(key)?];
        Some(NounId {
            account: AccountId("local".into()),
            kind: NounKind::register(noun_kind::PAGE),
            stable: StableId(p.id.clone()),
        })
    }

    fn contains(&self, key: &str) -> bool {
        self.by_path.contains_key(key) || key == PROJECT_KEY
    }

    fn source(&self, key: &str) -> Option<String> {
        self.by_path.get(key).map(|&i| self.pages[i].path.clone())
    }

    fn link(&self, key: &str) -> Option<Link> {
        let p = &self.pages[*self.by_path.get(key)?];
        p.output.as_ref().map(|o| Link {
            module: super::outputs::ID,
            key: o.clone(),
            label: "Open output".into(),
        })
    }

    fn key_for(&self, noun: &NounId) -> Option<String> {
        if noun.kind.as_str() != noun_kind::PAGE {
            return None;
        }
        self.by_id
            .get(&noun.stable.0)
            .map(|&i| self.pages[i].path.clone())
    }

    fn select(&mut self, key: Option<&str>) {
        self.focus = key.map(str::to_string);
    }

    fn commands(&self) -> Vec<CommandSpec> {
        lifecycle::commands()
    }

    fn run(&mut self, command: &str) -> Vec<Intent> {
        lifecycle::run(self, command)
    }

    fn answer(&mut self, command: &str, answer: &DialogAnswer) -> Vec<Intent> {
        lifecycle::answer(self, command, answer)
    }

    fn attr_rows(&self) -> Vec<NounAttrRow> {
        self.order
            .iter()
            .filter_map(|&i| {
                let p = &self.pages[i];
                let noun = self.noun(&p.path)?;
                let diags = self.diagnostics_for(&p.path);
                let t = |s: &str| AttrValue::Text(s.into());
                let mut f = BTreeMap::new();
                f.insert("path".into(), t(&p.path));
                f.insert("title".into(), t(&p.title));
                f.insert(
                    "role".into(),
                    t(if p.parent.is_none() {
                        "trunk"
                    } else {
                        "satellite"
                    }),
                );
                if let Some(par) = &p.parent {
                    f.insert("parent".into(), t(par));
                }
                f.insert("tags".into(), t(&p.tags.join(",")));
                f.insert("depth".into(), AttrValue::Int(self.depth[i] as i64));
                f.insert("diagnostics".into(), AttrValue::Int(diags.len() as i64));
                f.insert(
                    "errors".into(),
                    AttrValue::Int(
                        diags
                            .iter()
                            .filter(|d| d.severity == Severity::Error)
                            .count() as i64,
                    ),
                );
                Some(NounAttrRow { noun, fields: f })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests;
