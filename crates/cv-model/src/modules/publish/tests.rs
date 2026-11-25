use super::*;
use crate::modules::activity::tests::assert_index_matches_scan;
use cv_core::sys::Hash;

fn targets() -> Vec<TargetInfo> {
    let t = |id: &str, label: &str, verified, secret: Option<&str>, hosts_html| TargetInfo {
        id: id.into(),
        label: label.into(),
        verified,
        secret: secret.map(str::to_string),
        hosts_html,
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
    assert_eq!(row.badge.as_deref(), Some("unknown"));
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
    assert_eq!(pages.badge.as_deref(), Some("verified"));
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
    assert!(m.commands().is_empty(), "read-only: no verbs");
}
