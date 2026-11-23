//! Report + verdict. Blocking is decided here, at verdict time, from the
//! finding's authority tier — never from the rule pack's say-so.

use cv_core::rules::{CheckStatus, Finding, Report as CoreReport, Severity};
use cv_core::sys::Hash;
use serde::{Deserialize, Serialize};

/// A `cv_core` finding plus the producing pack and rule severity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditFinding {
    pub pack: String,
    pub severity: Severity,
    pub finding: Finding,
}

impl AuditFinding {
    /// Only a *failed* `Error` from a tier that may block can block a
    /// publish. `Heuristic` can never, whatever its severity or status.
    pub fn blocks_publish(&self) -> bool {
        self.finding.tier.may_block()
            && self.finding.status == CheckStatus::Failed
            && matches!(self.severity, Severity::Error)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub artifact_set_hash: Hash,
    pub ruleset_hash: Hash,
    /// Failed and Limited findings, stable-sorted (pack, code, subject, target).
    pub findings: Vec<AuditFinding>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct Verdict {
    /// Failed findings that block a publish.
    pub blocking: usize,
    /// Failed findings that do not block (warnings, info, any Heuristic).
    pub advisory: usize,
    /// Checks that could not run on the inputs given.
    pub limited: usize,
}

impl Verdict {
    pub fn blocks_publish(&self) -> bool {
        self.blocking > 0
    }
}

impl Report {
    pub fn blocks_publish(&self) -> bool {
        self.findings.iter().any(AuditFinding::blocks_publish)
    }

    pub fn verdict(&self) -> Verdict {
        let mut v = Verdict::default();
        for f in &self.findings {
            match f.finding.status {
                CheckStatus::Limited => v.limited += 1,
                CheckStatus::Failed if f.blocks_publish() => v.blocking += 1,
                CheckStatus::Failed => v.advisory += 1,
                CheckStatus::Checked | CheckStatus::Passed => {}
            }
        }
        v
    }

    pub fn failed(&self) -> impl Iterator<Item = &AuditFinding> {
        self.findings
            .iter()
            .filter(|f| f.finding.status == CheckStatus::Failed)
    }

    pub fn limited(&self) -> impl Iterator<Item = &AuditFinding> {
        self.findings
            .iter()
            .filter(|f| f.finding.status == CheckStatus::Limited)
    }

    /// Shared-contract view for events/store. `cv_core::Finding` carries no
    /// severity yet, so `CoreReport::has_blocking_errors` over-approximates
    /// for non-Error findings; its tier gate still keeps every Heuristic
    /// out. Preflight should ask [`Report::blocks_publish`].
    pub fn to_core(&self) -> CoreReport {
        CoreReport {
            artifact_set_hash: self.artifact_set_hash,
            ruleset_hash: self.ruleset_hash,
            findings: self.findings.iter().map(|f| f.finding.clone()).collect(),
        }
    }

    /// Canonical serialization; byte-identical for identical inputs.
    pub fn to_canonical_json(&self) -> String {
        serde_json::to_string(self).expect("report serializes")
    }
}
