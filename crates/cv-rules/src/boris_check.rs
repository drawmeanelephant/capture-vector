//! Boris `check --format json` findings as Audit findings. Boris owns
//! what each code means; this only carries its report into the Findings
//! mailbox under the `EngineDocumented` tier. `check` exits 0 with these
//! findings, so they are recorded as `Info`: Boris does not call them
//! failures, and Capture Vector does not upgrade them into blockers.

use crate::report::AuditFinding;
use crate::sha256;
use cv_core::id::{AccountId, NounId, NounKind, StableId, noun_kind};
use cv_core::rules::{
    AuthorityTier, CheckStatus, DedupKey, Evidence, Finding, FindingLifecycle, Severity,
};
use serde::Deserialize;

/// Pack name the findings are filed under.
pub const PACK: &str = "boris-check";

#[derive(Deserialize)]
struct CheckReport {
    #[serde(default)]
    findings: Vec<CheckFinding>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CheckFinding {
    code: String,
    #[serde(rename = "type")]
    kind: String,
    value: String,
    #[serde(default)]
    count: Option<u64>,
    #[serde(default)]
    source_path: Option<String>,
    #[serde(default)]
    line: Option<u64>,
    #[serde(default)]
    column: Option<u64>,
}

/// Parse one `check --format json` report; `Err` when it is not one.
pub fn check_findings(json: &str) -> Result<Vec<AuditFinding>, String> {
    let report: CheckReport =
        serde_json::from_str(json).map_err(|e| format!("boris check report: {e}"))?;
    Ok(report.findings.into_iter().map(audit_finding).collect())
}

fn audit_finding(f: CheckFinding) -> AuditFinding {
    let subject = NounId {
        account: AccountId("local".into()),
        kind: NounKind::register(if f.kind == "page" {
            noun_kind::PAGE
        } else {
            noun_kind::ARTIFACT
        }),
        stable: StableId(if f.kind == "page" {
            f.value.clone()
        } else {
            format!("{}:{}", f.kind, f.value)
        }),
    };
    let location = f.source_path.as_ref().map(|p| match (f.line, f.column) {
        (Some(l), Some(c)) => format!("{p}:{l}:{c}"),
        (Some(l), None) => format!("{p}:{l}"),
        _ => p.clone(),
    });
    let target = format!("{}:{}", f.kind, f.value);
    let summary = match &location {
        Some(at) => format!("Boris check reports {} on {} ({at})", f.code, f.value),
        None => format!("Boris check reports {} on {} {}", f.code, f.kind, f.value),
    };
    let detail = serde_json::json!({
        "type": f.kind,
        "value": f.value,
        "count": f.count,
        "location": location,
        "source": "boris check --format json",
    });
    AuditFinding {
        pack: PACK.into(),
        finding: Finding {
            code: f.code.clone(),
            tier: AuthorityTier::EngineDocumented,
            severity: Severity::Info,
            status: CheckStatus::Failed,
            key: DedupKey {
                code: f.code,
                subject: subject.clone(),
                target_hash: sha256(target.as_bytes()),
            },
            subject,
            evidence: Evidence {
                summary,
                detail: Some(detail),
            },
            fix: None,
            lifecycle: FindingLifecycle::Open,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn captured() -> String {
        let p = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/captures/verbs/happy/check/stderr.txt"
        );
        std::fs::read_to_string(p).unwrap()
    }

    #[test]
    fn captured_check_findings_become_engine_documented_advisories() {
        let found = check_findings(&captured()).unwrap();
        let codes: Vec<_> = found.iter().map(|f| f.finding.code.as_str()).collect();
        assert_eq!(codes, ["unreferenced_page", "zero_includes"]);
        for f in &found {
            assert_eq!(f.pack, PACK);
            assert_eq!(f.finding.tier, AuthorityTier::EngineDocumented);
            assert!(!f.blocks_publish());
        }
        assert_eq!(found[0].finding.subject.stable.0, "index");
        assert!(found[0].finding.evidence.summary.contains("index.md:1:1"));
    }

    #[test]
    fn a_report_that_is_not_check_json_is_refused() {
        assert!(check_findings("error: ETEXTILE").is_err());
    }
}
