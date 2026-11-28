//! Headless scale and interaction gates for the bounded graph viewport.

use cv_model::graph_scale::{SCALE_NODES, SCALE_RELATIONS, page_id, synthetic_scale_graph};
use cv_model::module::{ProjectSnapshot, Registry};
use cv_model::viewport::MAX_MOUNTED_NODES;
use cv_model::workbench::Workbench;
use cv_widgets::harness::Harness;
use rune::{MagnifyEvent, WheelEvent};
use std::sync::Arc;

fn scale_workbench() -> Workbench {
    let mut wb = Workbench::new(Registry::with_all());
    wb.load_snapshot(&ProjectSnapshot {
        epoch: 1,
        ir_graph: Some(Arc::new(synthetic_scale_graph())),
        ..Default::default()
    });
    let graph = wb
        .registry
        .iter()
        .position(|m| m.spec().id == cv_model::modules::graph::ID)
        .expect("graph module registered");
    wb.activate(graph);
    wb
}

fn graph_keys(h: &Harness) -> Vec<String> {
    h.rt.elements()
        .into_iter()
        .filter_map(|e| e.key)
        .filter(|k| k.starts_with("graph-node:") || k.starts_with("graph-cluster:"))
        .collect()
}

fn overlap(a: [f32; 4], b: [f32; 4]) -> bool {
    a[0] < b[0] + b[2] && a[0] + a[2] > b[0] && a[1] < b[1] + b[3] && a[1] + a[3] > b[1]
}

#[test]
fn corpus_scale_mounts_a_bounded_element_tree_with_explainable_disclosure() {
    let mut h = Harness::new(scale_workbench());
    h.settle();

    let keys = graph_keys(&h);
    assert!(
        keys.len() <= MAX_MOUNTED_NODES,
        "mounted graph node/cluster elements stay bounded at scale: {}",
        keys.len()
    );
    assert!(
        keys.iter().any(|k| k.starts_with("graph-cluster:")),
        "the default corpus overview aggregates instead of mounting 5k nodes: {keys:?}"
    );
    let total = h.rt.elements().len();
    assert!(
        total < 512,
        "shell chrome plus the graph stays bounded at {SCALE_NODES} nodes/{SCALE_RELATIONS} relations: {total}"
    );

    let model = h.state().active_viewport().expect("graph viewport model");
    assert_eq!(model.hidden.total, SCALE_NODES);
    assert!(model.hidden.culled + model.hidden.clustered_nodes + model.hidden.capped > 0);
    assert!(model.status.contains("outside view") || model.status.contains("folded"));
    assert!(model.status.contains("reveal"));

    h.state_mut()
        .viewport_input(cv_model::viewport::ViewportInput::Resize {
            width: 5_000.0,
            height: 5_000.0,
        });
    h.state_mut()
        .viewport_input(cv_model::viewport::ViewportInput::Zoom {
            factor: 4.9,
            x: 2_500.0,
            y: 2_500.0,
        });
    h.settle();
    let huge = h.state().active_viewport().expect("huge viewport model");
    assert!(
        huge.nodes.len() <= MAX_MOUNTED_NODES,
        "even an oversized viewport mounts at most {MAX_MOUNTED_NODES} nodes/clusters: {}",
        huge.nodes.len()
    );
    assert!(
        huge.hidden.capped > 0,
        "oversized-viewport overflow is disclosed as capped, not silently dropped"
    );
}

#[test]
fn aggregate_opening_and_camera_input_are_deterministic_and_reveal_more_detail() {
    let mut h = Harness::new(scale_workbench());
    h.settle();
    let first = h
        .state()
        .active_viewport()
        .expect("viewport")
        .nodes
        .iter()
        .map(|n| (n.key.clone(), n.center, n.cluster_count))
        .collect::<Vec<_>>();
    let mut second = Harness::new(scale_workbench());
    second.settle();
    let repeat = second
        .state()
        .active_viewport()
        .expect("viewport")
        .nodes
        .iter()
        .map(|n| (n.key.clone(), n.center, n.cluster_count))
        .collect::<Vec<_>>();
    assert_eq!(first, repeat, "same graph and camera produce the same LOD");

    let before = h.state().active_viewport().expect("viewport").camera.zoom;
    let cluster = graph_keys(&h)
        .into_iter()
        .find(|k| k.starts_with("graph-cluster:"))
        .expect("overview has a cluster to open");
    assert!(h.click(&cluster), "cluster element is clickable");
    let after_open = h.state().active_viewport().expect("viewport");
    assert!(
        after_open.camera.zoom > before,
        "opening a cluster zooms into its bounded region: {before} -> {}",
        after_open.camera.zoom
    );
    assert!(after_open.hidden.individual > 0);

    let (x, y) = h.rt.find("graph-canvas").expect("canvas mounted");
    let center_before = h.state().active_viewport().expect("viewport").camera.center;
    h.rt.wheel_event(WheelEvent::new(30.0, 40.0).at(x, y).precise(true));
    h.settle();
    let center_after = h.state().active_viewport().expect("viewport").camera.center;
    assert_ne!(
        center_before, center_after,
        "wheel/trackpad input pans the camera"
    );

    let zoom_before = h.state().active_viewport().expect("viewport").camera.zoom;
    h.rt.magnify_event(MagnifyEvent::new(0.5).at(x, y));
    h.settle();
    let zoom_after = h.state().active_viewport().expect("viewport").camera.zoom;
    assert!(
        zoom_after > zoom_before,
        "pinch/ctrl-wheel input zooms the camera"
    );
}

#[test]
fn dense_hub_neighborhood_keeps_direct_satellites_inspectable_without_clusters() {
    let mut wb = scale_workbench();
    wb.set_nav("neighborhood");
    wb.select_key(&page_id(0));
    let mut h = Harness::new(wb);
    h.settle();

    let els = h.rt.elements();
    let satellite_keys: Vec<String> = (1..=60)
        .map(|i| format!("graph-node:{}", page_id(i)))
        .filter(|key| els.iter().any(|e| e.key.as_deref() == Some(key)))
        .collect();
    assert!(
        satellite_keys.len() >= 8,
        "several of the trunk's 60 direct satellites are inspectable at once and the rest are pannable: {}",
        satellite_keys.len()
    );
    assert!(
        !els.iter().any(|e| e
            .key
            .as_deref()
            .is_some_and(|k| k.starts_with("graph-cluster:"))),
        "focused detail must not collapse the hub into aggregate labels"
    );

    let mut labelled: Vec<[f32; 4]> = els
        .iter()
        .filter(|e| {
            e.key
                .as_deref()
                .is_some_and(|k| k.starts_with("graph-node:"))
        })
        .map(|e| e.rect)
        .filter(|r| r[2] > 30.0)
        .collect();
    labelled.sort_by(|a, b| (a[1], a[0]).partial_cmp(&(b[1], b[0])).unwrap());
    for (i, a) in labelled.iter().enumerate() {
        for b in &labelled[i + 1..] {
            assert!(
                !overlap(*a, *b),
                "dense-hub labels must not collapse into one unreadable cluster: {a:?} overlaps {b:?}"
            );
        }
    }

    let disclosure = h
        .texts()
        .into_iter()
        .find(|t| t.contains("shown"))
        .expect("viewport disclosure text");
    assert!(disclosure.contains("outside view") || disclosure.contains("shown"));
}
