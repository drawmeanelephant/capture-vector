//! Fold the journal into per-run views. Run state comes from durable
//! records only — the UI never marks a run complete from optimism.

use crate::journal::{Entry, ExportEvidence, Record};
use crate::nouns::DeliveryNoun;
use cv_core::id::StableId;
use cv_core::publish::{PreflightGrant, RunState, RunStatus};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RunView {
    pub run: Option<StableId>,
    pub target: String,
    pub grant: Option<PreflightGrant>,
    pub plan: Option<serde_json::Value>,
    pub target_cfg: Option<serde_json::Value>,
    pub blocks: Vec<String>,
    pub approved_by: Option<String>,
    pub state: Option<RunState>,
    pub history: Vec<RunState>,
    pub refusals: Vec<String>,
    pub reconciliations: Vec<String>,
    pub exports: Option<Vec<ExportEvidence>>,
    pub advisory: Vec<String>,
    pub discarded_by: Option<String>,
    /// Durable pre-contact delivery evidence. Even without a `Dispatching`
    /// state record, this means the transport may have been contacted.
    pub deliveries: Vec<DeliveryNoun>,
}

/// UI-facing phase vocabulary (Publish module goldens map onto this).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunPhase {
    PreflightBlocked,
    Granted,
    Approved,
    Dispatching,
    Submitted,
    Unknown,
    Complete,
    Partial,
    Failed,
    Incomplete,
    Discarded,
}

impl RunPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PreflightBlocked => "preflight-blocked",
            Self::Granted => "granted",
            Self::Approved => "approved",
            Self::Dispatching => "dispatching",
            Self::Submitted => "submitted",
            Self::Unknown => "unknown",
            Self::Complete => "complete",
            Self::Partial => "partial",
            Self::Failed => "failed",
            Self::Incomplete => "incomplete",
            Self::Discarded => "discarded",
        }
    }
}

impl RunView {
    pub fn phase(&self) -> RunPhase {
        if self.delivery_contact_uncertain() {
            return RunPhase::Unknown;
        }
        match &self.state {
            Some(RunState::Dispatching) => RunPhase::Dispatching,
            Some(RunState::Submitted) => RunPhase::Submitted,
            Some(RunState::Unknown) => RunPhase::Unknown,
            Some(RunState::Complete(s)) => match s {
                RunStatus::Complete => RunPhase::Complete,
                RunStatus::Partial => RunPhase::Partial,
                RunStatus::Failed => RunPhase::Failed,
                RunStatus::Incomplete => RunPhase::Incomplete,
            },
            None if !self.blocks.is_empty() => RunPhase::PreflightBlocked,
            None if self.discarded_by.is_some() => RunPhase::Discarded,
            None if self.approved_by.is_some() => RunPhase::Approved,
            None => RunPhase::Granted,
        }
    }

    /// In flight or unresolved — blocks any new dispatch to the target
    /// until reconciled.
    pub fn is_unresolved(&self) -> bool {
        matches!(
            self.state,
            Some(RunState::Dispatching | RunState::Submitted | RunState::Unknown)
        ) || self.delivery_contact_uncertain()
    }

    /// Delivery evidence exists but no durable outcome explains it. This is
    /// the crash window between the pre-contact delivery append and every
    /// later state append: contact is possible and must be reconciled.
    pub fn delivery_contact_uncertain(&self) -> bool {
        !self.deliveries.is_empty()
            && !matches!(
                self.state,
                Some(RunState::Submitted | RunState::Unknown | RunState::Complete(_))
            )
    }

    /// Ever reached the transport — a run is dispatched at most once. A
    /// committed delivery noun is dispatch evidence even when the crash took
    /// the separate `Dispatching` state append.
    pub fn was_dispatched(&self) -> bool {
        !self.history.is_empty() || !self.deliveries.is_empty()
    }
}

fn view<'a>(out: &'a mut BTreeMap<StableId, RunView>, run: &StableId) -> &'a mut RunView {
    let v = out.entry(run.clone()).or_default();
    v.run = Some(run.clone());
    v
}

pub fn runs(records: &[Record]) -> BTreeMap<StableId, RunView> {
    let mut out: BTreeMap<StableId, RunView> = BTreeMap::new();
    for r in records {
        match &r.entry {
            Entry::GrantIssued {
                run,
                target,
                grant,
                plan,
                target_cfg,
            } => {
                let v = view(&mut out, run);
                v.target = target.clone();
                v.grant = Some(grant.clone());
                v.plan = Some(plan.clone());
                v.target_cfg = Some(target_cfg.clone());
            }
            Entry::PreflightBlocked {
                run,
                target,
                blocks,
            } => {
                let v = view(&mut out, run);
                v.target = target.clone();
                v.blocks = blocks.clone();
            }
            Entry::Approved { run, by, .. } => view(&mut out, run).approved_by = Some(by.clone()),
            Entry::RunState { run, state, .. } => {
                let v = view(&mut out, run);
                v.state = Some(state.clone());
                v.history.push(state.clone());
            }
            Entry::Refused { run, reason } => view(&mut out, run).refusals.push(reason.clone()),
            Entry::Reconciled {
                run, observation, ..
            } => view(&mut out, run)
                .reconciliations
                .push(observation.clone()),
            Entry::ExportsChecked { run, exports } => {
                view(&mut out, run).exports = Some(exports.clone())
            }
            Entry::Advisory { run, findings } => view(&mut out, run).advisory = findings.clone(),
            Entry::GrantDiscarded { run, by } => {
                view(&mut out, run).discarded_by = Some(by.clone())
            }
            Entry::Delivery(n) => view(&mut out, &n.run).deliveries.push(n.clone()),
            Entry::Artifact(_) | Entry::Check(_) | Entry::Claim(_) => {}
        }
    }
    out
}
