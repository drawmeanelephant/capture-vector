//! Read-time page resolution from Boris's own IR (`.boris/graph.json`).
//! Pages are keyed by their Boris id — the sidecar's join key — so a
//! renamed or reparented page resolves to its new path and parent.

use cv_core::id::StableId;
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageRef {
    pub id: StableId,
    pub source_path: String,
    #[serde(default)]
    pub parent: Option<StableId>,
    #[serde(default)]
    pub title: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct PageIndex {
    pages: BTreeMap<StableId, PageRef>,
}

#[derive(Deserialize)]
struct Graph {
    nodes: Vec<PageRef>,
}

impl PageIndex {
    pub fn from_graph_json(bytes: &[u8]) -> Result<Self, String> {
        let g: Graph = serde_json::from_slice(bytes).map_err(|e| format!("graph.json: {e}"))?;
        Ok(Self {
            pages: g.nodes.into_iter().map(|p| (p.id.clone(), p)).collect(),
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
