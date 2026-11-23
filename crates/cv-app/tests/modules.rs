//! L3 content modules mounted in the shell: fixture replay into Pages /
//! Outputs / Activity, shared diagnostic rendering, click-through, and the
//! 5,000-row scroll-at-frame-budget gate.

use cv_app::{BROKEN, SCENES, init, open_broken, snapshot};
use cv_core::diagnostic::{Diagnostic, Severity, Span};
use cv_core::events::engine::{BuildPhase, EngineEvent};
use cv_core::events::{AppEvent, BuildId, Cursor, Envelope, EventId, SessionId, SnapshotEpoch};
use cv_core::id::WorkspaceId;
use cv_model::module::{PageRecord, ProjectSnapshot};
use cv_model::workbench::Workbench;
use cv_widgets::harness::Harness;
use serde_json::Value;
use std::time::{Duration, Instant};

fn scene(name: &str) -> Harness {
    let (_, f) = SCENES.iter().find(|(n, _)| *n == name).unwrap();
    let mut wb = init();
    f(&mut wb);
    Harness::new(wb)
}

fn show(wb: &mut Workbench, id: &str) {
    let i = wb.registry.index_of(id).unwrap();
    wb.activate(i);
}

/// The diagnostic card subtree under `diag-0`, positions made relative.
fn card(h: &Harness) -> Vec<Value> {
    let tree = h.tree();
    let nodes = tree["nodes"].as_array().unwrap();
    let root = nodes
        .iter()
        .find(|n| n["key"] == "diag-0")
        .expect("diagnostic card mounted");
    let path = root["path"].as_str().unwrap().to_string();
    let (ox, oy) = (
        root["rect"][0].as_f64().unwrap(),
        root["rect"][1].as_f64().unwrap(),
    );
    nodes
        .iter()
        .filter(|n| n["path"].as_str().unwrap().starts_with(&path))
        .map(|n| {
            let mut n = n.clone();
            let p = n["path"].as_str().unwrap()[path.len()..].to_string();
            n["path"] = p.into();
            let r = n["rect"].as_array().unwrap().clone();
            n["rect"] = serde_json::json!([
                r[0].as_f64().unwrap() - ox,
                r[1].as_f64().unwrap() - oy,
                r[2],
                r[3]
            ]);
            n
        })
        .collect()
}

#[test]
fn every_broken_corpus_surfaces_its_boris_diagnostic_in_pages_and_activity() {
    for corpus in BROKEN {
        let mut wb = init();
        open_broken(&mut wb, corpus);
        show(&mut wb, "pages");
        wb.set_nav("issues");
        assert!(wb.list_len() > 0, "{corpus}: no page with diagnostics");
        wb.select_row(0);
        let key = wb.selected_key().unwrap().to_string();
        let pages = wb
            .registry
            .get(wb.active)
            .unwrap()
            .inspector(&key)
            .diagnostics;
        // ETEXTILE has no path:line:col, so neither engine adapter locates
        // it; Pages still discloses the failed build.
        if *corpus == "broken-textile" {
            assert_eq!(key, "(project)");
            assert_eq!(wb.row(0).tone, cv_model::module::Tone::Error);
            continue;
        }
        assert!(!pages.is_empty(), "{corpus}");
        show(&mut wb, "activity");
        wb.set_nav("errors");
        let act: Vec<Diagnostic> = (0..wb.list_len())
            .filter_map(|i| {
                let k = wb.row(i).key;
                wb.registry
                    .get(wb.active)
                    .unwrap()
                    .inspector(&k)
                    .diagnostics
                    .into_iter()
                    .next()
            })
            .collect();
        for d in &pages {
            assert!(
                act.contains(d),
                "{corpus}: {} missing from Activity",
                d.code
            );
        }
    }
}

#[test]
fn same_diagnostic_renders_identically_in_activity_and_pages() {
    let a = scene("activity-broken");
    let p = scene("broken-parent");
    let ka = a.state().selected_key().unwrap().to_string();
    let kp = p.state().selected_key().unwrap().to_string();
    let da = a
        .state()
        .registry
        .get(a.state().active)
        .unwrap()
        .inspector(&ka)
        .diagnostics;
    let dp = p
        .state()
        .registry
        .get(p.state().active)
        .unwrap()
        .inspector(&kp)
        .diagnostics;
    assert_eq!(da, dp, "same Diagnostic value");
    assert_eq!(card(&a), card(&p), "same rendered subtree");
}

#[test]
fn activity_click_through_reveals_the_page() {
    let mut h = scene("activity-broken");
    assert!(h.click("follow-link"));
    let s = h.state();
    assert_eq!(s.registry.get(s.active).unwrap().spec().id, "pages");
    assert_eq!(s.selected_key(), Some("orphan.md"));
    assert!(s.app.selection.noun.is_some());
    assert!(h.has("diag-0"));
    // and back out to the output from a happy page
    let mut h = scene("pages");
    h.script(&["key:mod+o"]);
    let s = h.state();
    assert_eq!(s.registry.get(s.active).unwrap().spec().id, "outputs");
    assert_eq!(s.selected_key(), Some("guides/publishing.html"));
}

#[test]
fn happy_snapshot_projects_pages_and_outputs() {
    let snap = snapshot::corpus_snapshot("happy");
    assert_eq!(snap.pages.len(), 3);
    assert_eq!(snap.compiler.as_deref(), Some("boris/0.9.0"));
    let h = scene("pages");
    let texts = h.texts();
    for t in [
        "My Boris Site",
        "Publishing",
        "Getting Started",
        "Draft sketch",
    ] {
        assert!(texts.iter().any(|x| x == t), "{t}");
    }
    assert!(h.has("breadcrumbs") && h.has("sketch"));
    let h = scene("outputs");
    assert!(h.texts().iter().any(|t| t == "boris/0.9.0"));
    assert!(h.has("nav-proof") && h.has("nav-llms"));
}

#[test]
fn empty_mid_watch_and_degraded_states() {
    let h = scene("empty");
    assert!(h.has("list-empty"));
    let h = scene("mid-watch");
    let texts = h.texts();
    assert!(texts.iter().any(|t| t == "Watch session started"));
    assert!(texts.iter().any(|t| t == "Build succeeded (initial)"));
    assert!(
        !texts
            .iter()
            .any(|t| t.starts_with("Build succeeded (rebuild)"))
    );
    for s in [
        "pages-engine-absent",
        "pages-version-mismatch",
        "pages-watch-dead",
        "pages-store-corrupt",
    ] {
        let h = scene(s);
        assert!(h.has("banner"), "{s}");
        assert!(h.state().list_len() > 0, "{s}: last snapshot stays visible");
    }
}

fn big() -> Workbench {
    let mut pages = vec![PageRecord {
        path: "index.md".into(),
        id: "index".into(),
        title: "Index".into(),
        ..Default::default()
    }];
    pages.extend((1..5_000).map(|i| PageRecord {
        path: format!("p/{i:04}.md"),
        id: format!("p/{i:04}"),
        title: format!("Page {i:04}"),
        parent: Some("index".into()),
        ..Default::default()
    }));
    let mut wb = init();
    wb.load_snapshot(&ProjectSnapshot {
        epoch: 1,
        pages,
        ..Default::default()
    });
    let diagnostics = (0..5_000)
        .map(|i| Diagnostic {
            code: "EREFERENCEMISSING".into(),
            severity: if i % 3 == 0 {
                Severity::Warning
            } else {
                Severity::Error
            },
            span: Some(Span {
                path: format!("p/{i:04}.md"),
                start: 0,
                end: 0,
                line: 1,
                col: 1,
            }),
            message: format!("missing reference {i}"),
            build_version: Some(BuildId("b1".into())),
        })
        .collect();
    let mut sink = wb.sink();
    sink(Envelope {
        workspace: WorkspaceId("t".into()),
        snapshot_epoch: SnapshotEpoch(1),
        build_id: Some(BuildId("b1".into())),
        cursor: Cursor {
            session: SessionId("t".into()),
            seq: 1,
        },
        event_id: EventId("big-1".into()),
        payload: AppEvent::Engine(EngineEvent::BuildFailed {
            phase: BuildPhase::Initial,
            diagnostics,
            recoverable: true,
        }),
    });
    wb.drain();
    wb
}

/// Debug builds (what CI runs) get headroom over the 60 Hz budget.
const BUDGET: Duration = if cfg!(debug_assertions) {
    Duration::from_millis(50)
} else {
    Duration::from_micros(16_667)
};

#[test]
fn five_thousand_rows_scroll_at_frame_budget() {
    for (module, nav, len) in [("pages", "all", 5_000), ("activity", "all", 5_001)] {
        let mut wb = big();
        show(&mut wb, module);
        wb.set_nav(nav);
        assert_eq!(wb.list_len(), len, "{module}");
        let mut h = Harness::new(wb);
        let mounted = |h: &Harness| {
            h.rt.elements()
                .iter()
                .filter(|e| e.key.as_deref().is_some_and(|k| k.starts_with("row-")))
                .count()
        };
        let (x, y) = h.rt.find("list-viewport").unwrap();
        let mut frames = Vec::new();
        let mut t = 1.0;
        for _ in 0..120 {
            h.rt.wheel(x, y, -240.0);
            t += 1.0 / 60.0;
            let start = Instant::now();
            h.rt.frame(t);
            frames.push(start.elapsed());
            assert!(mounted(&h) < 200, "{module}: bounded mount");
        }
        frames.sort();
        let p50 = frames[frames.len() / 2];
        let p95 = frames[frames.len() * 95 / 100];
        eprintln!("{module}: p50 {p50:?} p95 {p95:?}");
        assert!(p95 < BUDGET, "{module}: p95 frame {p95:?} over {BUDGET:?}");
        let first =
            h.rt.elements()
                .into_iter()
                .filter_map(|e| e.key)
                .find(|k| k.starts_with("row-"))
                .unwrap();
        assert_ne!(
            first, "row-index.md",
            "{module}: scrolled away from the top"
        );
    }
}
