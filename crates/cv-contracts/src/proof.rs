//! Proof-pack family: `dist/_boris/proof/*.json` + rendered search index.
//! Mirrors are shallow-but-typed: identity hashes and status fields are
//! typed; deeply nested evidence blocks stay `serde_json::Value` so
//! additive upstream fields pass through.

use serde::{Deserialize, Serialize};

/// File descriptor Boris embeds when referencing another artifact
/// (`artifact_inventory`, `inputs.*`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileDescriptor {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub schema_version: Option<u32>,
    #[serde(default)]
    pub target: Option<String>,
    /// Forward tolerance (`artifact_count`, `check_count`, …).
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// `boris-publication-artifacts` (artifacts.json).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArtifactIndex {
    pub format: String,
    pub schema_version: u32,
    pub target: String,
    pub artifacts: Vec<Artifact>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Artifact {
    pub path: String,
    pub kind: String,
    pub producer: String,
    pub required: bool,
    pub status: String,
    pub bytes: u64,
    pub sha256: String,
    #[serde(default)]
    pub format_version: Option<String>,
    #[serde(default)]
    pub dimensions: Option<serde_json::Value>,
    #[serde(default)]
    pub semantics: Option<String>,
    #[serde(default)]
    pub advertised: Option<bool>,
}

/// `boris-publication-checks` (checks.json).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChecksFile {
    pub format: String,
    pub schema_version: u32,
    pub target: String,
    #[serde(default)]
    pub artifact_inventory: Option<FileDescriptor>,
    #[serde(default)]
    pub checks: Vec<CheckEntry>,
    #[serde(default)]
    pub findings: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckEntry {
    pub id: String,
    pub eligible: bool,
    pub ran: bool,
    pub status: String,
    #[serde(default)]
    pub coverage: Option<String>,
    #[serde(default)]
    pub scope: Option<serde_json::Value>,
    #[serde(default)]
    pub counts: Option<CheckCounts>,
    #[serde(default)]
    pub finding_offset: Option<u64>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckCounts {
    pub eligible: u64,
    pub checked: u64,
    pub findings: u64,
}

/// `boris-publication-proof-pack` (proof-pack.json) — inputs/outputs are
/// maps of descriptors keyed by role.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProofPack {
    pub format: String,
    pub schema_version: u32,
    pub target: String,
    #[serde(default)]
    pub vcs_revision: Option<String>,
    #[serde(default)]
    pub inputs: serde_json::Map<String, serde_json::Value>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// `boris-publication-claims` / `boris-publication-touches` — nested
/// descriptor maps we mirror generically until a lane needs the leaves.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvidenceFile {
    pub format: String,
    pub schema_version: u32,
    pub target: String,
    #[serde(flatten)]
    pub inputs: serde_json::Map<String, serde_json::Value>,
}

/// `boris-rendered-search-index` (search-index.json).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchIndex {
    pub format: String,
    pub schema_version: u32,
    #[serde(default)]
    pub documents: Vec<SearchDocument>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchDocument {
    pub path: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub sections: Vec<SearchSection>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchSection {
    pub level: u32,
    pub heading: String,
    pub fragment: String,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub code: Option<String>,
}
