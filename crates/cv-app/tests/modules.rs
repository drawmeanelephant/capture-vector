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
            // Rounded again: the tree's 0.1 pt values don't subtract exactly.
            let rel = |v: &Value, o: f64| ((v.as_f64().unwrap() - o) * 10.0).round() / 10.0;
            n["rect"] = serde_json::json!([rel(&r[0], ox), rel(&r[1], oy), r[2], r[3]]);
            n
        })
        .collect()
}

/// Absolute tree rects are rounded to 0.1 pt before `card` subtracts the
/// origin, so one card at two offsets can differ by a rounding step.
fn same_card(a: &[Value], b: &[Value]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| {
            let (mut x, mut y) = (x.clone(), y.clone());
            let (rx, ry) = (x["rect"].take(), y["rect"].take());
            let (rx, ry) = (rx.as_array().unwrap(), ry.as_array().unwrap());
            x == y
                && rx.len() == ry.len()
                && rx
                    .iter()
                    .zip(ry)
                    .all(|(p, q)| (p.as_f64().unwrap() - q.as_f64().unwrap()).abs() <= 0.11)
        })
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
    let (ca, cp) = (card(&a), card(&p));
    assert!(
        same_card(&ca, &cp),
        "same rendered subtree:\n{ca:?}\n{cp:?}"
    );
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
        "Draft sketch · advisory",
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

fn big(n: usize) -> Workbench {
    let mut pages = vec![PageRecord {
        path: "index.md".into(),
        id: "index".into(),
        title: "Index".into(),
        ..Default::default()
    }];
    pages.extend((1..n).map(|i| PageRecord {
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
    let mut sink = wb.sink();
    sink(failed(1, BuildPhase::Initial, river(n)));
    wb.drain();
    wb
}

/// `n` standing diagnostics, one per page: the diagnostics river.
fn river(n: usize) -> Vec<Diagnostic> {
    (0..n)
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
        .collect()
}

fn failed(seq: u64, phase: BuildPhase, diagnostics: Vec<Diagnostic>) -> Envelope {
    Envelope {
        cause: None,
        workspace: WorkspaceId("t".into()),
        snapshot_epoch: SnapshotEpoch(1),
        build_id: Some(BuildId(format!("b{seq}"))),
        cursor: Cursor {
            session: SessionId("t".into()),
            seq,
        },
        event_id: EventId(format!("big-{seq}")),
        payload: AppEvent::Engine(EngineEvent::BuildFailed {
            phase,
            diagnostics,
            recoverable: true,
        }),
    }
}

/// Scroll the active list for 120 frames; returns the sorted frame times.
fn scroll(module: &str, n: usize) -> Vec<Duration> {
    let mut wb = big(n);
    show(&mut wb, module);
    wb.set_nav("all");
    assert_eq!(
        wb.list_len(),
        n + usize::from(module == "activity"),
        "{module}"
    );
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
        h.rt.wheel(x, y, -40.0);
        t += 1.0 / 60.0;
        let start = Instant::now();
        h.rt.frame(t);
        frames.push(start.elapsed());
        assert!(mounted(&h) < 200, "{module}: bounded mount");
    }
    let first =
        h.rt.elements()
            .into_iter()
            .filter_map(|e| e.key)
            .find(|k| k.starts_with("row-"))
            .unwrap();
    if n > 1_000 {
        assert_ne!(
            first, "row-index.md",
            "{module}: scrolled away from the top"
        );
    }
    frames.sort();
    frames
}

/// The 60 Hz budget holds in optimized builds. Debug builds on shared CI
/// runners are too noisy for wall-clock budgets, so there the gate is
/// that a 5,000-row frame costs what a 500-row frame costs: frame time
/// is independent of list length.
#[test]
fn five_thousand_rows_scroll_at_frame_budget() {
    for module in ["pages", "activity"] {
        let big = scroll(module, 5_000);
        let p95 = big[big.len() * 95 / 100];
        let p50 = big[big.len() / 2];
        let small = scroll(module, 500);
        let base = small[small.len() / 2];
        eprintln!("{module}: 5k p50 {p50:?} p95 {p95:?}; 500-row p50 {base:?}");
        if cfg!(debug_assertions) {
            assert!(
                p50 < base * 2 + Duration::from_millis(5),
                "{module}: 5k-row frame {p50:?} vs 500-row {base:?} — cost scales with list length"
            );
        } else {
            let budget = Duration::from_micros(16_667);
            assert!(p95 < budget, "{module}: p95 frame {p95:?} over {budget:?}");
        }
    }
}

/// Activity mounted over an `n`-row river while watch rebuilds land in
/// bursts: every frame first receives `BURST` rebuild envelopes (each
/// re-reporting 50 standing diagnostics), then drains + lays out. Returns
/// sorted frame times (drain + layout).
fn rebuild_burst(n: usize) -> Vec<Duration> {
    const BURST: u64 = 8;
    let wb = big(n);
    let mut sink = wb.sink();
    let mut h = Harness::new(wb);
    show(h.state_mut(), "activity");
    h.state_mut().set_nav("all");
    let rows = h.state().list_len();
    let all = river(n);
    let mut seq = 2;
    let mut frames = Vec::new();
    let mut t = 1.0;
    for f in 0..120usize {
        for _ in 0..BURST {
            let at = (seq as usize * 50) % n;
            sink(failed(seq, BuildPhase::Rebuild, all[at..at + 50].to_vec()));
            seq += 1;
        }
        t += 1.0 / 60.0;
        let start = Instant::now();
        h.rt.frame(t);
        frames.push(start.elapsed());
        let mounted =
            h.rt.elements()
                .iter()
                .filter(|e| e.key.as_deref().is_some_and(|k| k.starts_with("row-")))
                .count();
        assert!(mounted < 200, "burst {f}: bounded mount ({mounted})");
    }
    assert!(!h.state_mut().drain(), "every burst was drained in-frame");
    // Rebuilds fold into one burst entry; re-reported diagnostics fold
    // into their open entry. The river does not grow with the bursts.
    assert_eq!(h.state().list_len(), rows + 1, "one rebuild-burst entry");
    frames.sort();
    frames
}

/// Gate: 5,000 pages + a 5,000-diagnostic river + rebuild bursts hold the
/// frame budget (same debug/release split as the scroll gate above).
#[test]
fn rebuild_bursts_over_5k_river_hold_frame_budget() {
    let big = rebuild_burst(5_000);
    let (p50, p95) = (big[big.len() / 2], big[big.len() * 95 / 100]);
    let small = rebuild_burst(500);
    let base = small[small.len() / 2];
    eprintln!("burst: 5k p50 {p50:?} p95 {p95:?}; 500-row p50 {base:?}");
    if cfg!(debug_assertions) {
        assert!(
            p50 < base * 2 + Duration::from_millis(5),
            "5k-river burst frame {p50:?} vs 500-row {base:?} — cost scales with river length"
        );
    } else {
        let budget = Duration::from_micros(16_667);
        assert!(p95 < budget, "burst p95 frame {p95:?} over {budget:?}");
    }
}

#[test]
fn dogfood_keeps_watch_diagnostics_and_offers_only_live_links() {
    let mut wb = init();
    open_broken(&mut wb, "dogfood");
    show(&mut wb, "pages");
    wb.set_nav("issues");
    assert!(wb.list_len() > 0, "watch ELAYOUT survives in Pages");
    show(&mut wb, "activity");
    wb.set_nav("all");
    let mut offered = 0;
    for i in 0..wb.list_len() {
        let key = wb.row(i).key;
        if let Some(l) = wb.link_for(&key) {
            offered += 1;
            let mut probe = init();
            open_broken(&mut probe, "dogfood");
            show(&mut probe, "activity");
            probe.set_nav("all");
            probe.select_key(&key);
            probe.follow_link();
            assert_eq!(
                probe.selected_key(),
                Some(l.key.as_str()),
                "link lands on a row"
            );
        }
    }
    assert!(offered < wb.list_len());
}

/// Every module's `index_of_key` agrees with the first-match row scan it
/// replaces, in every mailbox, over every scene and broken corpus.
#[test]
fn index_of_key_matches_the_row_scan_in_every_module_and_mailbox() {
    let mut cases: Vec<(String, Workbench)> = SCENES
        .iter()
        .map(|(name, f)| {
            let mut wb = init();
            f(&mut wb);
            (format!("scene {name}"), wb)
        })
        .collect();
    for corpus in BROKEN.iter().chain(&["dogfood"]) {
        let mut wb = init();
        open_broken(&mut wb, corpus);
        cases.push((format!("corpus {corpus}"), wb));
    }
    cases.push(("300-page river".into(), big(300)));
    for (case, wb) in &cases {
        for m in wb.registry.iter() {
            let id = m.spec().id;
            let mut navs: Vec<Option<String>> =
                m.nav().items.into_iter().map(|i| Some(i.key)).collect();
            navs.push(None);
            for nav in navs.iter().map(Option::as_deref) {
                let mut first = std::collections::HashMap::new();
                for i in 0..m.list_len(nav) {
                    first.entry(m.row(nav, i).key).or_insert(i);
                }
                for (k, &i) in &first {
                    assert_eq!(m.index_of_key(nav, k), Some(i), "{case}: {id} {nav:?} {k}");
                }
                if !first.contains_key("no/such-row.md") {
                    assert_eq!(m.index_of_key(nav, "no/such-row.md"), None, "{case}: {id}");
                }
            }
        }
    }
}
