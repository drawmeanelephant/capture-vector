//! Preflight vocabulary. The flow itself lives on [`crate::Publisher`]:
//! build the target profile into its **own** output dir (never the
//! loopback watch build), run validate + check + ruleset against those
//! exact bytes, then seal a `PreflightGrant`.

use crate::plan::PlanFile;
use crate::targets::TargetConfig;
use crate::{PublishError, Result};
use cv_core::engine::Verb;
use cv_core::id::StableId;
use cv_core::publish::PreflightGrant;
use cv_core::sys::Hash;
use cv_store::paths::portable_relative;
use std::path::Path;

/// Ruleset port (cv-rules implements it). Only engine-documented
/// findings may land in `blocking`; heuristics are advisory by contract.
pub trait Ruleset {
    /// Identity of the ruleset configuration (`ruleset_hash`).
    fn hash(&self) -> Hash;
    fn evaluate(&self, artifacts: &PreflightArtifacts<'_>) -> RulesetVerdict;
}

/// Everything publication rules may evaluate, all produced by the same
/// preflight run and sealed before delivery.
pub struct PreflightArtifacts<'a> {
    pub site: &'a Path,
    pub files: &'a [PlanFile],
    /// Boris's own IR `graph.json` projection for this preflight, when the
    /// engine produced it. Entity identity comes from here, never from
    /// string-repairing output routes.
    pub ir_graph: Option<&'a Path>,
    pub config: &'a TargetConfig,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RulesetVerdict {
    pub blocking: Vec<String>,
    pub advisory: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct PreflightRequest {
    pub config: TargetConfig,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PreflightOutcome {
    Granted {
        run: StableId,
        grant: PreflightGrant,
    },
    Blocked {
        run: StableId,
        blocks: Vec<String>,
    },
}

impl PreflightOutcome {
    pub fn run(&self) -> &StableId {
        match self {
            Self::Granted { run, .. } | Self::Blocked { run, .. } => run,
        }
    }
}

/// Project-relative workspace for one run.
pub fn run_dir(run: &StableId) -> Result<String> {
    if run.0.contains('/') || portable_relative(&run.0).is_err() {
        return Err(PublishError::Refused("invalid publish run ID".into()));
    }
    Ok(format!(".cv/publish/{}", run.0))
}

pub fn site_dir(run: &StableId) -> Result<String> {
    Ok(format!("{}/site", run_dir(run)?))
}

pub fn ir_dir(run: &StableId) -> Result<String> {
    Ok(format!("{}/ir", run_dir(run)?))
}

pub fn graph_file(run: &StableId) -> Result<String> {
    Ok(format!("{}/graph.json", ir_dir(run)?))
}

/// The engine verbs preflight issues, in order. Argv is mapped by
/// cv-engine; nothing here carries a secret.
///
/// Static-dist is explicitly Boris's synthetic `default` HTML target:
/// `build --html-dir SITE` and `validate --html-dir SITE` name the same
/// target configuration, so validation, proof artifacts and the seal all
/// describe the bytes actually built. Named publication profiles cannot be
/// redirected into a private output directory by Boris 0.9.0; refusing them
/// is honest where pretending otherwise would seal a different proof target.
pub fn verbs(cfg: &TargetConfig, run: &StableId) -> Result<Vec<(&'static str, Verb)>> {
    if cfg.profile.is_some() {
        return Err(PublishError::Refused(
            "publication-profile preflight cannot redirect Boris 0.9.0 into a private run dir"
                .into(),
        ));
    }
    let c = |name: &str, args: &[&str]| Verb::Custom {
        name: name.into(),
        args: args.iter().map(|s| s.to_string()).collect(),
    };
    let site = site_dir(run)?;
    let ir = ir_dir(run)?;
    Ok(vec![
        ("build", c("build", &["--html-dir", &site])),
        ("validate", c("validate", &["--html-dir", &site])),
        ("check", c("check", &[])),
        ("ir", c("build", &["--out", &ir])),
    ])
}
