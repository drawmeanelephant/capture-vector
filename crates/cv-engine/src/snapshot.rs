//! Disk truth. `snapshot()` rereads what Boris last wrote — it never
//! derives anything Boris didn't emit. Paths are Boris's documented
//! defaults (relative to the workspace).

use cv_core::rules::{ArtifactKind, ArtifactRef, ArtifactSet};
use cv_core::sys::Hash;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;

/// Artifact kind → canonical file Boris writes for it.
pub fn artifact_paths(html_dir: &str) -> Vec<(ArtifactKind, String)> {
    vec![
        (ArtifactKind::Ir, ".boris/graph.json".into()),
        (ArtifactKind::Html, format!("{html_dir}/index.html")),
        (ArtifactKind::Sitemap, format!("{html_dir}/sitemap.xml")),
        (ArtifactKind::RobotsTxt, format!("{html_dir}/robots.txt")),
        (ArtifactKind::Rss, "rss.xml".into()),
        (ArtifactKind::Llms, "llms.txt".into()),
        (ArtifactKind::Rag, "rag/manifest.json".into()),
        (ArtifactKind::ContextBundle, "context/manifest.json".into()),
    ]
}

/// Last run reports Boris leaves behind.
pub const REPORTS: &[&str] = &[
    ".boris/build-report.json",
    ".boris/completion.json",
    ".boris/manifest.json",
];

pub fn proof_reports(html_dir: &str) -> Vec<String> {
    ["artifacts", "checks", "claims", "proof-pack", "touches"]
        .iter()
        .map(|n| format!("{html_dir}/_boris/proof/{n}.json"))
        .collect()
}

fn hash_file(root: &Path, rel: &str) -> Option<ArtifactRef> {
    let bytes = crate::write::retry_sharing(|| std::fs::read(root.join(rel))).ok()?;
    Some(ArtifactRef {
        path: rel.to_string(),
        sha256: Hash(Sha256::digest(&bytes).into()),
    })
}

/// Everything `snapshot()` reread, including reports `ArtifactSet` has
/// no slot for.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskTruth {
    pub artifacts: ArtifactSet,
    pub reports: Vec<ArtifactRef>,
}

pub fn read(root: &Path, html_dir: &str) -> DiskTruth {
    let mut set = ArtifactSet {
        ir: None,
        html: None,
        exports: vec![],
        deploy: None,
        missing: vec![],
    };
    for (kind, rel) in artifact_paths(html_dir) {
        match (kind, hash_file(root, &rel)) {
            (ArtifactKind::Ir, Some(r)) => set.ir = Some(r),
            (ArtifactKind::Html, Some(r)) => set.html = Some(r),
            (_, Some(r)) => set.exports.push(r),
            (k, None) => set.missing.push(k),
        }
    }
    let reports = REPORTS
        .iter()
        .map(|s| s.to_string())
        .chain(proof_reports(html_dir))
        .filter_map(|rel| hash_file(root, &rel))
        .collect();
    DiskTruth {
        artifacts: set,
        reports,
    }
}
