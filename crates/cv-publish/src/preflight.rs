//! Preflight vocabulary. The flow itself lives on [`crate::Publisher`]:
//! build the target profile into its **own** output dir (never the
//! loopback watch build), run validate + check + ruleset against those
//! exact bytes, then seal a `PreflightGrant`.

use crate::plan::PlanFile;
use crate::targets::TargetConfig;
use cv_core::engine::Verb;
use cv_core::id::StableId;
use cv_core::publish::PreflightGrant;
use cv_core::sys::Hash;
use std::path::Path;

/// Ruleset port (cv-rules implements it). Only engine-documented
/// findings may land in `blocking`; heuristics are advisory by contract.
pub trait Ruleset {
    /// Identity of the ruleset configuration (`ruleset_hash`).
    fn hash(&self) -> Hash;
    fn evaluate(&self, site: &Path, files: &[PlanFile]) -> RulesetVerdict;
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
pub fn run_dir(run: &StableId) -> String {
    format!(".cv/publish/{}", run.0)
}

pub fn site_dir(run: &StableId) -> String {
    format!("{}/site", run_dir(run))
}

pub fn graph_file(run: &StableId) -> String {
    format!("{}/graph.mmd", run_dir(run))
}

/// The engine verbs preflight issues, in order. Argv is mapped by
/// cv-engine; nothing here carries a secret.
pub fn verbs(cfg: &TargetConfig, run: &StableId) -> Vec<(&'static str, Verb)> {
    let c = |name: &str, args: &[&str]| Verb::Custom {
        name: name.into(),
        args: args.iter().map(|s| s.to_string()).collect(),
    };
    let site = site_dir(run);
    let graph = graph_file(run);
    vec![
        (
            "build",
            c("build", &["--profile", &cfg.profile, "--html-dir", &site]),
        ),
        ("validate", c("validate", &["--profile", &cfg.profile])),
        ("check", c("check", &["--html-dir", &site])),
        ("graph", c("graph", &["--out", &graph])),
    ]
}
