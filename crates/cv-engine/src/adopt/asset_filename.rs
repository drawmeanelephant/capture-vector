//! Operation-based in-place adoption of `asset-filename` utility output.
//!
//! The lab's candidate tree is evidence, not a tree to copy. This module
//! verifies that evidence against the live workspace, then applies only the
//! operations the pinned manifests authorize: fingerprinted asset moves and
//! fingerprinted Markdown reference writes. Rejected and collision-bound rows
//! stay untouched. The transformation is journaled before the first write so
//! an interrupted attempt converges instead of overwriting user edits.

use super::{
    AdoptError, AdoptionSource, ExpectedWorkspaceInstance, IdentityCheckPhase, RecordAdoptionInput,
    TxReport, content_input_authority, content_input_target_authority, copy_provenance,
    record_adoption, revalidate_workspace_instance,
};
use crate::write;
use cv_contracts::lab::{
    AssetFilenameAsset, AssetFilenameManifest, AssetFilenameRewrite, AssetFilenameRewriteManifest,
    Document, Report,
};
use cv_core::engine::{Fingerprint, MoveOutcome, RemoveOutcome, WriteOutcome};
use cv_store::imports as store;
use cv_store::paths::{confined, confined_file, portable_relative};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Inputs for an existing-workspace in-place asset-filename transformation.
pub struct AssetFilenameTransaction<'a> {
    pub target_root: &'a Path,
    pub src: &'a AdoptionSource,
    pub content_input: &'a str,
    pub at_ms: i64,
    pub expected_instance: &'a ExpectedWorkspaceInstance,
}

type WorkspaceFileState = (String, u64, Fingerprint, Vec<u8>);

#[derive(Debug)]
struct Evidence {
    manifest: AssetFilenameManifest,
    rewrites: AssetFilenameRewriteManifest,
}

#[derive(Debug, Default)]
struct Metadata {
    collision_sources: BTreeSet<String>,
    collision_dests: BTreeSet<String>,
    contaminated_pages: BTreeSet<String>,
    rewrites_by_page: BTreeMap<String, Vec<AssetFilenameRewrite>>,
}

#[derive(Debug, Default)]
struct Plan {
    operations: Vec<store::PendingAssetOperation>,
}

#[derive(Debug)]
struct CandidateFile {
    sha256: String,
    bytes: u64,
}

#[derive(Debug)]
struct Candidate {
    accepted_assets: BTreeSet<String>,
    pages: BTreeMap<String, CandidateFile>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LiveEntry {
    File,
    Symlink,
    Other,
}

#[derive(Debug)]
struct OperationAuthority {
    kind: store::AssetOperationKind,
    source: String,
    target: String,
    source_sha256: Option<String>,
    source_bytes: Option<u64>,
    target_sha256: String,
    target_bytes: u64,
}

fn hex(bytes: &[u8]) -> String {
    store::sha256_hex(bytes)
}

fn fp_hex(fp: Fingerprint) -> String {
    fp.sha256.to_hex()
}

fn sha256_hex_shape(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

fn stale(message: impl Into<String>) -> AdoptError {
    AdoptError::Stale(format!(
        "asset-filename utility evidence is stale against the live workspace: {}; nothing was overwritten",
        message.into()
    ))
}

fn malformed(message: impl Into<String>) -> AdoptError {
    AdoptError::Report(format!(
        "asset-filename lab evidence is inconsistent: {}",
        message.into()
    ))
}

fn check_rel(rel: &str, what: &str) -> Result<(), AdoptError> {
    portable_relative(rel)
        .map_err(|_| malformed(format!("{what} path {rel:?} is not portable")))?;
    Ok(())
}

fn ws_path(input: &str, rel: &str) -> Result<String, AdoptError> {
    check_rel(rel, "content")?;
    let out = format!("{input}/{rel}");
    portable_relative(&out)
        .map_err(|_| malformed(format!("workspace target {out:?} is not portable")))?;
    Ok(out)
}

fn read_workspace_file(
    root: &Path,
    rel: &str,
) -> Result<Option<(Vec<u8>, Fingerprint)>, AdoptError> {
    portable_relative(rel)
        .map_err(|_| AdoptError::Content(format!("path {rel:?} is not portable")))?;
    let abs = confined_file(root, Path::new(rel))
        .map_err(|e| AdoptError::Content(format!("{rel}: {e}")))?;
    let meta = match std::fs::symlink_metadata(&abs) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    if cv_store::paths::is_link(&meta) || !meta.is_file() {
        return Err(AdoptError::Content(format!(
            "{rel} is not an ordinary file inside the workspace"
        )));
    }
    let bytes = std::fs::read(&abs)?;
    let Some(fp) = write::fingerprint(&abs)? else {
        return Ok(None);
    };
    if fp_hex(fp) != hex(&bytes) {
        return Err(stale(format!("{rel} changed while it was being read")));
    }
    Ok(Some((bytes, fp)))
}

fn file_state(root: &Path, rel: &str) -> Result<Option<WorkspaceFileState>, AdoptError> {
    Ok(read_workspace_file(root, rel)?.map(|(bytes, fp)| {
        let len = bytes.len() as u64;
        (fp_hex(fp), len, fp, bytes)
    }))
}

fn read_evidence_json(
    target_root: &Path,
    src: &AdoptionSource,
    name: &str,
) -> Result<Vec<u8>, AdoptError> {
    store::read_out_file(target_root, &src.run_id, name)
        .map_err(|e| AdoptError::Report(format!("{name} is unreadable: {e}")))
}

fn decode_evidence(target_root: &Path, src: &AdoptionSource) -> Result<Evidence, AdoptError> {
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
        check_rel(&asset.source_path, "asset source")?;
        if !asset.dest_path.is_empty() {
            check_rel(&asset.dest_path, "asset destination")?;
        }
        if let Some(hash) = &asset.sha256 {
            if !sha256_hex_shape(hash) {
                return Err(malformed(format!(
                    "asset {} carries malformed sha256 evidence",
                    asset.source_path
                )));
            }
        } else if asset.reason != "symlink" {
            return Err(malformed(format!(
                "non-symlink asset {} has no sha256 evidence",
                asset.source_path
            )));
        }
        if !sources.insert(asset.source_path.clone()) {
            return Err(malformed(format!(
                "duplicate asset row for {}",
                asset.source_path
            )));
        }
    }
    let accepted: BTreeMap<&str, &AssetFilenameAsset> = manifest
        .assets
        .iter()
        .filter(|a| a.action != "rejected" && !a.dest_path.is_empty())
        .map(|a| (a.dest_path.as_str(), a))
        .collect();
    for rewrite in &rewrites.rewrites {
        check_rel(&rewrite.page_path, "rewrite page")?;
        check_rel(&rewrite.rewritten_dest, "rewritten destination")?;
        check_rel(&rewrite.original_dest, "original destination")?;
        if rewrite.original_dest.is_empty() || rewrite.rewritten_dest.is_empty() {
            return Err(malformed("a rewrite row has an empty endpoint"));
        }
        if rewrite.original_dest == rewrite.rewritten_dest {
            return Err(malformed(format!(
                "rewrite row for {} is a no-op",
                rewrite.page_path
            )));
        }
        let Some(asset) = accepted.get(rewrite.rewritten_dest.as_str()) else {
            return Err(malformed(format!(
                "rewrite destination {} has no accepted asset row",
                rewrite.rewritten_dest
            )));
        };
        if asset.action != "rewritten" {
            return Err(malformed(format!(
                "rewrite destination {} belongs to an unchanged asset",
                rewrite.rewritten_dest
            )));
        }
    }
    Ok(Evidence { manifest, rewrites })
}

fn metadata(evidence: &Evidence) -> Metadata {
    let mut claims: BTreeMap<String, BTreeSet<&str>> = BTreeMap::new();
    for asset in &evidence.manifest.assets {
        if !asset.dest_path.is_empty() {
            claims
                .entry(asset.dest_path.to_ascii_lowercase())
                .or_default()
                .insert(asset.source_path.as_str());
        }
    }
    let mut out = Metadata::default();
    for asset in &evidence.manifest.assets {
        if asset.action == "rewritten"
            && claims
                .get(&asset.dest_path.to_ascii_lowercase())
                .is_some_and(|sources| sources.len() > 1)
        {
            out.collision_sources.insert(asset.source_path.clone());
            out.collision_dests.insert(asset.dest_path.clone());
        }
    }
    for rewrite in &evidence.rewrites.rewrites {
        if out.collision_dests.contains(&rewrite.rewritten_dest) {
            out.contaminated_pages.insert(rewrite.page_path.clone());
        }
        out.rewrites_by_page
            .entry(rewrite.page_path.clone())
            .or_default()
            .push(rewrite.clone());
    }
    out
}

fn replace_once(haystack: &[u8], needle: &[u8], replacement: &[u8]) -> Option<Vec<u8>> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    let at = haystack.windows(needle.len()).position(|w| w == needle)?;
    let mut out =
        Vec::with_capacity(haystack.len() + replacement.len().saturating_sub(needle.len()));
    out.extend_from_slice(&haystack[..at]);
    out.extend_from_slice(replacement);
    out.extend_from_slice(&haystack[at + needle.len()..]);
    Some(out)
}

fn rewritten_bytes(mut bytes: Vec<u8>, rows: &[AssetFilenameRewrite]) -> Vec<u8> {
    for row in rows {
        if let Some(next) = replace_once(
            &bytes,
            row.original_dest.as_bytes(),
            row.rewritten_dest.as_bytes(),
        ) {
            bytes = next;
        }
    }
    bytes
}

fn read_candidate_bytes(
    target_root: &Path,
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

fn validate_candidate(
    target_root: &Path,
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
        if Some(hex(&bytes)) != asset.sha256 || bytes.len() as u64 != asset.bytes {
            return Err(malformed(format!(
                "candidate asset {} does not match its manifest hash",
                asset.dest_path
            )));
        }
    }
    let mut pages = BTreeMap::new();
    for file in &src.files {
        if accepted_assets.contains(&file.relative) {
            continue;
        }
        if !(file.relative.ends_with(".md") || file.relative.ends_with(".mdx")) {
            return Err(malformed(format!(
                "candidate file {} is neither an accepted asset nor a Markdown page",
                file.relative
            )));
        }
        let bytes = read_candidate_bytes(target_root, src, &file.relative)?;
        pages.insert(
            file.relative.clone(),
            CandidateFile {
                sha256: hex(&bytes),
                bytes: bytes.len() as u64,
            },
        );
    }
    Ok(Candidate {
        accepted_assets,
        pages,
    })
}

fn scan_live_dir(
    dir: &Path,
    prefix: &str,
    out: &mut BTreeMap<String, LiveEntry>,
) -> Result<(), AdoptError> {
    let entries = std::fs::read_dir(dir)
        .map_err(|e| AdoptError::Content(format!("cannot read {}: {e}", dir.display())))?;
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name().into_string().map_err(|_| {
            AdoptError::Content(format!(
                "non-UTF-8 file name under the live content input {}",
                dir.display()
            ))
        })?;
        let rel = format!("{prefix}{name}");
        let path = entry.path();
        let meta = std::fs::symlink_metadata(&path)?;
        if cv_store::paths::is_link(&meta) {
            out.insert(rel, LiveEntry::Symlink);
        } else if meta.is_dir() {
            scan_live_dir(&path, &format!("{rel}/"), out)?;
        } else if meta.is_file() {
            out.insert(rel, LiveEntry::File);
        } else {
            out.insert(rel, LiveEntry::Other);
        }
    }
    Ok(())
}

/// Inventory the live content input without following links. Directories
/// are traversal structure, not evidence rows; files, symlinks, and other
/// objects are what freshness reconciliation must account for.
fn live_inventory(
    target_root: &Path,
    input: &str,
) -> Result<BTreeMap<String, LiveEntry>, AdoptError> {
    let base = confined(target_root, Path::new(input)).map_err(|e| {
        AdoptError::Content(format!(
            "content input {input:?} is not confined to the workspace: {e}"
        ))
    })?;
    let meta = std::fs::symlink_metadata(&base).map_err(|e| {
        AdoptError::Content(format!(
            "content input {input:?} cannot be inventoried: {e}"
        ))
    })?;
    if cv_store::paths::is_link(&meta) || !meta.is_dir() {
        return Err(AdoptError::Content(format!(
            "content input {input:?} is not an ordinary directory"
        )));
    }
    let mut out = BTreeMap::new();
    scan_live_dir(&base, "", &mut out)?;
    Ok(out)
}

fn fresh_plan(
    target_root: &Path,
    input: &str,
    src: &AdoptionSource,
    evidence: &Evidence,
    meta: &Metadata,
    candidate: &Candidate,
) -> Result<Plan, AdoptError> {
    let live = live_inventory(target_root, input)?;
    let mut accounted = BTreeSet::new();
    let mut plan = Plan::default();
    for asset in &evidence.manifest.assets {
        let source_ws = ws_path(input, &asset.source_path)?;
        accounted.insert(asset.source_path.clone());
        if asset.reason == "symlink" && asset.sha256.is_none() {
            if live.get(&asset.source_path) != Some(&LiveEntry::Symlink) {
                return Err(stale(format!(
                    "{} is no longer the rejected symlink the lab saw",
                    asset.source_path
                )));
            }
            continue;
        }
        if live.get(&asset.source_path) != Some(&LiveEntry::File) {
            return Err(stale(format!(
                "{} is missing or is no longer an ordinary file",
                asset.source_path
            )));
        }
        let state = file_state(target_root, &source_ws)?;
        let Some((hash, len, _fp, _bytes)) = state else {
            return Err(stale(format!("{} is missing", asset.source_path)));
        };
        if Some(hash.clone()) != asset.sha256 || len != asset.bytes {
            return Err(stale(format!(
                "{} no longer matches the lab's sha256/byte evidence",
                asset.source_path
            )));
        }
        if asset.action == "rewritten" && !meta.collision_sources.contains(&asset.source_path) {
            let dest_ws = ws_path(input, &asset.dest_path)?;
            if write::paths_are_case_aliases(target_root, &source_ws, &dest_ws)? {
                return Err(malformed(format!(
                    "asset destination {} aliases its source on this filesystem",
                    asset.dest_path
                )));
            }
            if file_state(target_root, &dest_ws)?.is_some() {
                return Err(stale(format!(
                    "destination {} appeared after the lab ran; overwrite is refused",
                    asset.dest_path
                )));
            }
            plan.operations.push(store::PendingAssetOperation {
                kind: store::AssetOperationKind::Asset,
                source: source_ws,
                target: dest_ws,
                source_sha256: hash.clone(),
                source_bytes: len,
                target_sha256: hash,
                target_bytes: len,
            });
        }
    }

    for file in &src.files {
        if candidate.accepted_assets.contains(&file.relative) {
            continue;
        }
        let page = file.relative.as_str();
        if meta.contaminated_pages.contains(page) {
            accounted.insert(page.to_string());
            continue;
        }
        accounted.insert(page.to_string());
        if live.get(page) != Some(&LiveEntry::File) {
            return Err(stale(format!(
                "page {page} is missing or is no longer an ordinary file"
            )));
        }
        let ws = ws_path(input, page)?;
        let Some((_, _, _fp, current)) = file_state(target_root, &ws)? else {
            return Err(stale(format!("page {page} is missing")));
        };
        let empty = Vec::new();
        let rows = meta.rewrites_by_page.get(page).unwrap_or(&empty);
        let final_bytes = rewritten_bytes(current.clone(), rows);
        let page_evidence = candidate
            .pages
            .get(page)
            .ok_or_else(|| malformed(format!("candidate page evidence for {page} is missing")))?;
        let candidate_bytes = read_candidate_bytes(target_root, src, page)?;
        if hex(&candidate_bytes) != page_evidence.sha256
            || candidate_bytes.len() as u64 != page_evidence.bytes
        {
            return Err(malformed(format!(
                "candidate page {page} changed after validation"
            )));
        }
        if final_bytes != candidate_bytes {
            return Err(stale(format!(
                "page {page} does not reproduce the lab candidate through the pinned rewrite manifest"
            )));
        }
        if !rows.is_empty() {
            let source_hash = hex(&current);
            let target_hash = hex(&final_bytes);
            plan.operations.push(store::PendingAssetOperation {
                kind: store::AssetOperationKind::Markdown,
                source: ws.clone(),
                target: ws,
                source_sha256: source_hash,
                source_bytes: current.len() as u64,
                target_sha256: target_hash,
                target_bytes: final_bytes.len() as u64,
            });
        }
    }
    if let Some(extra) = live.keys().find(|path| !accounted.contains(*path)) {
        return Err(stale(format!(
            "{extra} is not accounted for by the utility evidence; the workspace changed after the lab ran or the evidence is incomplete"
        )));
    }
    Ok(plan)
}

fn operation_authority(
    input: &str,
    src: &AdoptionSource,
    evidence: &Evidence,
    meta: &Metadata,
    candidate: &Candidate,
) -> Result<Vec<OperationAuthority>, AdoptError> {
    let mut out = Vec::new();
    for asset in &evidence.manifest.assets {
        if asset.action != "rewritten" || meta.collision_sources.contains(&asset.source_path) {
            continue;
        }
        let hash = asset.sha256.clone().ok_or_else(|| {
            malformed(format!(
                "rewritten asset {} has no sha256 evidence",
                asset.source_path
            ))
        })?;
        out.push(OperationAuthority {
            kind: store::AssetOperationKind::Asset,
            source: ws_path(input, &asset.source_path)?,
            target: ws_path(input, &asset.dest_path)?,
            source_sha256: Some(hash.clone()),
            source_bytes: Some(asset.bytes),
            target_sha256: hash,
            target_bytes: asset.bytes,
        });
    }
    for file in &src.files {
        if candidate.accepted_assets.contains(&file.relative) {
            continue;
        }
        let page = file.relative.as_str();
        if meta.contaminated_pages.contains(page) {
            continue;
        }
        let rows = meta.rewrites_by_page.get(page);
        if rows.is_none_or(|rows| rows.is_empty()) {
            continue;
        }
        let evidence = candidate
            .pages
            .get(page)
            .ok_or_else(|| malformed(format!("candidate page evidence for {page} is missing")))?;
        let ws = ws_path(input, page)?;
        out.push(OperationAuthority {
            kind: store::AssetOperationKind::Markdown,
            source: ws.clone(),
            target: ws,
            source_sha256: None,
            source_bytes: None,
            target_sha256: evidence.sha256.clone(),
            target_bytes: evidence.bytes,
        });
    }
    Ok(out)
}

fn journal_error(reason: impl Into<String>) -> AdoptError {
    AdoptError::Run(format!(
        "{} is not the complete asset transformation authorized by asset_filename_manifest.json and rewrite_manifest.json: {}; the journal was left in place and this attempt recorded no adoption",
        store::PENDING,
        reason.into()
    ))
}

fn validate_journal(
    journal: &store::PendingAdoption,
    authority: &[OperationAuthority],
) -> Result<Vec<store::PendingAssetOperation>, AdoptError> {
    let Some(operations) = journal.asset_operations.clone() else {
        return Err(AdoptError::Run(format!(
            "{} is a foreign-tree copy recovery journal, not an asset transformation; complete or remove it first",
            store::PENDING
        )));
    };
    if operations.len() != authority.len() {
        return Err(journal_error(format!(
            "it records {} operation(s), while the authorized transformation requires exactly {}",
            operations.len(),
            authority.len()
        )));
    }
    for op in &operations {
        if !sha256_hex_shape(&op.source_sha256) || !sha256_hex_shape(&op.target_sha256) {
            return Err(journal_error("it records malformed hash evidence"));
        }
    }
    for (index, (op, expected)) in operations.iter().zip(authority).enumerate() {
        let mismatch = |what: &str| {
            journal_error(format!(
                "operation {} has {what} that does not match the authorized transformation",
                index + 1
            ))
        };
        if op.kind != expected.kind {
            return Err(mismatch("a kind"));
        }
        if op.source != expected.source {
            return Err(mismatch("a source"));
        }
        if op.target != expected.target {
            return Err(mismatch("a target"));
        }
        if op.target_sha256 != expected.target_sha256 || op.target_bytes != expected.target_bytes {
            return Err(mismatch("target byte evidence"));
        }
        if expected.kind == store::AssetOperationKind::Asset
            && (Some(&op.source_sha256) != expected.source_sha256.as_ref()
                || Some(op.source_bytes) != expected.source_bytes)
        {
            return Err(mismatch("source byte evidence"));
        }
    }
    Ok(operations)
}

fn execute_asset(
    root: &Path,
    op: &store::PendingAssetOperation,
) -> Result<store::AssetOperationStatus, AdoptError> {
    let source = file_state(root, &op.source)?;
    let dest = file_state(root, &op.target)?;
    let source_matches = source
        .as_ref()
        .is_some_and(|(h, l, _, _)| *h == op.source_sha256 && *l == op.source_bytes);
    let dest_matches = dest
        .as_ref()
        .is_some_and(|(h, l, _, _)| *h == op.target_sha256 && *l == op.target_bytes);
    if write::paths_are_case_aliases(root, &op.source, &op.target)? {
        return Err(AdoptError::Run(format!(
            "journaled asset operation {} -> {} aliases one file on this filesystem; removing the source could destroy the only copy",
            op.source, op.target
        )));
    }
    if dest_matches && source.is_none() {
        return Ok(store::AssetOperationStatus::Resumed);
    }
    if dest_matches && source_matches {
        let (_, _, fp, _) = source.expect("source matches");
        return match write::remove_staged(root, &op.source, fp)? {
            RemoveOutcome::Removed => Ok(store::AssetOperationStatus::Resumed),
            RemoveOutcome::StaleBase { .. } => Err(stale(format!(
                "{} changed while removing a partial asset copy",
                op.source
            ))),
        };
    }
    if dest.is_some() {
        return Err(stale(format!(
            "{} already exists and does not carry the transformation bytes",
            op.target
        )));
    }
    let Some((_, _, fp, bytes)) = source else {
        return Err(stale(format!("{} is missing", op.source)));
    };
    if !source_matches {
        return Err(stale(format!(
            "{} no longer matches the journaled source bytes",
            op.source
        )));
    }
    match write::move_unchanged_staged(root, &op.source, &op.target, &bytes, fp)? {
        MoveOutcome::Moved { .. } => Ok(store::AssetOperationStatus::Applied),
        MoveOutcome::SourceStale { .. } => Err(stale(format!(
            "{} changed during the asset move",
            op.source
        ))),
        MoveOutcome::TargetExists { .. } => Err(stale(format!(
            "{} appeared during the asset move; nothing was overwritten",
            op.target
        ))),
    }
}

fn execute_markdown(
    root: &Path,
    op: &store::PendingAssetOperation,
    rows: &[AssetFilenameRewrite],
) -> Result<store::AssetOperationStatus, AdoptError> {
    let Some((hash, _, fp, current)) = file_state(root, &op.target)? else {
        return Err(stale(format!("{} is missing", op.target)));
    };
    if hash == op.target_sha256 {
        return Ok(store::AssetOperationStatus::Resumed);
    }
    if hash != op.source_sha256 {
        return Err(stale(format!(
            "{} changed after the transformation journal was written",
            op.target
        )));
    }
    let final_bytes = rewritten_bytes(current, rows);
    if hex(&final_bytes) != op.target_sha256 || final_bytes.len() as u64 != op.target_bytes {
        return Err(stale(format!(
            "{} no longer reproduces the journaled candidate bytes",
            op.target
        )));
    }
    match write::write_staged(root, &op.target, &final_bytes, Some(fp))? {
        WriteOutcome::Written { .. } => Ok(store::AssetOperationStatus::Applied),
        WriteOutcome::StaleBase { .. } => Err(stale(format!(
            "{} changed during the Markdown write; nothing was overwritten",
            op.target
        ))),
    }
}

fn execute_operations(
    root: &Path,
    input: &str,
    operations: &[store::PendingAssetOperation],
    meta: &Metadata,
) -> Result<(usize, usize, BTreeMap<String, store::AssetOperationStatus>), AdoptError> {
    let mut written = 0;
    let mut resumed = 0;
    let mut outcomes = BTreeMap::new();
    for op in operations {
        let status = match op.kind {
            store::AssetOperationKind::Asset => execute_asset(root, op)?,
            store::AssetOperationKind::Markdown => {
                let page = op
                    .target
                    .strip_prefix(&format!("{input}/"))
                    .ok_or_else(|| {
                        AdoptError::Run(format!(
                            "{} is outside the journaled content input",
                            op.target
                        ))
                    })?;
                let empty = Vec::new();
                execute_markdown(root, op, meta.rewrites_by_page.get(page).unwrap_or(&empty))?
            }
        };
        if status == store::AssetOperationStatus::Applied {
            written += 1;
        } else {
            resumed += 1;
        }
        outcomes.insert(op.target.clone(), status);
    }
    Ok((written, resumed, outcomes))
}

fn provenance(
    evidence: &Evidence,
    meta: &Metadata,
    input: &str,
    outcomes: &BTreeMap<String, store::AssetOperationStatus>,
) -> Vec<store::AssetOperationProvenance> {
    let mut out = Vec::new();
    for asset in &evidence.manifest.assets {
        let (kind, status, target) = if asset.action == "unchanged" {
            (
                store::AssetOperationKind::Asset,
                store::AssetOperationStatus::Unchanged,
                asset.dest_path.clone(),
            )
        } else if asset.action == "rejected" {
            (
                store::AssetOperationKind::Asset,
                store::AssetOperationStatus::Rejected,
                asset.dest_path.clone(),
            )
        } else if meta.collision_sources.contains(&asset.source_path) {
            (
                store::AssetOperationKind::Asset,
                store::AssetOperationStatus::SkippedCollision,
                asset.dest_path.clone(),
            )
        } else {
            let ws = format!("{input}/{}", asset.dest_path);
            (
                store::AssetOperationKind::Asset,
                outcomes
                    .get(&ws)
                    .copied()
                    .unwrap_or(store::AssetOperationStatus::Applied),
                asset.dest_path.clone(),
            )
        };
        out.push(store::AssetOperationProvenance {
            kind,
            status,
            source: asset.source_path.clone(),
            target,
            reason: asset.reason.clone(),
            sha256: asset.sha256.clone(),
            bytes: asset.bytes,
            rewrites: 0,
        });
    }
    for (page, rows) in &meta.rewrites_by_page {
        let skipped = meta.contaminated_pages.contains(page);
        let ws = format!("{input}/{page}");
        let status = if skipped {
            store::AssetOperationStatus::SkippedCollision
        } else {
            outcomes
                .get(&ws)
                .copied()
                .unwrap_or(store::AssetOperationStatus::Applied)
        };
        out.push(store::AssetOperationProvenance {
            kind: store::AssetOperationKind::Markdown,
            status,
            source: page.clone(),
            target: page.clone(),
            reason: if skipped {
                "asset destination collision; page left unchanged".into()
            } else {
                "image_or_link_dest".into()
            },
            sha256: None,
            bytes: 0,
            rewrites: rows.len() as u64,
        });
    }
    out
}

fn verify_run_input(
    src: &AdoptionSource,
    target_root: &Path,
    input: &str,
) -> Result<(), AdoptError> {
    let expected = target_root.join(input).canonicalize().map_err(|e| {
        AdoptError::Content(format!(
            "content input {input:?} cannot be canonicalized: {e}"
        ))
    })?;
    let matches = src
        .record
        .inputs
        .iter()
        .filter(|i| i.flag == "--root")
        .any(|i| {
            PathBuf::from(&i.path)
                .canonicalize()
                .is_ok_and(|p| p == expected)
        });
    if matches {
        Ok(())
    } else {
        Err(AdoptError::Run(format!(
            "run {} was not captured against this workspace's {input:?} content input",
            src.run_id
        )))
    }
}

/// Transform the workspace named by an asset-filename utility run in place.
/// This is deliberately separate from foreign-tree copy adoption: it adds no
/// page, invents no page id, and therefore cannot create duplicate explicit
/// Boris ids. Existing ids are left byte-for-byte inside their Markdown files.
pub fn transact_existing_with_identity(
    tx: AssetFilenameTransaction<'_>,
) -> Result<TxReport, AdoptError> {
    let input = content_input_authority(tx.content_input)?;
    content_input_target_authority(tx.target_root, input)?;
    if tx.src.mode != "asset-filename" {
        return Err(AdoptError::Run(format!(
            "run {} is mode {}, not asset-filename",
            tx.src.run_id, tx.src.mode
        )));
    }
    let mutation_lock = cv_store::workspace_mutation::lock(tx.target_root).map_err(|e| {
        AdoptError::Run(format!(
            "asset transformation blocked: the workspace mutation lock is unavailable or held by a concurrent writer ({e}); nothing was written"
        ))
    })?;
    revalidate_workspace_instance(
        tx.target_root,
        tx.expected_instance,
        IdentityCheckPhase::BeforeWrites,
        Some(&mutation_lock),
    )?;
    verify_run_input(tx.src, tx.target_root, input)?;
    let lock = super::adoption_lock(tx.target_root, &tx.src.run_id)?;
    let evidence = decode_evidence(tx.target_root, tx.src)?;
    let meta = metadata(&evidence);
    let candidate = validate_candidate(tx.target_root, tx.src, &evidence)?;
    let authority = operation_authority(input, tx.src, &evidence, &meta, &candidate)?;
    store::read_adoption_record(tx.target_root, &tx.src.run_id).map_err(AdoptError::Report)?;
    let pending =
        store::read_pending(tx.target_root, &tx.src.run_id).map_err(AdoptError::Report)?;

    let (operations, already_present) = match &pending {
        Some(journal) if journal.matches("existing", input, None) => {
            (validate_journal(journal, &authority)?, true)
        }
        Some(_) => {
            return Err(AdoptError::Run(format!(
                "an interrupted adoption of {} into this workspace has a different target shape; complete that retry first or remove {}",
                tx.src.run_id,
                store::PENDING
            )));
        }
        None => (Vec::new(), false),
    };
    let operations = if already_present {
        operations
    } else {
        if let Some(prior) = store::scan_adoptions(tx.target_root)?
            .into_iter()
            .find(|r| r.run_id == tx.src.run_id)
            && !prior.asset_operations.is_empty()
        {
            return Ok(TxReport {
                planned: 0,
                written: 0,
                resumed: 0,
                provenance_copied: 0,
                asset_moves: 0,
                markdown_writes: 0,
            });
        }
        let plan = fresh_plan(tx.target_root, input, tx.src, &evidence, &meta, &candidate)?;
        let mut journal = store::PendingAdoption::new(&tx.src.run_id, "existing", input);
        journal.asset_operations = Some(plan.operations.clone());
        store::write_pending(&lock, &journal)?;
        plan.operations
    };

    let copied = (|| {
        let (written, resumed, outcomes) =
            execute_operations(tx.target_root, input, &operations, &meta)?;
        let provenance_copied = copy_provenance(tx.src, tx.target_root, &lock)?;
        let rows = provenance(&evidence, &meta, input, &outcomes);
        record_adoption(RecordAdoptionInput {
            src: tx.src,
            target_root: tx.target_root,
            lock: &lock,
            at_ms: tx.at_ms,
            target_kind: "existing",
            subfolder_name: None,
            planned: &[],
            asset_operations: rows,
        })?;
        Ok::<_, AdoptError>((written, resumed, provenance_copied))
    })();
    let (written, resumed, provenance_copied) = match copied {
        Ok(done) => done,
        Err(e) => {
            revalidate_workspace_instance(
                tx.target_root,
                tx.expected_instance,
                IdentityCheckPhase::AfterContent,
                Some(&mutation_lock),
            )?;
            return Err(e);
        }
    };
    store::clear_pending(&lock)?;
    revalidate_workspace_instance(
        tx.target_root,
        tx.expected_instance,
        IdentityCheckPhase::AfterCommit,
        Some(&mutation_lock),
    )?;
    Ok(TxReport {
        planned: operations.len(),
        written,
        resumed,
        provenance_copied,
        asset_moves: operations
            .iter()
            .filter(|op| op.kind == store::AssetOperationKind::Asset)
            .count(),
        markdown_writes: operations
            .iter()
            .filter(|op| op.kind == store::AssetOperationKind::Markdown)
            .count(),
    })
}
