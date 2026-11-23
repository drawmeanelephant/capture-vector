//! cv-notes — the research/notes silo (L6).
//!
//! Notes live beside the Boris tree, never inside `content/`. Truth is
//! append-only JSONL under `.cv/research/`; SQLite FTS5 is a rebuildable
//! index. A note becomes a page only through [`promote`]: the draft goes
//! through the engine's fingerprinted staged write and `boris validate`
//! judges it. Provenance (note → page stable id) is a `.cv/` sidecar map,
//! never frontmatter.

pub mod graph;
pub mod model;
pub mod notebook;
pub mod pages;
pub mod port;
pub mod promote;
pub mod record;
pub mod render;
pub mod session;

#[cfg(feature = "disk")]
pub mod disk;

pub use graph::{Link, Target};
pub use model::{Capture, Format, Note, Triage};
pub use notebook::{Notebook, SmartMailbox};
pub use pages::{PageIndex, PageRef};
pub use port::{MemoryPort, ResearchPort};
pub use promote::{PromoteError, PromoteSpec, promote, promote_with};
pub use record::{Provenance, Record};
pub use render::{OliverRender, RenderCache};
pub use session::{Promotion, Research};

/// Store namespace for the research silo (`research.*`).
pub const NAMESPACE: &str = "research.notes";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum NotesError {
    #[error("unknown note {0}")]
    UnknownNote(String),
    #[error("duplicate id {0}")]
    Duplicate(String),
    #[error("unknown link {0}")]
    UnknownLink(String),
    #[error("promote: {0}")]
    Promote(#[from] PromoteError),
    #[error("persist: {0}")]
    Port(String),
}

pub type Result<T> = std::result::Result<T, NotesError>;
