//! Typed link graph. Every endpoint and every edge is a `StableId` —
//! paths never identify anything, so edges survive renames and reparents.

use cv_core::id::{NounId, StableId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "lowercase")]
pub enum Target {
    Note(StableId),
    /// A Boris page by its stable (frontmatter/graph) id.
    Page(StableId),
    /// An audit finding by its stable id.
    Finding(StableId),
}

impl Target {
    pub fn kind(&self) -> &'static str {
        match self {
            Target::Note(_) => cv_core::id::noun_kind::NOTE,
            Target::Page(_) => cv_core::id::noun_kind::PAGE,
            Target::Finding(_) => cv_core::id::noun_kind::FINDING,
        }
    }
    pub fn id(&self) -> &StableId {
        match self {
            Target::Note(s) | Target::Page(s) | Target::Finding(s) => s,
        }
    }
    pub fn from_noun(n: &NounId) -> Option<Self> {
        let s = n.stable.clone();
        match n.kind.0.as_str() {
            cv_core::id::noun_kind::NOTE => Some(Target::Note(s)),
            cv_core::id::noun_kind::PAGE => Some(Target::Page(s)),
            cv_core::id::noun_kind::FINDING => Some(Target::Finding(s)),
            _ => None,
        }
    }
}

/// One directed edge `from` (always a note) → `to`. Note↔note links are
/// stored once; the reverse direction is a backlink resolved at read time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Link {
    pub id: StableId,
    pub from: StableId,
    pub to: Target,
}
