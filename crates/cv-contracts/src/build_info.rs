//! `boris --build-info` / `fixtures/engine-version.txt`.

use serde::{Deserialize, Serialize};

pub const FORMAT: &str = "boris-build-info";

/// `{"format":"boris-build-info","schemaVersion":"1","version":"boris/0.9.0","vcsRevision":"66efcfb"}`
/// — note the camelCase fields; upstream emits them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildInfo {
    pub format: String,
    #[serde(rename = "schemaVersion")]
    pub schema_version: String,
    pub version: String,
    #[serde(rename = "vcsRevision")]
    pub vcs_revision: String,
}

impl BuildInfo {
    pub fn is_boris(&self) -> bool {
        self.format == FORMAT
    }
}
