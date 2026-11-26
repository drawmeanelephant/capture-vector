//! Proof-evidence evaluation for preflight: `checks.json` is consumed as
//! evidence binding and display detail only. The deploy verdict itself is
//! Boris's own `proof verify` exit evidence — never reimplemented here.

use std::path::Path;

use crate::targets::DEFAULT_HTML_TARGET;
use cv_ledger::ingest;

/// Best-effort display detail from Boris's own findings when the verify
/// verdict blocks: parsing here never decides the verdict, it only surfaces
/// what Boris already recorded.
pub(crate) fn proof_finding_detail(site: &Path) -> String {
    let Ok(bytes) = std::fs::read(site.join("_boris/proof/checks.json")) else {
        return String::new();
    };
    let Ok(file) = serde_json::from_slice::<ingest::ChecksFile>(&bytes) else {
        return String::new();
    };
    // G7: findings attribute per check through finding_offset + count ranges,
    // never globally, and severity is quoted only for what Boris recorded.
    let mut details: Vec<String> = Vec::new();
    for check in &file.checks {
        let offset = check.finding_offset.unwrap_or(0) as usize;
        let count = check.counts.as_ref().map(|c| c.findings).unwrap_or(0) as usize;
        for f in file.findings.iter().skip(offset).take(count) {
            if f.get("severity").and_then(|v| v.as_str()) != Some("error") {
                continue;
            }
            let code = f.get("code").and_then(|v| v.as_str()).unwrap_or("unknown");
            let page = f
                .get("subject")
                .and_then(|s| s.get("id"))
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let output = f
                .get("output_location")
                .and_then(|o| o.get("path"))
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            details.push(format!("{}: {code} on {page} at {output}", check.id));
        }
    }
    let failed: Vec<&str> = file
        .checks
        .iter()
        .filter(|c| c.ran && matches!(c.status.as_str(), "failed" | "incomplete"))
        .map(|c| c.id.as_str())
        .collect();
    let mut out = String::new();
    if !failed.is_empty() {
        out.push_str(&format!(" — {}", failed.join(", ")));
    }
    if !details.is_empty() {
        out.push_str(&format!(" ({})", details.join("; ")));
    }
    out
}

/// Evidence binding, not verdict: the artifact-inventory descriptor inside
/// checks.json must match the exact artifacts.json bytes — path, byte
/// length, lowercase sha256, artifact count, format/schema and target — so
/// proof evidence and the sealed inventory cannot diverge (G4). Shape
/// strictness mirrors what Boris's strict parser refuses: exactly the three
/// ordered check ids, contiguous finding offsets, closed vocabularies.
pub(crate) fn evaluate_proof_checks(
    site: &Path,
    artifacts: &[u8],
    artifact_count: usize,
    artifacts_sha: &str,
    blocks: &mut Vec<String>,
) {
    let path = site.join("_boris/proof/checks.json");
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            blocks.push(format!("proof checks: checks.json unreadable: {e}"));
            return;
        }
    };
    let file: ingest::ChecksFile = match serde_json::from_slice(&bytes) {
        Ok(f) => f,
        Err(e) => {
            blocks.push(format!("proof checks: checks.json: {e}"));
            return;
        }
    };
    if file.format != "boris-publication-checks" {
        blocks.push(format!(
            "proof checks: format {:?} is not boris-publication-checks",
            file.format
        ));
    }
    if file.schema_version != 1 {
        blocks.push(format!(
            "proof checks: schema_version {} is not 1",
            file.schema_version
        ));
    }
    if file.target != DEFAULT_HTML_TARGET {
        blocks.push(format!(
            "proof checks: target {} is not {DEFAULT_HTML_TARGET}",
            file.target
        ));
    }
    // Exactly the three ordered check ids Boris commits.
    let ids: Vec<&str> = file.checks.iter().map(|c| c.id.as_str()).collect();
    if ids != ["artifact-integrity", "rendered-html", "rendered-search"] {
        blocks.push(format!(
            "proof checks: check ids {ids:?} are not the three ordered Boris checks"
        ));
    }
    for check in &file.checks {
        if !matches!(
            check.status.as_str(),
            "passed" | "failed" | "incomplete" | "not-applicable"
        ) {
            blocks.push(format!(
                "proof checks: {} has unknown status {:?} — deploy evidence untrusted",
                check.id, check.status
            ));
        }
        if let Some(coverage) = &check.coverage
            && !matches!(
                coverage.as_str(),
                "complete" | "incomplete" | "not-applicable"
            )
        {
            blocks.push(format!(
                "proof checks: {} has unknown coverage {:?} — deploy evidence untrusted",
                check.id, coverage
            ));
        }
        if !check.eligible && check.ran {
            blocks.push(format!(
                "proof checks: {} is not eligible but ran — deploy evidence inconsistent",
                check.id
            ));
        }
    }
    for check in file.checks.iter().filter(|c| c.eligible && !c.ran) {
        blocks.push(format!(
            "proof checks: {} was eligible but did not run — deploy evidence incomplete",
            check.id
        ));
    }
    // Finding offsets must be contiguous disjoint ranges over the findings.
    let mut cursor = 0u64;
    for check in &file.checks {
        let offset = check.finding_offset.unwrap_or(0);
        let count = check.counts.as_ref().map(|c| c.findings).unwrap_or(0);
        if offset != cursor {
            blocks.push(format!(
                "proof checks: {} finding_offset {offset} is not contiguous at {cursor}",
                check.id
            ));
            break;
        }
        cursor += count;
    }
    if cursor != file.findings.len() as u64 {
        blocks.push(format!(
            "proof checks: finding ranges cover {cursor} of {} findings",
            file.findings.len()
        ));
    }
    for finding in &file.findings {
        if !matches!(
            finding.get("severity").and_then(|v| v.as_str()),
            Some("error" | "warning" | "info")
        ) {
            blocks.push("proof checks: finding with unknown severity — evidence untrusted".into());
        }
    }
    // G4: the binding descriptor must match the exact artifacts.json bytes.
    match &file.artifact_inventory {
        None => blocks.push(
            "proof checks: artifact-inventory descriptor missing — evidence binding unverifiable"
                .to_string(),
        ),
        Some(inv) => {
            if inv.path != "_boris/proof/artifacts.json" {
                blocks.push(format!(
                    "proof checks: artifact-inventory path {:?} is not _boris/proof/artifacts.json",
                    inv.path
                ));
            }
            if inv.bytes != artifacts.len() as u64 {
                blocks.push(format!(
                    "proof checks: artifact-inventory bytes {} does not match the sealed artifacts.json",
                    inv.bytes
                ));
            }
            if inv.sha256 != artifacts_sha {
                blocks.push(format!(
                    "proof checks: artifact-inventory digest {} does not match the sealed artifacts.json",
                    inv.sha256
                ));
            }
            if inv.format.as_deref() != Some("boris-publication-artifacts") {
                blocks.push(
                    "proof checks: artifact-inventory format is not boris-publication-artifacts"
                        .into(),
                );
            }
            if inv.schema_version != Some(1) {
                blocks.push("proof checks: artifact-inventory schema_version is not 1".into());
            }
            if inv.target.as_deref() != Some(DEFAULT_HTML_TARGET) {
                blocks.push(format!(
                    "proof checks: artifact-inventory target is not {DEFAULT_HTML_TARGET}"
                ));
            }
            let count = inv.extra.get("artifact_count").and_then(|v| v.as_u64());
            if count != Some(artifact_count as u64) {
                blocks.push(format!(
                    "proof checks: artifact-inventory count {count:?} does not match {artifact_count} sealed artifacts"
                ));
            }
        }
    }
}
