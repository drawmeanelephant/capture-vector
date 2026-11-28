//! The inspector pane: disclosure sections, the window debug toggle,
//! status chips, and hash fields that keep their whole value.

use cv_app::{SCENES, init};
use cv_core::diagnostic::{Diagnostic, Severity, Span};
use cv_core::events::Envelope;
use cv_core::id::{AccountId, NounId, NounKind, StableId, noun_kind};
use cv_model::module::{
    InspectorModel, Module, ModuleSpec, NavItem, NavModel, ReadingModel, Registry, RowModel, Slot,
    Tone,
};
use cv_model::workbench::Workbench;
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

struct EvidenceModule(&'static str);

const MULTI_LINE: &str = "first line \nsecond line of evidence\nthird line 🔍";
const DIAG_PATH: &str = "content/docs/a/very/long/path/that/cannot/reasonably/fit/the/inspector.md";

impl EvidenceModule {
    fn cjk_value() -> String {
        format!("{}\u{1f50d}", "\u{6771}".repeat(14))
    }

    fn diagnostic_location() -> String {
        format!("{DIAG_PATH}:12:4")
    }
}

impl Module for EvidenceModule {
    fn spec(&self) -> ModuleSpec {
        ModuleSpec {
            id: self.0,
            title: "Evidence",
            slot: Slot::Extra,
        }
    }

    fn ingest(&mut self, _env: &Envelope) -> bool {
        false
    }

    fn nav(&self) -> NavModel {
        NavModel {
            items: vec![NavItem {
                key: "evidence".into(),
                label: "Evidence".into(),
                count: Some(1),
            }],
        }
    }

    fn list_len(&self, _nav: Option<&str>) -> usize {
        1
    }

    fn row(&self, _nav: Option<&str>, _index: usize) -> RowModel {
        RowModel {
            key: "evidence-row".into(),
            title: "Evidence row".into(),
            detail: "fixture".into(),
            tone: Tone::Normal,
            ..RowModel::default()
        }
    }

    fn reading(&self, _key: &str) -> ReadingModel {
        ReadingModel::default()
    }

    fn inspector(&self, _key: &str) -> InspectorModel {
        InspectorModel {
            fields: vec![
                ("cjk".into(), Self::cjk_value()),
                ("multi".into(), MULTI_LINE.into()),
            ],
            diagnostics: vec![Diagnostic {
                code: "EFRONTMATTER".into(),
                severity: Severity::Error,
                span: Some(Span {
                    path: DIAG_PATH.into(),
                    start: 0,
                    end: 1,
                    line: 12,
                    col: 4,
                }),
                message: "fixture diagnostic".into(),
                build_version: None,
            }],
            lists: vec![(
                "Evidence".into(),
                (0..15).map(|i| format!("entry-{i:02}")).collect(),
            )],
            ..InspectorModel::default()
        }
    }

    fn noun(&self, key: &str) -> Option<NounId> {
        (key == "evidence-row").then(|| NounId {
            account: AccountId("local".into()),
            kind: NounKind::register(noun_kind::IMPORT_RUN),
            stable: StableId(key.into()),
        })
    }
}

fn evidence_scene() -> Harness {
    let mut registry = Registry::default();
    registry
        .register(Box::new(EvidenceModule("fixture-evidence")))
        .expect("fixture module registers once");
    let mut wb = Workbench::new(registry);
    wb.select_row(0);
    Harness::new(wb)
}

fn dual_evidence_scene() -> Harness {
    let mut registry = Registry::default();
    registry
        .register(Box::new(EvidenceModule("fixture-a")))
        .expect("first fixture module registers");
    registry
        .register(Box::new(EvidenceModule("fixture-b")))
        .expect("second fixture module registers");
    let mut wb = Workbench::new(registry);
    wb.select_row(0);
    Harness::new(wb)
}

fn focused_key(h: &Harness) -> Option<String> {
    h.rt.elements()
        .into_iter()
        .find(|e| e.focused)
        .and_then(|e| e.key)
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

#[test]
fn shell_tab_cycles_mailboxes_only_while_no_control_is_focused() {
    let mut h = scene("migrate");
    assert_eq!(focused_key(&h), None);
    let mailbox = h.state().active_nav().map(str::to_string);
    h.key("tab");
    assert_eq!(
        focused_key(&h),
        None,
        "shell Tab does not enter focus traversal"
    );
    assert_ne!(
        h.state().active_nav().map(str::to_string),
        mailbox,
        "the unfocused shell keymap consumed Tab"
    );
}

#[test]
fn disclosure_controls_are_keyboard_discoverable_and_operable() {
    let mut h = scene("migrate");
    assert_eq!(focused_key(&h), None);
    let control =
        h.rt.elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some("field-argv-expand"))
            .expect("argv disclosure");
    assert!(control.focusable, "the disclosure is a keyboard focus stop");

    // F6 is the deliberate landmark entry into the inspector's controls.
    h.key("f6");
    let first = focused_key(&h).expect("F6 focuses an inspector disclosure");
    assert!(
        first.ends_with("-expand"),
        "F6 landed on a disclosure control: {first}"
    );
    let focused =
        h.rt.elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some(first.as_str()))
            .expect("focused disclosure");
    assert!(focused.focus_visible, "keyboard focus is visible");
    let full_key = format!("{}-full", first.strip_suffix("-expand").unwrap());

    h.key("enter");
    assert!(h.has(&full_key), "Enter activates the focused disclosure");
    assert!(
        h.state().windows.companions.is_empty(),
        "the disclosure consumes Enter instead of the shell opening a companion"
    );
    h.key("space");
    assert!(!h.has(&full_key), "Space activates the focused disclosure");

    // Once focus is inside the controls, Rune's traversal owns Tab: it does
    // not fall through to the shell's mailbox cycling keymap.
    let mailbox = h.state().active_nav().map(str::to_string);
    h.key("tab");
    assert_eq!(h.state().active_nav().map(str::to_string), mailbox);
    assert_ne!(focused_key(&h).as_deref(), Some(first.as_str()));
    h.key("shift+tab");
    assert_eq!(focused_key(&h).as_deref(), Some(first.as_str()));

    // Row selection is shell navigation; moving it returns focus to the
    // shell rather than retargeting the old record's control.
    h.key("down");
    assert_eq!(focused_key(&h), None);
}

/// Keyboard focus walks from the F6 landmark entry to the control keyed
/// `key`. Panics if traversal never reaches it — the control is the test.
fn focus_key(h: &mut Harness, key: &str) {
    h.key("f6");
    for _ in 0..60 {
        if focused_key(h).as_deref() == Some(key) {
            return;
        }
        h.key("tab");
    }
    panic!("Tab traversal never reached {key}");
}

#[test]
fn enter_on_a_focused_button_activates_it_instead_of_opening_a_companion() {
    let mut h = scene("activity-broken");
    assert!(h.has("follow-link"));
    assert_eq!(focused_key(&h), None);

    focus_key(&mut h, "follow-link");
    assert_eq!(
        focused_key(&h).as_deref(),
        Some("follow-link"),
        "the button is a keyboard focus stop"
    );

    h.key("enter");
    let s = h.state();
    assert!(
        s.windows.companions.is_empty(),
        "Enter activated the focused control instead of reaching the shell's companion keymap"
    );
    assert_eq!(s.registry.get(s.active).unwrap().spec().id, "pages");
    assert_eq!(s.selected_key(), Some("orphan.md"));
}

#[test]
fn space_on_a_focused_button_still_activates_it() {
    let mut h = scene("activity-broken");
    focus_key(&mut h, "follow-link");

    h.key("space");
    let s = h.state();
    assert!(
        s.windows.companions.is_empty(),
        "Space activated the focused control instead of the shell keymap"
    );
    assert_eq!(s.registry.get(s.active).unwrap().spec().id, "pages");
    assert_eq!(s.selected_key(), Some("orphan.md"));
}

#[test]
fn wide_cjk_and_emoji_values_earn_disclosure_by_rendered_width() {
    let mut h = evidence_scene();
    assert!(
        h.has("field-cjk-expand"),
        "fifteen wide glyphs exceed the field even though they are fewer than twenty chars"
    );
    assert!(h.click("field-cjk-expand"));
    let (text, _, _) = element(&h, "field-cjk-full");
    assert_eq!(text.as_deref(), Some(EvidenceModule::cjk_value().as_str()));
}

#[test]
fn collapsed_multiline_values_show_only_the_first_line_and_expand_exactly() {
    let mut h = evidence_scene();
    assert!(h.has("field-multi-expand"));
    let collapsed = texts_under(&h, "field-multi");
    assert!(collapsed.iter().any(|t| t == "first line "));
    assert!(
        !collapsed.iter().any(|t| t.contains("second line")),
        "later lines are not partially shown in the collapsed row: {collapsed:?}"
    );
    let (_, collapsed_rect, _) = element(&h, "field-multi");
    assert!(
        collapsed_rect[3] <= 16.0,
        "the collapsed shape is one line plus its control, not a partial paragraph: {collapsed_rect:?}"
    );

    assert!(h.click("field-multi-expand"));
    let (text, rect, _) = element(&h, "field-multi-full");
    assert_eq!(text.as_deref(), Some(MULTI_LINE));
    assert!(
        rect[3] > 13.8 * 2.0,
        "all three lines contribute to the expanded geometry: {rect:?}"
    );
}

#[test]
fn evidence_beyond_the_section_preview_is_reachable() {
    let mut h = evidence_scene();
    assert!(h.has("list-0-11"));
    assert!(!h.has("list-0-12"), "the preview still stops at twelve");
    assert_eq!(texts_under(&h, "list-0-overflow"), ["Show 3 more"]);

    assert!(h.click("list-0-overflow"));
    assert_eq!(texts_under(&h, "list-0-overflow"), ["Show fewer"]);
    assert_eq!(texts_under(&h, "list-0-12"), ["entry-12"]);
    assert_eq!(texts_under(&h, "list-0-14"), ["entry-14"]);

    assert!(h.click("list-0-overflow"));
    assert!(
        !h.has("list-0-12"),
        "collapsing restores the bounded preview"
    );
}

#[test]
fn a_long_diagnostic_location_expands_with_the_shared_control() {
    let mut h = evidence_scene();
    assert!(h.has("diag-0-at-expand"));
    assert!(h.click("diag-0-at-expand"));
    let (text, _, _) = element(&h, "diag-0-at-full");
    assert_eq!(
        text.as_deref(),
        Some(EvidenceModule::diagnostic_location().as_str())
    );
}

#[test]
fn expansion_is_scoped_per_module_even_when_row_and_element_keys_collide() {
    let mut h = dual_evidence_scene();
    assert_eq!(
        h.state()
            .registry
            .get(h.state().active)
            .map(|m| m.spec().id),
        Some("fixture-a")
    );
    assert!(h.click("field-cjk-expand"));
    assert!(h.has("field-cjk-full"));

    h.state_mut().activate(1);
    h.state_mut().select_row(0);
    h.settle();
    assert_eq!(
        h.state()
            .registry
            .get(h.state().active)
            .map(|m| m.spec().id),
        Some("fixture-b")
    );
    assert!(
        !h.has("field-cjk-full"),
        "the colliding row and element in another module starts collapsed"
    );

    h.state_mut().activate(0);
    h.state_mut().select_row(0);
    h.settle();
    assert!(
        h.has("field-cjk-full"),
        "returning to the first module preserves that record's expansion"
    );
}
