//! Research module (L6): inbox, notebooks, smart mailboxes over the
//! `cv-notes` session. Pure: all persistence and page writes go through
//! the injected `ResearchPort`; this file never touches disk or spawns.

mod filter;

use crate::command::{CommandSpec, Intent};
use crate::dialog::{ActionTone, DialogAnswer, DialogKind, DialogSpec, Field};
use crate::module::{
    Chip, InspectorModel, Module, ModuleSpec, NavItem, NavModel, ReadingLine, ReadingModel,
    Registry, RowModel, Slot, Tone,
};
use cv_core::engine::WriteOutcome;
use cv_core::events::engine::EngineEvent;
use cv_core::events::{AppEvent, Envelope};
use cv_core::id::{NounId, StableId};
use cv_notes::{
    Capture, MemoryPort, Note, Notebook, PromoteSpec, Research, ResearchPort, Target, Triage,
};
use filter::Filter;
use std::cell::RefCell;

pub const ID: &str = "research";
/// Profile input the default promotion plan writes under.
pub const CONTENT_DIR: &str = "content";

type Seed = (Box<dyn ResearchPort>, Notebook, String);

thread_local! {
    static SEED: RefCell<Option<Seed>> = const { RefCell::new(None) };
}

/// Composition-root hook: the next `register` (i.e. `Registry::with_all`)
/// on this thread uses `port` + the notebook replayed from its truth.
pub fn install(port: Box<dyn ResearchPort>, notebook: Notebook) {
    install_in(port, notebook, CONTENT_DIR);
}

/// [`install`] for a profile whose content input is `content_dir`
/// (workspace-relative); promotions are written under it.
pub fn install_in(port: Box<dyn ResearchPort>, notebook: Notebook, content_dir: &str) {
    SEED.with(|s| *s.borrow_mut() = Some((port, notebook, content_dir.into())));
}

pub fn register(r: &mut Registry) {
    let (port, nb, dir) = SEED.with(|s| s.borrow_mut().take()).unwrap_or_else(|| {
        (
            Box::new(MemoryPort::default()),
            Notebook::default(),
            CONTENT_DIR.into(),
        )
    });
    let mut m = ResearchModule::new(Research::new(nb, port));
    m.content_dir = dir;
    r.register(Box::new(m))
        .expect("research module id is unique");
}

pub struct ResearchModule {
    session: Research<Box<dyn ResearchPort>>,
    /// The selection the shell told us just before a command (the module
    /// grammar passes no key); consumed by that command.
    focus: Option<StableId>,
    filter: Filter,
    promote_error: Option<(StableId, String)>,
    last_build: Option<(String, Tone)>,
    /// Workspace-relative content input promotions write under.
    content_dir: String,
}

const EMPTY: &str = "research-empty";

impl ResearchModule {
    pub fn new(session: Research<Box<dyn ResearchPort>>) -> Self {
        Self {
            session,
            focus: None,
            filter: Filter::default(),
            promote_error: None,
            last_build: None,
            content_dir: CONTENT_DIR.into(),
        }
    }

    fn nb(&self) -> &Notebook {
        &self.session.notebook
    }

    fn notes(&self, nav: Option<&str>) -> Vec<&Note> {
        let nb = self.nb();
        match nav.unwrap_or("inbox") {
            "inbox" => nb.inbox(),
            "all" => nb.all(),
            "filter" => nb.select(&self.filter.predicate()),
            k => {
                if let Some(name) = k.strip_prefix("nb:") {
                    nb.in_notebook(name)
                } else if let Some(v) = k.strip_prefix("view:") {
                    nb.view(&StableId(v.into()))
                        .map(|v| nb.select(&v.predicate))
                        .unwrap_or_default()
                } else {
                    Vec::new()
                }
            }
        }
    }

    fn target(&mut self) -> Option<StableId> {
        let focus = self.focus.take();
        focus.filter(|f| self.nb().note(f).is_some())
    }

    fn title_of(&self, id: &StableId) -> String {
        self.nb()
            .note(id)
            .map(|n| n.title.clone())
            .unwrap_or_else(|| id.0.clone())
    }

    fn with_target(
        &mut self,
        f: impl FnOnce(&mut Self, StableId) -> Result<String, String>,
    ) -> Vec<Intent> {
        let Some(id) = self.target() else {
            return vec![Intent::Status("research: no note selected".into())];
        };
        let msg = f(self, id).unwrap_or_else(|e| format!("research: {e}"));
        vec![Intent::Status(msg)]
    }

    fn promote(&mut self, id: StableId) -> Result<String, String> {
        let note = self.nb().note(&id).ok_or("unknown note")?;
        let spec = PromoteSpec::for_note(note, &self.content_dir);
        match self.session.promote(&id, &spec) {
            Ok(p) => match p.outcome {
                WriteOutcome::Written { .. } => {
                    self.promote_error = None;
                    Ok(format!(
                        "promoted → {} ({}); boris validate judges it",
                        spec.page.0, p.draft.path
                    ))
                }
                WriteOutcome::StaleBase { .. } => {
                    let e = format!("{} changed on disk; not overwritten", p.draft.path);
                    self.promote_error = Some((id, e.clone()));
                    Err(e)
                }
            },
            Err(e) => {
                let e = format!("promote unavailable: {e}");
                self.promote_error = Some((id, e.clone()));
                Err(e)
            }
        }
    }
}

fn line(label: &str, text: impl Into<String>, tone: Tone) -> ReadingLine {
    ReadingLine {
        label: (!label.is_empty()).then(|| label.to_string()),
        text: text.into(),
        tone,
        mono: false,
    }
}

impl Module for ResearchModule {
    fn spec(&self) -> ModuleSpec {
        ModuleSpec {
            id: ID,
            title: "Research",
            slot: Slot::Research,
        }
    }

    fn ingest(&mut self, env: &Envelope) -> bool {
        let AppEvent::Engine(ev) = &env.payload else {
            return false;
        };
        self.last_build = match ev {
            EngineEvent::BuildFinished { pages_written, .. } => {
                Some((format!("ok ({pages_written} pages)"), Tone::Ok))
            }
            EngineEvent::BuildFailed { diagnostics, .. } => {
                let codes: Vec<&str> = diagnostics.iter().map(|d| d.code.as_str()).collect();
                Some((format!("failed {}", codes.join(" ")), Tone::Error))
            }
            _ => return false,
        };
        self.nb().provenance_map().next().is_some()
    }

    fn nav(&self) -> NavModel {
        let nb = self.nb();
        let item = |key: String, label: String, count: usize| NavItem {
            key,
            label,
            count: Some(count as u64),
        };
        let mut items = vec![
            item("inbox".into(), "Inbox".into(), nb.inbox().len()),
            item("all".into(), "All notes".into(), nb.len()),
        ];
        for (name, n) in nb.notebooks() {
            items.push(item(format!("nb:{name}"), format!("Notebook · {name}"), n));
        }
        for v in nb.views() {
            items.push(item(
                format!("view:{}", v.id.0),
                format!("Saved · {}", v.name),
                nb.select(&v.predicate).len(),
            ));
        }
        items.push(item(
            "filter".into(),
            format!("Filter · {}", self.filter.label()),
            self.notes(Some("filter")).len(),
        ));
        NavModel { items }
    }

    fn list_len(&self, nav: Option<&str>) -> usize {
        match self.notes(nav).len() {
            0 if nav.unwrap_or("inbox") == "inbox" => 1,
            n => n,
        }
    }

    fn row(&self, nav: Option<&str>, index: usize) -> RowModel {
        let notes = self.notes(nav);
        let Some(n) = notes.get(index) else {
            return RowModel {
                key: EMPTY.into(),
                title: "Inbox zero".into(),
                detail: "Capture with Research: Capture idea (palette)".into(),
                badge: None,
                tone: Tone::Dim,
                rail: None,
                path: false,
            };
        };
        let promoted = self.nb().provenance(&n.id).is_some();
        let kind = n.captures.first().map(Capture::kind).unwrap_or("note");
        let badge = if promoted {
            "promoted".to_string()
        } else {
            match &n.triage {
                Triage::Filed { notebook } => notebook.clone(),
                t => t.label().to_string(),
            }
        };
        RowModel {
            key: n.id.0.clone(),
            title: n.title.clone(),
            detail: format!(
                "{kind} · {}",
                n.body_md
                    .lines()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("")
            ),
            badge: Some(Chip::new(
                badge,
                if promoted { Tone::Ok } else { Tone::Normal },
            )),
            tone: if promoted { Tone::Ok } else { Tone::Normal },
            rail: None,
            path: false,
        }
    }

    /// One `notes` query instead of one per row.
    fn index_of_key(&self, nav: Option<&str>, key: &str) -> Option<usize> {
        let notes = self.notes(nav);
        let empty_inbox = notes.is_empty() && nav.unwrap_or("inbox") == "inbox";
        match notes.iter().position(|n| n.id.0 == key) {
            None if empty_inbox && key == EMPTY => Some(0),
            found => found,
        }
    }

    fn reading(&self, key: &str) -> ReadingModel {
        let nb = self.nb();
        let Some(n) = nb.note(&StableId(key.into())) else {
            return ReadingModel {
                title: "Inbox zero".into(),
                lines: vec![line(
                    "",
                    "Nothing to triage. Captures land here first.",
                    Tone::Dim,
                )],
                ..Default::default()
            };
        };
        let mut lines: Vec<ReadingLine> = n
            .body_md
            .lines()
            .map(|l| line("", l, Tone::Normal))
            .collect();
        for c in &n.captures {
            lines.push(line(
                "capture",
                format!("{} · {}", c.kind(), c.summary()),
                Tone::Dim,
            ));
        }
        for l in nb.outgoing(&n.id) {
            let to = match &l.to {
                Target::Note(t) => format!("note · {}", self.title_of(t)),
                t => format!("{} · {}", t.kind(), t.id().0),
            };
            lines.push(line("link", to, Tone::Normal));
        }
        for l in nb.backlinks(&Target::Note(n.id.clone())) {
            lines.push(line("backlink", self.title_of(&l.from), Tone::Normal));
        }
        if let Some(p) = nb.provenance(&n.id) {
            lines.push(line("promoted", format!("page {}", p.page.0), Tone::Ok));
            lines.push(line("path", p.path.clone(), Tone::Dim));
            for other in nb
                .page_backlinks(&p.page)
                .into_iter()
                .filter(|o| o != &n.id)
            {
                lines.push(line("page backlink", self.title_of(&other), Tone::Normal));
            }
        }
        ReadingModel {
            title: n.title.clone(),
            lines,
            ..Default::default()
        }
    }

    fn inspector(&self, key: &str) -> InspectorModel {
        let nb = self.nb();
        let Some(n) = nb.note(&StableId(key.into())) else {
            return InspectorModel {
                fields: vec![("store".into(), self.session.port.describe())],
                ..Default::default()
            };
        };
        let tags: Vec<&str> = n.tags.iter().map(String::as_str).collect();
        let promotion = match (nb.provenance(&n.id), &self.promote_error) {
            (Some(p), _) => format!("page {}", p.page.0),
            (None, Some((id, e))) if id == &n.id => e.clone(),
            _ => "not promoted".into(),
        };
        let mut fields = vec![
            ("id".into(), n.id.0.clone()),
            ("triage".into(), n.triage.label().into()),
            ("format".into(), n.format.oliver_from().into()),
            ("tags".into(), tags.join(", ")),
            ("captures".into(), n.captures.len().to_string()),
            ("links".into(), n.links.len().to_string()),
            (
                "backlinks".into(),
                nb.backlinks(&Target::Note(n.id.clone())).len().to_string(),
            ),
            ("promotion".into(), promotion),
        ];
        if let Triage::Filed { notebook } = &n.triage {
            fields.insert(2, ("notebook".into(), notebook.clone()));
        }
        if let Some(s) = &n.source_uri {
            fields.push(("source".into(), s.clone()));
        }
        if let (Some(_), Some((b, _))) = (nb.provenance(&n.id), &self.last_build) {
            fields.push(("last build".into(), b.clone()));
        }
        fields.push(("store".into(), self.session.port.describe()));
        InspectorModel {
            fields,
            ..Default::default()
        }
    }

    fn noun(&self, key: &str) -> Option<NounId> {
        let id = StableId(key.into());
        self.nb().note(&id)?;
        Some(self.nb().noun(&id))
    }

    fn commands(&self) -> Vec<CommandSpec> {
        let mut v = vec![
            CommandSpec::new("research.capture.idea", "Research: Capture idea"),
            CommandSpec::new("research.triage.file", "Research: File note (first tag)"),
            CommandSpec::new("research.triage.archive", "Research: Archive note"),
            CommandSpec::new(
                "research.link.recent",
                "Research: Link note to most recent note",
            ),
            CommandSpec::new("research.promote", "Research: Promote note to page"),
        ];
        v.extend(filter::commands());
        v
    }

    fn run(&mut self, command: &str) -> Vec<Intent> {
        let out = self.dispatch(command);
        self.focus = None;
        out
    }

    fn select(&mut self, key: Option<&str>) {
        self.focus = key.map(|k| StableId(k.into()));
    }

    fn answer(&mut self, command: &str, a: &DialogAnswer) -> Vec<Intent> {
        if a.action.is_none() {
            return Vec::new();
        }
        let msg = match command {
            CAPTURE => {
                let text = a.text("text").unwrap_or_default().trim().to_string();
                if text.is_empty() {
                    return vec![Intent::dialog(
                        capture_prompt("").error("Type the idea, or press Escape to cancel."),
                    )];
                }
                match self.session.capture(Capture::Idea { text }, None) {
                    Ok(id) => format!("captured {} to inbox", id.0),
                    Err(e) => format!("research: {e}"),
                }
            }
            ARCHIVE => {
                let Some(id) = a.context.clone().map(StableId) else {
                    return Vec::new();
                };
                match self.session.archive(&id) {
                    Ok(_) => format!("archived {}", id.0),
                    Err(e) => format!("research: {e}"),
                }
            }
            filter::BUILD => match self.filter.answer(a, &mut self.session) {
                Ok(msg) => msg,
                Err(e) => return vec![Intent::dialog(self.filter.form(Some(a), Some(e)))],
            },
            _ => format!("unknown answer {command}"),
        };
        vec![Intent::Status(msg)]
    }
}

const CAPTURE: &str = "research.capture.idea";
const ARCHIVE: &str = "research.triage.archive";

fn capture_prompt(text: &str) -> DialogSpec {
    DialogSpec::new(
        CAPTURE,
        "Capture idea",
        DialogKind::Prompt(
            Field::text("text", "Idea", text)
                .placeholder("What is the idea?")
                .hint("It lands in the inbox; triage it later."),
        ),
    )
    .cancel("Cancel")
    .action("capture", "Capture", ActionTone::Primary)
}

impl ResearchModule {
    fn dispatch(&mut self, command: &str) -> Vec<Intent> {
        let verb = command.strip_prefix("research.").unwrap_or(command);
        if verb == "filter.build" {
            return vec![Intent::dialog(self.filter.form(None, None))];
        }
        if let Some(f) = verb.strip_prefix("filter.") {
            let msg = self.filter.run(f, &mut self.session);
            return vec![Intent::Status(msg)];
        }
        match verb {
            "capture.idea" => vec![Intent::dialog(capture_prompt(""))],
            "triage.file" => self.with_target(|m, id| {
                let nb = m.nb().note(&id).and_then(|n| n.tags.iter().next().cloned());
                let nb = nb.unwrap_or_else(|| "unsorted".into());
                m.session.file(&id, &nb).map_err(|e| e.to_string())?;
                Ok(format!("filed {} → {nb}", id.0))
            }),
            "triage.archive" => {
                let Some(id) = self.target() else {
                    return vec![Intent::Status("research: no note selected".into())];
                };
                let title = self.title_of(&id);
                vec![Intent::dialog(
                    DialogSpec::new(ARCHIVE, "Archive note?", DialogKind::Confirm)
                        .body(format!(
                            "“{title}” leaves the inbox. It stays in All notes and in the journal."
                        ))
                        .context(id.0)
                        .cancel("Cancel")
                        .action("archive", "Archive", ActionTone::Primary),
                )]
            }
            "link.recent" => self.with_target(|m, id| {
                let other = m
                    .nb()
                    .all()
                    .into_iter()
                    .find(|n| n.id != id)
                    .map(|n| n.id.clone());
                let other = other.ok_or("no other note to link")?;
                let l = m
                    .session
                    .link(&id, Target::Note(other.clone()))
                    .map_err(|e| e.to_string())?;
                Ok(format!("linked {} → {} ({})", id.0, other.0, l.0))
            }),
            "promote" => self.with_target(Self::promote),
            _ => vec![Intent::Status(format!("unknown command {command}"))],
        }
    }
}

#[cfg(test)]
mod tests;
