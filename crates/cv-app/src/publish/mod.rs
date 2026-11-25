//! Publish port: the target registry from `cv_publish` and the workspace's
//! `.cv/ledger/` journal, read for display (`cv_ledger::journal::read`).
//! Reading never writes: no run is recovered, sealed or dispatched by a
//! read, so an interrupted `dispatching` run stays as the ledger recorded
//! it until a verb opens the publisher. Verbs are offered only by a port
//! built [`DiskPort::writable`].

pub mod scene;
pub mod write;

use cv_core::id::StableId;
use cv_core::publish::{RunState, RunStatus};
use cv_ledger::journal::{self, Committed, Entry, JOURNAL_REL};
use cv_ledger::{RunPhase, RunView, runs};
use cv_model::modules::publish::{Done, Observed};
use cv_model::modules::publish::{
    EvidenceKind, EvidenceRecord, Ledger, LedgerRead, Phase, PlanSummary, PublishPort, RunRecord,
    TargetInfo, TargetSettings,
};
use cv_publish::{SealedPlan, TargetConfig, registry};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use write::Writer;

pub struct DiskPort {
    /// The read taken at open, handed out once; later reads go to disk.
    read: Option<LedgerRead>,
    root: Option<PathBuf>,
    writer: Option<Writer>,
    describe: String,
}

impl DiskPort {
    /// No workspace: targets only.
    pub fn ephemeral() -> Self {
        Self {
            read: Some(LedgerRead::NoWorkspace),
            root: None,
            writer: None,
            describe: "no workspace".into(),
        }
    }

    /// Reads `root`'s journal now, so a failure can be disclosed before
    /// the module is built.
    pub fn open(root: &Path) -> Self {
        Self {
            read: Some(read(root)),
            root: Some(root.to_path_buf()),
            writer: None,
            describe: JOURNAL_REL.into(),
        }
    }

    /// Reads like [`Self::open`] and publishes through `writer`.
    pub fn writable(root: &Path, writer: Writer) -> Self {
        Self {
            writer: Some(writer),
            ..Self::open(root)
        }
    }

    /// Why the journal could not be shown, if it could not.
    pub fn error(&self) -> Option<&str> {
        match &self.read {
            Some(LedgerRead::Unreadable(e)) => Some(e),
            _ => None,
        }
    }

    fn writer(&mut self) -> Result<(&Path, &mut Writer), String> {
        match (&self.root, &mut self.writer) {
            (Some(root), Some(w)) => Ok((root, w)),
            _ => Err("this publish ledger is read-only".into()),
        }
    }
}

impl PublishPort for DiskPort {
    fn targets(&self) -> Vec<TargetInfo> {
        targets()
    }
    fn ledger(&mut self) -> LedgerRead {
        match (self.read.take(), &self.root) {
            (Some(r), _) => r,
            (None, Some(root)) => read(root),
            (None, None) => LedgerRead::NoWorkspace,
        }
    }
    fn describe(&self) -> String {
        self.describe.clone()
    }
    fn adapters(&self) -> Vec<String> {
        match self.writer {
            Some(_) => vec![cv_publish::adapters::COPY_TO_FOLDER.into()],
            None => Vec::new(),
        }
    }
    fn preflight(&mut self, target: &str) -> Result<Done, String> {
        let (root, w) = self.writer()?;
        w.preflight(root, target)
    }
    fn approve(&mut self, run: &str) -> Result<Done, String> {
        let (root, w) = self.writer()?;
        w.approve(root, run)
    }
    fn dispatch(&mut self, run: &str) -> Result<Done, String> {
        let (root, w) = self.writer()?;
        w.dispatch(root, run)
    }
    fn reconcile(&mut self, run: &str, how: Observed) -> Result<Done, String> {
        let (root, w) = self.writer()?;
        w.reconcile(root, run, how)
    }
    fn discard(&mut self, run: &str) -> Result<Done, String> {
        let (root, w) = self.writer()?;
        w.discard(root, run)
    }
}

pub fn targets() -> Vec<TargetInfo> {
    registry()
        .into_iter()
        .map(|t| TargetInfo {
            id: t.id().into(),
            label: t.label().into(),
            verified: t.verified(),
            secret: t.secret_name().map(str::to_string),
            hosts_html: t.hosts_html(),
            delivery: write::delivery_config(t)
                .map(|c| format!("{} → {}", c.adapter, c.destination)),
        })
        .collect()
}

pub fn read(root: &Path) -> LedgerRead {
    match journal::read(root) {
        Ok(None) => LedgerRead::Absent,
        Ok(Some(c)) => LedgerRead::Read(fold(&c)),
        Err(e) => LedgerRead::Unreadable(format!("{JOURNAL_REL}: {e}")),
    }
}

fn phase(p: RunPhase) -> Phase {
    match p {
        RunPhase::PreflightBlocked => Phase::PreflightBlocked,
        RunPhase::Granted => Phase::Granted,
        RunPhase::Approved => Phase::Approved,
        RunPhase::Dispatching => Phase::Dispatching,
        RunPhase::Submitted => Phase::Submitted,
        RunPhase::Unknown => Phase::Unknown,
        RunPhase::Complete => Phase::Complete,
        RunPhase::Partial => Phase::Partial,
        RunPhase::Failed => Phase::Failed,
        RunPhase::Incomplete => Phase::Incomplete,
        RunPhase::Discarded => Phase::Discarded,
    }
}

fn state_label(s: &RunState) -> &'static str {
    match s {
        RunState::Dispatching => "dispatching",
        RunState::Submitted => "submitted",
        RunState::Unknown => "unknown",
        RunState::Complete(RunStatus::Complete) => "complete",
        RunState::Complete(RunStatus::Partial) => "partial",
        RunState::Complete(RunStatus::Failed) => "failed",
        RunState::Complete(RunStatus::Incomplete) => "incomplete",
    }
}

fn entry_run(e: &Entry) -> &StableId {
    match e {
        Entry::Artifact(n) => &n.run,
        Entry::Check(n) => &n.run,
        Entry::Claim(n) => &n.run,
        Entry::Delivery(n) => &n.run,
        Entry::GrantIssued { run, .. }
        | Entry::PreflightBlocked { run, .. }
        | Entry::Approved { run, .. }
        | Entry::RunState { run, .. }
        | Entry::Refused { run, .. }
        | Entry::Reconciled { run, .. }
        | Entry::ExportsChecked { run, .. }
        | Entry::Advisory { run, .. }
        | Entry::GrantDiscarded { run, .. } => run,
    }
}

fn run_record(id: &str, v: &RunView) -> RunRecord {
    let mut r = RunRecord::new(id, &v.target, phase(v.phase()));
    r.grant = v.grant.clone();
    r.blocks = v.blocks.clone();
    r.approved_by = v.approved_by.clone();
    r.discarded_by = v.discarded_by.clone();
    r.advisory = v.advisory.clone();
    r.history = v.history.iter().map(|s| state_label(s).into()).collect();
    r.refusals = v.refusals.clone();
    r.exports_checked = v.exports.is_some();
    if let Some(p) = &v.plan {
        match serde_json::from_value::<SealedPlan>(p.clone()) {
            Ok(p) => {
                r.plan = Some(PlanSummary {
                    files: p.files.len() as u64,
                    bytes: p.total_bytes(),
                    adapter: p.adapter,
                    destination: p.destination,
                })
            }
            Err(e) => r.notes.push(format!("sealed plan not shown: {e}")),
        }
    }
    if let Some(c) = &v.target_cfg {
        match serde_json::from_value::<TargetConfig>(c.clone()) {
            Ok(c) => {
                r.settings = Some(TargetSettings {
                    profile: c.profile,
                    base_url: c.deploy.base_url,
                    verified: c.verified,
                })
            }
            Err(e) => r.notes.push(format!("sealed target config not shown: {e}")),
        }
    }
    r
}

/// Fold committed records for display: runs in the order they were first
/// recorded, evidence in record order. Verdicts are copied, never judged.
pub fn fold(c: &Committed) -> Ledger {
    let views = runs(&c.records);
    let mut out = Ledger::default();
    let mut at: BTreeMap<&str, usize> = BTreeMap::new();
    for rec in &c.records {
        let run = entry_run(&rec.entry);
        if !at.contains_key(run.0.as_str())
            && let Some(v) = views.get(run)
        {
            at.insert(&run.0, out.runs.len());
            out.runs.push(run_record(&run.0, v));
        }
    }
    let evidence =
        |id: &StableId, run: &StableId, kind, name: &str, status: &str, detail| EvidenceRecord {
            id: id.0.clone(),
            run: run.0.clone(),
            kind,
            name: name.into(),
            status: status.into(),
            detail,
        };
    for rec in &c.records {
        let run = at.get(entry_run(&rec.entry).0.as_str()).copied();
        match &rec.entry {
            Entry::Artifact(_) => {
                if let Some(i) = run {
                    out.runs[i].artifacts += 1;
                }
            }
            Entry::Check(n) => {
                let mut detail = format!("{} checked · {} findings", n.checked, n.findings);
                if let Some(c) = &n.coverage {
                    detail.push_str(&format!(" · coverage {c}"));
                }
                if !n.ran {
                    detail.push_str(" · did not run");
                }
                out.evidence.push(evidence(
                    &n.id,
                    &n.run,
                    EvidenceKind::Check,
                    &n.check_id,
                    &n.status,
                    detail,
                ));
            }
            Entry::Claim(n) => out.evidence.push(evidence(
                &n.id,
                &n.run,
                EvidenceKind::Claim,
                &n.claim_id,
                &n.status,
                n.statement.clone(),
            )),
            Entry::Delivery(n) => {
                let hex = n.plan_hash.to_hex();
                out.evidence.push(evidence(
                    &n.id,
                    &n.run,
                    EvidenceKind::Delivery,
                    &n.adapter,
                    "dispatched",
                    format!("{} files · {} B · plan {}", n.files, n.bytes, &hex[..12]),
                ))
            }
            Entry::ExportsChecked { run: id, exports } => {
                // A later read-back (after reconciliation) supersedes the
                // earlier one, as `cv_ledger::runs` folds it.
                out.evidence
                    .retain(|e| !(e.run == id.0 && e.kind == EvidenceKind::Receipt));
                for x in exports {
                    let detail = match &x.sha256 {
                        Some(h) => format!("{} export · sha256 {}", x.kind, &h[..h.len().min(12)]),
                        None => format!("{} export", x.kind),
                    };
                    out.evidence.push(EvidenceRecord {
                        id: format!("rcpt:{}:{}", id.0, x.path),
                        run: id.0.clone(),
                        kind: EvidenceKind::Receipt,
                        name: x.path.clone(),
                        status: x.status.clone(),
                        detail,
                    });
                }
            }
            Entry::RunState {
                state,
                note: Some(n),
                ..
            } => {
                if let Some(i) = run {
                    out.runs[i]
                        .notes
                        .push(format!("{}: {n}", state_label(state)));
                }
            }
            Entry::Reconciled {
                observation, by, ..
            } => {
                if let Some(i) = run {
                    out.runs[i]
                        .reconciliations
                        .push(format!("{observation} by {by}"));
                }
            }
            Entry::GrantIssued { .. }
            | Entry::PreflightBlocked { .. }
            | Entry::Approved { .. }
            | Entry::RunState { note: None, .. }
            | Entry::Refused { .. }
            | Entry::Advisory { .. }
            | Entry::GrantDiscarded { .. } => {}
        }
    }
    if let Some(t) = &c.torn_tail {
        out.notes.push(format!(
            "an unfinished final append ({} bytes) was never committed and is not shown",
            t.len()
        ));
    }
    out
}

#[cfg(test)]
mod tests;
