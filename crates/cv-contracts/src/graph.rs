//! Boris IR graph (`graph.json`, written by IR-mode `build --no-rag`,
//! default `.boris/`). Only the page fields the app displays are typed;
//! nav and the reverse index are left to the lanes that need them.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Graph {
    pub schema_version: String,
    pub nodes: Vec<Node>,
    /// Absent when the corpus declares no semantic relations.
    #[serde(default)]
    pub relations: Vec<Relation>,
    /// Authoring edges (`parent`, `reference`) used for impact disclosure.
    #[serde(default)]
    pub edges: Vec<Relation>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Node {
    pub id: String,
    /// Relative to the content input.
    pub source_path: String,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
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
