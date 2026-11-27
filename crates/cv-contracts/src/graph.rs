//! Boris IR graph (`graph.json`, written by IR-mode `build --no-rag`,
//! default `.boris/`). The typed shape follows the frozen IR contract;
//! the Cooklang `recipe` facet remains intentionally untyped and ignored by
//! consumers that do not understand it.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Graph {
    pub schema_version: String,
    pub frozen: bool,
    pub nodes: Vec<Node>,
    /// Build-dependency edges (`parent`, `include`, `reference`). These are
    /// not semantic relations.
    pub edges: Vec<Relation>,
    /// Boris's build-invalidation reverse index for dependency `edges`.
    /// Semantic relations deliberately have no upstream reverse index.
    pub reverse_index: Vec<ReverseIndexEntry>,
    /// Derived page navigation over the already-validated frozen forest.
    pub nav: Vec<NavEntry>,
    /// Absent in relation-free IR 0.2 artifacts; present (possibly empty) in
    /// semantic-relation-bearing IR 0.3+ artifacts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relations: Vec<Relation>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Node {
    /// Stable 0-based index in this frozen graph's id-sorted node array.
    pub index: usize,
    pub id: String,
    /// Relative to the content input.
    pub source_path: String,
    /// Boris's validated role token (`trunk` or `satellite` in IR 0.2+).
    pub role: String,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub parent_index: Option<usize>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub body_offset: usize,
}

/// One per-page navigation entry, in the same order as `nodes`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NavEntry {
    pub index: usize,
    pub id: String,
    /// Root-to-self inclusive, as Boris node indices.
    pub breadcrumb: Vec<usize>,
    /// Direct child node indices in entity-id order.
    pub children: Vec<usize>,
    /// Direct peers sharing the same parent, excluding self; empty for trunks.
    pub siblings: Vec<usize>,
}

/// Target-keyed incoming dependency-edge lookup. This is build invalidation
/// evidence, not semantic-relation adjacency.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReverseIndexEntry {
    pub target: Endpoint,
    pub incoming_edges: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Relation {
    pub from: Endpoint,
    pub to: Endpoint,
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Endpoint {
    #[serde(rename = "type")]
    pub kind: String,
    pub value: String,
}
