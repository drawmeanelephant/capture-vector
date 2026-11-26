use super::*;
use cv_core::publish::PreflightGrant;
use cv_core::sys::{Hash, SeqIdGen, Timestamp};
use cv_ledger::Journal;
use cv_publish::PreflightRequest;
use cv_publish::Target;
use cv_publish::adapters::{
    COPY_TO_FOLDER, Delivered, Delivery, DeliveryAdapter, TransportFailure,
};

fn seeded() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    scene::seed(d.path());
    d
}

fn ledger(root: &Path) -> Ledger {
    match read(root) {
        LedgerRead::Read(l) => l,
        other => panic!("expected a readable ledger, got {other:?}"),
    }
}

fn journal_bytes(root: &Path) -> Vec<u8> {
    std::fs::read(root.join(JOURNAL_REL)).unwrap()
}

#[test]
fn targets_mirror_the_publish_registry() {
    let t = targets();
    let ids: Vec<_> = t.iter().map(|t| (t.id.as_str(), t.verified)).collect();
    assert_eq!(
        ids,
        [
            ("github-pages", true),
            ("standard-site", true),
            ("nostr-nip23", false),
            ("static-dist", false),
        ]
    );
    assert_eq!(t[2].secret.as_deref(), Some("nostr-nsec"));
    assert!(t[3].hosts_html && !t[1].hosts_html);
}

#[test]
fn ephemeral_port_reads_no_workspace() {
    let mut p = DiskPort::ephemeral();
    assert_eq!(p.ledger(), LedgerRead::NoWorkspace);
    assert_eq!(p.targets().len(), 4);
    assert!(p.error().is_none());
}

#[test]
fn a_workspace_without_a_ledger_reads_absent_and_is_left_untouched() {
    let d = tempfile::tempdir().unwrap();
    let mut p = DiskPort::open(d.path());
    assert_eq!(p.ledger(), LedgerRead::Absent);
    assert_eq!(p.describe(), JOURNAL_REL);
    assert!(!d.path().join(".cv").exists());
}

#[test]
fn seeded_runs_fold_in_record_order_with_their_ledger_phases() {
    let d = seeded();
    let l = ledger(d.path());
    let runs: Vec<_> = l
        .runs
        .iter()
        .map(|r| (r.target.as_str(), r.phase))
        .collect();
    assert_eq!(
        runs,
        [
            ("static-dist", Phase::Complete),
            ("github-pages", Phase::PreflightBlocked),
            ("nostr-nip23", Phase::Unknown),
            ("standard-site", Phase::Approved),
        ]
    );
    assert!(l.notes.is_empty());
}

#[test]
fn delivered_run_carries_its_plan_settings_and_evidence() {
    let d = seeded();
    let l = ledger(d.path());
    let r = &l.runs[0];
    let plan = r.plan.as_ref().unwrap();
    assert_eq!(
        (plan.adapter.as_str(), plan.destination.as_str()),
        (COPY_TO_FOLDER, "deployed")
    );
    assert!(plan.files > 0 && plan.bytes > 0);
    let s = r.settings.as_ref().unwrap();
    assert_eq!(s.profile, "default HTML target");
    assert_eq!(s.base_url, "unhosted local copy");
    assert!(!s.verified);
    assert_eq!(r.grant.as_ref().unwrap().boris_version, "boris/0.9.0");
    assert_eq!(r.approved_by.as_deref(), Some("local"));
    assert_eq!(r.history, ["dispatching", "complete"]);
    assert!(r.artifacts > 0, "proof-pack artifacts were lifted");
    assert!(!r.exports_checked);
    let mine = |k: EvidenceKind| {
        l.evidence
            .iter()
            .filter(|e| e.run == r.id && e.kind == k)
            .count()
    };
    assert_eq!(mine(EvidenceKind::Delivery), 1);
    assert_eq!(
        mine(EvidenceKind::Receipt),
        0,
        "unhosted static-dist declares no exports"
    );
    assert!(mine(EvidenceKind::Check) > 0 && mine(EvidenceKind::Claim) > 0);
}

#[test]
fn blocks_and_transport_notes_are_kept_as_recorded() {
    let d = seeded();
    let l = ledger(d.path());
    assert_eq!(l.runs[1].blocks, ["check: ContentFailure"]);
    assert!(l.runs[1].plan.is_none() && l.runs[1].grant.is_none());
    assert_eq!(
        l.runs[2].notes,
        ["unknown: connection reset after the upload began"]
    );
    assert_eq!(l.runs[2].history, ["dispatching", "unknown"]);
    assert!(l.runs[3].history.is_empty());
}

struct Crash;

impl DeliveryAdapter for Crash {
    fn id(&self) -> &str {
        COPY_TO_FOLDER
    }
    fn deliver(&self, _: &Delivery<'_>) -> Result<Delivered, TransportFailure> {
        panic!("simulated crash after contact");
    }
}

#[test]
fn an_interrupted_dispatch_reads_as_unknown_and_is_not_recovered() {
    let d = tempfile::tempdir().unwrap();
    let eng = scene::CapturedBuild {
        root: d.path().into(),
        check: cv_core::engine::ExitClass::Ok,
    };
    let cfg = scene::config(Target::StaticDist, COPY_TO_FOLDER, "deployed");
    let mut p = scene::open(d.path(), &eng).with_adapter(Crash);
    let run = scene::granted(p.preflight(
        &PreflightRequest {
            config: cfg.clone(),
        },
        &scene::SceneRules,
        &mut SeqIdGen::new("run"),
    ));
    p.approve(&run, "local").unwrap();
    let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        p.publish(&run, &cfg, &scene::SceneRules)
    }));
    assert!(crashed.is_err());
    drop(p);
    let before = journal_bytes(d.path());
    let l = ledger(d.path());
    assert_eq!(l.runs[0].phase, Phase::Unknown);
    assert_eq!(l.runs[0].history, ["dispatching"]);
    assert_eq!(journal_bytes(d.path()), before, "read-only: no recovery");
}

#[test]
fn a_torn_tail_is_disclosed_and_left_on_disk() {
    let d = seeded();
    let mut raw = journal_bytes(d.path());
    raw.extend_from_slice(b"{\"seq\":");
    std::fs::write(d.path().join(JOURNAL_REL), &raw).unwrap();
    let l = ledger(d.path());
    assert_eq!(l.runs.len(), 4);
    assert_eq!(
        l.notes,
        ["an unfinished final append (7 bytes) was never committed and is not shown"]
    );
    assert_eq!(journal_bytes(d.path()), raw);
}

#[test]
fn a_corrupt_committed_line_is_unreadable_and_disclosed() {
    let d = seeded();
    scene::corrupt(d.path());
    let before = journal_bytes(d.path());
    let p = DiskPort::open(d.path());
    let e = p.error().unwrap();
    assert!(
        e.starts_with(".cv/ledger/ledger.jsonl: journal line 2 is corrupt"),
        "{e}"
    );
    assert_eq!(journal_bytes(d.path()), before);
}

#[test]
fn an_undecodable_sealed_plan_becomes_a_run_note() {
    let d = tempfile::tempdir().unwrap();
    let mut j = Journal::open(d.path()).unwrap();
    let h = Hash([1; 32]);
    j.append(
        Timestamp(0),
        Entry::GrantIssued {
            run: StableId("run-x".into()),
            target: "static-dist".into(),
            grant: PreflightGrant {
                artifact_set_hash: h,
                graph_hash: h,
                plan_hash: h,
                target_cfg_hash: h,
                ruleset_hash: h,
                boris_version: "boris/0.9.0".into(),
            },
            plan: serde_json::json!({ "files": 3 }),
            target_cfg: serde_json::json!(null),
        },
    )
    .unwrap();
    drop(j);
    let l = ledger(d.path());
    let r = &l.runs[0];
    assert_eq!(r.phase, Phase::Granted);
    assert!(r.plan.is_none() && r.settings.is_none());
    assert_eq!(r.notes.len(), 2, "{:?}", r.notes);
    assert!(r.notes[0].starts_with("sealed plan not shown"));
    assert!(r.notes[1].starts_with("sealed target config not shown"));
}
