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
    let details: Vec<String> = file
        .findings
        .iter()
        .filter(|f| f.get("severity").and_then(|v| v.as_str()) == Some("error"))
        .map(|f| {
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
            format!("{code} on {page} at {output}")
        })
        .collect();
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

/// inventory digest inside checks.json must match the sealed artifacts.json
/// bytes so proof evidence and the sealed inventory cannot diverge.
pub(crate) fn evaluate_proof_checks(site: &Path, artifacts_sha: &str, blocks: &mut Vec<String>) {
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
    if file.target != DEFAULT_HTML_TARGET {
        blocks.push(format!(
            "proof checks: target {} is not {DEFAULT_HTML_TARGET}",
            file.target
        ));
    }
    match &file.artifact_inventory {
        None => blocks.push(
            "proof checks: artifact-inventory descriptor missing — evidence binding unverifiable"
                .to_string(),
        ),
        Some(inv) if inv.sha256 != artifacts_sha => {
            blocks.push(format!(
                "proof checks: artifact-inventory digest {} does not match the sealed artifacts.json",
                inv.sha256
            ));
        }
        _ => {}
    }
    for check in file.checks.iter().filter(|c| c.eligible && !c.ran) {
        blocks.push(format!(
            "proof checks: {} was eligible but did not run — deploy evidence incomplete",
            check.id
        ));
    }
}
