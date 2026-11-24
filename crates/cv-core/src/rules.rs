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
}
