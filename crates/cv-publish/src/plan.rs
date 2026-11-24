//! The sealed plan: exactly which bytes go where. Hashes are sha256 over
//! canonical JSON (struct field order + sorted file list).

use crate::{PublishError, Result};
use cv_core::sys::Hash;
use cv_store::paths::{confined_file, is_link, portable_relative};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PlanFile {
    /// Output-relative, `/`-separated.
    pub path: String,
    pub bytes: u64,
    pub sha256: Hash,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SealedPlan {
    pub target: String,
    pub adapter: String,
    pub destination: String,
    /// Project-relative output dir the preflight built into.
    pub site: String,
    pub files: Vec<PlanFile>,
}

pub fn sha256(bytes: &[u8]) -> Hash {
    Hash(Sha256::digest(bytes).into())
}

pub fn hash_json<T: Serialize>(v: &T) -> Result<Hash> {
    Ok(sha256(&serde_json::to_vec(v)?))
}

impl SealedPlan {
    pub(crate) fn validate_file_paths(&self) -> Result<()> {
        for f in &self.files {
            safe_rel_path(&f.path)?;
        }
        Ok(())
    }

    pub fn hash(&self) -> Result<Hash> {
        hash_json(self)
    }

    /// `artifact_set_hash`: the byte inventory alone, independent of
    /// where it's being delivered.
    pub fn artifact_set_hash(&self) -> Result<Hash> {
        hash_json(&self.files)
    }

    pub fn total_bytes(&self) -> u64 {
        self.files.iter().map(|f| f.bytes).sum()
    }

    pub fn file(&self, path: &str) -> Option<&PlanFile> {
        self.files.iter().find(|f| f.path == path)
    }
}

pub(crate) fn safe_rel_path(path: &str) -> Result<&Path> {
    portable_relative(path)
        .map_err(|_| PublishError::Refused(format!("invalid relative publish path: {path:?}")))
}

/// Walk an output tree into a sorted inventory.
pub fn inventory(root: &Path) -> Result<Vec<PlanFile>> {
    let mut out = Vec::new();
    walk(root, root, &mut out)?;
    out.sort();
    Ok(out)
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<PlanFile>) -> Result<()> {
    for e in std::fs::read_dir(dir)? {
        let e = e?;
        let p = e.path();
        let meta = e.metadata()?;
        if is_link(&meta) {
            return Err(PublishError::Refused(format!(
                "linked file in output: {p:?}"
            )));
        }
        if meta.is_dir() {
            walk(root, &p, out)?;
        } else if meta.is_file() {
            let rel = p
                .strip_prefix(root)
                .map_err(|_| PublishError::Refused(format!("path escapes output: {p:?}")))?;
            let rel = rel
                .components()
                .map(|c| c.as_os_str().to_str())
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| PublishError::Refused("non-UTF-8 path in output".into()))?
                .join("/");
            safe_rel_path(&rel)?;
            let bytes = std::fs::read(&p)?;
            out.push(PlanFile {
                path: rel,
                bytes: bytes.len() as u64,
                sha256: sha256(&bytes),
            });
        } else {
            return Err(PublishError::Refused(format!(
                "non-regular file in output: {p:?}"
            )));
        }
    }
    Ok(())
}

/// Read every sealed file and verify its bytes. Adapters receive only
/// these verified buffers — never a path to re-read.
pub fn load_sealed(site: &Path, plan: &SealedPlan) -> Result<Vec<(PlanFile, Vec<u8>)>> {
    plan.validate_file_paths()?;
    let mut out = Vec::with_capacity(plan.files.len());
    for f in &plan.files {
        let path = confined_file(site, Path::new(&f.path))?;
        let bytes = std::fs::read(path)?;
        if bytes.len() as u64 != f.bytes || sha256(&bytes) != f.sha256 {
            return Err(PublishError::SealBroken(vec!["artifact_set"]));
        }
        out.push((f.clone(), bytes));
    }
    Ok(out)
}
