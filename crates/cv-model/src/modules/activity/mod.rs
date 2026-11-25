//! Activity — the diagnostic/event river. Every committed engine/shell
//! event lands in exactly one entry; rebuild bursts and persisting
//! diagnostics coalesce into one entry that keeps every occurrence, so
//! nothing is dropped. Diagnostics are carried as the engine's own
//! `Diagnostic` value — the Pages inspector shows the very same value.

use crate::module::{
    Chip, InspectorModel, Link, Module, ModuleSpec, NavCache, NavItem, NavModel, ReadingLine,
    ReadingModel, Registry, RowModel, Slot, Tone,
};
use cv_core::diagnostic::{Diagnostic, Severity};
use cv_core::events::engine::{BuildPhase, EngineEvent, WatchEndReason};
use cv_core::events::shell::{DegradedReason, ShellEvent};
use cv_core::events::{AppEvent, Envelope};
use cv_core::id::{AccountId, AttrValue, NounAttrRow, NounId, NounKind, StableId, noun_kind};
use std::collections::{BTreeMap, BTreeSet};

pub const ID: &str = "activity";
/// File groups listed in the mailbox pane, busiest first.
pub const MAX_FILE_GROUPS: usize = 48;
const DIAGNOSTICS: &str = "diagnostics";

pub fn register(r: &mut Registry) {
    r.register(Box::new(ActivityModule::default()))
        .expect("activity module id is unique");
}

pub fn severity_tone(s: Severity) -> Tone {
    match s {
        Severity::Error => Tone::Error,
        Severity::Warning => Tone::Warn,
        Severity::Note => Tone::Dim,
    }
}

pub fn severity_label(s: Severity) -> &'static str {
    match s {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Note => "note",
    }
}

/// `path:line:col`, or `(project)` for span-less diagnostics.
pub fn location(d: &Diagnostic) -> String {
    match &d.span {
        Some(s) if s.line > 0 => format!("{}:{}:{}", s.path, s.line, s.col),
        Some(s) => s.path.clone(),
        None => "(project)".into(),
    }
}

/// A diagnostic row's title in the river.
pub fn diagnostic_title(code: &str, message: &str) -> String {
    format!("{code}: {message}")
}

/// One diagnostic's identity across rebuilds: everything but the build.
type DiagIdentity = (String, String, String, String);

fn identity(d: &Diagnostic) -> DiagIdentity {
    (
        d.code.clone(),
        severity_label(d.severity).into(),
        location(d),
        d.message.clone(),
    )
}

#[derive(Debug, Clone, PartialEq)]
pub enum EntryKind {
    Session {
        schema: u32,
        compiler: String,
    },
    Build {
        phase: BuildPhase,
        ok: bool,
        pages: u64,
        duration_ms: u64,
        diagnostics: usize,
    },
    Diagnostic(Diagnostic),
    Verb {
        verb: String,
        exit: String,
    },
    VerbFailed {
        verb: String,
        error: String,
    },
    WatchEnded(WatchEndReason),
    Malformed(String),
    Degraded(DegradedReason),
    Other(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Occurrence {
    pub event: String,
    pub build: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub key: String,
    pub kind: EntryKind,
    pub occurrences: Vec<Occurrence>,
}

impl Entry {
    fn severity(&self) -> Option<Severity> {
        match &self.kind {
            EntryKind::Diagnostic(d) => Some(d.severity),
            EntryKind::Build { ok: false, .. }
            | EntryKind::VerbFailed { .. }
            | EntryKind::Malformed(_) => Some(Severity::Error),
            EntryKind::Degraded(_) => Some(Severity::Warning),
            _ => None,
        }
    }

    pub fn title(&self) -> String {
        let phase = |p: &BuildPhase| match p {
            BuildPhase::Initial => "initial",
            BuildPhase::Rebuild => "rebuild",
        };
        match &self.kind {
            EntryKind::Session { .. } => "Watch session started".into(),
            EntryKind::Build {
                phase: p, ok: true, ..
            } => format!("Build succeeded ({})", phase(p)),
            EntryKind::Build {
                phase: p,
                ok: false,
                ..
            } => format!("Build failed ({})", phase(p)),
            EntryKind::Diagnostic(d) => diagnostic_title(&d.code, &d.message),
            EntryKind::Verb { verb, .. } => format!("{} finished", verb.to_lowercase()),
            EntryKind::VerbFailed { verb, .. } => format!("{} failed", verb.to_lowercase()),
            EntryKind::WatchEnded(r) => format!("Watch session ended ({r:?})"),
            EntryKind::Malformed(_) => "Malformed engine line".into(),
            EntryKind::Degraded(r) => format!("Degraded: {r:?}"),
            EntryKind::Other(t) => t.clone(),
        }
    }

    fn detail(&self) -> String {
        match &self.kind {
            EntryKind::Session { compiler, schema } => format!("{compiler} · schema {schema}"),
            EntryKind::Build {
                pages,
                duration_ms,
                diagnostics,
                ..
            } => {
                format!("{pages} pages · {duration_ms} ms · {diagnostics} diagnostics")
            }
            EntryKind::Diagnostic(d) => location(d),
            EntryKind::Verb { verb, exit } => ir_verb_detail(verb, exit),
            EntryKind::VerbFailed { verb, error } => ir_verb_detail(verb, error),
            EntryKind::Malformed(raw) => raw.clone(),
            _ => String::new(),
        }
    }
}

/// Mailboxes: fixed views plus one per cited file.
const ALL: &str = "all";
const ERRORS: &str = "errors";
const WARNINGS: &str = "warnings";
const BUILDS: &str = "builds";
const FILE: &str = "file:";

#[derive(Default)]
pub struct ActivityModule {
    entries: Vec<Entry>,
    /// Mailbox → entry indices, appended as entries are pushed.
    views: BTreeMap<String, Vec<usize>>,
    /// Open diagnostics that coalesce until a green build closes them.
    open: BTreeMap<DiagIdentity, usize>,
    /// The burst a further rebuild of the same outcome folds into.
    last_build: Option<usize>,
    /// Repeated one-shot initial build summaries fold into one entry.
    initial_builds: BTreeMap<(bool, u64, usize), usize>,
    /// Repeated identical verb outcomes fold into one entry. The first
    /// key digit separates process-exited from failed-before-exit.
    verb_outcomes: BTreeMap<(u8, String, String), usize>,
    /// Built from `views` only, so `push` is its one invalidation point.
    nav_cache: NavCache,
}

impl ActivityModule {
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Every event id the river accounts for (the "nothing dropped" proof).
    pub fn accounted(&self) -> BTreeSet<String> {
        self.entries
            .iter()
            .flat_map(|e| e.occurrences.iter().map(|o| o.event.clone()))
            .collect()
    }

    fn view(&self, nav: Option<&str>) -> &[usize] {
        self.views
            .get(nav.unwrap_or(ALL))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    fn entry(&self, key: &str) -> Option<&Entry> {
        let n: usize = key.strip_prefix("act-")?.parse().ok()?;
        self.entries.get(n)
    }

    fn push(&mut self, kind: EntryKind, occ: Occurrence) -> usize {
        let i = self.entries.len();
        let mut views = vec![ALL.to_string()];
        match &kind {
            EntryKind::Diagnostic(d) => {
                views.push(
                    match d.severity {
                        Severity::Error => ERRORS,
                        _ => WARNINGS,
                    }
                    .into(),
                );
                views.push(format!("{FILE}{}", location_file(d)));
                views.push(DIAGNOSTICS.into());
            }
            EntryKind::Build { .. } => views.push(BUILDS.into()),
            _ => {}
        }
        if matches!(
            kind,
            EntryKind::Malformed(_) | EntryKind::Build { ok: false, .. }
        ) {
            views.push(ERRORS.into());
        }
        for v in views {
            self.views.entry(v).or_default().push(i);
        }
        self.nav_cache.invalidate();
        self.entries.push(Entry {
            key: format!("act-{i:05}"),
            kind,
            occurrences: vec![occ],
        });
        i
    }

    fn build_nav(&self) -> NavModel {
        let count = |k: &str| Some(self.view(Some(k)).len() as u64);
        let mut items = vec![
            NavItem {
                key: ALL.into(),
                label: "All activity".into(),
                count: count(ALL),
            },
            NavItem {
                key: ERRORS.into(),
                label: "Errors".into(),
                count: count(ERRORS),
            },
            NavItem {
                key: WARNINGS.into(),
                label: "Warnings & notes".into(),
                count: count(WARNINGS),
            },
            NavItem {
                key: BUILDS.into(),
                label: "Builds".into(),
                count: count(BUILDS),
            },
        ];
        let mut files: Vec<(&String, usize)> = self
            .views
            .range(FILE.to_string()..)
            .take_while(|(k, _)| k.starts_with(FILE))
            .map(|(k, v)| (k, v.len()))
            .collect();
        // Busiest files first; the mailbox list stays bounded and every
        // diagnostic remains reachable through "All diagnostics".
        files.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
        let more = files.len().saturating_sub(MAX_FILE_GROUPS);
        for (k, n) in files.into_iter().take(MAX_FILE_GROUPS) {
            items.push(NavItem {
                key: k.clone(),
                label: k[FILE.len()..].into(),
                count: Some(n as u64),
            });
        }
        if more > 0 {
            items.push(NavItem {
                key: DIAGNOSTICS.into(),
                label: format!("All diagnostics (+{more} files)"),
                count: count(DIAGNOSTICS),
            });
        }
        NavModel { items }
    }

    fn ingest_build(
        &mut self,
        phase: BuildPhase,
        ok: bool,
        pages: u64,
        duration_ms: u64,
        diagnostics: &[Diagnostic],
        occ: Occurrence,
    ) {
        let kind = EntryKind::Build {
            phase,
            ok,
            pages,
            duration_ms,
            diagnostics: diagnostics.len(),
        };
        let initial_key = (ok, pages, diagnostics.len());
        let fold = if phase == BuildPhase::Rebuild {
            self.last_build.filter(|&i| {
                matches!(self.entries[i].kind, EntryKind::Build { phase: BuildPhase::Rebuild, ok: o, .. } if o == ok)
            })
        } else {
            self.initial_builds.get(&initial_key).copied()
        };
        match fold {
            Some(i) => {
                self.entries[i].kind = kind;
                self.entries[i].occurrences.push(occ.clone());
            }
            None => {
                let i = self.push(kind, occ.clone());
                if phase == BuildPhase::Rebuild {
                    self.last_build = Some(i);
                } else {
                    self.initial_builds.insert(initial_key, i);
                }
            }
        }
        if ok && diagnostics.iter().all(|d| d.severity != Severity::Error) {
            self.open.clear();
        }
        for d in diagnostics {
            let id = identity(d);
            match self.open.get(&id) {
                Some(&i) => {
                    // Latest value wins, so Activity and Pages show the
                    // same build's diagnostic; every occurrence is kept.
                    self.entries[i].kind = EntryKind::Diagnostic(d.clone());
                    self.entries[i].occurrences.push(occ.clone());
                }
                None => {
                    let i = self.push(EntryKind::Diagnostic(d.clone()), occ.clone());
                    self.open.insert(id, i);
                }
            }
        }
    }
}

const IR_FALLBACK_DETAIL: &str =
    "Pages keeps the last published Boris IR; when none exists, it keeps marked source fields";

fn ir_verb_detail(verb: &str, outcome: &str) -> String {
    if verb == "ir" && outcome != "Ok" {
        format!("{outcome} · {IR_FALLBACK_DETAIL}")
    } else {
        outcome.into()
    }
}

fn location_file(d: &Diagnostic) -> String {
    d.span
        .as_ref()
        .map_or_else(|| "(project)".into(), |s| s.path.clone())
}

impl Module for ActivityModule {
    fn spec(&self) -> ModuleSpec {
        ModuleSpec {
            id: ID,
            title: "Activity",
            slot: Slot::Activity,
        }
    }

    fn ingest(&mut self, env: &Envelope) -> bool {
        let occ = Occurrence {
            event: env.event_id.0.clone(),
            build: env.build_id.as_ref().map(|b| b.0.clone()),
        };
        let boundary = |s: &mut Self, kind| {
            s.last_build = None;
            s.push(kind, occ.clone());
        };
        match &env.payload {
            AppEvent::Engine(ev) => match ev {
                EngineEvent::BuildFinished {
                    phase,
                    pages_written,
                    diagnostics,
                    duration_ms,
                } => {
                    self.ingest_build(*phase, true, *pages_written, *duration_ms, diagnostics, occ)
                }
                EngineEvent::BuildFailed {
                    phase, diagnostics, ..
                } => self.ingest_build(*phase, false, 0, 0, diagnostics, occ),
                EngineEvent::WatchSessionStarted { schema, compiler } => boundary(
                    self,
                    EntryKind::Session {
                        schema: *schema,
                        compiler: compiler.clone(),
                    },
                ),
                EngineEvent::WatchSessionEnded { reason } => {
                    boundary(self, EntryKind::WatchEnded(reason.clone()))
                }
                EngineEvent::VerbFinished { verb, exit } => {
                    let exit = format!("{exit:?}");
                    let key = (0u8, verb.clone(), exit.clone());
                    self.last_build = None;
                    match self.verb_outcomes.get(&key).copied() {
                        Some(i) => self.entries[i].occurrences.push(occ.clone()),
                        None => {
                            let i = self.push(
                                EntryKind::Verb {
                                    verb: verb.clone(),
                                    exit,
                                },
                                occ.clone(),
                            );
                            self.verb_outcomes.insert(key, i);
                        }
                    }
                }
                EngineEvent::VerbFailed { verb, error } => {
                    let key = (1u8, verb.clone(), error.clone());
                    self.last_build = None;
                    match self.verb_outcomes.get(&key).copied() {
                        Some(i) => self.entries[i].occurrences.push(occ.clone()),
                        None => {
                            let i = self.push(
                                EntryKind::VerbFailed {
                                    verb: verb.clone(),
                                    error: error.clone(),
                                },
                                occ.clone(),
                            );
                            self.verb_outcomes.insert(key, i);
                        }
                    }
                }
                EngineEvent::MalformedLine { raw } => {
                    boundary(self, EntryKind::Malformed(raw.clone()))
                }
            },
            AppEvent::Shell(ShellEvent::Degraded { reason }) => {
                boundary(self, EntryKind::Degraded(reason.clone()))
            }
            AppEvent::Shell(_) => return false,
            other => {
                let name = serde_json::to_value(other)
                    .ok()
                    .and_then(|v| v.as_object().and_then(|o| o.keys().next().cloned()))
                    .unwrap_or_else(|| "event".into());
                boundary(self, EntryKind::Other(format!("{name} event")))
            }
        }
        true
    }

    fn nav(&self) -> NavModel {
        self.nav_cache.get_or_build(|| self.build_nav())
    }

    fn list_len(&self, nav: Option<&str>) -> usize {
        self.view(nav).len()
    }

    /// Newest first.
    fn row(&self, nav: Option<&str>, i: usize) -> RowModel {
        let v = self.view(nav);
        let Some(e) = v
            .len()
            .checked_sub(i + 1)
            .and_then(|j| self.entries.get(v[j]))
        else {
            return RowModel::default();
        };
        let sev = e.severity();
        let n = e.occurrences.len();
        RowModel {
            key: e.key.clone(),
            title: e.title(),
            detail: e.detail(),
            badge: (n > 1).then(|| Chip::new(format!("×{n}"), Tone::Normal)),
            tone: sev.map_or(Tone::Normal, severity_tone),
            rail: Some(sev.map_or(Tone::Ok, severity_tone)),
            path: false,
        }
    }

    fn index_of_key(&self, nav: Option<&str>, key: &str) -> Option<usize> {
        let n: usize = key.strip_prefix("act-")?.parse().ok()?;
        if self.entries.get(n)?.key != key {
            return None;
        }
        // `push` appends each entry once per view, in entry order.
        let v = self.view(nav);
        let j = v.binary_search(&n).ok()?;
        Some(v.len() - 1 - j)
    }

    fn reading(&self, key: &str) -> ReadingModel {
        let Some(e) = self.entry(key) else {
            return ReadingModel {
                title: key.into(),
                ..Default::default()
            };
        };
        let line = |l: &str, t: String, tone, mono| ReadingLine {
            label: Some(l.into()),
            text: t,
            tone,
            mono,
        };
        let mut lines = vec![line("detail", e.detail(), Tone::Normal, false)];
        if let EntryKind::Diagnostic(d) = &e.kind {
            lines = vec![
                line(
                    "severity",
                    severity_label(d.severity).into(),
                    severity_tone(d.severity),
                    false,
                ),
                line("code", d.code.clone(), Tone::Normal, true),
                line("at", location(d), Tone::Normal, true),
                line("message", d.message.clone(), Tone::Normal, false),
            ];
        }
        for o in &e.occurrences {
            let at = o.build.clone().unwrap_or_else(|| "—".into());
            lines.push(line("seen", format!("{} · {at}", o.event), Tone::Dim, true));
        }
        ReadingModel {
            title: e.title(),
            lines,
            ..Default::default()
        }
    }

    fn inspector(&self, key: &str) -> InspectorModel {
        let Some(e) = self.entry(key) else {
            return InspectorModel::default();
        };
        let first = e
            .occurrences
            .first()
            .map(|o| o.event.clone())
            .unwrap_or_default();
        let last = e
            .occurrences
            .last()
            .map(|o| o.event.clone())
            .unwrap_or_default();
        InspectorModel {
            badges: vec![],
            fields: vec![
                ("entry".into(), e.key.clone()),
                ("seen".into(), e.occurrences.len().to_string()),
                ("first".into(), first),
                ("last".into(), last),
            ],
            diagnostics: match &e.kind {
                EntryKind::Diagnostic(d) => vec![d.clone()],
                _ => vec![],
            },
            lists: vec![(
                "Builds".into(),
                e.occurrences
                    .iter()
                    .filter_map(|o| o.build.clone())
                    .collect(),
            )],
        }
    }

    fn noun(&self, key: &str) -> Option<NounId> {
        let e = self.entry(key)?;
        let kind = match e.kind {
            EntryKind::Diagnostic(_) => noun_kind::DIAGNOSTIC,
            _ => noun_kind::EVENT,
        };
        Some(NounId {
            account: AccountId("local".into()),
            kind: NounKind::register(kind),
            stable: StableId(format!("{ID}/{}", e.key)),
        })
    }

    fn link(&self, key: &str) -> Option<Link> {
        match &self.entry(key)?.kind {
            EntryKind::Diagnostic(d) => d.span.as_ref().map(|s| Link {
                module: super::pages::ID,
                key: s.path.clone(),
                label: "Open in Pages".into(),
            }),
            _ => None,
        }
    }

    fn attr_rows(&self) -> Vec<NounAttrRow> {
        self.entries
            .iter()
            .filter_map(|e| {
                let noun = self.noun(&e.key)?;
                let mut f = BTreeMap::new();
                f.insert("title".into(), AttrValue::Text(e.title()));
                f.insert(
                    "occurrences".into(),
                    AttrValue::Int(e.occurrences.len() as i64),
                );
                if let Some(s) = e.severity() {
                    f.insert("severity".into(), AttrValue::Text(severity_label(s).into()));
                }
                if let EntryKind::Diagnostic(d) = &e.kind {
                    f.insert("code".into(), AttrValue::Text(d.code.clone()));
                    f.insert("path".into(), AttrValue::Text(location_file(d)));
                }
                Some(NounAttrRow { noun, fields: f })
            })
            .collect()
    }
}

#[cfg(test)]
pub(crate) mod tests;
