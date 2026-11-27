//! The inspector pane: disclosure sections, the window debug toggle,
//! status chips, and hash fields that keep their whole value.

use cv_app::{SCENES, init};
use cv_widgets::harness::Harness;

fn scene(name: &str) -> Harness {
    let (_, f) = SCENES
        .iter()
        .find(|(n, _)| *n == name)
        .unwrap_or_else(|| panic!("no scene {name}"));
    let mut wb = init();
    f(&mut wb);
    Harness::new(wb)
}

fn expanded(h: &Harness, key: &str) -> Option<bool> {
    h.rt.elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some(key))
        .unwrap_or_else(|| panic!("{key} not mounted"))
        .expanded
}

/// The texts under the element keyed `key`, in tree order.
fn texts_under(h: &Harness, key: &str) -> Vec<String> {
    let els = h.rt.elements();
    let path = els
        .iter()
        .find(|e| e.key.as_deref() == Some(key))
        .unwrap_or_else(|| panic!("{key} not mounted"))
        .path
        .clone();
    els.iter()
        .filter(|e| e.path.starts_with(&path))
        .filter_map(|e| e.text.clone())
        .collect()
}

#[test]
fn empty_sections_start_closed_and_full_ones_open() {
    let h = scene("broken-parent");
    assert_eq!(expanded(&h, "diagnostics"), Some(true));
    assert!(h.has("diag-0"));
    assert_eq!(texts_under(&h, "list-1"), ["Referenced by", "0"]);
    assert_eq!(expanded(&h, "list-1"), Some(false));
}

#[test]
fn pressing_a_section_header_flips_it() {
    let mut h = scene("broken-parent");
    assert!(h.click("diagnostics"));
    assert_eq!(expanded(&h, "diagnostics"), Some(false));
    assert!(!h.has("diag-0"), "a closed section hides its cards");
    assert!(h.click("diagnostics"));
    assert!(h.has("diag-0"));
    assert!(h.click("list-1"));
    assert_eq!(expanded(&h, "list-1"), Some(true), "an empty one opens too");
}

#[test]
fn the_windows_section_shows_only_behind_the_debug_toggle() {
    let mut h = scene("canary");
    assert!(!h.has("windows") && !h.has("field-primary"));
    h.script(&["key:mod+k", "key:text:window debug", "key:enter"]);
    assert!(h.state().debug_windows);
    assert!(h.has("windows") && h.has("field-primary"));
}

#[test]
fn audit_findings_show_authority_and_the_publish_gate_as_chips() {
    let h = scene("audit-detail");
    assert_eq!(
        texts_under(&h, "inspector-badges"),
        ["heuristic (advisory)", "does not block publishing"]
    );
}

#[test]
fn a_cut_hash_keeps_its_whole_value_on_hover() {
    let h = scene("audit-detail");
    let els = h.rt.elements();
    let path = els
        .iter()
        .find(|e| e.key.as_deref() == Some("field-ruleset-head"))
        .expect("ruleset field header")
        .path
        .clone();
    let v = els
        .iter()
        .find(|e| e.path == format!("{path}/#1"))
        .expect("ruleset value");
    let full = v.text.clone().unwrap();
    assert_eq!(full.len(), 64, "a sha256 digest");
    assert_eq!(v.help.as_deref(), Some(full.as_str()));
}

/// The one mounted element keyed `key`: key, text, rect and `expanded`.
fn element(h: &Harness, key: &str) -> (Option<String>, [f32; 4], Option<bool>) {
    let e =
        h.rt.elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some(key) && !e.exiting)
            .unwrap_or_else(|| panic!("{key} not mounted"));
    (e.text, e.rect, e.expanded)
}

#[test]
fn long_migration_evidence_expands_to_its_full_text_inline() {
    let mut h = scene("migrate");
    // Collapsed, the argv field truncates behind its disclosure control.
    assert_eq!(expanded(&h, "field-argv-expand"), Some(false));
    assert!(!h.has("field-argv-full"));
    let (preview, _, _) = element(&h, "field-argv-expand");
    assert_eq!(
        preview.as_deref(),
        None,
        "the control is a chevron, not the text"
    );

    assert!(h.click("field-argv-expand"));
    assert_eq!(expanded(&h, "field-argv-expand"), Some(true));
    let (text, rect, _) = element(&h, "field-argv-full");
    let full = text.expect("the expanded value carries its text");
    assert!(
        full.starts_with("--mode=hugo") && full.contains("--out="),
        "the complete argv, not a cut: {full:?}"
    );
    let [x, _, w, height] = rect;
    let line = 13.8;
    assert!(
        height > line * 2.0,
        "argv wraps over several lines: height {height}"
    );
    assert!(
        w > 200.0,
        "expansion uses the pane's full width, not the field column: {w}"
    );
    let scroll = element(&h, "inspector-scroll").1;
    assert!(
        x >= scroll[0] && x + w <= scroll[0] + scroll[2] + 0.5,
        "the full value stays inside the pane: {rect:?} in {scroll:?}"
    );

    assert!(h.click("field-argv-expand"));
    assert_eq!(expanded(&h, "field-argv-expand"), Some(false));
    assert!(!h.has("field-argv-full"), "collapsing hides the full text");
}

#[test]
fn a_long_input_record_expands_with_its_whole_path_and_hash() {
    let mut h = scene("migrate");
    assert!(h.click("list-0-0-expand"));
    let els = h.rt.elements();
    let row = els
        .iter()
        .find(|e| e.key.as_deref() == Some("list-0-0"))
        .expect("input row")
        .path
        .clone();
    let text = els
        .iter()
        .filter(|e| e.path.starts_with(&row) && e.key.is_none())
        .filter_map(|e| e.text.clone())
        .find(|t| t.len() > 40)
        .expect("the expanded input text");
    assert!(
        text.chars()
            .collect::<Vec<_>>()
            .windows(64)
            .any(|w| w.iter().all(|c| c.is_ascii_hexdigit())),
        "the whole sha256 survives, unclipped: {text:?}"
    );
    let (_, rect, _) = element(&h, "list-0-0");
    assert!(rect[3] > 13.8 * 2.0, "the item wraps: {:?}", rect);
}

#[test]
fn short_values_earn_no_disclosure_control() {
    let h = scene("migrate");
    assert!(h.has("field-mode"), "the short field still renders");
    assert!(
        !h.has("field-mode-expand"),
        "a value that fits stays a plain row"
    );
}

#[test]
fn expansion_is_keyed_per_record_not_per_field_name() {
    let mut h = scene("migrate");
    assert!(h.click("field-argv-expand"));
    assert!(h.has("field-argv-full"));
    h.state_mut().select_row(1);
    h.settle();
    assert!(
        !h.has("field-argv-full"),
        "another run's argv starts collapsed"
    );
    assert_eq!(expanded(&h, "field-argv-expand"), Some(false));
    h.state_mut().select_row(0);
    h.settle();
    assert!(
        h.has("field-argv-full"),
        "returning to the run keeps its expansion"
    );
}
