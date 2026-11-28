//! The command palette overlay: scrim, modal panel, one highlight shared
//! by pointer and keys, keycaps, and reduced motion.

use cv_app::{SCENES, init};
use cv_widgets::harness::Harness;
use cv_widgets::keys;
use serde_json::Value;

fn scene(name: &str) -> Harness {
    let (_, f) = SCENES
        .iter()
        .find(|(n, _)| *n == name)
        .unwrap_or_else(|| panic!("no scene {name}"));
    let mut wb = init();
    f(&mut wb);
    Harness::new(wb)
}

fn node(h: &Harness, key: &str) -> Value {
    h.tree()["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["key"] == key)
        .unwrap_or_else(|| panic!("{key} not in the tree"))
        .clone()
}

fn open(name: &str) -> Harness {
    let mut h = scene(name);
    h.script(&["key:mod+k"]);
    assert!(h.state().palette.open);
    h
}

#[test]
fn the_palette_opens_as_a_rounded_shadowed_panel_over_a_scrim() {
    let h = open("canary");
    let panel = node(&h, "palette");
    assert_eq!(panel["role"], "dialog");
    assert_eq!(panel["radius"], h.state().theme.shape.lg as f64);
    assert!(panel["shadows"].as_array().is_some_and(|s| !s.is_empty()));
    let scrim = node(&h, "palette-scrim");
    assert!(scrim["fill"].is_string(), "the scrim dims the window");
    let [w, hh] = [scrim["rect"][2].as_f64(), scrim["rect"][3].as_f64()];
    assert_eq!((w, hh), (Some(1280.0), Some(800.0)), "the scrim covers it");
}

#[test]
fn an_empty_query_shows_the_placeholder_and_typing_replaces_it() {
    let mut h = open("canary");
    assert_eq!(node(&h, "palette-query")["text"], "Type a command");
    assert!(h.has("palette-caret"));
    h.script(&["key:text:theme"]);
    assert_eq!(node(&h, "palette-query")["text"], "theme");
}

#[test]
fn continuous_primary_hold_in_one_input_burst_types_leading_chars() {
    let mut h = scene("canary");
    h.primary_modifier(true);
    for c in ['k', 'b', 'u'] {
        h.primary_command(c);
    }
    h.primary_modifier(false);
    for ev in keys::parse("text:ild") {
        h.rt.key(ev);
    }
    h.settle();
    assert!(h.state().palette.open);
    assert_eq!(h.state().palette.query, "build");
    assert_eq!(node(&h, "palette-query")["text"], "build");
}

fn open_palette_with_physical_primary(name: &str) -> Harness {
    let mut h = scene(name);
    h.primary_modifier(true);
    h.primary_command('k');
    h.settle();
    assert!(h.state().palette.open);
    h
}

#[test]
fn release_then_repress_mod_i_does_not_type_or_execute_in_the_palette() {
    let mut h = open_palette_with_physical_primary("canary");
    let inspector = h.state().inspector_open;
    h.primary_modifier(false);
    h.primary_modifier(true);
    h.primary_command('i');
    h.settle();
    assert!(h.state().palette.open);
    assert_eq!(h.state().palette.query, "");
    assert_eq!(h.state().inspector_open, inspector);
}

#[test]
fn alt_or_counterpart_command_modifier_does_not_type_in_the_palette() {
    for alt in [true, false] {
        let mut h = open_palette_with_physical_primary("canary");
        if alt {
            h.alt_primary_command('i');
        } else {
            h.counterpart_primary_command('i');
        }
        h.settle();
        assert_eq!(h.state().palette.query, "", "alt case {alt}");
    }
}

#[test]
fn a_click_outside_the_panel_closes_the_palette() {
    let mut h = open("canary");
    h.rt.pointer_down(20.0, 760.0);
    h.rt.pointer_up(20.0, 760.0);
    h.settle();
    assert!(!h.state().palette.open);
    assert!(!h.has("palette") && !h.has("palette-scrim"));
}

#[test]
fn keys_reach_the_model_while_the_panel_holds_focus() {
    let mut h = open("canary");
    assert_eq!(node(&h, "palette")["focused"], true);
    h.script(&["key:text:shell", "key:tab", "key:tab", "key:down", "key:up"]);
    assert_eq!(
        h.state().palette.cursor,
        2,
        "Tab and the arrows move the cursor"
    );
    h.script(&["key:bksp", "key:bksp", "key:bksp", "key:bksp", "key:bksp"]);
    let before = h.state().inspector_open;
    h.script(&["key:text:toggle insp", "key:enter"]);
    assert!(!h.state().palette.open);
    assert_ne!(h.state().inspector_open, before, "Enter ran the command");
    h.script(&["key:mod+k", "key:esc"]);
    assert!(!h.state().palette.open, "Escape closes it");
}

#[test]
fn pointer_and_keys_share_one_highlight() {
    let mut h = open("canary");
    h.script(&["key:text:shell"]);
    // Another row than the cursor's, chosen among what `visible` renders —
    // a hardcoded id can fall outside `Palette::VISIBLE` when the command
    // count changes.
    let (first, target) = {
        let hits = h.state().commands();
        let visible = h.state().palette.visible(&hits);
        let first = format!("cmd-{}", visible[h.state().palette.cursor].id);
        let target = format!("cmd-{}", visible.last().unwrap().id);
        assert_ne!(first, target, "test needs a second row");
        (first, target)
    };
    let selection = node(&h, &first)["fill"].clone();
    assert!(selection.is_string(), "the cursor row is highlighted");
    assert!(h.hover(&target));
    let hits = h.state().commands();
    let visible = h.state().palette.visible(&hits);
    let at = visible
        .iter()
        .position(|c| format!("cmd-{}", c.id) == target)
        .unwrap();
    assert_eq!(h.state().palette.cursor, at, "hover moves the cursor");
    let lit = |h: &Harness| {
        h.tree()["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|n| n["role"] == "option" && n["fill"] == selection)
            .map(|n| n["key"].as_str().unwrap().to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(lit(&h), vec![target.clone()]);
    h.script(&["key:down"]);
    assert_eq!(lit(&h).len(), 1, "one highlight after a key moves it");
    assert_ne!(lit(&h), [target]);
}

#[test]
fn chords_draw_as_keycaps() {
    let mut h = open("canary");
    h.script(&["key:text:command palette"]);
    let row = node(&h, "cmd-shell.palette");
    let path = row["path"].as_str().unwrap().to_string();
    let caps: Vec<String> = h.tree()["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["path"].as_str().is_some_and(|p| p.starts_with(&path)))
        .filter_map(|n| n["text"].as_str().map(str::to_string))
        .collect();
    assert!(caps.ends_with(&["⌘".into(), "K".into()]), "{caps:?}");
}

/// The panel's opacity and scale one frame after the palette opens.
fn first_frame(name: &str) -> (f32, f32) {
    let mut h = scene(name);
    for ev in keys::parse("mod+k") {
        h.rt.key(ev);
    }
    h.rt.frame(1.0 / 60.0);
    let e =
        h.rt.elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some("palette"))
            .expect("palette mounted");
    (e.opacity, e.scale)
}

#[test]
fn the_palette_eases_in_unless_motion_is_reduced() {
    let (opacity, scale) = first_frame("canary");
    assert!(opacity < 1.0 || scale < 1.0, "eases in: {opacity} {scale}");
    assert_eq!(first_frame("light"), (1.0, 1.0), "reduced motion");
}
