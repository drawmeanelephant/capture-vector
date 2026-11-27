//! Publish verbs and their dialogs. The port performs every ledger
//! write; after each verb the module re-reads the ledger and selects the
//! run it touched, so what is shown is what the journal recorded.

use super::{Item, Observed, Phase, PublishModule, RunRecord, Source};
use crate::command::{CommandSpec, Intent};
use crate::dialog::{ActionTone, CHOICE, Choice, DialogAnswer, DialogKind, DialogSpec};

/// `publish.preflight.<target id>`: one verb per registered target.
pub const PREFLIGHT_PREFIX: &str = "publish.preflight.";
pub const APPROVE: &str = "publish.approve";
pub const DISPATCH: &str = "publish.dispatch";
pub const RECONCILE: &str = "publish.reconcile";
pub const DISCARD: &str = "publish.discard-grant";

/// How `cv-publish` prefixes a ruleset block in the ledger.
pub(super) const RULESET_BLOCK: &str = "ruleset: ";
pub(super) const UNWIRED_DELIVERY: &str =
    "not wired yet; copy-to-folder is the only wired delivery";

const ADVISORY_SHOWN: usize = 6;
const APPROVE_DISPATCH: &str = "approve-dispatch";

/// `(code, subject)` of a ruleset block that names an Audit finding.
pub(super) fn audit_finding(block: &str) -> Option<(&str, &str)> {
    crate::modules::audit::parse_key_id(block.strip_prefix(RULESET_BLOCK)?)
}

fn workspace_gate(m: &PublishModule) -> Option<String> {
    match m.source {
        Source::NoWorkspace => Some("open a workspace first".into()),
        Source::Unreadable(_) => Some("the publish ledger is unreadable".into()),
        Source::Absent | Source::Read => None,
    }
}

/// Whether this port can deliver the run's sealed plan.
fn deliverable(m: &PublishModule, r: &RunRecord) -> Result<(), String> {
    match &r.plan {
        Some(p) if m.adapters.contains(&p.adapter) => Ok(()),
        _ => Err(format!(
            "{} delivery is {UNWIRED_DELIVERY}",
            m.label_of(&r.target)
        )),
    }
}

fn approvable(m: &PublishModule, r: &RunRecord) -> bool {
    r.phase == Phase::Granted && deliverable(m, r).is_ok()
}

fn dispatchable(m: &PublishModule, r: &RunRecord) -> bool {
    r.phase == Phase::Approved && deliverable(m, r).is_ok()
}

fn reconcilable(r: &RunRecord) -> bool {
    matches!(
        r.phase,
        Phase::Dispatching | Phase::Submitted | Phase::Unknown
    )
}

fn discardable(r: &RunRecord) -> bool {
    matches!(r.phase, Phase::Granted | Phase::Approved)
}

pub fn commands(m: &PublishModule) -> Vec<CommandSpec> {
    let gate = workspace_gate(m);
    let mut out: Vec<CommandSpec> = m
        .targets
        .iter()
        .map(|t| {
            let why = gate.clone().or_else(|| {
                t.delivery
                    .is_none()
                    .then(|| format!("{} delivery is {UNWIRED_DELIVERY}", t.label))
            });
            CommandSpec::new(
                format!("{PREFLIGHT_PREFIX}{}", t.id),
                format!("Publish: Preflight {}", t.label),
            )
            .disabled(why)
        })
        .collect();
    let verb = |id: &str, title: &str, ok: bool, none: &str| {
        CommandSpec::new(id, title).disabled(gate.clone().or_else(|| (!ok).then(|| none.into())))
    };
    out.push(verb(
        APPROVE,
        "Publish: Approve selected run…",
        m.runs.iter().any(|r| approvable(m, r)),
        "no granted run with a configured delivery",
    ));
    out.push(verb(
        DISPATCH,
        "Publish: Dispatch selected approved run",
        m.runs.iter().any(|r| dispatchable(m, r)),
        "no approved run with a configured delivery",
    ));
    out.push(verb(
        RECONCILE,
        "Publish: Reconcile selected run…",
        m.runs.iter().any(reconcilable),
        "no unresolved run",
    ));
    out.push(verb(
        DISCARD,
        "Publish: Discard selected run's grant…",
        m.runs.iter().any(discardable),
        "no undispatched grant",
    ));
    out
}

/// What the selected run's reading offers next; `Err` when nothing can
/// be done with it from here, and why.
pub(super) fn next_step(m: &PublishModule, r: &RunRecord) -> Result<String, String> {
    if let Some(why) = workspace_gate(m) {
        return Err(why);
    }
    match r.phase {
        Phase::PreflightBlocked if r.blocks.iter().any(|b| audit_finding(b).is_some()) => Err(
            "fix the blocking findings, then preflight again".into(),
        ),
        Phase::PreflightBlocked => Err("fix what blocked preflight, then preflight again".into()),
        Phase::Granted => deliverable(m, r).map(|_| {
            "approve it (Publish: Approve selected run…) or discard the grant".into()
        }),
        Phase::Approved if r.refusals.iter().any(|x| x.starts_with("seal broken")) => Err(
            "sealed bytes changed after preflight: discard this grant and preflight again".into(),
        ),
        Phase::Approved => deliverable(m, r).map(|_| {
            "dispatch it (Publish: Dispatch selected approved run): the sealed plan is delivered once"
                .into()
        }),
        Phase::Dispatching | Phase::Submitted | Phase::Unknown => Ok(
            "reconcile it (Publish: Reconcile selected run…): record what reached the target. \
             Runs are never retried"
                .into(),
        ),
        Phase::Complete | Phase::Partial | Phase::Failed | Phase::Incomplete | Phase::Discarded => {
            Err("finished: preflight again to publish new bytes".into())
        }
    }
}

fn status(s: impl Into<String>) -> Vec<Intent> {
    vec![Intent::Status(s.into())]
}

fn run_at<'a>(m: &'a PublishModule, key: Option<&str>) -> Option<&'a RunRecord> {
    match m.by_key.get(key?)? {
        Item::Run(i) => m.runs.get(*i),
        _ => None,
    }
}

/// Re-read the ledger the verb wrote, then select the run it touched.
fn after(
    m: &mut PublishModule,
    verb: &str,
    run: Option<&str>,
    done: Result<super::Done, String>,
) -> Vec<Intent> {
    m.reload();
    let reveal = |key: &str| Intent::Reveal {
        module: super::ID.into(),
        key: key.into(),
    };
    match done {
        Ok(d) => vec![reveal(&d.run), Intent::Status(d.status)],
        Err(e) => {
            let mut out: Vec<Intent> = run.map(reveal).into_iter().collect();
            out.push(Intent::Status(format!("{verb} refused: {e}")));
            out
        }
    }
}

pub fn run(m: &mut PublishModule, command: &str) -> Vec<Intent> {
    if let Some(why) = workspace_gate(m) {
        return status(format!("publish: {why}"));
    }
    if let Some(target) = command.strip_prefix(PREFLIGHT_PREFIX) {
        return preflight(m, target);
    }
    let Some(r) = run_at(m, m.selected.as_deref()).cloned() else {
        return status("publish: select a run first");
    };
    let label = m.label_of(&r.target);
    match command {
        APPROVE => {
            if r.phase != Phase::Granted {
                return status(format!("{}: only a granted run can be approved", r.id));
            }
            match deliverable(m, &r) {
                Ok(()) => vec![Intent::dialog(approve_dialog(m, &r))],
                Err(e) => status(e),
            }
        }
        DISPATCH => {
            if r.phase != Phase::Approved {
                return status(format!(
                    "{}: only an approved run can be dispatched ({})",
                    r.id,
                    r.phase.label()
                ));
            }
            if let Err(e) = deliverable(m, &r) {
                return status(e);
            }
            let done = m.port.dispatch(&r.id);
            after(m, &format!("dispatch to {label}"), Some(&r.id), done)
        }
        RECONCILE => {
            if !reconcilable(&r) {
                return status(format!(
                    "{}: only an unresolved run can be reconciled ({})",
                    r.id,
                    r.phase.label()
                ));
            }
            vec![Intent::dialog(reconcile_dialog(m, &r))]
        }
        DISCARD => {
            if !discardable(&r) {
                return status(format!(
                    "{}: only an undispatched grant can be discarded ({})",
                    r.id,
                    r.phase.label()
                ));
            }
            vec![Intent::dialog(discard_dialog(m, &r))]
        }
        _ => status(format!("unknown command {command}")),
    }
}

fn preflight(m: &mut PublishModule, target: &str) -> Vec<Intent> {
    let Some(t) = m.targets.iter().find(|t| t.id == target).cloned() else {
        return status(format!("publish: no target {target}"));
    };
    if t.delivery.is_none() {
        return status(format!("{} delivery is {UNWIRED_DELIVERY}", t.label));
    }
    let done = m.port.preflight(&t.id);
    after(m, &format!("preflight {}", t.label), None, done)
}

fn short(h: &cv_core::sys::Hash) -> String {
    let hex = h.to_hex();
    format!("{}…", &hex[..16])
}

fn approve_dialog(m: &PublishModule, r: &RunRecord) -> DialogSpec {
    let mut body = Vec::new();
    if let Some(p) = &r.plan {
        body.push(format!(
            "{} · {} files · {} B by {} to {}",
            m.label_of(&r.target),
            p.files,
            p.bytes,
            p.adapter,
            p.destination
        ));
    }
    body.push(
        "Approval is recorded in the ledger before anything is dispatched. Dispatch delivers \
         exactly these sealed bytes, once; a hash that no longer matches refuses."
            .into(),
    );
    if let Some(g) = &r.grant {
        body.push(String::new());
        body.push("Sealed hashes".into());
        for (k, v) in [
            ("artifact set", short(&g.artifact_set_hash)),
            ("graph", short(&g.graph_hash)),
            ("plan", short(&g.plan_hash)),
            ("target cfg", short(&g.target_cfg_hash)),
            ("ruleset", short(&g.ruleset_hash)),
            ("boris", g.boris_version.clone()),
        ] {
            body.push(format!("  {k}: {v}"));
        }
    }
    body.push(String::new());
    if r.advisory.is_empty() {
        body.push("Advisory findings: none".into());
    } else {
        body.push(format!(
            "Advisory findings ({}): heuristic or below error, these never block",
            r.advisory.len()
        ));
        for a in r.advisory.iter().take(ADVISORY_SHOWN) {
            body.push(format!("  advisory · {a}"));
        }
        if r.advisory.len() > ADVISORY_SHOWN {
            body.push(format!(
                "  … {} more in the run's reading",
                r.advisory.len() - ADVISORY_SHOWN
            ));
        }
    }
    DialogSpec::new(APPROVE, "Approve publish run?", DialogKind::Confirm)
        .body(body.join("\n"))
        .context(r.id.clone())
        .cancel("Cancel")
        .action("approve", "Approve", ActionTone::Primary)
        .action(APPROVE_DISPATCH, "Approve and dispatch", ActionTone::Ghost)
}

fn reconcile_dialog(m: &PublishModule, r: &RunRecord) -> DialogSpec {
    let mut choices = Vec::new();
    if deliverable(m, r).is_ok() {
        choices.push(Choice {
            id: "probe".into(),
            label: "Check the destination now (read-only)".into(),
        });
    }
    for (id, label) in [
        ("complete", "I checked: everything was delivered"),
        ("partial", "I checked: only part was delivered"),
        ("not-delivered", "I checked: nothing was delivered"),
    ] {
        choices.push(Choice {
            id: id.into(),
            label: label.into(),
        });
    }
    DialogSpec::new(RECONCILE, "Reconcile run", DialogKind::Choose(choices))
        .body(format!(
            "{} · {} is {}. Record what reached the target. Nothing is sent again.",
            m.label_of(&r.target),
            r.id,
            r.phase.label()
        ))
        .context(r.id.clone())
        .cancel("Cancel")
        .action("record", "Record", ActionTone::Primary)
}

fn discard_dialog(m: &PublishModule, r: &RunRecord) -> DialogSpec {
    DialogSpec::new(DISCARD, "Discard grant?", DialogKind::Confirm)
        .body(format!(
            "{} · {} ({}) can no longer be approved or dispatched. Its evidence stays in the \
             ledger.",
            m.label_of(&r.target),
            r.id,
            r.phase.label()
        ))
        .context(r.id.clone())
        .cancel("Cancel")
        .action("discard", "Discard grant", ActionTone::Danger)
}

pub fn answer(m: &mut PublishModule, command: &str, a: &DialogAnswer) -> Vec<Intent> {
    if a.action.is_none() {
        return Vec::new();
    }
    let Some(r) = run_at(m, a.context.as_deref()).cloned() else {
        return status("publish: that run is no longer in the ledger");
    };
    let label = m.label_of(&r.target);
    match command {
        APPROVE => {
            if !approvable(m, &r) {
                return status(format!("{}: no longer awaiting approval", r.id));
            }
            let mut done = m.port.approve(&r.id);
            if a.is(APPROVE_DISPATCH) && done.is_ok() {
                m.reload();
                done = m.port.dispatch(&r.id);
            }
            after(m, &format!("approval of {label}"), Some(&r.id), done)
        }
        RECONCILE => {
            if !reconcilable(&r) {
                return status(format!("{}: no longer unresolved", r.id));
            }
            let how = match a.text(CHOICE) {
                Some("probe") => Observed::Probe,
                Some("complete") => Observed::Complete,
                Some("partial") => Observed::Partial,
                Some("not-delivered") => Observed::NotDelivered,
                _ => return status("reconcile: choose what was observed"),
            };
            let done = m.port.reconcile(&r.id, how);
            after(m, &format!("reconcile of {label}"), Some(&r.id), done)
        }
        DISCARD if a.is("discard") => {
            let done = m.port.discard(&r.id);
            after(m, &format!("discard of {label}"), Some(&r.id), done)
        }
        _ => Vec::new(),
    }
}
