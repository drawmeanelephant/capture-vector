use super::*;
use crate::dialog::{ActionTone, DialogKind, DialogSpec};
use crate::modules::activity::tests::assert_index_matches_scan;
use cv_core::sys::Hash;

fn targets() -> Vec<TargetInfo> {
    let t = |id: &str, label: &str, verified, secret: Option<&str>, hosts_html| TargetInfo {
        id: id.into(),
        label: label.into(),
        verified,
        secret: secret.map(str::to_string),
        hosts_html,
        delivery: (id == "static-dist").then(|| "copy-to-folder → deployed".into()),
    };
    vec![
        t("github-pages", "GitHub Pages", true, None, true),
        t(
            "standard-site",
            "Standard.site (ATProto)",
            true,
            Some("atproto-app-password"),
            false,
        ),
        t("static-dist", "Static dist", false, None, true),
    ]
}

fn grant() -> PreflightGrant {
    PreflightGrant {
        artifact_set_hash: Hash([1; 32]),
        graph_hash: Hash([2; 32]),
        plan_hash: Hash([3; 32]),
        target_cfg_hash: Hash([4; 32]),
        ruleset_hash: Hash([5; 32]),
        boris_version: "boris/0.9.0".into(),
    }
}

fn run(id: &str, target: &str, phase: Phase) -> RunRecord {
    let mut r = RunRecord::new(id, target, phase);
    if phase != Phase::PreflightBlocked {
        r.grant = Some(grant());
        r.plan = Some(PlanSummary {
            adapter: "copy-to-folder".into(),
            destination: "deployed".into(),
            files: 10,
            bytes: 2048,
        });
    }
    r
}

fn evidence(id: &str, run: &str, kind: EvidenceKind, status: &str) -> EvidenceRecord {
    EvidenceRecord {
        id: id.into(),
        run: run.into(),
        kind,
        name: id.into(),
        status: status.into(),
        detail: String::new(),
    }
}

fn ledger() -> Ledger {
    let mut blocked = run("run-1", "static-dist", Phase::PreflightBlocked);
    blocked.blocks = vec!["check: ContentFailure".into()];
    let mut unknown = run("run-2", "static-dist", Phase::Unknown);
    unknown.history = vec!["dispatching".into(), "unknown".into()];
    unknown.notes = vec!["interrupted after dispatch".into()];
    let mut done = run("run-0", "static-dist", Phase::Complete);
    done.exports_checked = true;
    Ledger {
        runs: vec![
            done,
            blocked,
            unknown,
            run("run-3", "github-pages", Phase::Approved),
            run("run-4", "github-pages", Phase::Dispatching),
        ],
        evidence: vec![
            evidence("chk-a", "run-0", EvidenceKind::Check, "passed"),
            evidence("dlv-a", "run-0", EvidenceKind::Delivery, "dispatched"),
            evidence(
                "rcpt:run-0:llms.txt",
                "run-0",
                EvidenceKind::Receipt,
                "missing",
            ),
            evidence(
                "rcpt:run-0:feed.xml",
                "run-0",
                EvidenceKind::Receipt,
                "present",
            ),
        ],
        notes: vec![],
    }
}

fn module(read: LedgerRead) -> PublishModule {
    PublishModule::new(Box::new(MemoryPort {
        targets: targets(),
        read,
    }))
}

fn keys(m: &PublishModule, nav: &str) -> Vec<String> {
    (0..m.list_len(Some(nav)))
        .map(|i| m.row(Some(nav), i).key)
        .collect()
}

fn count(m: &PublishModule, nav: &str) -> Option<u64> {
    m.nav().items.into_iter().find(|i| i.key == nav)?.count
}

#[test]
fn runs_fall_into_mailboxes_by_their_durable_state_newest_first() {
    let m = module(LedgerRead::Read(ledger()));
    assert_eq!(m.spec().slot, Slot::Publish);
    assert_eq!(
        keys(&m, RUNS),
        ["run-4", "run-3", "run-2", "run-1", "run-0"]
    );
    assert_eq!(m.list_len(None), 5, "All runs is the default mailbox");
    assert_eq!(keys(&m, PREFLIGHT), ["run-3", "run-1"]);
    assert_eq!(keys(&m, UNRESOLVED), ["run-4", "run-2"]);
    assert_eq!(keys(&m, COMPLETED), ["run-0"]);
    assert_eq!(keys(&m, "target:github-pages"), ["run-4", "run-3"]);
    assert_eq!(m.list_len(Some(TARGETS)), 3);
    assert_eq!(m.list_len(Some(EVIDENCE)), 4);
    assert_eq!(m.badge(), Some(2), "unresolved runs badge the rail");
}

#[test]
fn nav_counts_match_every_mailbox_and_targets_with_runs_get_one() {
    let m = module(LedgerRead::Read(ledger()));
    let nav = m.nav();
    for item in &nav.items {
        assert_eq!(
            item.count,
            Some(m.list_len(Some(&item.key)) as u64),
            "{}",
            item.key
        );
    }
    let labels: Vec<&str> = nav.items.iter().map(|i| i.label.as_str()).collect();
    assert!(labels.contains(&"Target · GitHub Pages"));
    assert!(labels.contains(&"Target · Static dist"));
    assert!(
        !labels.iter().any(|l| l.contains("Standard.site")),
        "no runs, no mailbox"
    );
    assert_eq!(nav, m.build_nav());
}

#[test]
fn rows_show_state_and_unknown_runs_say_to_reconcile() {
    let m = module(LedgerRead::Read(ledger()));
    let i = m.index_of_key(None, "run-2").unwrap();
    let row = m.row(None, i);
    assert_eq!(row.title, "Static dist · run-2");
    assert_eq!(row.badge.map(|c| c.text).as_deref(), Some("unknown"));
    assert_eq!(row.rail, Some(Tone::Warn));
    let r = m.reading("run-2");
    assert!(r.lines.iter().any(|l| l.text.contains("Reconcile before")));
    assert!(r.lines.iter().any(|l| l.text == "dispatching → unknown"));
    let blocked = m.row(Some(PREFLIGHT), 1);
    assert_eq!(blocked.detail, "check: ContentFailure");
    assert_eq!(blocked.rail, Some(Tone::Error));
    let dispatching = m.reading("run-4");
    assert!(
        dispatching
            .lines
            .iter()
            .any(|l| l.text.contains("no outcome was recorded")),
        "a run left dispatching is not presented as in progress"
    );
}

#[test]
fn inspector_exposes_the_sealed_grant_and_the_runs_evidence() {
    let m = module(LedgerRead::Read(ledger()));
    let ins = m.inspector("run-0");
    let f = |k: &str| ins.fields.iter().find(|(n, _)| n == k).unwrap().1.clone();
    assert_eq!(f("plan"), Hash([3; 32]).to_hex());
    assert_eq!(f("boris"), "boris/0.9.0");
    assert_eq!(f("destination"), "deployed");
    let list = |t: &str| ins.lists.iter().find(|(n, _)| n == t).unwrap().1.clone();
    assert_eq!(
        list("Exports"),
        [
            "rcpt:run-0:llms.txt: missing",
            "rcpt:run-0:feed.xml: present"
        ]
    );
    assert_eq!(list("Checks & claims"), ["chk-a: passed"]);
    let reading = m.reading("run-0");
    assert!(
        reading
            .lines
            .iter()
            .any(|l| l.text == "1 missing · 1 present")
    );
    assert_eq!(
        m.inspector("run-1")
            .fields
            .iter()
            .find(|(k, _)| k == "plan")
            .unwrap()
            .1,
        "—"
    );
}

#[test]
fn targets_list_the_registry_with_their_latest_run() {
    let m = module(LedgerRead::Read(ledger()));
    let pages = m.row(Some(TARGETS), 0);
    assert_eq!(pages.key, "target:github-pages");
    assert_eq!(pages.detail, "2 run(s) · latest dispatching");
    assert_eq!(pages.badge.map(|c| c.text).as_deref(), Some("verified"));
    assert_eq!(m.row(Some(TARGETS), 1).detail, "no runs");
    let r = m.reading("target:standard-site");
    assert!(
        r.lines
            .iter()
            .any(|l| l.text == "keychain item atproto-app-password")
    );
    assert!(m.noun("target:github-pages").is_none());
}

#[test]
fn nouns_carry_the_ledger_kinds() {
    let m = module(LedgerRead::Read(ledger()));
    let kind = |k: &str| m.noun(k).unwrap().kind.as_str().to_string();
    assert_eq!(kind("run-0"), noun_kind::PLAN);
    assert_eq!(kind("dlv-a"), noun_kind::DELIVERY);
    assert_eq!(kind("chk-a"), noun_kind::CHECK);
    assert_eq!(kind("rcpt:run-0:llms.txt"), noun_kind::RECEIPT);
    assert_eq!(m.attr_rows().len(), 9);
    assert!(m.contains("run-3") && !m.contains("run-9"));
}

#[test]
fn empty_absent_and_unreadable_ledgers_are_disclosed_not_blank() {
    for (read, title, tone) in [
        (LedgerRead::NoWorkspace, "No workspace open", Tone::Dim),
        (LedgerRead::Absent, "No publish runs yet", Tone::Dim),
        (
            LedgerRead::Read(Ledger::default()),
            "No publish runs yet",
            Tone::Dim,
        ),
        (
            LedgerRead::Unreadable("journal line 2 is corrupt".into()),
            "Publish ledger unreadable",
            Tone::Error,
        ),
    ] {
        let unreadable = matches!(read, LedgerRead::Unreadable(_));
        let m = module(read);
        assert_eq!(keys(&m, RUNS), [STATUS], "{title}");
        assert_eq!(
            count(&m, RUNS),
            Some(0),
            "{title}: the status row is not a run"
        );
        let row = m.row(None, 0);
        assert_eq!((row.title.as_str(), row.tone), (title, tone));
        assert_eq!(m.reading(STATUS).title, title);
        assert_eq!(
            m.list_len(Some(TARGETS)),
            3,
            "{title}: targets still listed"
        );
        assert!(m.noun(STATUS).is_none());
        if unreadable {
            assert!(
                m.reading(STATUS)
                    .lines
                    .iter()
                    .any(|l| l.text.contains("line 2"))
            );
            assert_eq!(m.badge(), None);
        }
    }
}

#[test]
fn ledger_notes_reach_every_reading() {
    let mut l = ledger();
    l.notes = vec!["an incomplete final record was ignored".into()];
    let m = module(LedgerRead::Read(l));
    for k in ["run-0", STATUS] {
        assert!(
            m.reading(k)
                .lines
                .iter()
                .any(|x| x.text.contains("incomplete final")),
            "{k}"
        );
    }
}

#[test]
fn index_of_key_matches_the_row_scan_in_every_mailbox() {
    for read in [
        LedgerRead::Read(ledger()),
        LedgerRead::Absent,
        LedgerRead::NoWorkspace,
        LedgerRead::Unreadable("x".into()),
    ] {
        let m = module(read);
        assert_index_matches_scan(&m, &[STATUS, "target:nope", "run-9", ""]);
    }
}

#[test]
fn unregistered_module_has_no_workspace() {
    let r = Registry::with_all();
    let m = r.get(r.index_of(ID).unwrap()).unwrap();
    assert_eq!(m.row(None, 0).title, "No workspace open");
    assert!(
        m.commands()
            .iter()
            .all(|c| c.disabled.as_deref() == Some("open a workspace first")),
        "no workspace: every verb is disabled"
    );
}

// ── verbs ────────────────────────────────────────────────────────────

use crate::dialog::{CHOICE, Value};
use std::cell::RefCell as Shared;
use std::rc::Rc;

#[derive(Default)]
struct Script {
    ledger: Ledger,
    calls: Vec<String>,
    refuse_dispatch: Option<String>,
}

struct FakePort(Rc<Shared<Script>>);

impl FakePort {
    fn set(s: &mut Script, run: &str, f: impl FnOnce(&mut RunRecord)) {
        f(s.ledger.runs.iter_mut().find(|r| r.id == run).unwrap());
    }
}

impl PublishPort for FakePort {
    fn targets(&self) -> Vec<TargetInfo> {
        targets()
    }
    fn ledger(&mut self) -> LedgerRead {
        LedgerRead::Read(self.0.borrow().ledger.clone())
    }
    fn describe(&self) -> String {
        "scripted".into()
    }
    fn adapters(&self) -> Vec<String> {
        vec!["copy-to-folder".into()]
    }
    fn preflight(&mut self, target: &str) -> Result<Done, String> {
        let mut s = self.0.borrow_mut();
        s.calls.push(format!("preflight {target}"));
        s.ledger.runs.push(run("run-new", target, Phase::Granted));
        Ok(Done {
            run: "run-new".into(),
            status: "sealed".into(),
        })
    }
    fn approve(&mut self, id: &str) -> Result<Done, String> {
        let mut s = self.0.borrow_mut();
        s.calls.push(format!("approve {id}"));
        Self::set(&mut s, id, |r| {
            r.phase = Phase::Approved;
            r.approved_by = Some("local".into());
        });
        Ok(Done {
            run: id.into(),
            status: "approved".into(),
        })
    }
    fn dispatch(&mut self, id: &str) -> Result<Done, String> {
        let mut s = self.0.borrow_mut();
        s.calls.push(format!("dispatch {id}"));
        if let Some(why) = s.refuse_dispatch.clone() {
            Self::set(&mut s, id, |r| r.refusals.push(why.clone()));
            return Err(why);
        }
        Self::set(&mut s, id, |r| r.phase = Phase::Complete);
        Ok(Done {
            run: id.into(),
            status: "complete".into(),
        })
    }
    fn reconcile(&mut self, id: &str, how: Observed) -> Result<Done, String> {
        let mut s = self.0.borrow_mut();
        s.calls.push(format!("reconcile {id} {how:?}"));
        Self::set(&mut s, id, |r| r.phase = Phase::Failed);
        Ok(Done {
            run: id.into(),
            status: "reconciled".into(),
        })
    }
    fn discard(&mut self, id: &str) -> Result<Done, String> {
        let mut s = self.0.borrow_mut();
        s.calls.push(format!("discard {id}"));
        Self::set(&mut s, id, |r| r.phase = Phase::Discarded);
        Ok(Done {
            run: id.into(),
            status: "discarded".into(),
        })
    }
}

fn scripted(l: Ledger) -> (PublishModule, Rc<Shared<Script>>) {
    let s = Rc::new(Shared::new(Script {
        ledger: l,
        ..Default::default()
    }));
    (PublishModule::new(Box::new(FakePort(s.clone()))), s)
}

fn command(m: &PublishModule, id: &str) -> CommandSpec {
    m.commands().into_iter().find(|c| c.id == id).unwrap()
}

fn dialog(intents: &[Intent]) -> &DialogSpec {
    match intents {
        [Intent::Dialog(d)] => d,
        other => panic!("expected one dialog, got {other:?}"),
    }
}

fn reveals(intents: &[Intent], key: &str) -> bool {
    intents
        .iter()
        .any(|i| matches!(i, Intent::Reveal { module, key: k } if module == ID && k == key))
}

fn status_of(intents: &[Intent]) -> String {
    intents
        .iter()
        .find_map(|i| match i {
            Intent::Status(s) => Some(s.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

fn confirmed(spec: &DialogSpec, action: &str) -> DialogAnswer {
    DialogAnswer {
        action: Some(action.into()),
        context: spec.context.clone(),
        ..Default::default()
    }
}

#[test]
fn only_a_target_with_a_configured_delivery_can_be_preflighted() {
    let (m, _) = scripted(ledger());
    assert_eq!(command(&m, "publish.preflight.static-dist").disabled, None);
    for t in ["github-pages", "standard-site"] {
        let why = command(&m, &format!("publish.preflight.{t}"))
            .disabled
            .unwrap();
        assert!(why.contains("not wired yet"), "{t}: {why}");
    }
    let closed = module(LedgerRead::NoWorkspace);
    assert!(
        closed
            .commands()
            .iter()
            .all(|c| c.disabled.as_deref() == Some("open a workspace first"))
    );
}

#[test]
fn preflight_asks_the_port_and_selects_the_run_it_recorded() {
    let (mut m, s) = scripted(ledger());
    let out = m.run("publish.preflight.static-dist");
    assert_eq!(s.borrow().calls, ["preflight static-dist"]);
    assert!(reveals(&out, "run-new"), "{out:?}");
    assert!(m.contains("run-new"), "the ledger is read again");
    assert!(
        m.run("publish.preflight.github-pages")[0]
            == Intent::Status(
                "GitHub Pages delivery is not wired yet; copy-to-folder is the only wired delivery"
                    .into()
            )
    );
    assert_eq!(
        s.borrow().calls.len(),
        1,
        "a pending target never reaches the port"
    );
}

#[test]
fn approval_confirms_the_sealed_hashes_file_count_and_advisory_findings() {
    let mut l = ledger();
    let mut granted = run("run-5", "static-dist", Phase::Granted);
    granted.advisory = vec!["SEO-META-DESCRIPTION on index · heuristic".into()];
    l.runs.push(granted);
    let (mut m, s) = scripted(l);
    m.select(Some("run-5"));
    let out = m.run(APPROVE);
    let spec = dialog(&out);
    assert_eq!(spec.kind, DialogKind::Confirm);
    let body = spec.body.as_deref().unwrap();
    assert!(
        body.contains("10 files · 2048 B by copy-to-folder to deployed"),
        "{body}"
    );
    assert!(body.contains(&format!("plan: {}…", &Hash([3; 32]).to_hex()[..16])));
    assert!(body.contains("boris: boris/0.9.0"));
    assert!(body.contains("advisory · SEO-META-DESCRIPTION on index · heuristic"));
    assert!(body.contains("never block"));
    assert!(
        s.borrow().calls.is_empty(),
        "nothing is approved before the answer"
    );

    let spec = spec.clone();
    let out = m.answer(APPROVE, &confirmed(&spec, "approve"));
    assert_eq!(s.borrow().calls, ["approve run-5"]);
    assert!(reveals(&out, "run-5"));
    assert_eq!(m.reading("run-5").title, "Static dist · approved");

    m.select(Some("run-5"));
    let out = m.run(DISPATCH);
    assert_eq!(s.borrow().calls, ["approve run-5", "dispatch run-5"]);
    assert_eq!(status_of(&out), "complete");
    assert_eq!(m.reading("run-5").title, "Static dist · complete");
}

#[test]
fn approve_and_dispatch_records_the_approval_first() {
    let mut l = ledger();
    l.runs.push(run("run-5", "static-dist", Phase::Granted));
    let (mut m, s) = scripted(l);
    m.select(Some("run-5"));
    let spec = dialog(&m.run(APPROVE)).clone();
    m.answer(APPROVE, &confirmed(&spec, "approve-dispatch"));
    assert_eq!(s.borrow().calls, ["approve run-5", "dispatch run-5"]);
    m.answer(APPROVE, &DialogAnswer::default());
    assert_eq!(s.borrow().calls.len(), 2, "a cancelled dialog does nothing");
}

#[test]
fn a_refused_dispatch_is_shown_and_the_run_keeps_its_refusal() {
    let (mut m, s) = scripted(ledger());
    s.borrow_mut()
        .ledger
        .runs
        .push(run("run-5", "static-dist", Phase::Approved));
    s.borrow_mut().refuse_dispatch = Some("seal broken: artifact_set, plan".into());
    m.reload();
    m.select(Some("run-5"));
    let out = m.run(DISPATCH);
    assert_eq!(
        status_of(&out),
        "dispatch to Static dist refused: seal broken: artifact_set, plan"
    );
    assert!(reveals(&out, "run-5"));
    assert!(
        m.reading("run-5")
            .lines
            .iter()
            .any(|l| l.label.as_deref() == Some("refused") && l.text.contains("seal broken"))
    );
}

#[test]
fn a_run_for_unwired_delivery_cannot_be_approved_or_dispatched() {
    let mut l = ledger();
    let mut r = run("run-5", "standard-site", Phase::Granted);
    r.plan.as_mut().unwrap().adapter = "http-post".into();
    l.runs.push(r);
    let (mut m, s) = scripted(l);
    m.select(Some("run-5"));
    assert!(status_of(&m.run(APPROVE)).contains("not wired yet"));
    assert!(s.borrow().calls.is_empty());
    let next = m
        .reading("run-5")
        .lines
        .into_iter()
        .find(|l| l.label.as_deref() == Some("next"))
        .unwrap();
    assert!(next.text.contains("not wired yet") && next.tone == Tone::Dim);
}

#[test]
fn an_unknown_run_reconciles_from_a_choice_and_is_never_redispatched() {
    let (mut m, s) = scripted(ledger());
    m.select(Some("run-2"));
    let spec = dialog(&m.run(RECONCILE)).clone();
    let DialogKind::Choose(choices) = &spec.kind else {
        panic!("{spec:?}")
    };
    let ids: Vec<&str> = choices.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, ["probe", "complete", "partial", "not-delivered"]);
    let mut a = confirmed(&spec, "record");
    a.values
        .insert(CHOICE.into(), Value::Choice("not-delivered".into()));
    let out = m.answer(RECONCILE, &a);
    assert_eq!(s.borrow().calls, ["reconcile run-2 NotDelivered"]);
    assert!(reveals(&out, "run-2"));
    assert_eq!(m.reading("run-2").title, "Static dist · failed");
    assert!(!s.borrow().calls.iter().any(|c| c.starts_with("dispatch")));
    m.select(Some("run-2"));
    assert!(status_of(&m.run(RECONCILE)).contains("only an unresolved run"));
}

#[test]
fn a_grant_is_discarded_only_through_a_danger_confirm() {
    let (mut m, s) = scripted(ledger());
    m.select(Some("run-3"));
    let out = m.run(DISCARD);
    let spec = dialog(&out).clone();
    assert!(
        spec.actions
            .iter()
            .any(|a| a.id == "discard" && a.tone == ActionTone::Danger)
    );
    m.answer(DISCARD, &confirmed(&spec, "discard"));
    assert_eq!(s.borrow().calls, ["discard run-3"]);
    assert_eq!(m.reading("run-3").title, "GitHub Pages · discarded");
    assert_eq!(keys(&m, COMPLETED), ["run-3", "run-0"]);
}

#[test]
fn a_blocking_ruleset_finding_is_not_linked_into_an_absent_audit_row() {
    let key = format!("A11Y-HTML-LANG|page:index|{}", Hash([7; 32]).to_hex());
    let mut l = ledger();
    let mut blocked = run("run-5", "static-dist", Phase::PreflightBlocked);
    blocked.blocks = vec![format!("ruleset: {key}"), "check: ContentFailure".into()];
    l.runs.push(blocked);
    let (m, _) = scripted(l);
    assert!(
        m.link("run-5").is_none(),
        "publish must not reveal a row Audit does not contain"
    );
    let r = m.reading("run-5");
    assert!(
        r.lines
            .iter()
            .any(|l| l.text.starts_with("A11Y-HTML-LANG on index"))
    );
    assert!(
        m.link("run-1").is_none(),
        "an engine block has no finding to reveal"
    );
}
