//! cv-rules — SEO/AEO rules engine (L4 lane).
//!
//! [`eval`]`(&ArtifactSet, &Ruleset) -> Report` is pure and deterministic:
//! no filesystem, no network, no clock, no randomness, no hash-map order.
//! Callers read artifact bytes and build an [`ArtifactSet`]; rule packs are
//! TOML (`rulepacks/*.toml`, compiled in for v1). Inputs that are not
//! available produce `CheckStatus::Limited`, never a silent pass, and only
//! `NormativeSpec`/`EngineDocumented` `Error` failures can block a publish.
//! See `spec/lanes/L4-audit-seo-aeo.md`.

pub mod artifacts;
pub mod boris_check;
mod eval;
pub mod html;
pub mod jsonpath;
pub mod report;
pub mod ruleset;

pub use artifacts::{
    ArtifactSet, ArtifactSetBuilder, Availability, Export, ExportEntry, HtmlPage, IrGraph, IrNode,
};
pub use eval::eval;
pub use report::{AuditFinding, Report, Verdict};
pub use ruleset::{PackMeta, RulePack, Ruleset, V1_PACKS};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("rule pack: {0}")]
    Pack(String),
    #[error("selector: {0}")]
    Selector(String),
    #[error("parse {artifact}: {msg}")]
    Parse { artifact: String, msg: String },
}

pub fn sha256(bytes: &[u8]) -> cv_core::sys::Hash {
    use sha2::{Digest, Sha256};
    cv_core::sys::Hash(Sha256::digest(bytes).into())
}
