//! In-memory research state: a pure fold over [`Record`]s and
//! [`Provenance`] lines. Backlinks are resolved here at read time.

use crate::graph::{Link, Target};
use crate::model::{Note, Triage};
use crate::record::{Provenance, Record};
use crate::{NotesError, Result};
use cv_core::id::{AccountId, AttrValue, NounAttrRow, NounId, NounKind, StableId};
use cv_core::store::Predicate;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A saved smart mailbox: a conjunction of `field op value` terms over
/// the note's [`NounAttrRow`]. No grammar, no joins, no regex.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SmartMailbox {
    pub id: StableId,
    pub name: String,
    pub predicate: Predicate,
}

#[derive(Debug, Clone)]
pub struct Notebook {
    pub account: AccountId,
    notes: BTreeMap<StableId, Note>,
    links: BTreeMap<StableId, Link>,
    views: BTreeMap<StableId, SmartMailbox>,
    provenance: BTreeMap<StableId, Provenance>,
    seq: u64,
}

impl Default for Notebook {
    fn default() -> Self {
        Self::new(AccountId("local".into()))
    }
}

impl Notebook {
    pub fn new(account: AccountId) -> Self {
        Self {
            account,
            notes: BTreeMap::new(),
            links: BTreeMap::new(),
            views: BTreeMap::new(),
            provenance: BTreeMap::new(),
            seq: 0,
        }
    }

    /// Next stable id. Monotonic over the fold, so replays reproduce it.
    pub fn next_id(&self, prefix: &str) -> StableId {
        StableId(format!("{prefix}-{:04}", self.seq + 1))
    }

    pub fn apply(&mut self, rec: &Record) -> Result<()> {
        match rec {
            Record::Captured { note } => {
                if self.notes.contains_key(&note.id) {
                    return Err(NotesError::Duplicate(note.id.0.clone()));
                }
                self.seq += 1;
                self.notes.insert(note.id.clone(), note.clone());
            }
            Record::Edited {
                note,
                title,
                body_md,
                at,
            } => {
                let n = self.note_mut(note)?;
                if let Some(t) = title {
                    n.title = t.clone();
                }
                if let Some(b) = body_md {
                    n.body_md = b.clone();
                }
                n.modified = *at;
            }
            Record::Tagged { note, tag, at } => {
                let n = self.note_mut(note)?;
                n.tags.insert(tag.clone());
                n.modified = *at;
            }
            Record::Filed { note, notebook, at } => {
                let n = self.note_mut(note)?;
                n.triage = Triage::Filed {
                    notebook: notebook.clone(),
                };
                n.modified = *at;
            }
            Record::Archived { note, at } => {
                let n = self.note_mut(note)?;
                n.triage = Triage::Archived;
                n.modified = *at;
            }
            Record::Linked { link, at } => {
                if self.links.contains_key(&link.id) {
                    return Err(NotesError::Duplicate(link.id.0.clone()));
                }
                if let Target::Note(to) = &link.to
                    && !self.notes.contains_key(to)
                {
                    return Err(NotesError::UnknownNote(to.0.clone()));
                }
                let n = self.note_mut(&link.from)?;
                n.links.push(link.id.clone());
                n.modified = *at;
                self.seq += 1;
                self.links.insert(link.id.clone(), link.clone());
            }
            Record::Unlinked { link, at } => {
                let l = self
                    .links
                    .remove(link)
                    .ok_or_else(|| NotesError::UnknownLink(link.0.clone()))?;
                let n = self.note_mut(&l.from)?;
                n.links.retain(|x| x != link);
                n.modified = *at;
            }
            Record::ViewSaved { view } => {
                if !self.views.contains_key(&view.id) {
                    self.seq += 1;
                }
                self.views.insert(view.id.clone(), view.clone());
            }
        }
        Ok(())
    }

    pub fn apply_provenance(&mut self, p: &Provenance) -> Result<()> {
        if !self.notes.contains_key(&p.note) {
            return Err(NotesError::UnknownNote(p.note.0.clone()));
        }
        self.provenance.insert(p.note.clone(), p.clone());
        Ok(())
    }

    fn note_mut(&mut self, id: &StableId) -> Result<&mut Note> {
        self.notes
            .get_mut(id)
            .ok_or_else(|| NotesError::UnknownNote(id.0.clone()))
    }

    pub fn note(&self, id: &StableId) -> Option<&Note> {
        self.notes.get(id)
    }
    pub fn notes(&self) -> impl Iterator<Item = &Note> {
        self.notes.values()
    }
    pub fn len(&self) -> usize {
        self.notes.len()
    }
    pub fn is_empty(&self) -> bool {
        self.notes.is_empty()
    }
    pub fn link(&self, id: &StableId) -> Option<&Link> {
        self.links.get(id)
    }
    pub fn links(&self) -> impl Iterator<Item = &Link> {
        self.links.values()
    }
    pub fn views(&self) -> impl Iterator<Item = &SmartMailbox> {
        self.views.values()
    }
    pub fn view(&self, id: &StableId) -> Option<&SmartMailbox> {
        self.views.get(id)
    }

    /// Newest first, ties by id — the order every list surface uses.
    fn sorted<'a>(&'a self, it: impl Iterator<Item = &'a Note>) -> Vec<&'a Note> {
        let mut v: Vec<&Note> = it.collect();
        v.sort_by(|a, b| b.modified.cmp(&a.modified).then(a.id.cmp(&b.id)));
        v
    }

    pub fn inbox(&self) -> Vec<&Note> {
        self.sorted(self.notes().filter(|n| n.triage == Triage::Inbox))
    }
    pub fn all(&self) -> Vec<&Note> {
        self.sorted(self.notes())
    }
    pub fn in_notebook(&self, name: &str) -> Vec<&Note> {
        self.sorted(
            self.notes()
                .filter(|n| matches!(&n.triage, Triage::Filed { notebook } if notebook == name)),
        )
    }
    /// Notebook name → note count.
    pub fn notebooks(&self) -> BTreeMap<String, usize> {
        let mut out = BTreeMap::new();
        for n in self.notes() {
            if let Triage::Filed { notebook } = &n.triage {
                *out.entry(notebook.clone()).or_insert(0) += 1;
            }
        }
        out
    }
    pub fn select(&self, pred: &Predicate) -> Vec<&Note> {
        self.sorted(self.notes().filter(|n| pred.matches(&self.attr_row(n))))
    }

    pub fn outgoing(&self, note: &StableId) -> Vec<&Link> {
        self.links.values().filter(|l| &l.from == note).collect()
    }

    /// Edges pointing at `target`, resolved at read time.
    pub fn backlinks(&self, target: &Target) -> Vec<&Link> {
        self.links.values().filter(|l| &l.to == target).collect()
    }

    /// Notes behind a page: explicit note→page edges plus the provenance
    /// sidecar (promoted-from). Keyed by page stable id, so a renamed or
    /// reparented page keeps its backlinks.
    pub fn page_backlinks(&self, page: &StableId) -> Vec<StableId> {
        let mut out: Vec<StableId> = self
            .provenance
            .values()
            .filter(|p| &p.page == page)
            .map(|p| p.note.clone())
            .chain(
                self.backlinks(&Target::Page(page.clone()))
                    .into_iter()
                    .map(|l| l.from.clone()),
            )
            .collect();
        out.sort();
        out.dedup();
        out
    }

    pub fn provenance(&self, note: &StableId) -> Option<&Provenance> {
        self.provenance.get(note)
    }
    pub fn provenance_map(&self) -> impl Iterator<Item = &Provenance> {
        self.provenance.values()
    }

    pub fn noun(&self, note: &StableId) -> NounId {
        NounId {
            account: self.account.clone(),
            kind: NounKind::register(cv_core::id::noun_kind::NOTE),
            stable: note.clone(),
        }
    }

    /// Uniform attribute projection smart mailboxes evaluate.
    /// `tags` is `,a,b,` so `Contains ",a,"` is an exact tag test.
    pub fn attr_row(&self, n: &Note) -> NounAttrRow {
        let mut f = BTreeMap::new();
        let text = |s: &str| AttrValue::Text(s.to_string());
        f.insert("title".into(), text(&n.title));
        f.insert("body".into(), text(&n.body_md));
        f.insert("triage".into(), text(n.triage.label()));
        if let Triage::Filed { notebook } = &n.triage {
            f.insert("notebook".into(), text(notebook));
        }
        let tags: String = n.tags.iter().map(|t| format!("{t},")).collect();
        f.insert("tags".into(), AttrValue::Text(format!(",{tags}")));
        f.insert("format".into(), text(n.format.oliver_from()));
        if let Some(c) = n.captures.first() {
            f.insert("capture".into(), text(c.kind()));
        }
        f.insert("captures".into(), AttrValue::Int(n.captures.len() as i64));
        f.insert("links".into(), AttrValue::Int(n.links.len() as i64));
        let back = self.backlinks(&Target::Note(n.id.clone())).len();
        f.insert("backlinks".into(), AttrValue::Int(back as i64));
        f.insert(
            "promoted".into(),
            AttrValue::Bool(self.provenance.contains_key(&n.id)),
        );
        f.insert("created".into(), AttrValue::Int(n.created.0));
        f.insert("modified".into(), AttrValue::Int(n.modified.0));
        if let Some(s) = &n.source_uri {
            f.insert("source".into(), text(s));
        }
        NounAttrRow {
            noun: self.noun(&n.id),
            fields: f,
        }
    }

    /// Plain substring scan — the fallback when no FTS5 index is attached.
    pub fn scan(&self, query: &str) -> Vec<StableId> {
        let q = query.to_lowercase();
        let terms: Vec<&str> = q.split_whitespace().collect();
        self.all()
            .into_iter()
            .filter(|n| {
                let hay = format!("{} {}", n.title, n.body_md).to_lowercase();
                !terms.is_empty() && terms.iter().all(|t| hay.contains(t))
            })
            .map(|n| n.id.clone())
            .collect()
    }
}
