//! IO boundary for the page lifecycle verbs. Paths are workspace-relative;
//! the composition root implements this over the staged, serialized content
//! writer. Tests implement it over memory, keeping the module pure.

use cv_core::engine::{Fingerprint, MoveOutcome, PageMove, RemoveOutcome, WriteOutcome};

/// A page source as read, with the fingerprint a later write, move or
/// delete must still find on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub text: String,
    pub base: Fingerprint,
}

pub trait PagesPort {
    fn read(&self, path: &str) -> Result<Source, String>;
    /// Staged write. `draft.base: None` creates; an existing file is then
    /// a stale base and is never overwritten implicitly.
    fn write(&mut self, draft: &cv_core::engine::PageDraft) -> Result<WriteOutcome, String>;
    fn remove(&mut self, path: &str, base: Fingerprint) -> Result<RemoveOutcome, String>;
    /// Destination write + source removal as one serialized engine mutation.
    fn rename(&mut self, mv: &PageMove) -> Result<MoveOutcome, String>;
}
