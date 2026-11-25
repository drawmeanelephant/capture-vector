use super::*;
use crate::module::{OutputRecord, PageRecord};
use crate::modules::activity::tests::env;
use cv_core::events::engine::BuildPhase;

fn fixture(rel: &str) -> Vec<u8> {
    std::fs::read(
        cv_fixtures::fixtures_root()
            .join("captures/happy/dist/dist")
            .join(rel),
    )
    .unwrap()
}

const PAGES: &[(&str, &str)] = &[
    ("index.md", "index.html"),
    ("guides/getting-started.md", "guides/getting-started.html"),
    ("guides/publishing.md", "guides/publishing.html"),
];

fn snap() -> ProjectSnapshot {
    ProjectSnapshot {
        epoch: 1,
        outputs: PAGES
            .iter()
            .map(|(_, h)| OutputRecord {
                path: (*h).into(),
                kind: "html-page".into(),
                ..Default::default()
            })
            .collect(),
        pages: PAGES
            .iter()
            .map(|(p, h)| PageRecord {
                path: (*p).into(),
                id: p.trim_end_matches(".md").into(),
                output: Some((*h).into()),
                ..Default::default()
            })
            .collect(),
        artifact_root: Some("dist".into()),
        ..Default::default()
    }
}

fn port(records: Vec<(DedupKey, FindingLifecycle)>) -> MemoryPort {
    MemoryPort {
        files: PAGES
            .iter()
            .map(|(_, h)| (h.to_string(), fixture(h)))
            .collect(),
        records,
    }
}

fn built(n: u64) -> Envelope {
    env(
        n,
        EngineEvent::BuildFinished {
            phase: BuildPhase::Initial,
            pages_written: 3,
            diagnostics: vec![],
            duration_ms: 5,
        },
    )
}

fn module(records: Vec<(DedupKey, FindingLifecycle)>) -> AuditModule {
    let mut m = AuditModule::new(Box::new(port(records)), default_ruleset());
    m.load(&snap());
    m
}

fn first_open(m: &AuditModule) -> String {
    m.row(Some("open"), 0).key
}

#[test]
fn build_finished_evaluates_into_the_mailbox() {
    let mut m = module(vec![]);
    assert_eq!(m.badge(), None, "no badge before the first build");
    assert!(m.ingest(&built(1)));
    let open = m.list_len(Some("open"));
    assert!(open > 0, "the happy corpus has open findings");
    assert_eq!(m.badge(), Some(open as u64));
    assert_eq!(
        m.refresh(),
        Some(Refresh {
            opened: open as u32,
            fixed: 0,
            carried: 0
        })
    );
    let r = m.row(Some("open"), 0);
    assert!(r.rail.is_some() && r.badge.as_ref().is_some_and(|c| c.text == "open"));
    let nav = m.nav();
    for k in ["open", "snoozed", "pack:seo-core", "pack:a11y-content"] {
        assert!(nav.items.iter().any(|i| i.key == k), "{k}");
    }
    assert!(nav.items.iter().any(|i| i.key.starts_with("rule:")));
    assert!(nav.items.iter().any(|i| i.key.starts_with("noun:")));
    assert!(m.ingest(&built(2)));
    let r2 = m.refresh().unwrap();
    assert_eq!(
        (r2.opened, r2.carried),
        (0, open as u32),
        "same build carries"
    );
}

#[test]
fn heuristic_findings_never_block() {
    let mut m = module(vec![]);
    m.ingest(&built(1));
    let heuristic: Vec<_> = m
        .entries()
        .iter()
        .filter(|e| e.af.finding.tier == AuthorityTier::Heuristic)
        .collect();
    assert!(!heuristic.is_empty());
    assert!(heuristic.iter().all(|e| !e.af.blocks_publish()));
    for i in 0..m.list_len(Some("blocking")) {
        let k = m.row(Some("blocking"), i).key;
        assert_ne!(
            m.entry(&k).unwrap().af.finding.tier,
            AuthorityTier::Heuristic
        );
    }
}

#[test]
fn the_inspector_shows_the_publish_gate_as_a_chip() {
    let mut m = module(vec![]);
    m.ingest(&built(1));
    let gate = |k: &str| m.inspector(k).badges.last().cloned().unwrap();
    let heuristic: Vec<_> = m
        .entries()
        .iter()
        .filter(|e| e.af.finding.tier == AuthorityTier::Heuristic)
        .map(|e| e.id.clone())
        .collect();
    assert!(!heuristic.is_empty());
    for k in &heuristic {
        assert_eq!(gate(k), Chip::new("does not block publishing", Tone::Dim));
        assert_eq!(m.inspector(k).badges[0].text, "heuristic (advisory)");
    }
    for i in 0..m.list_len(Some("blocking")) {
        let k = m.row(Some("blocking"), i).key;
        assert_eq!(gate(&k), Chip::new("blocks publishing", Tone::Error));
    }
}

#[test]
fn snooze_survives_the_next_build_and_a_reopen() {
    let mut m = module(vec![]);
    m.ingest(&built(1));
    let before = m.badge().unwrap();
    let key = first_open(&m);
    m.select(Some(&key));
    let st = m.run("audit.snooze");
    assert!(matches!(&st[..], [Intent::Status(s)] if s.ends_with("snoozed")));
    assert_eq!(m.badge(), Some(before - 1));
    assert_eq!(m.list_len(Some("snoozed")), 1);
    assert_eq!(
        m.row(Some("snoozed"), 0).badge.map(|c| c.text).as_deref(),
        Some("snoozed")
    );
    m.ingest(&built(2));
    assert_eq!(m.entry(&key).unwrap().state, FindingLifecycle::Ignored);
    assert_eq!(m.badge(), Some(before - 1));

    // A fresh module over the same persisted records keeps the snooze.
    let dk = m.entry(&key).unwrap().af.finding.key.clone();
    let mut m2 = module(vec![(dk.clone(), FindingLifecycle::Ignored)]);
    m2.ingest(&built(1));
    assert_eq!(m2.entry(&key).unwrap().state, FindingLifecycle::Ignored);
    assert_eq!(m2.badge(), Some(before - 1));
    let mut m3 = module(vec![
        (dk.clone(), FindingLifecycle::Ignored),
        (dk, FindingLifecycle::Open),
    ]);
    m3.ingest(&built(1));
    assert_eq!(m3.badge(), Some(before), "reopen is the last word");

    m.select(Some(&key));
    m.run("audit.reopen");
    assert_eq!(m.badge(), Some(before));
}

#[test]
fn vanished_findings_become_fixed_and_reopen_on_return() {
    let mut m = module(vec![]);
    m.ingest(&built(1));
    let mut s = snap();
    s.outputs.retain(|o| o.path != "index.html");
    m.load(&s);
    m.ingest(&built(2));
    let fixed = m.list_len(Some("fixed"));
    assert!(fixed > 0);
    assert_eq!(m.refresh().unwrap().fixed as usize, fixed);
    assert_eq!(m.row(Some("fixed"), 0).tone, Tone::Ok);
    m.load(&snap());
    m.ingest(&built(3));
    assert_eq!(m.list_len(Some("fixed")), 0);
}

#[test]
fn unreadable_artifacts_degrade_to_not_checked() {
    let mut m = AuditModule::new(Box::new(MemoryPort::default()), default_ruleset());
    m.load(&snap());
    m.ingest(&built(1));
    assert!(m.list_len(Some("limited")) > 0);
    let k = m.row(Some("limited"), 0).key;
    let r = m.reading(&k);
    assert!(
        r.lines
            .iter()
            .any(|l| l.tone == Tone::Warn && l.text.contains("unreadable"))
    );
    m.select(Some(&k));
    assert!(matches!(&m.run("audit.snooze")[..], [Intent::Status(s)] if s.contains("not checked")));
}

#[test]
fn unreadable_rebuild_keeps_previous_findings_unresolved() {
    let mut m = module(vec![]);
    m.ingest(&built(1));
    let before = m.open_count();
    let key = first_open(&m);
    m.select(Some(&key));
    m.run("audit.snooze");
    let mut unavailable = snap();
    unavailable.artifact_root = None;
    m.load(&unavailable);
    m.ingest(&built(2));
    assert_eq!(m.list_len(Some("fixed")), 0);
    assert_eq!(m.open_count(), before - 1);
    assert_eq!(m.entry(&key).unwrap().state, FindingLifecycle::Ignored);
    assert!(m.list_len(Some("limited")) > 0);
    assert!(
        m.reading(&key)
            .lines
            .iter()
            .any(|l| l.text.contains("last good build"))
    );
    m.load(&snap());
    m.ingest(&built(3));
    assert_eq!(m.entry(&key).unwrap().state, FindingLifecycle::Ignored);
    assert_eq!(m.open_count(), before - 1);
}

#[test]
fn failed_build_marks_findings_stale() {
    let mut m = module(vec![]);
    m.ingest(&built(1));
    let k = first_open(&m);
    assert!(m.ingest(&env(
        2,
        EngineEvent::BuildFailed {
            phase: BuildPhase::Rebuild,
            diagnostics: vec![],
            recoverable: true,
        }
    )));
    assert!(
        m.reading(&k)
            .lines
            .iter()
            .any(|l| l.text.contains("last good build"))
    );
}

#[test]
fn default_ruleset_is_documented_packs() {
    let ids: Vec<_> = default_ruleset()
        .packs()
        .iter()
        .map(|p| p.meta.id.clone())
        .collect();
    assert_eq!(ids, DEFAULT_PACKS);
}
