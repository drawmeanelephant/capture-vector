//! Audit vocabulary — `cv-rules` evaluates `eval(&ArtifactSet, &Ruleset)
//! -> Report` as a pure function. Every rule cites its source; folklore
//! (`Heuristic`) can never block a publish.

use crate::id::NounId;
use crate::sys::Hash;
use serde::{Deserialize, Serialize};

/// Content-hashed manifest of everything a ruleset may inspect.
/// Canonical/robots/JSON-LD checks need **final HTML**, not IR.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactSet {
    pub ir: Option<IrRef>,
    pub html: Option<HtmlRef>,
    pub exports: Vec<ExportRef>,
    pub deploy: Option<TargetDeployProfile>,
    /// Explicit unavailable inputs — checks needing them report
    /// `limited`, never a silent pass.
    pub missing: Vec<ArtifactKind>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArtifactKind {
    Ir,
    Html,
    Sitemap,
    Rss,
    Llms,
    Rag,
    ContextBundle,
    RobotsTxt,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactRef {
    pub path: String,
    pub sha256: Hash,
}

pub type IrRef = ArtifactRef;
pub type HtmlRef = ArtifactRef;
pub type ExportRef = ArtifactRef;

/// Where this build is destined — canonical and parity checks compare
/// against it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetDeployProfile {
    pub base_url: String,
    pub trailing_slash: SlashPolicy,
    pub expects_robots_txt: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SlashPolicy {
    Always,
    Never,
    Either,
}

/// Who says so. Blocking authority ends at `EngineDocumented` —
/// `Heuristic` findings are advisory forever.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum AuthorityTier {
    NormativeSpec,
    EngineDocumented,
    Heuristic,
}

impl AuthorityTier {
    /// The only tiers allowed to produce a blocking verdict.
    pub fn may_block(self) -> bool {
        matches!(self, Self::NormativeSpec | Self::EngineDocumented)
    }
}

/// Content archetype the rule scopes to — keeps narrative pages out of
/// answer-first checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Archetype {
    Trunk,
    Satellite,
    Faq,
    Article,
    Changelog,
    Any,
}

pub type AuditCode = String;

/// Honesty vocabulary: anything requiring live HTTP headers, redirects,
/// or a real browser reports `limited` — never `passed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheckStatus {
    Checked,
    Passed,
    Failed,
    Limited,
}

/// Closed versioned algebra — typed selectors + thresholds. v1 ops.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CheckSpec {
    /// Count nodes selected from an artifact; compare to a threshold.
    Count {
        artifact: ArtifactKind,
        selector: Selector,
        op: CompareOp,
        threshold: u64,
    },
    /// The same field must agree across artifacts (canonical ≡ sitemap ≡
    /// RSS ≡ llms.txt ≡ internal hrefs ≡ deploy profile).
    Parity { field: ParityField },
    /// Selected value exists (e.g. JSON-LD present on article pages).
    Exists {
        artifact: ArtifactKind,
        selector: Selector,
    },
    /// Draft/noindex content must not appear in machine exports.
    NotPresent {
        artifact: ArtifactKind,
        selector: Selector,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Selector {
    /// JSONPath-ish path into a JSON artifact — intentionally a small
    /// closed subset, interpreted by cv-rules.
    JsonPath(String),
    /// CSS-ish selector over final HTML DOM.
    Html(String),
    /// IR node selection by node kind + optional field.
    IrNode { kind: String, field: Option<String> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompareOp {
    Lt,
    Le,
    Eq,
    Ge,
    Gt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ParityField {
    CanonicalUrl,
    PageSet,
    LastModified,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Citation {
    pub source: String,
    pub url: String,
    pub frozen_quote: String,
    pub captured: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rule {
    pub id: AuditCode,
    pub severity: Severity,
    pub tier: AuthorityTier,
    pub archetypes: Vec<Archetype>,
    pub check: CheckSpec,
    pub remediation: String,
    pub citation: Citation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

impl Severity {
    fn legacy_finding_default() -> Self {
        Self::Error
    }
}

/// `(code, subject, hash(target))` — N broken links on one page stay N
/// findings; the same finding on an unchanged target dedups across runs.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DedupKey {
    pub code: AuditCode,
    pub subject: NounId,
    pub target_hash: Hash,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FindingLifecycle {
    Open,
    Fixed,
    Ignored,
    Wontfix,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Evidence {
    /// What was checked and what was found — human-readable, citable.
    pub summary: String,
    pub detail: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FixIt {
    pub description: String,
    /// Optional machine-applicable edit; advisory at v1.
    pub replacement: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    pub code: AuditCode,
    pub tier: AuthorityTier,
    /// Copied from the producing `Rule`. Payloads serialized before
    /// findings carried severity load as `Error`, which is what
    /// `severity()` reported for every finding back then.
    #[serde(default = "Severity::legacy_finding_default")]
    pub severity: Severity,
    pub status: CheckStatus,
    pub subject: NounId,
    pub key: DedupKey,
    pub evidence: Evidence,
    pub fix: Option<FixIt>,
    pub lifecycle: FindingLifecycle,
}

/// Deterministic output — byte-identical for identical inputs; carries
/// the hashes it was computed against.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub artifact_set_hash: Hash,
    pub ruleset_hash: Hash,
    pub findings: Vec<Finding>,
}

impl Report {
    /// True when any blocking-tier error is present. Heuristic findings
    /// cannot produce this by construction (see `AuthorityTier::may_block`).
    pub fn has_blocking_errors(&self) -> bool {
        self.findings.iter().any(Finding::blocks_publish)
    }
}

impl Finding {
    pub fn severity(&self) -> Severity {
        self.severity
    }

    pub fn blocks_publish(&self) -> bool {
        self.tier.may_block()
            && self.status == CheckStatus::Failed
            && self.severity == Severity::Error
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heuristics_cannot_block() {
        assert!(!AuthorityTier::Heuristic.may_block());
        assert!(AuthorityTier::NormativeSpec.may_block());
        assert!(AuthorityTier::EngineDocumented.may_block());
    }

    fn failed(tier: AuthorityTier, severity: Severity) -> Finding {
        let subject = NounId {
            account: crate::id::AccountId("acct".into()),
            kind: crate::id::NounKind::new("page".into()),
            stable: crate::id::StableId("p".into()),
        };
        Finding {
            code: "X".into(),
            tier,
            severity,
            status: CheckStatus::Failed,
            key: DedupKey {
                code: "X".into(),
                subject: subject.clone(),
                target_hash: Hash([0; 32]),
            },
            subject,
            evidence: Evidence {
                summary: "failed".into(),
                detail: None,
            },
            fix: None,
            lifecycle: FindingLifecycle::Open,
        }
    }

    fn report(findings: Vec<Finding>) -> Report {
        Report {
            artifact_set_hash: Hash([1; 32]),
            ruleset_hash: Hash([2; 32]),
            findings,
        }
    }

    #[test]
    fn failed_warning_or_info_at_a_blocking_tier_does_not_block() {
        for tier in [
            AuthorityTier::NormativeSpec,
            AuthorityTier::EngineDocumented,
        ] {
            for severity in [Severity::Warning, Severity::Info] {
                let f = failed(tier, severity);
                assert_eq!(f.severity(), severity);
                assert!(!f.blocks_publish(), "{tier:?}/{severity:?}");
                assert!(
                    !report(vec![f]).has_blocking_errors(),
                    "{tier:?}/{severity:?}"
                );
            }
        }
    }

    #[test]
    fn failed_error_at_a_blocking_tier_blocks() {
        for tier in [
            AuthorityTier::NormativeSpec,
            AuthorityTier::EngineDocumented,
        ] {
            let r = report(vec![
                failed(tier, Severity::Warning),
                failed(tier, Severity::Error),
            ]);
            assert!(r.has_blocking_errors(), "{tier:?}");
        }
    }

    #[test]
    fn failed_heuristic_error_never_blocks() {
        let f = failed(AuthorityTier::Heuristic, Severity::Error);
        assert_eq!(f.severity(), Severity::Error);
        assert!(!f.blocks_publish());
        assert!(!report(vec![f.clone(), f]).has_blocking_errors());
    }

    #[test]
    fn non_failed_error_at_a_blocking_tier_does_not_block() {
        for status in [
            CheckStatus::Checked,
            CheckStatus::Passed,
            CheckStatus::Limited,
        ] {
            let mut f = failed(AuthorityTier::NormativeSpec, Severity::Error);
            f.status = status;
            assert!(!report(vec![f]).has_blocking_errors(), "{status:?}");
        }
    }

    #[test]
    fn finding_serialized_without_severity_loads_as_error() {
        let mut legacy = serde_json::to_value(failed(AuthorityTier::Heuristic, Severity::Info))
            .expect("finding serializes");
        legacy
            .as_object_mut()
            .expect("finding is a JSON object")
            .remove("severity");
        let f: Finding = serde_json::from_value(legacy).expect("legacy finding loads");
        assert_eq!(f.severity(), Severity::Error);
        assert!(!f.blocks_publish());

        let mut legacy = serde_json::to_value(report(vec![failed(
            AuthorityTier::EngineDocumented,
            Severity::Info,
        )]))
        .expect("report serializes");
        legacy["findings"][0]
            .as_object_mut()
            .expect("finding is a JSON object")
            .remove("severity");
        let r: Report = serde_json::from_value(legacy).expect("legacy report loads");
        assert_eq!(r.findings[0].severity(), Severity::Error);
        assert!(r.has_blocking_errors());
    }

    #[test]
    fn severity_round_trips_through_serialization() {
        for severity in [Severity::Error, Severity::Warning, Severity::Info] {
            let f = failed(AuthorityTier::EngineDocumented, severity);
            let back: Finding =
                serde_json::from_str(&serde_json::to_string(&f).expect("serializes"))
                    .expect("deserializes");
            assert_eq!(back, f);
        }
    }
}
