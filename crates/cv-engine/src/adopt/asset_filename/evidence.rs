//! Pinned lab evidence for an asset-filename run: the report, the asset
//! manifest, the rewrite manifest, and the candidate tree. Every document
//! is read without following links inside the run's `out/` directory, and
//! the digest of every byte read is what the transformation journal binds —
//! evidence mutated after journaling no longer matches the journaled
//! digests and the recovery attempt refuses.

use super::{AdoptError, AdoptionSource, malformed, sha256_hex_shape};
use cv_contracts::lab::{AssetFilenameManifest, AssetFilenameRewriteManifest, Document, Report};
use cv_store::imports as store;
use std::collections::{BTreeMap, BTreeSet};

/// The lab documents plus the digests the journal binds.
pub(super) struct Evidence {
    pub manifest: AssetFilenameManifest,
    pub rewrites: AssetFilenameRewriteManifest,
    pub report_sha256: String,
    pub manifest_sha256: String,
    pub rewrite_sha256: String,
}

/// One candidate-tree file's byte evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CandidateFile {
    pub sha256: String,
    pub bytes: u64,
}

/// The candidate tree projected to the parts the plan needs: the accepted
/// asset destinations (sha-checked against the manifest) and the Markdown
/// pages that carry post-image bytes for rewrite operations.
pub(super) struct Candidate {
    pub accepted_assets: BTreeSet<String>,
    /// Every candidate file that is not an accepted asset destination,
    /// keyed by its input-relative path.
    pub pages: BTreeMap<String, CandidateFile>,
    /// Digest over the whole candidate file list, journaled so a mutated
    /// candidate tree is detected on resume.
    pub digest: String,
}

fn read_evidence_json(
    target_root: &std::path::Path,
    src: &AdoptionSource,
    name: &str,
) -> Result<Vec<u8>, AdoptError> {
    store::read_out_file(target_root, &src.run_id, name)
        .map_err(|e| AdoptError::Report(format!("{name} is unreadable: {e}")))
}

/// Read a candidate-tree file without following links inside `out/`.
pub(super) fn read_candidate_bytes(
    target_root: &std::path::Path,
    src: &AdoptionSource,
    rel: &str,
) -> Result<Vec<u8>, AdoptError> {
    let out_rel = format!("content/{rel}");
    store::read_out_file(target_root, &src.run_id, &out_rel).map_err(|e| {
        malformed(format!(
            "candidate file {rel} is unreadable without following links: {e}"
        ))
    })
}

pub(super) fn load(
    target_root: &std::path::Path,
    src: &AdoptionSource,
) -> Result<Evidence, AdoptError> {
    let report_bytes = read_evidence_json(target_root, src, "report.json")?;
    let report = Report::decode(&report_bytes).map_err(|e| AdoptError::Report(e.to_string()))?;
    let Report::AssetFilename(report) = report else {
        return Err(malformed("report.json is not an asset-filename report"));
    };
    if !report.policy.source_readonly
        || report.policy.remote_fetch
        || report.policy.execute_js
        || report.policy.silent_overwrite
        || !report.policy.boris_core_unchanged
    {
        return Err(malformed(
            "the report policy is not the pinned read-only/no-overwrite contract",
        ));
    }
    let manifest_bytes = read_evidence_json(target_root, src, "asset_filename_manifest.json")?;
    let rewrite_bytes = read_evidence_json(target_root, src, "rewrite_manifest.json")?;
    let manifest = match Document::decode("asset_filename_manifest.json", &manifest_bytes)
        .map_err(|e| AdoptError::Report(e.to_string()))?
    {
        Document::AssetFilenameManifest(m) => m,
        _ => {
            return Err(malformed(
                "asset_filename_manifest.json decoded as the wrong document",
            ));
        }
    };
    let rewrites = match Document::decode("rewrite_manifest.json", &rewrite_bytes)
        .map_err(|e| AdoptError::Report(e.to_string()))?
    {
        Document::AssetFilenameRewriteManifest(r) => r,
        _ => {
            return Err(malformed(
                "rewrite_manifest.json decoded as the wrong document",
            ));
        }
    };
    let count = |action: &str| {
        manifest
            .assets
            .iter()
            .filter(|a| a.action == action)
            .count() as u64
    };
    if manifest.assets.len() as u64 != report.counts.assets
        || count("unchanged") != report.counts.unchanged
        || count("rewritten") != report.counts.rewritten
        || count("rejected") != report.counts.rejected
        || rewrites.rewrites.len() as u64 != report.counts.markdown_rewrites
    {
        return Err(malformed(
            "report counts do not match the machine manifests",
        ));
    }
    let mut sources = BTreeSet::new();
    for asset in &manifest.assets {
        if !matches!(
            asset.action.as_str(),
            "unchanged" | "rewritten" | "rejected"
        ) {
            return Err(malformed(format!(
                "unknown asset action {:?}",
                asset.action
            )));
        }
        if !sources.insert(asset.source_path.clone()) {
            return Err(malformed(format!(
                "duplicate asset row for {}",
                asset.source_path
            )));
        }
        if let Some(hash) = &asset.sha256
            && !sha256_hex_shape(hash)
        {
            return Err(malformed(format!(
                "asset {} carries malformed sha256 evidence",
                asset.source_path
            )));
        }
        if asset.sha256.is_none() && asset.reason != "symlink" {
            return Err(malformed(format!(
                "non-symlink asset {} has no sha256 evidence",
                asset.source_path
            )));
        }
        if asset.action == "unchanged" && asset.dest_path != asset.source_path {
            return Err(malformed(format!(
                "unchanged asset {} names a different destination {}",
                asset.source_path, asset.dest_path
            )));
        }
    }
    for rewrite in &rewrites.rewrites {
        if rewrite.original_dest.is_empty() || rewrite.rewritten_dest.is_empty() {
            return Err(malformed("a rewrite row has an empty endpoint"));
        }
        if rewrite.original_dest == rewrite.rewritten_dest {
            return Err(malformed(format!(
                "rewrite row for {} is a no-op",
                rewrite.page_path
            )));
        }
    }
    Ok(Evidence {
        manifest,
        rewrites,
        report_sha256: store::sha256_hex(&report_bytes),
        manifest_sha256: store::sha256_hex(&manifest_bytes),
        rewrite_sha256: store::sha256_hex(&rewrite_bytes),
    })
}

/// Load the candidate tree: verify every accepted asset destination exists
/// with manifest bytes and classify every other candidate file as a page.
/// The resulting digest is journaled, so a candidate mutated between the
/// journal and a resume attempt is detected by the binding, not re-hashed
/// silently.
pub(super) fn candidate(
    target_root: &std::path::Path,
    src: &AdoptionSource,
    evidence: &Evidence,
) -> Result<Candidate, AdoptError> {
    let accepted_assets: BTreeSet<String> = evidence
        .manifest
        .assets
        .iter()
        .filter(|a| a.action != "rejected" && !a.dest_path.is_empty())
        .map(|a| a.dest_path.clone())
        .collect();
    let by_rel: BTreeMap<_, _> = src.files.iter().map(|f| (f.relative.as_str(), f)).collect();
    for asset in &evidence.manifest.assets {
        if asset.action == "rejected" || asset.dest_path.is_empty() {
            continue;
        }
        if !by_rel.contains_key(asset.dest_path.as_str()) {
            return Err(malformed(format!(
                "candidate is missing accepted asset {}",
                asset.dest_path
            )));
        }
        let bytes = read_candidate_bytes(target_root, src, &asset.dest_path)?;
        if Some(store::sha256_hex(&bytes)) != asset.sha256 || bytes.len() as u64 != asset.bytes {
            return Err(malformed(format!(
                "candidate asset {} does not match its manifest hash",
                asset.dest_path
            )));
        }
    }
    let mut pages = BTreeMap::new();
    let mut digest_lines = Vec::new();
    for file in &src.files {
        if accepted_assets.contains(&file.relative) {
            // Already hash-verified against the manifest above.
            let bytes = read_candidate_bytes(target_root, src, &file.relative)?;
            digest_lines.push(format!(
                "{}\0{}\0{}\n",
                file.relative,
                store::sha256_hex(&bytes),
                bytes.len()
            ));
            continue;
        }
        if !(file.relative.ends_with(".md") || file.relative.ends_with(".mdx")) {
            return Err(malformed(format!(
                "candidate file {} is neither an accepted asset nor a Markdown page",
                file.relative
            )));
        }
        let bytes = read_candidate_bytes(target_root, src, &file.relative)?;
        let candidate_file = CandidateFile {
            sha256: store::sha256_hex(&bytes),
            bytes: bytes.len() as u64,
        };
        digest_lines.push(format!(
            "{}\0{}\0{}\n",
            file.relative, candidate_file.sha256, candidate_file.bytes
        ));
        pages.insert(file.relative.clone(), candidate_file);
    }
    // The candidate digest must be a pure function of the tree, not of the
    // scan's directory order, or a resume on another host would compare
    // different bytes.
    digest_lines.sort();
    Ok(Candidate {
        accepted_assets,
        pages,
        digest: store::sha256_hex(digest_lines.concat().as_bytes()),
    })
}
