//! Pane chrome: depth order, sticky headers, the inspector's edge
//! transition, the advisory sketch frame and centred empty states.

use cv_app::{SCENES, init};
use cv_model::theme::Rgba;
use cv_widgets::harness::Harness;
use cv_widgets::keys;

fn scene(name: &str) -> Harness {
    let (_, f) = SCENES
        .iter()
        .find(|(n, _)| *n == name)
        .unwrap_or_else(|| panic!("no scene {name}"));
    let mut wb = init();
    f(&mut wb);
    Harness::new(wb)
}

/// What these tests read of one mounted element.
struct Seen {
    path: String,
    text: Option<String>,
    rect: [f32; 4],
    radius: f32,
    stuck: f32,
}

fn el(h: &Harness, key: &str) -> Seen {
    let e =
        h.rt.elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some(key) && !e.exiting)
            .unwrap_or_else(|| panic!("{key} not mounted"));
    Seen {
        path: e.path,
        text: e.text,
        rect: e.rect,
        radius: e.radius,
        stuck: e.stuck,
    }
}

fn fill(h: &Harness, key: &str) -> String {
    let n = h.tree()["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["key"] == key)
        .unwrap_or_else(|| panic!("{key} not in the tree"))
        .clone();
    n["fill"].as_str().unwrap_or_default().to_string()
}

fn luma(c: Rgba) -> f32 {
    0.2126 * f32::from(c.0) + 0.7152 * f32::from(c.1) + 0.0722 * f32::from(c.2)
}

#[test]
fn the_reading_pane_is_the_raised_surface_in_both_modes() {
    for name in ["pages", "light"] {
        let h = scene(name);
        let c = h.state().theme.colors.clone();
        assert_eq!(fill(&h, "rail"), c.rail.hex(), "{name}");
        for side in ["nav", "list-pane", "inspector"] {
            assert_eq!(fill(&h, side), c.bg.hex(), "{name}: {side}");
        }
        assert_eq!(fill(&h, "reading"), c.pane.hex(), "{name}");
        assert!(
            luma(c.rail) < luma(c.bg) && luma(c.bg) < luma(c.pane),
            "{name}"
        );
    }
}

#[test]
fn the_reading_header_sticks_while_the_page_scrolls() {
    let mut h = scene("pages");
    assert_eq!(el(&h, "reading-head").stuck, 0.0);
    let [x, y, w, hh] = el(&h, "reading-scroll").rect;
    // Negative deltas scroll toward the end, as a wheel turned down does.
    h.rt.wheel(x + w / 2.0, y + hh / 2.0, -240.0);
    h.settle();
    let head = el(&h, "reading-head");
    assert!(
        head.stuck > 0.0,
        "the header holds its place: {}",
        head.stuck
    );
    assert_eq!(head.rect[1], y, "pinned to the top of the pane");
}

#[test]
fn the_draft_sketch_is_framed_and_labelled_advisory() {
    let h = scene("pages");
    let t = h.state().theme.clone();
    assert_eq!(el(&h, "sketch").radius, t.shape.md);
    let label = el(&h, "sketch-label");
    let texts: Vec<String> =
        h.rt.elements()
            .into_iter()
            .filter(|e| e.path.starts_with(&label.path))
            .filter_map(|e| e.text)
            .collect();
    assert_eq!(texts, ["Draft sketch · advisory"]);
    let title = el(&h, "reading-title");
    assert!(
        title.rect[1] < label.rect[1],
        "the reading title, not the sketch, leads the pane"
    );
}

/// How far `inner`'s centre sits from `outer`'s, x and y.
fn off_centre(h: &Harness, inner: &str, outer: &str) -> (f32, f32) {
    let [ix, iy, iw, ih] = el(h, inner).rect;
    let [ox, oy, ow, oh] = el(h, outer).rect;
    (
        (ix + iw / 2.0 - (ox + ow / 2.0)).abs(),
        (iy + ih / 2.0 - (oy + oh / 2.0)).abs(),
    )
}

#[test]
fn empty_panes_centre_a_glyph_title_and_hint() {
    let h = scene("empty");
    for (state, pane) in [("list-empty", "list-pane"), ("reading-empty", "reading")] {
        let (dx, dy) = off_centre(&h, &format!("{state}-title"), pane);
        assert!(dx < 1.0 && dy < 40.0, "{state}: off by {dx} {dy}");
    }
    let hint = el(&h, "list-empty-hint").text.unwrap();
    assert_eq!(hint, "Open a workspace with ⇧⌘O");
    assert_eq!(
        el(&h, "reading-empty-title").text.as_deref(),
        Some("Nothing selected")
    );
}

/// The inspector's left edge one frame after it opens, and once settled.
fn inspector_entry(name: &str) -> (f32, f32) {
    let mut h = scene(name);
    h.script(&["key:mod+i"]);
    assert!(!h.state().inspector_open);
    for ev in keys::parse("mod+i") {
        h.rt.key(ev);
    }
    h.rt.frame(1.0 / 60.0);
    let first = el(&h, "inspector").rect[0];
    h.settle();
    (first, el(&h, "inspector").rect[0])
}

#[test]
fn the_inspector_slides_in_from_its_edge_unless_motion_is_reduced() {
    let (first, settled) = inspector_entry("canary");
    assert!(
        first > settled,
        "enters from the right: {first} vs {settled}"
    );
    let (first, settled) = inspector_entry("light");
    assert_eq!(first, settled, "reduced motion lands at once");
}
