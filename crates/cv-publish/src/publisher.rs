//! The publish loop over the ledger. Every decision reads durable
//! journal state; every transition is fsynced before the next step.

use crate::adapters::{Delivered, Delivery, DeliveryAdapter};
use crate::plan::{SealedPlan, hash_json, inventory, load_sealed, sha256};
use crate::preflight::{
    PreflightOutcome, PreflightRequest, Ruleset, graph_file, run_dir, site_dir, verbs,
};
use crate::secrets::SecretBackend;
use crate::targets::TargetConfig;
use crate::{PublishError, Result};
use cv_core::engine::{Engine, ExitClass};
use cv_core::events::publish::PublishEvent;
use cv_core::id::StableId;
use cv_core::publish::{PreflightGrant, RunState, RunStatus};
use cv_core::sys::{Clock, IdGen};
use cv_ledger::journal::Entry;
use cv_ledger::nouns::{DeliveryNoun, derive_id};
use cv_ledger::{Journal, RunView, ingest, runs};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// What a human (or a read-only adapter probe) observed at the target
/// while reconciling an `Unknown`/`Submitted` run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Observation {
    Delivered(RunStatus),
    NotDelivered,
}

pub struct Publisher<'a> {
    root: PathBuf,
    journal: Journal,
    engine: &'a dyn Engine,
    clock: &'a dyn Clock,
    secrets: &'a dyn SecretBackend,
    adapters: BTreeMap<String, Box<dyn DeliveryAdapter + 'a>>,
    events: Vec<PublishEvent>,
}

impl<'a> Publisher<'a> {
    /// Open the project ledger and recover: any run whose last durable
    /// state is `Dispatching` was interrupted after the transport may have
    /// been touched → `Unknown`. Never re-dispatched.
    pub fn open(
        root: &Path,
        engine: &'a dyn Engine,
        clock: &'a dyn Clock,
        secrets: &'a dyn SecretBackend,
    ) -> Result<Self> {
        let mut p = Self {
            root: root.to_path_buf(),
            journal: Journal::open(root)?,
            engine,
            clock,
            secrets,
            adapters: BTreeMap::new(),
            events: Vec::new(),
        };
        p.recover()?;
        Ok(p)
    }

    pub fn with_adapter(mut self, a: impl DeliveryAdapter + 'a) -> Self {
        self.adapters.insert(a.id().to_string(), Box::new(a));
        self
    }

    pub fn journal(&self) -> &Journal {
        &self.journal
    }

    pub fn run(&self, run: &StableId) -> Option<RunView> {
        runs(self.journal.records()).remove(run)
    }

    pub fn runs(&self) -> BTreeMap<StableId, RunView> {
        runs(self.journal.records())
    }

    /// Drain publish events emitted since the last call.
    pub fn take_events(&mut self) -> Vec<PublishEvent> {
        std::mem::take(&mut self.events)
    }

    fn append(&mut self, e: Entry) -> Result<()> {
        self.journal.append(self.clock.now(), e)?;
        Ok(())
    }

    fn set_state(&mut self, run: &StableId, state: RunState, note: Option<String>) -> Result<()> {
        self.append(Entry::RunState {
            run: run.clone(),
            state: state.clone(),
            note,
        })?;
        self.events.push(PublishEvent::RunStateChanged {
            run: run.clone(),
            to: state,
        });
        Ok(())
    }

    fn refuse(&mut self, run: &StableId, reason: String) -> PublishError {
        if let Err(e) = self.append(Entry::Refused {
            run: run.clone(),
            reason: reason.clone(),
        }) {
            return e;
        }
        PublishError::Refused(reason)
    }

    fn recover(&mut self) -> Result<Vec<StableId>> {
        let stuck: Vec<_> = self
            .runs()
            .into_iter()
            .filter(|(_, v)| v.state == Some(RunState::Dispatching))
            .map(|(id, _)| id)
            .collect();
        for run in &stuck {
            self.set_state(
                run,
                RunState::Unknown,
                Some("interrupted after dispatch; reconcile before publishing again".into()),
            )?;
        }
        Ok(stuck)
    }

    // ── preflight ────────────────────────────────────────────────────

    /// Build the target profile into the run's own output dir, run
    /// validate/check/ruleset on those bytes, seal a grant (or record
    /// the blocks). Nothing is delivered.
    pub fn preflight(
        &mut self,
        req: &PreflightRequest,
        ruleset: &dyn Ruleset,
        ids: &mut dyn IdGen,
    ) -> Result<PreflightOutcome> {
        let run = ids.next();
        let cfg = &req.config;
        let target = cfg.target.id().to_string();
        let dir = self.root.join(run_dir(&run));
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
        std::fs::create_dir_all(self.root.join(site_dir(&run)))?;

        let mut blocks = Vec::new();
        for (step, verb) in verbs(cfg, &run) {
            match self.engine.run(&verb, Box::new(|_| {})) {
                Ok(ExitClass::Ok) => {}
                Ok(x) => blocks.push(format!("{step}: {x:?}")),
                Err(e) => blocks.push(format!("{step}: {e}")),
            }
            if step == "build" && !blocks.is_empty() {
                break;
            }
        }
        let site = self.root.join(site_dir(&run));
        let files = inventory(&site)?;
        if blocks.is_empty() && files.is_empty() {
            blocks.push("build: output dir is empty".into());
        }
        let graph = std::fs::read(self.root.join(graph_file(&run))).ok();
        if blocks.is_empty() && graph.is_none() {
            blocks.push("graph: not produced".into());
        }
        if blocks.is_empty() {
            blocks.extend(
                ruleset
                    .evaluate(&site, &files)
                    .blocking
                    .into_iter()
                    .map(|b| format!("ruleset: {b}")),
            );
        }

        let proof = ingest::from_output_dir(&run, &site)?;
        for n in proof.artifacts {
            self.append(Entry::Artifact(n))?;
        }
        for n in proof.checks {
            self.append(Entry::Check(n))?;
        }
        for n in proof.claims {
            self.append(Entry::Claim(n))?;
        }

        if !blocks.is_empty() {
            self.append(Entry::PreflightBlocked {
                run: run.clone(),
                target,
                blocks: blocks.clone(),
            })?;
            return Ok(PreflightOutcome::Blocked { run, blocks });
        }

        let plan = SealedPlan {
            target: target.clone(),
            adapter: cfg.adapter.clone(),
            destination: cfg.destination.clone(),
            site: site_dir(&run),
            files,
        };
        let grant = PreflightGrant {
            artifact_set_hash: plan.artifact_set_hash()?,
            graph_hash: sha256(graph.as_deref().unwrap_or_default()),
            plan_hash: plan.hash()?,
            target_cfg_hash: hash_json(cfg)?,
            ruleset_hash: ruleset.hash(),
            boris_version: self.engine.locate()?.version,
        };
        self.append(Entry::GrantIssued {
            run: run.clone(),
            target,
            grant: grant.clone(),
            plan: serde_json::to_value(&plan)?,
            target_cfg: serde_json::to_value(cfg)?,
        })?;
        self.events.push(PublishEvent::PreflightIssued {
            grant: Box::new(grant.clone()),
        });
        Ok(PreflightOutcome::Granted { run, grant })
    }

    // ── approve / publish ────────────────────────────────────────────

    /// Explicit user intent, durably recorded before any dispatch.
    pub fn approve(&mut self, run: &StableId, by: &str) -> Result<()> {
        let v = self
            .run(run)
            .ok_or_else(|| PublishError::UnknownRun(run.clone()))?;
        let Some(grant) = &v.grant else {
            return Err(self.refuse(run, "no grant: preflight blocked".into()));
        };
        let grant_digest = hash_json(grant)?;
        self.append(Entry::Approved {
            run: run.clone(),
            by: by.into(),
            grant_digest,
        })
    }

    /// Re-derive every sealed hash from what is on disk *now* and name
    /// each one that no longer matches.
    fn broken_seals(
        &self,
        v: &RunView,
        grant: &PreflightGrant,
        plan: &SealedPlan,
        cfg: &TargetConfig,
        ruleset: &dyn Ruleset,
    ) -> Result<Vec<&'static str>> {
        let mut broken = Vec::new();
        let site = self.root.join(&plan.site);
        let current = SealedPlan {
            files: if site.is_dir() {
                inventory(&site)?
            } else {
                vec![]
            },
            ..plan.clone()
        };
        if current.artifact_set_hash()? != grant.artifact_set_hash {
            broken.push("artifact_set");
        }
        if plan.hash()? != grant.plan_hash || current.hash()? != grant.plan_hash {
            broken.push("plan");
        }
        let graph = std::fs::read(self.root.join(graph_file(v.run.as_ref().expect("run id"))))
            .unwrap_or_default();
        if sha256(&graph) != grant.graph_hash {
            broken.push("graph");
        }
        if hash_json(cfg)? != grant.target_cfg_hash
            || v.target_cfg.as_ref() != Some(&serde_json::to_value(cfg)?)
        {
            broken.push("target_cfg");
        }
        if ruleset.hash() != grant.ruleset_hash {
            broken.push("ruleset");
        }
        if self.engine.locate()?.version != grant.boris_version {
            broken.push("boris_version");
        }
        Ok(broken)
    }

    /// Deliver exactly the sealed plan, once. Returns the resulting state.
    pub fn publish(
        &mut self,
        run: &StableId,
        cfg: &TargetConfig,
        ruleset: &dyn Ruleset,
    ) -> Result<RunState> {
        let all = self.runs();
        let v = all
            .get(run)
            .cloned()
            .ok_or_else(|| PublishError::UnknownRun(run.clone()))?;
        if v.was_dispatched() {
            return Err(self.refuse(
                run,
                format!(
                    "already dispatched ({}); runs are never retried",
                    v.phase().as_str()
                ),
            ));
        }
        let (Some(grant), Some(plan)) = (v.grant.clone(), v.plan.clone()) else {
            return Err(self.refuse(run, "no grant: preflight blocked".into()));
        };
        let approved = self.journal.records().iter().any(|r| {
            matches!(&r.entry, Entry::Approved { run: a, grant_digest, .. }
                if a == run && hash_json(&grant).is_ok_and(|h| h == *grant_digest))
        });
        if !approved {
            return Err(self.refuse(run, "not approved".into()));
        }
        if let Some((other, _)) = all
            .iter()
            .find(|(id, o)| *id != run && o.target == v.target && o.is_unresolved())
        {
            let other = other.0.clone();
            return Err(self.refuse(
                run,
                format!("run {other} on this target is unresolved; reconcile it first"),
            ));
        }
        let plan: SealedPlan = serde_json::from_value(plan)?;
        let broken = self.broken_seals(&v, &grant, &plan, cfg, ruleset)?;
        if !broken.is_empty() {
            let _ = self.refuse(run, format!("seal broken: {}", broken.join(", ")));
            return Err(PublishError::SealBroken(broken));
        }
        if !self.adapters.contains_key(&cfg.adapter) {
            return Err(self.refuse(run, format!("no delivery adapter '{}'", cfg.adapter)));
        }
        let files = match load_sealed(&self.root.join(&plan.site), &plan) {
            Ok(f) => f,
            Err(e) => {
                let _ = self.refuse(run, e.to_string());
                return Err(e);
            }
        };

        // Durable before contact: delivery noun + Dispatching.
        self.append(Entry::Delivery(DeliveryNoun {
            id: derive_id("dlv", &[&run.0, &cfg.adapter, &grant.plan_hash.to_hex()]),
            run: run.clone(),
            target: plan.target.clone(),
            adapter: cfg.adapter.clone(),
            plan_hash: grant.plan_hash,
            files: plan.files.len() as u64,
            bytes: plan.total_bytes(),
        }))?;
        self.set_state(run, RunState::Dispatching, None)?;

        let adapter = &self.adapters[&cfg.adapter];
        let result = adapter.deliver(&Delivery {
            run,
            plan: &plan,
            files: &files,
            site: &self.root.join(&plan.site),
            config: cfg,
            secrets: self.secrets,
            engine: self.engine,
        });
        let (state, note) = match result {
            Ok(Delivered::Submitted) => (RunState::Submitted, None),
            Ok(Delivered::Complete(s)) => (RunState::Complete(s), None),
            Err(f) if f.contacted => (RunState::Unknown, Some(f.message)),
            Err(f) => (RunState::Complete(RunStatus::Failed), Some(f.message)),
        };
        self.set_state(run, state.clone(), note)?;
        if matches!(
            state,
            RunState::Complete(RunStatus::Complete | RunStatus::Partial)
        ) {
            self.ship_check(run, &plan, cfg)?;
        }
        Ok(state)
    }

    fn ship_check(&mut self, run: &StableId, plan: &SealedPlan, cfg: &TargetConfig) -> Result<()> {
        if cfg.exports.is_empty() {
            return Ok(());
        }
        let adapter = &self.adapters[&cfg.adapter];
        let read = |p: &str| adapter.read_back(cfg, p);
        let exports = crate::exports::check(
            plan,
            &cfg.exports,
            adapter
                .can_read_back()
                .then_some(&read as crate::exports::ReadBack<'_>),
        );
        self.append(Entry::ExportsChecked {
            run: run.clone(),
            exports,
        })
    }

    // ── reconcile ────────────────────────────────────────────────────

    /// Resolve an `Unknown`/`Submitted` run from an observation. Records
    /// evidence and a terminal state; never re-delivers.
    pub fn reconcile(&mut self, run: &StableId, obs: Observation, by: &str) -> Result<RunState> {
        let v = self
            .run(run)
            .ok_or_else(|| PublishError::UnknownRun(run.clone()))?;
        if !matches!(v.state, Some(RunState::Unknown | RunState::Submitted)) {
            return Err(self.refuse(
                run,
                format!(
                    "reconcile needs unknown/submitted, run is {}",
                    v.phase().as_str()
                ),
            ));
        }
        let status = match obs {
            Observation::Delivered(s) => s,
            Observation::NotDelivered => RunStatus::Failed,
        };
        self.append(Entry::Reconciled {
            run: run.clone(),
            observation: format!("{obs:?}"),
            by: by.into(),
        })?;
        let state = RunState::Complete(status);
        self.set_state(run, state.clone(), Some(format!("reconciled by {by}")))?;
        if matches!(status, RunStatus::Complete | RunStatus::Partial)
            && let (Some(plan), Some(cfg)) = (v.plan, v.target_cfg)
        {
            let plan: SealedPlan = serde_json::from_value(plan)?;
            let cfg: TargetConfig = serde_json::from_value(cfg)?;
            if self.adapters.contains_key(&cfg.adapter) {
                self.ship_check(run, &plan, &cfg)?;
            }
        }
        Ok(state)
    }

    /// Reconcile from the adapter's read-only probe, if it has one. The
    /// probe inspects the destination sealed for *this* run, not the
    /// current target config.
    pub fn reconcile_by_probe(&mut self, run: &StableId) -> Result<Option<RunState>> {
        let v = self
            .run(run)
            .ok_or_else(|| PublishError::UnknownRun(run.clone()))?;
        let (Some(plan), Some(cfg)) = (v.plan, v.target_cfg) else {
            return Ok(None);
        };
        let plan: SealedPlan = serde_json::from_value(plan)?;
        let cfg: TargetConfig = serde_json::from_value(cfg)?;
        let Some(adapter) = self.adapters.get(&cfg.adapter) else {
            return Ok(None);
        };
        let Some(obs) = adapter.probe(&plan, &cfg) else {
            return Ok(None);
        };
        let by = format!("probe:{}", cfg.adapter);
        self.reconcile(run, obs, &by).map(Some)
    }
}
