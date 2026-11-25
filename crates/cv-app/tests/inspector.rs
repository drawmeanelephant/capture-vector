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
        .find(|e| e.key.as_deref() == Some("field-ruleset"))
        .expect("ruleset field")
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
