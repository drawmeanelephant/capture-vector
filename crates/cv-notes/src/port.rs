//! Persistence/write port. The pure session commits every record through
//! it *before* applying, so the in-memory fold never runs ahead of truth.

use crate::record::{Provenance, Record};
use cv_core::engine::{PageDraft, WriteOutcome};
use cv_core::id::StableId;
use cv_core::sys::Timestamp;

pub trait ResearchPort {
    fn now(&self) -> Timestamp;
    fn commit(&mut self, rec: &Record) -> Result<(), String>;
    fn commit_provenance(&mut self, p: &Provenance) -> Result<(), String>;
    /// Promotion write: the engine's fingerprinted staged write
    /// (`Engine::write_content`). `None` = no writer attached (degraded).
    fn write_page(&mut self, draft: &PageDraft) -> Option<Result<WriteOutcome, String>>;
    /// FTS5 search when an index is attached.
    fn search(&self, _query: &str) -> Option<Vec<StableId>> {
        None
    }
    /// Short human label of where truth lives (inspector/status).
    fn describe(&self) -> String;
}

/// In-memory port: records are kept (for tests/replay) but nothing
/// persists and no page writer is attached.
#[derive(Debug, Default)]
pub struct MemoryPort {
    pub now: i64,
    pub records: Vec<Record>,
    pub provenance: Vec<Provenance>,
}

impl ResearchPort for MemoryPort {
    fn now(&self) -> Timestamp {
        Timestamp(self.now)
    }
    fn commit(&mut self, rec: &Record) -> Result<(), String> {
        self.now += 1;
        self.records.push(rec.clone());
        Ok(())
    }
    fn commit_provenance(&mut self, p: &Provenance) -> Result<(), String> {
        self.provenance.push(p.clone());
        Ok(())
    }
    fn write_page(&mut self, _draft: &PageDraft) -> Option<Result<WriteOutcome, String>> {
        None
    }
    fn describe(&self) -> String {
        "memory (not persisted)".into()
    }
}

impl<P: ResearchPort + ?Sized> ResearchPort for Box<P> {
    fn now(&self) -> Timestamp {
        (**self).now()
    }
    fn commit(&mut self, rec: &Record) -> Result<(), String> {
        (**self).commit(rec)
    }
    fn commit_provenance(&mut self, p: &Provenance) -> Result<(), String> {
        (**self).commit_provenance(p)
    }
    fn write_page(&mut self, draft: &PageDraft) -> Option<Result<WriteOutcome, String>> {
        (**self).write_page(draft)
    }
    fn search(&self, query: &str) -> Option<Vec<StableId>> {
        (**self).search(query)
    }
    fn describe(&self) -> String {
        (**self).describe()
    }
}
