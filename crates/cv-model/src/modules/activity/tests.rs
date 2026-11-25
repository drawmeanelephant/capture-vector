use super::*;
use cv_core::diagnostic::Span;
use cv_core::events::{BuildId, Cursor, EventId, SessionId, SnapshotEpoch};
use cv_core::id::WorkspaceId;

pub(crate) fn env(n: u64, payload: EngineEvent) -> Envelope {
    Envelope {
        workspace: WorkspaceId("t".into()),
        snapshot_epoch: SnapshotEpoch(0),
        build_id: Some(BuildId(format!("b{n}"))),
        cursor: Cursor {
            session: SessionId("s".into()),
            seq: n,
        },
        event_id: EventId(format!("e{n}")),
        payload: AppEvent::Engine(payload),
    }
}

pub(crate) fn diag(code: &str, path: &str, build: u64) -> Diagnostic {
    Diagnostic {
        code: code.into(),
        severity: Severity::Error,
        span: Some(Span {
            path: path.into(),
            start: 0,
            end: 0,
            line: 3,
            col: 1,
        }),
        message: format!("{code} happened"),
        build_version: Some(BuildId(format!("b{build}"))),
    }
}

fn failed(n: u64, phase: BuildPhase, d: Vec<Diagnostic>) -> Envelope {
    env(
        n,
        EngineEvent::BuildFailed {
            phase,
            diagnostics: d,
            recoverable: true,
        },
    )
}

fn ok(n: u64, phase: BuildPhase) -> Envelope {
    env(
        n,
        EngineEvent::BuildFinished {
            phase,
            pages_written: 3,
            diagnostics: vec![],
            duration_ms: 5,
        },
    )
}

#[test]
fn rebuild_bursts_coalesce_without_dropping_events() {
    let mut m = ActivityModule::default();
    let mut ids = BTreeSet::new();
    let mut feed = |m: &mut ActivityModule, e: Envelope| {
        ids.insert(e.event_id.0.clone());
        assert!(m.ingest(&e));
    };
    feed(
        &mut m,
        env(
            0,
            EngineEvent::WatchSessionStarted {
                schema: 1,
                compiler: "boris/0.9.0".into(),
            },
        ),
    );
    feed(&mut m, ok(1, BuildPhase::Initial));
    for n in 2..12 {
        feed(&mut m, ok(n, BuildPhase::Rebuild));
    }
    // session + initial + one coalesced rebuild burst
    assert_eq!(m.list_len(None), 3);
    let top = m.row(None, 0);
    assert_eq!(top.title, "Build succeeded (rebuild)");
    assert_eq!(top.badge.map(|c| c.text).as_deref(), Some("×10"));
    assert_eq!(m.accounted(), ids);
}

#[test]
fn repeated_ir_projection_outcomes_fold_without_dropping_events_or_hiding_fallback() {
    use cv_core::engine::ExitClass;

    let mut m = ActivityModule::default();
    let mut ids = BTreeSet::new();
    for n in 0..6 {
        for payload in [
            EngineEvent::BuildFinished {
                phase: BuildPhase::Initial,
                pages_written: 0,
                diagnostics: vec![],
                duration_ms: n,
            },
            EngineEvent::VerbFinished {
                verb: "ir".into(),
                exit: ExitClass::ContentFailure,
            },
        ] {
            let is_verb = matches!(&payload, EngineEvent::VerbFinished { .. });
            let e = env(n * 2 + u64::from(is_verb), payload);
            ids.insert(e.event_id.0.clone());
            m.ingest(&e);
        }
    }

    assert_eq!(m.entries().len(), 2);
    assert_eq!(m.accounted(), ids);
    let verb = m
        .entries()
        .iter()
        .find(|e| matches!(e.kind, EntryKind::Verb { .. }))
        .unwrap();
    assert_eq!(verb.occurrences.len(), 6);
    assert!(verb.title().contains("ir"));
    assert!(verb.detail().contains("ContentFailure"));
    assert!(
        verb.detail().contains("last published Boris IR")
            && verb.detail().contains("marked source fields"),
        "{}",
        verb.detail()
    );
}

#[test]
fn persisting_diagnostic_coalesces_and_keeps_latest_value() {
    let mut m = ActivityModule::default();
    m.ingest(&failed(
        1,
        BuildPhase::Initial,
        vec![diag("ELAYOUT", "a.md", 1)],
    ));
    m.ingest(&failed(
        2,
        BuildPhase::Rebuild,
        vec![diag("ELAYOUT", "a.md", 2)],
    ));
    m.ingest(&failed(
        3,
        BuildPhase::Rebuild,
        vec![diag("ELAYOUT", "a.md", 3)],
    ));
    let d: Vec<_> = m
        .entries()
        .iter()
        .filter(|e| matches!(e.kind, EntryKind::Diagnostic(_)))
        .collect();
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].occurrences.len(), 3);
    assert_eq!(d[0].kind, EntryKind::Diagnostic(diag("ELAYOUT", "a.md", 3)));
    // A green build closes it; a recurrence is a new entry.
    m.ingest(&ok(4, BuildPhase::Rebuild));
    m.ingest(&failed(
        5,
        BuildPhase::Rebuild,
        vec![diag("ELAYOUT", "a.md", 5)],
    ));
    // two failed builds (initial, rebuild burst), the late failure, two diagnostics
    assert_eq!(m.list_len(Some("errors")), 5);
    assert_eq!(m.accounted().len(), 5);
}

#[test]
fn severity_rails_file_groups_and_click_through() {
    let mut m = ActivityModule::default();
    let mut w = diag("EWARN", "b.md", 1);
    w.severity = Severity::Warning;
    m.ingest(&failed(
        1,
        BuildPhase::Initial,
        vec![diag("EPARENTMISSING", "a.md", 1), w],
    ));
    let nav: Vec<String> = m.nav().items.into_iter().map(|i| i.key).collect();
    assert!(nav.contains(&"file:a.md".to_string()) && nav.contains(&"file:b.md".to_string()));
    assert_eq!(m.list_len(Some("file:a.md")), 1);
    let r = m.row(Some("warnings"), 0);
    assert_eq!((r.tone, r.rail), (Tone::Warn, Some(Tone::Warn)));
    let key = m.row(Some("file:a.md"), 0).key;
    assert_eq!(m.row(Some("file:a.md"), 0).rail, Some(Tone::Error));
    let link = m.link(&key).unwrap();
    assert_eq!((link.module, link.key.as_str()), ("pages", "a.md"));
    assert_eq!(
        m.inspector(&key).diagnostics,
        vec![diag("EPARENTMISSING", "a.md", 1)]
    );
}

#[test]
fn attr_rows_project_every_entry() {
    let mut m = ActivityModule::default();
    m.ingest(&failed(
        1,
        BuildPhase::Initial,
        vec![diag("EFRONTMATTER", "index.md", 1)],
    ));
    m.ingest(&env(
        2,
        EngineEvent::MalformedLine {
            raw: "{oops".into(),
        },
    ));
    let rows = m.attr_rows();
    assert_eq!(rows.len(), m.entries().len());
    let d = rows
        .iter()
        .find(|r| r.noun.kind.as_str() == noun_kind::DIAGNOSTIC)
        .unwrap();
    assert_eq!(d.fields["code"], AttrValue::Text("EFRONTMATTER".into()));
    assert_eq!(d.fields["path"], AttrValue::Text("index.md".into()));
}

#[test]
fn five_thousand_entries_stay_indexed() {
    let mut m = ActivityModule::default();
    let diags = (0..5_000)
        .map(|i| diag("ECODE", &format!("p{i:04}.md"), 1))
        .collect();
    m.ingest(&failed(1, BuildPhase::Initial, diags));
    assert_eq!(m.list_len(Some("errors")), 5_001);
    assert_eq!(m.row(None, 0).title, "ECODE: ECODE happened");
    let nav = m.nav().items;
    assert_eq!(
        nav.len(),
        4 + MAX_FILE_GROUPS + 1,
        "file groups stay bounded"
    );
    assert_eq!(m.list_len(Some(&nav.last().unwrap().key)), 5_000);
    assert_eq!(
        m.list_len(Some("file:p4999.md")),
        1,
        "every file group stays addressable"
    );
}

/// `index_of_key` agrees with the first-match row scan in every mailbox
/// (plus the unfiltered and an unknown one), for row keys and `extra`.
pub(crate) fn assert_index_matches_scan(m: &dyn Module, extra: &[&str]) {
    let mut navs: Vec<Option<String>> = m.nav().items.into_iter().map(|i| Some(i.key)).collect();
    navs.extend([None, Some("no-such-mailbox".into())]);
    for nav in navs.iter().map(Option::as_deref) {
        let keys: Vec<String> = (0..m.list_len(nav)).map(|i| m.row(nav, i).key).collect();
        for k in keys.iter().map(String::as_str).chain(extra.iter().copied()) {
            let scan = keys.iter().position(|x| x == k);
            assert_eq!(m.index_of_key(nav, k), scan, "{nav:?} / {k:?}");
        }
    }
}

#[test]
fn index_of_key_matches_the_row_scan_newest_first() {
    let mut m = ActivityModule::default();
    m.ingest(&env(
        0,
        EngineEvent::WatchSessionStarted {
            schema: 1,
            compiler: "boris/0.9.0".into(),
        },
    ));
    m.ingest(&failed(1, BuildPhase::Initial, vec![diag("E1", "a.md", 1)]));
    m.ingest(&failed(2, BuildPhase::Rebuild, vec![diag("E2", "b.md", 2)]));
    m.ingest(&failed(3, BuildPhase::Rebuild, vec![diag("E2", "b.md", 3)]));
    m.ingest(&ok(4, BuildPhase::Rebuild));
    assert_index_matches_scan(&m, &["act-0", "act-+0", "act-00099", "act-", "", "x"]);
    assert_eq!(m.index_of_key(None, &m.row(None, 0).key), Some(0));
}

#[test]
fn nav_cache_refreshes_when_an_entry_lands() {
    let all = |n: &NavModel| n.items.iter().find(|i| i.key == ALL).unwrap().count;
    let mut m = ActivityModule::default();
    m.ingest(&failed(1, BuildPhase::Initial, vec![diag("E1", "a.md", 1)]));
    let before = m.nav();
    assert_eq!(all(&before), Some(2));
    m.ingest(&failed(2, BuildPhase::Rebuild, vec![diag("E2", "b.md", 2)]));
    let after = m.nav();
    assert_eq!(all(&after), Some(4));
    assert!(after.items.iter().any(|i| i.key == "file:b.md"));
    assert_eq!(after, m.build_nav());
    // A folding rebuild changes no mailbox; the cached model still holds.
    m.ingest(&failed(3, BuildPhase::Rebuild, vec![diag("E2", "b.md", 3)]));
    assert_eq!(m.nav(), after);
    assert_eq!(m.nav(), m.build_nav());
}
