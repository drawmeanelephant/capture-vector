//! Headless scale evidence for the graph navigation surface at the #228
//! floor: 5,000 nodes / 30,000 semantic relations / a 60-satellite trunk.
//! The input is deterministic in-memory synthetic data, not a Boris capture
//! or a hand-authored Boris JSON fixture.

use cv_app::init;
use cv_model::dialog::{DialogAnswer, Value};
use cv_model::module::ProjectSnapshot;
use cv_model::modules::graph::SEARCH_COMMAND;
use cv_model::viewport::MAX_MOUNTED_NODES;
use cv_model::workbench::Workbench;
use cv_widgets::harness::Harness;
use cv_widgets::keys;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

const FRAMES: usize = 90;
const SEARCHES: usize = 30;
const FOCUS_CHANGES: usize = 40;

fn graph_workbench(nodes: usize) -> Workbench {
    let graph = cv_fixtures::graph_scale::synthetic_graph(nodes);
    let mut wb = init();
    wb.load_snapshot(&ProjectSnapshot {
        epoch: 1,
        ir_graph: Some(Arc::new(graph)),
        ..Default::default()
    });
    let graph_index = wb.registry.index_of("graph").expect("graph registered");
    wb.activate(graph_index);
    wb.set_nav("corpus");
    wb
}

fn mounted_graph_rows(h: &Harness) -> usize {
    h.rt.elements()
        .iter()
        .filter(|e| {
            e.key
                .as_deref()
                .is_some_and(|key| key.starts_with("row-page-"))
        })
        .count()
}

fn mounted_viewport_nodes(h: &Harness) -> usize {
    h.rt.elements()
        .iter()
        .filter(|e| {
            e.key.as_deref().is_some_and(|key| {
                key.starts_with("graph-node:") || key.starts_with("graph-cluster:")
            })
        })
        .count()
}

fn assert_viewport_bounded(h: &Harness) {
    let mounted = mounted_viewport_nodes(h);
    assert!(
        mounted <= MAX_MOUNTED_NODES,
        "viewport mounts {mounted} nodes/clusters, over cap {MAX_MOUNTED_NODES}"
    );
    assert!(
        h.rt.elements().len() < 700,
        "shell plus viewport element tree stays bounded: {}",
        h.rt.elements().len()
    );
}

fn graph_node_keys(h: &Harness) -> BTreeSet<String> {
    h.rt.elements()
        .into_iter()
        .filter_map(|e| e.key)
        .filter(|key| key.starts_with("graph-node:"))
        .collect()
}

fn overlap(a: [f32; 4], b: [f32; 4]) -> bool {
    a[0] < b[0] + b[2] && a[0] + a[2] > b[0] && a[1] < b[1] + b[3] && a[1] + a[3] > b[1]
}

fn assert_visible_labels_do_not_overlap(h: &Harness) {
    let mut labelled: Vec<[f32; 4]> =
        h.rt.elements()
            .into_iter()
            .filter(|e| {
                e.key
                    .as_deref()
                    .is_some_and(|key| key.starts_with("graph-node:"))
            })
            .map(|e| e.rect)
            .filter(|rect| rect[2] > 30.0)
            .collect();
    labelled.sort_by(|a, b| (a[1], a[0]).partial_cmp(&(b[1], b[0])).unwrap());
    for (i, a) in labelled.iter().enumerate() {
        for b in &labelled[i + 1..] {
            assert!(
                !overlap(*a, *b),
                "visible dense-hub labels overlap: {a:?} intersects {b:?}"
            );
        }
    }
}

fn percentile(sorted: &[Duration], numerator: usize) -> Duration {
    sorted[sorted.len() * numerator / 100]
}

fn report(label: &str, mut times: Vec<Duration>) -> (Duration, Duration) {
    times.sort();
    let (min, p50, p95, max) = (
        times[0],
        percentile(&times, 50),
        percentile(&times, 95),
        times[times.len() - 1],
    );
    eprintln!(
        "graph-scale {label}: p50 {p50:?}, p95 {p95:?}, range {min:?}..{max:?}, {} runs",
        times.len()
    );
    (p50, p95)
}

fn scroll_cost(nodes: usize) -> Vec<Duration> {
    let mut h = Harness::new(graph_workbench(nodes));
    assert_eq!(h.state().list_len(), nodes);
    assert!(mounted_graph_rows(&h) > 0);
    assert!(mounted_graph_rows(&h) < 200);
    let (x, y) = h.rt.find("list-viewport").expect("graph list viewport");
    let mut times = Vec::new();
    let mut t = 1.0;
    for _ in 0..FRAMES {
        h.rt.wheel(x, y, -40.0);
        t += 1.0 / 60.0;
        let start = Instant::now();
        h.rt.frame(t);
        times.push(start.elapsed());
        assert!(mounted_graph_rows(&h) < 200, "scroll mounts stay bounded");
    }
    assert_ne!(
        h.rt.elements()
            .into_iter()
            .filter_map(|e| e.key)
            .find(|key| key.starts_with("row-page-"))
            .as_deref(),
        Some("row-page-00000"),
        "the scroll input moved through the corpus"
    );
    times
}

fn answer_search(h: &mut Harness, needle: &str) {
    let mut values = BTreeMap::new();
    values.insert("page".to_string(), Value::Text(needle.into()));
    let answer = DialogAnswer {
        action: Some("apply".into()),
        context: None,
        values,
        secret: None,
    };
    let index = h
        .state()
        .registry
        .index_of("graph")
        .expect("graph registered");
    let intents = h
        .state_mut()
        .registry
        .get_mut(index)
        .expect("graph module")
        .answer(SEARCH_COMMAND, &answer);
    for intent in intents {
        h.state_mut().apply(intent);
    }
}

fn pan_cost(nodes: usize) -> Vec<Duration> {
    let mut h = Harness::new(graph_workbench(nodes));
    assert_viewport_bounded(&h);
    let (x, y) = h.rt.find("graph-canvas").expect("graph canvas mounted");
    let initial = h.state().active_viewport().expect("viewport").camera.center;
    let mut times = Vec::new();
    let mut t = 1.0;
    for _ in 0..FRAMES {
        h.rt.wheel_xy(x, y, 36.0, 24.0);
        t += 1.0 / 60.0;
        let start = Instant::now();
        h.rt.frame(t);
        times.push(start.elapsed());
        assert_viewport_bounded(&h);
    }
    assert_ne!(
        h.state().active_viewport().expect("viewport").camera.center,
        initial,
        "wheel/trackpad input panned the graph camera"
    );
    times
}

fn zoom_cost(nodes: usize) -> Vec<Duration> {
    let mut h = Harness::new(graph_workbench(nodes));
    assert_viewport_bounded(&h);
    assert!(h.rt.find("graph-control-in").is_some());
    assert!(h.rt.find("graph-control-out").is_some());
    let mut times = Vec::new();
    let mut t = 1.0;
    let mut changed = 0usize;
    for i in 0..SEARCHES {
        let key = if i < SEARCHES / 2 {
            "graph-control-in"
        } else {
            "graph-control-out"
        };
        let (x, y) = h.rt.find(key).expect("zoom control mounted");
        let before = h.state().active_viewport().expect("viewport").camera.zoom;
        h.rt.pointer_down(x, y);
        h.rt.pointer_up(x, y);
        t += 1.0 / 60.0;
        let start = Instant::now();
        h.rt.frame(t);
        times.push(start.elapsed());
        let after = h.state().active_viewport().expect("viewport").camera.zoom;
        changed += usize::from((after - before).abs() > f32::EPSILON);
        assert_viewport_bounded(&h);
    }
    assert_eq!(
        changed, SEARCHES,
        "every zoom control input changed the camera"
    );
    times
}

fn search_cost(nodes: usize) -> Vec<Duration> {
    let mut h = Harness::new(graph_workbench(nodes));
    let mut times = Vec::new();
    let mut t = 1.0;
    for i in 0..SEARCHES {
        let needle = if i % 2 == 0 { "page-004" } else { "page-000" };
        t += 1.0 / 60.0;
        let start = Instant::now();
        answer_search(&mut h, needle);
        h.rt.frame(t);
        times.push(start.elapsed());
        assert_eq!(h.state().active_nav(), Some("search"));
        assert!(
            h.state().list_len() > 0,
            "search matched pages rather than exiting early"
        );
        assert!(mounted_graph_rows(&h) < 200, "search mounts stay bounded");
    }
    times
}

fn focus_cost(nodes: usize) -> Vec<Duration> {
    let mut h = Harness::new(graph_workbench(nodes));
    h.state_mut().select_key("page-00000");
    h.settle();
    let mut times = Vec::new();
    let mut t = 1.0;
    for i in 0..FOCUS_CHANGES {
        let token = if i % 2 == 0 { "right" } else { "left" };
        t += 1.0 / 60.0;
        let start = Instant::now();
        for event in keys::parse(token) {
            h.rt.key(event);
        }
        h.rt.frame(t);
        times.push(start.elapsed());
        let want = if i % 2 == 0 {
            "page-00001"
        } else {
            "page-00000"
        };
        assert_eq!(
            h.state().selected_key(),
            Some(want),
            "focus changed through graph hierarchy"
        );
        assert!(mounted_graph_rows(&h) < 200, "focus mounts stay bounded");
        assert_viewport_bounded(&h);
    }
    times
}

fn assert_cost(label: &str, big: Vec<Duration>, small: Vec<Duration>, linear: bool) {
    let (big_p50, big_p95) = report(label, big);
    let (small_p50, _) = report(&format!("{label} 500-node baseline"), small);
    if cfg!(debug_assertions) {
        if linear {
            // Search necessarily scans the corpus. The debug gate rejects worse
            // than linear behavior while allowing the exact 10x size ratio.
            assert!(
                big_p50 < small_p50 * 24 + Duration::from_millis(5),
                "{label}: 5k cost {big_p50:?} vs 500-node {small_p50:?} suggests superlinear work"
            );
        } else {
            assert!(
                big_p50 < small_p50 * 2 + Duration::from_millis(5),
                "{label}: 5k frame {big_p50:?} vs 500-node {small_p50:?} — cost scales with corpus length"
            );
        }
    } else {
        let budget = Duration::from_micros(16_667);
        assert!(
            big_p95 < budget,
            "{label}: p95 input+frame cost {big_p95:?} over {budget:?}"
        );
    }
}

#[test]
fn dense_trunk_neighborhood_at_scale_stays_bounded_and_inspectable() {
    let mut wb = graph_workbench(5_000);
    wb.set_nav("neighborhood");
    wb.select_key("page-00000");
    let mut h = Harness::new(wb);
    h.settle();

    assert_viewport_bounded(&h);
    let keys = graph_node_keys(&h);
    let satellite_keys = |keys: &BTreeSet<String>| {
        (1..=cv_fixtures::graph_scale::TRUNK_DIRECT_SATELLITES)
            .map(|i| format!("graph-node:{}", cv_fixtures::graph_scale::page_id(i)))
            .filter(|key| keys.contains(key))
            .collect::<BTreeSet<_>>()
    };
    let before_satellites = satellite_keys(&keys);
    assert!(
        before_satellites.len() >= 8,
        "several of the trunk's 60 direct satellites are inspectable at once; the rest are pannable: {}",
        before_satellites.len()
    );
    assert!(
        h.rt.elements().iter().all(|e| {
            !e.key
                .as_deref()
                .is_some_and(|key| key.starts_with("graph-cluster:"))
        }),
        "focused detail must not collapse the dense hub into aggregate clusters"
    );
    assert_visible_labels_do_not_overlap(&h);
    let model = h.state().active_viewport().expect("graph viewport model");
    assert!(
        model.status.contains("shown"),
        "viewport disclosure says what is shown: {}",
        model.status
    );

    let before_center = model.camera.center;
    let before_keys = keys;
    let (x, y) = h.rt.find("graph-canvas").expect("graph canvas mounted");
    h.rt.wheel_xy(x, y, -360.0, 0.0);
    h.settle();
    assert_viewport_bounded(&h);
    assert_visible_labels_do_not_overlap(&h);
    let after = h.state().active_viewport().expect("graph viewport model");
    assert_ne!(
        after.camera.center, before_center,
        "panning the dense hub moves the camera"
    );
    let after_keys = graph_node_keys(&h);
    assert_ne!(
        after_keys, before_keys,
        "panning the dense hub reveals a different bounded window"
    );
    let after_satellites = satellite_keys(&after_keys);
    assert!(
        after_satellites
            .iter()
            .any(|key| !before_satellites.contains(key)),
        "panning reveals previously hidden direct satellites"
    );
}

#[test]
fn graph_scale_surface_interactions_have_bounded_mounts_and_frame_evidence() {
    let floor = cv_fixtures::graph_scale::synthetic_scale_graph();
    assert_eq!(floor.nodes.len(), 5_000);
    assert_eq!(floor.relations.len(), 30_000);
    let trunk_children = floor
        .nav
        .iter()
        .find(|entry| entry.id == "page-00000")
        .expect("trunk nav")
        .children
        .len();
    assert_eq!(trunk_children, 60);

    assert_cost("scroll", scroll_cost(5_000), scroll_cost(500), false);
    assert_cost("pan", pan_cost(5_000), pan_cost(500), true);
    assert_cost("zoom", zoom_cost(5_000), zoom_cost(500), true);
    assert_cost("search", search_cost(5_000), search_cost(500), true);
    assert_cost("focus", focus_cost(5_000), focus_cost(500), false);
    eprintln!(
        "graph-scale omissions: headless input/layout CPU measurements leave out GPU upload, \
         compositor presentation, OS input delivery, and real Boris capture cost. Pan uses \
         wheel/trackpad input on the canvas; zoom uses the mounted zoom controls across aggregate, compact and detail LODs. Camera planning \
         scans the immutable corpus projection linearly; mounted elements stay bounded. The corpus is \
         deterministic synthetic in-memory input, not a Boris capture."
    );
}
