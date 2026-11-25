//! The publish verbs over the workspace ledger. Each verb opens the
//! `cv_publish` publisher for its own duration, so the ledger's writer
//! lock is held only while a verb runs, and drops it before returning.
//! Copy-to-folder is the only delivery wired; every other target stays
//! unconfigured until #185 decides the v1 adapters.

use cv_core::engine::Engine;
use cv_core::id::StableId;
use cv_core::publish::{RunState, RunStatus};
use cv_core::rules::{AuthorityTier, CheckStatus};
use cv_core::sys::{Clock, Hash, IdGen};
use cv_model::modules::publish::{Done, Observed};
use cv_publish::adapters::{COPY_TO_FOLDER, CopyToFolder};
use cv_publish::plan::PlanFile;
use cv_publish::secrets::NoSecrets;
use cv_publish::{
    Observation, PreflightOutcome, PreflightRequest, PublishError, Publisher, Ruleset,
    RulesetVerdict, Target, TargetConfig,
};
use std::path::Path;

/// Who the ledger records as approving, discarding and reconciling.
pub const ACTOR: &str = "local";
/// Project-relative folder a static-dist run is copied into.
pub const STATIC_DIST_FOLDER: &str = "publish/static-dist";
/// Boris profile the preflight build validates.
pub const PROFILE: &str = "boris.json";
/// A folder copy has no host; the deploy profile still needs a base.
const LOCAL_BASE_URL: &str = "http://localhost/";

/// The configuration a new preflight for `target` seals, if its delivery
/// is wired.
pub fn delivery_config(target: Target) -> Option<TargetConfig> {
    match target {
        Target::StaticDist => Some(TargetConfig::new(
            target,
            PROFILE,
            LOCAL_BASE_URL,
            COPY_TO_FOLDER,
            STATIC_DIST_FOLDER,
        )),
        Target::GithubPages | Target::StandardSite | Target::NostrNip23 => None,
    }
}

/// The shipped Audit packs over the sealed HTML. Blocking findings are
/// labelled with their Audit row key so the run can reveal them there;
/// the tier alone decides which side a finding lands on.
pub struct AuditRules(cv_rules::Ruleset);

impl AuditRules {
    pub fn shipped() -> Self {
        Self(cv_model::modules::audit::default_ruleset())
    }
}

impl Ruleset for AuditRules {
    fn hash(&self) -> Hash {
        self.0.hash()
    }
    fn evaluate(&self, site: &Path, files: &[PlanFile]) -> RulesetVerdict {
        let mut set = cv_rules::ArtifactSet::builder();
        let mut unreadable = Vec::new();
        for f in files.iter().filter(|f| f.path.ends_with(".html")) {
            match std::fs::read(site.join(&f.path))
                .map_err(|e| e.to_string())
                .and_then(|b| {
                    cv_rules::HtmlPage::parse(None, &f.path, &b).map_err(|e| e.to_string())
                }) {
                Ok(p) => set = set.page(p),
                Err(e) => unreadable.push(format!("{}: {e}", f.path)),
            }
        }
        let report = cv_rules::eval(&set.build(), &self.0);
        let mut v = RulesetVerdict::default();
        for af in report
            .findings
            .iter()
            .filter(|f| f.finding.status == CheckStatus::Failed)
        {
            if af.blocks_publish() {
                v.blocking
                    .push(cv_model::modules::audit::key_id(&af.finding.key));
            } else {
                v.advisory.push(format!(
                    "{} on {} · {}",
                    af.finding.code,
                    af.finding.subject.stable.0,
                    tier(af.finding.tier)
                ));
            }
        }
        v.advisory
            .extend(unreadable.into_iter().map(|e| format!("not audited: {e}")));
        v
    }
}

fn tier(t: AuthorityTier) -> &'static str {
    match t {
        AuthorityTier::NormativeSpec => "normative spec",
        AuthorityTier::EngineDocumented => "engine documented",
        AuthorityTier::Heuristic => "heuristic",
    }
}

/// What the writable port publishes with.
pub struct Writer {
    pub engine: Box<dyn Engine>,
    pub rules: Box<dyn Ruleset>,
    pub clock: Box<dyn Clock>,
    pub ids: Box<dyn IdGen>,
}

fn why(e: PublishError) -> String {
    match e {
        PublishError::Refused(s) => s,
        e => e.to_string(),
    }
}

fn status_label(s: &RunState) -> &'static str {
    match s {
        RunState::Dispatching => "dispatching",
        RunState::Submitted => "submitted",
        RunState::Unknown => "unknown: reconcile before publishing again",
        RunState::Complete(RunStatus::Complete) => "complete",
        RunState::Complete(RunStatus::Partial) => "partial",
        RunState::Complete(RunStatus::Failed) => "failed",
        RunState::Complete(RunStatus::Incomplete) => "incomplete",
    }
}

fn open<'a>(
    root: &Path,
    engine: &'a dyn Engine,
    clock: &'a dyn Clock,
) -> Result<Publisher<'a>, String> {
    Publisher::open(root, engine, clock, &NoSecrets)
        .map(|p| p.with_adapter(CopyToFolder::new(root)))
        .map_err(why)
}

impl Writer {
    fn open(&self, root: &Path) -> Result<Publisher<'_>, String> {
        open(root, &*self.engine, &*self.clock)
    }

    pub fn preflight(&mut self, root: &Path, target: &str) -> Result<Done, String> {
        let t = Target::from_id(target).ok_or_else(|| format!("no target {target}"))?;
        let config = delivery_config(t).ok_or_else(|| format!("{} has no delivery", t.label()))?;
        let Self {
            engine,
            rules,
            clock,
            ids,
        } = self;
        let out = open(root, &**engine, &**clock)?
            .preflight(&PreflightRequest { config }, &**rules, &mut **ids)
            .map_err(why)?;
        Ok(match out {
            PreflightOutcome::Granted { run, .. } => Done {
                status: format!("preflight sealed {}: approve it to publish", run.0),
                run: run.0,
            },
            PreflightOutcome::Blocked { run, blocks } => Done {
                status: format!(
                    "preflight blocked {}: {} blocking finding(s)",
                    run.0,
                    blocks.len()
                ),
                run: run.0,
            },
        })
    }

    pub fn approve(&self, root: &Path, run: &str) -> Result<Done, String> {
        let id = StableId(run.into());
        self.open(root)?.approve(&id, ACTOR).map_err(why)?;
        Ok(Done {
            run: run.into(),
            status: format!("approved {run} by {ACTOR}; not dispatched yet"),
        })
    }

    /// Delivers the plan with the configuration the grant sealed, never
    /// the current one: the publisher re-derives every seal first.
    pub fn dispatch(&self, root: &Path, run: &str) -> Result<Done, String> {
        let id = StableId(run.into());
        let mut p = self.open(root)?;
        let cfg = p
            .run(&id)
            .and_then(|v| v.target_cfg)
            .ok_or_else(|| "the run sealed no target configuration".to_string())?;
        let cfg: TargetConfig = serde_json::from_value(cfg).map_err(|e| e.to_string())?;
        let state = p.publish(&id, &cfg, &*self.rules).map_err(why)?;
        Ok(Done {
            run: run.into(),
            status: format!("dispatched {run}: {}", status_label(&state)),
        })
    }

    pub fn reconcile(&self, root: &Path, run: &str, how: Observed) -> Result<Done, String> {
        let id = StableId(run.into());
        let mut p = self.open(root)?;
        let obs = match how {
            Observed::Probe => {
                let state = p
                    .reconcile_by_probe(&id)
                    .map_err(why)?
                    .ok_or("this delivery has no read-only probe; record what you observed")?;
                return Ok(Done {
                    run: run.into(),
                    status: format!("reconciled {run} by probe: {}", status_label(&state)),
                });
            }
            Observed::Complete => Observation::Delivered(RunStatus::Complete),
            Observed::Partial => Observation::Delivered(RunStatus::Partial),
            Observed::NotDelivered => Observation::NotDelivered,
        };
        let state = p.reconcile(&id, obs, ACTOR).map_err(why)?;
        Ok(Done {
            run: run.into(),
            status: format!("reconciled {run} by {ACTOR}: {}", status_label(&state)),
        })
    }

    pub fn discard(&self, root: &Path, run: &str) -> Result<Done, String> {
        let id = StableId(run.into());
        self.open(root)?.discard(&id, ACTOR).map_err(why)?;
        Ok(Done {
            run: run.into(),
            status: format!("discarded the grant of {run}; nothing was delivered"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cv_model::modules::audit::parse_key_id;

    fn one_failing_rule(tier: &str) -> AuditRules {
        let pack = cv_rules::RulePack::from_toml(&format!(
            r#"
[pack]
id = "gate"
version = 1
description = "t"
[[rule]]
id = "GATE-H2"
severity = "Error"
tier = "{tier}"
archetypes = ["Any"]
remediation = "r"
check = {{ Exists = {{ artifact = "Html", selector = {{ Html = "h2" }} }} }}
[rule.citation]
source = "s"
url = "https://example.org/"
frozen_quote = "q"
captured = "2026-11-23"
"#
        ))
        .unwrap();
        AuditRules(cv_rules::Ruleset::new(vec![pack]).unwrap())
    }

    fn verdict(rules: &AuditRules) -> RulesetVerdict {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("index.html"), "<h1>t</h1>").unwrap();
        let files = cv_publish::plan::inventory(d.path()).unwrap();
        rules.evaluate(d.path(), &files)
    }

    #[test]
    fn blocking_findings_are_labelled_with_their_audit_row_key() {
        let v = verdict(&one_failing_rule("NormativeSpec"));
        assert_eq!(v.blocking.len(), 1, "{v:?}");
        let (code, subject) = parse_key_id(&v.blocking[0]).expect("an Audit key");
        assert_eq!((code, subject), ("GATE-H2", "index.html"));
        assert!(v.advisory.is_empty());
    }

    #[test]
    fn heuristic_findings_are_advisory_only() {
        let v = verdict(&one_failing_rule("Heuristic"));
        assert!(v.blocking.is_empty(), "{v:?}");
        assert_eq!(v.advisory, ["GATE-H2 on index.html · heuristic"]);
    }

    #[test]
    fn only_static_dist_has_a_delivery_until_185_decides() {
        for t in cv_publish::registry() {
            let c = delivery_config(t);
            assert_eq!(c.is_some(), t == Target::StaticDist, "{t:?}");
        }
        let c = delivery_config(Target::StaticDist).unwrap();
        assert_eq!(
            (c.adapter.as_str(), c.destination.as_str()),
            (COPY_TO_FOLDER, STATIC_DIST_FOLDER)
        );
    }
}
