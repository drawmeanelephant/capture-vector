//! Research verbs: capture → triage → link → promote. Every verb checks
//! its preconditions, commits through the port, then folds the record.

use crate::graph::{Link, Target};
use crate::model::{Capture, Format, Note, Triage, first_line};
use crate::notebook::{Notebook, SmartMailbox};
use crate::port::ResearchPort;
use crate::promote::{PromoteSpec, promote_with};
use crate::record::{Provenance, Record};
use crate::{NotesError, Result};
use cv_core::engine::{PageDraft, WriteOutcome};
use cv_core::id::StableId;
use cv_core::store::Predicate;

pub struct Research<P: ResearchPort> {
    pub notebook: Notebook,
    pub port: P,
}

#[derive(Debug, Clone)]
pub struct Promotion {
    pub draft: PageDraft,
    pub outcome: WriteOutcome,
}

impl<P: ResearchPort> Research<P> {
    pub fn new(notebook: Notebook, port: P) -> Self {
        Self { notebook, port }
    }

    fn commit(&mut self, rec: Record) -> Result<()> {
        self.port.commit(&rec).map_err(NotesError::Port)?;
        self.notebook.apply(&rec)
    }

    fn ensure(&self, note: &StableId) -> Result<&Note> {
        self.notebook
            .note(note)
            .ok_or_else(|| NotesError::UnknownNote(note.0.clone()))
    }

    /// Fast capture into the inbox. Title defaults to the capture summary.
    pub fn capture(&mut self, capture: Capture, title: Option<&str>) -> Result<StableId> {
        let body = match &capture {
            Capture::Selection { text, .. } | Capture::Idea { text } => text.clone(),
            other => other.source_uri().unwrap_or_default(),
        };
        let title = title.map(str::to_string).unwrap_or_else(|| {
            let s = first_line(&capture.summary());
            s.chars().take(80).collect()
        });
        self.capture_note(title, body, Format::Markdown, vec![capture])
    }

    pub fn capture_note(
        &mut self,
        title: String,
        body_md: String,
        format: Format,
        captures: Vec<Capture>,
    ) -> Result<StableId> {
        let id = self.notebook.next_id("note");
        let at = self.port.now();
        let source_uri = captures.iter().find_map(Capture::source_uri);
        self.commit(Record::Captured {
            note: Note {
                id: id.clone(),
                title,
                body_md,
                format,
                captures,
                links: Vec::new(),
                tags: Default::default(),
                created: at,
                modified: at,
                source_uri,
                triage: Triage::Inbox,
            },
        })?;
        Ok(id)
    }

    pub fn edit(&mut self, note: &StableId, title: Option<&str>, body: Option<&str>) -> Result<()> {
        self.ensure(note)?;
        let at = self.port.now();
        self.commit(Record::Edited {
            note: note.clone(),
            title: title.map(str::to_string),
            body_md: body.map(str::to_string),
            at,
        })
    }

    pub fn tag(&mut self, note: &StableId, tag: &str) -> Result<()> {
        if self.ensure(note)?.tags.contains(tag) {
            return Ok(());
        }
        let at = self.port.now();
        self.commit(Record::Tagged {
            note: note.clone(),
            tag: tag.to_string(),
            at,
        })
    }

    /// Triage: file into a notebook.
    pub fn file(&mut self, note: &StableId, notebook: &str) -> Result<()> {
        self.ensure(note)?;
        let at = self.port.now();
        self.commit(Record::Filed {
            note: note.clone(),
            notebook: notebook.to_string(),
            at,
        })
    }

    pub fn archive(&mut self, note: &StableId) -> Result<()> {
        self.ensure(note)?;
        let at = self.port.now();
        self.commit(Record::Archived {
            note: note.clone(),
            at,
        })
    }

    pub fn link(&mut self, from: &StableId, to: Target) -> Result<StableId> {
        self.ensure(from)?;
        if let Target::Note(n) = &to {
            self.ensure(n)?;
        }
        if let Some(l) = self
            .notebook
            .outgoing(from)
            .into_iter()
            .find(|l| l.to == to)
        {
            return Ok(l.id.clone());
        }
        let id = self.notebook.next_id("link");
        let at = self.port.now();
        self.commit(Record::Linked {
            link: Link {
                id: id.clone(),
                from: from.clone(),
                to,
            },
            at,
        })?;
        Ok(id)
    }

    pub fn unlink(&mut self, link: &StableId) -> Result<()> {
        if self.notebook.link(link).is_none() {
            return Err(NotesError::UnknownLink(link.0.clone()));
        }
        let at = self.port.now();
        self.commit(Record::Unlinked {
            link: link.clone(),
            at,
        })
    }

    pub fn save_view(&mut self, name: &str, predicate: Predicate) -> Result<StableId> {
        let id = self.notebook.next_id("view");
        self.commit(Record::ViewSaved {
            view: SmartMailbox {
                id: id.clone(),
                name: name.to_string(),
                predicate,
            },
        })?;
        Ok(id)
    }

    /// Promote through the port's staged write. On `Written` the sidecar
    /// records note → page; `StaleBase` returns without provenance.
    pub fn promote(&mut self, note: &StableId, spec: &PromoteSpec) -> Result<Promotion> {
        let n = self.ensure(note)?;
        if let Some(p) = self.notebook.provenance(note) {
            return Err(NotesError::Duplicate(format!(
                "{} already promoted to {}",
                note.0, p.page.0
            )));
        }
        let draft = promote_with(n, spec)?;
        let outcome = match self.port.write_page(&draft) {
            None => return Err(NotesError::Port("no content writer attached".into())),
            Some(r) => r.map_err(NotesError::Port)?,
        };
        if let WriteOutcome::Written { fingerprint } = &outcome {
            let p = Provenance {
                note: note.clone(),
                page: spec.page.clone(),
                path: draft.path.clone(),
                fingerprint: *fingerprint,
                at: self.port.now(),
            };
            self.port.commit_provenance(&p).map_err(NotesError::Port)?;
            self.notebook.apply_provenance(&p)?;
        }
        Ok(Promotion { draft, outcome })
    }

    /// FTS5 through the port when attached, else an in-memory scan.
    pub fn search(&self, query: &str) -> Vec<StableId> {
        self.port
            .search(query)
            .unwrap_or_else(|| self.notebook.scan(query))
    }
}
