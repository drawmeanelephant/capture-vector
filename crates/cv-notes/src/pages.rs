//! Read-time page resolution from Boris's own IR (`.boris/graph.json`).
//! Pages are keyed by their Boris id — the sidecar's join key — so a
//! renamed or reparented page resolves to its new path and parent.
//!
//! The bytes decode through `cv-contracts`' version-keyed mirror: an
//! unsupported schema version or a missing required key means no page
//! authority at all, never a narrowed guess at one.

use cv_core::id::StableId;
use std::collections::BTreeMap;

/// One page node projected for note resolution. IR node keys are
/// required even when nullable, so a decoded node always carries them;
/// the `Option`s here are nullability, not absence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageRef {
    pub id: StableId,
    pub source_path: String,
    pub parent: Option<StableId>,
    pub title: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct PageIndex {
    pages: BTreeMap<StableId, PageRef>,
}

impl PageIndex {
    pub fn from_graph_json(bytes: &[u8]) -> Result<Self, String> {
        let g = cv_contracts::graph::decode_graph(bytes).map_err(|e| format!("graph.json: {e}"))?;
        Ok(Self {
            pages: g
                .nodes
                .into_iter()
                .map(|n| {
                    let id = StableId(n.id);
                    (
                        id.clone(),
                        PageRef {
                            id,
                            source_path: n.source_path,
                            parent: n.parent.map(StableId),
                            title: n.title,
                        },
                    )
                })
                .collect(),
        })
    }
    pub fn get(&self, page: &StableId) -> Option<&PageRef> {
        self.pages.get(page)
    }
    pub fn len(&self) -> usize {
        self.pages.len()
    }
    pub fn is_empty(&self) -> bool {
        self.pages.is_empty()
    }
}
