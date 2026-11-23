//! Editor session: N open buffers keyed by [`BufferId`], each with its
//! problems and last save status. Windows bind to buffers in the host
//! layer; this type holds no window or UI state.

use crate::buffer::Buffer;
use crate::completion::CompletionIndex;
use crate::problems::{Problems, same_path};
use crate::save::{SaveStatus, SaveTicket};
use cv_core::diagnostic::Diagnostic;
use cv_core::engine::{Fingerprint, WriteOutcome};
use cv_core::events::BuildId;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BufferId(pub u32);

#[derive(Debug, Clone)]
pub struct Entry {
    pub buffer: Buffer,
    pub problems: Problems,
    pub save: SaveStatus,
}

#[derive(Debug, Default)]
pub struct EditorSession {
    entries: BTreeMap<BufferId, Entry>,
    next: u32,
    pub completion: Option<CompletionIndex>,
    build: Option<BuildId>,
    generation: u64,
}

impl EditorSession {
    pub fn new() -> Self {
        Self::default()
    }

    /// Open `path`, or return the buffer already open on it.
    pub fn open(&mut self, path: &str, text: &str, base: Option<Fingerprint>) -> BufferId {
        if let Some(id) = self.find(path) {
            return id;
        }
        self.next += 1;
        let id = BufferId(self.next);
        self.entries.insert(
            id,
            Entry {
                buffer: Buffer::open(path, text, base),
                problems: Problems::default(),
                save: SaveStatus::Idle,
            },
        );
        id
    }

    pub fn find(&self, path: &str) -> Option<BufferId> {
        self.entries
            .iter()
            .find(|(_, e)| same_path(e.buffer.path(), path))
            .map(|(id, _)| *id)
    }

    pub fn get(&self, id: BufferId) -> Option<&Entry> {
        self.entries.get(&id)
    }

    pub fn get_mut(&mut self, id: BufferId) -> Option<&mut Entry> {
        self.entries.get_mut(&id)
    }

    pub fn ids(&self) -> impl Iterator<Item = BufferId> + '_ {
        self.entries.keys().copied()
    }

    pub fn is_dirty(&self, id: BufferId) -> bool {
        self.entries.get(&id).is_some_and(|e| e.buffer.is_dirty())
    }

    /// Drop a buffer. Refuses a dirty one unless `force` (the caller asks
    /// the user first — see `cv_model::windows::CloseOutcome`).
    pub fn close(&mut self, id: BufferId, force: bool) -> bool {
        if self.is_dirty(id) && !force {
            return false;
        }
        self.entries.remove(&id).is_some()
    }

    pub fn current_build(&self) -> Option<&BuildId> {
        self.build.as_ref()
    }

    /// A build finished: hand each open buffer its diagnostics.
    pub fn ingest_build(&mut self, build: BuildId, diags: &[Diagnostic]) {
        for e in self.entries.values_mut() {
            e.problems.ingest(build.clone(), diags, &e.buffer);
        }
        self.build = Some(build);
        self.generation += 1;
    }

    /// Bumped by every session-level change (builds, saves, open/close)
    /// so windows know to re-read; buffer edits show in `revision()`.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn begin_save(&mut self, id: BufferId) -> Option<SaveTicket> {
        let e = self.entries.get_mut(&id)?;
        let t = e.buffer.begin_save();
        e.save = SaveStatus::Saving {
            revision: t.revision,
        };
        self.generation += 1;
        Some(t)
    }

    pub fn finish_save(
        &mut self,
        id: BufferId,
        ticket: &SaveTicket,
        result: cv_core::Result<WriteOutcome>,
    ) -> Option<&SaveStatus> {
        self.generation += 1;
        let e = self.entries.get_mut(&id)?;
        e.save = e.buffer.finish_save(ticket, result);
        Some(&e.save)
    }
}
