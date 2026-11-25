//! Cross-OS UI gate: element-tree + layout JSON goldens per named scene,
//! driven headless in-process (no GPU). Bless: `CV_BLESS=1 cargo test -p cv-app`.

use cv_app::{SCENES, init};
use cv_core::events::shell::DegradedReason;
use cv_widgets::harness::{Harness, check_golden};
use std::path::PathBuf;

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/ui/golden/tree")
}

fn scene(name: &str) -> Harness {
    let (_, f) = SCENES
        .iter()
        .find(|(n, _)| *n == name)
        .unwrap_or_else(|| panic!("no scene {name}"));
    let mut wb = init();
    f(&mut wb);
    Harness::new(wb)
}

const TREE_SCENES: &[&str] = &[
    "canary",
    "pages-5k",
    "palette",
    "light",
    "pages",
    "outputs",
    "activity",
    "broken-duplicate-id",
    "broken-frontmatter",
    "broken-parent",
    "broken-textile",
    "broken-wikilink",
    "dogfood",
    "activity-broken",
    "content-only",
    "empty",
    "mid-watch",
    "pages-watch-dead",
    "audit-empty",
    "audit",
    "audit-detail",
    "audit-degraded",
    "publish",
    "publish-detail",
    "publish-unknown",
    "publish-empty",
    "publish-degraded",
    "dialog-confirm",
    "dialog-prompt",
    "dialog-form",
    "dialog-choose",
    "dialog-secret",
    "dialog-progress",
    "effects",
    "engine-absent-toast",
    "doctor",
    "doctor-engine-absent",
    "doctor-version-mismatch",
    "doctor-watch-dead",
    "migrate-empty",
    "migrate",
    "migrate-review",
    "migrate-lab-absent",
    "migrate-undecodable",
    "verb-build",
    "verb-content-failure",
    "verb-usage-error",
    "verb-impact",
    "verb-check-audit",
];

#[test]
fn scene_goldens() {
    let mut errs = vec![];
    for (name, _) in SCENES {
        let h = scene(name);
        if let Err(e) = check_golden(&dir(), &format!("{name}.layout"), &h.layout()) {
            errs.push(e);
        }
        if TREE_SCENES.contains(name)
            && let Err(e) = check_golden(&dir(), &format!("{name}.tree"), &h.tree())
        {
            errs.push(e);
        }
    }
    assert!(errs.is_empty(), "{}", errs.join("\n"));
}

#[test]
fn all_fifteen_init_archetype_choices_stay_inside_the_form_dialog() {
    let names = [
        "docs",
        "garden",
        "cookbook",
        "blog",
        "textile",
        "landing",
        "wiki",
        "link-in-bio",
        "changelog",
        "resume",
        "newsletter",
        "portfolio",
        "archive",
        "dashboard",
        "photo-gallery",
    ];
    let mut wb = init();
    wb.archetypes = Ok(cv_model::new_workspace::Archetypes {
        names: names.iter().map(|n| n.to_string()).collect(),
        default: 0,
    });
    wb.run_command(cv_model::new_workspace::NEW);
    let h = Harness::new(wb);
    let elements = h.rt.elements();
    let rect = |key: &str| {
        elements
            .iter()
            .find(|e| e.key.as_deref() == Some(key))
            .unwrap_or_else(|| panic!("{key} not mounted"))
            .rect
    };
    let dialog = rect("dialog-dialog");
    let group = rect("dialog-field-archetype-options");
    assert!(
        group[3] > 22.0,
        "fifteen choices should wrap onto multiple rows: {group:?}"
    );
    for i in 0..names.len() {
        let key = format!("dialog-field-archetype-option-{i}");
        let r = rect(&key);
        assert!(
            r[0] >= dialog[0] && r[0] + r[2] <= dialog[0] + dialog[2],
            "{key} escapes horizontally: option {r:?}, dialog {dialog:?}"
        );
        assert!(
            r[1] >= dialog[1] && r[1] + r[3] <= dialog[1] + dialog[3],
            "{key} escapes vertically: option {r:?}, dialog {dialog:?}"
        );
    }
}

#[test]
fn tree_is_deterministic() {
    let a = scene("canary").tree();
    let b = scene("canary").tree();
    assert_eq!(a, b);
}

#[test]
fn canary_replays_watch_session_into_ui() {
    let h = scene("canary");
    let texts = h.texts();
    assert!(texts.iter().any(|t| t == "Watch session started"));
    assert!(
        texts
            .iter()
            .any(|t| t.starts_with("Build succeeded (rebuild)"))
    );
    assert!(texts.iter().any(|t| t == "boris/0.9.0"));
    assert!(h.has("rail-demo") && h.has("rail-pages") && h.has("rail-activity"));
}

#[test]
fn virtualized_5k_pages_mount_bounded_rows() {
    let mut h = scene("pages-5k");
    assert_eq!(h.state().list_len(), 5_000);
    let mounted = |h: &Harness| {
        h.rt.elements()
            .iter()
            .filter(|e| e.key.as_deref().is_some_and(|k| k.starts_with("row-page-")))
            .count()
    };
    let n = mounted(&h);
    assert!(n > 0 && n < 200, "mounted {n} of 5000 rows");
    // Keyboard selection deep into the list reveals the row.
    for _ in 0..60 {
        h.key("down");
    }
    let sel = h.state().selected_key().unwrap().to_string();
    assert_eq!(sel, "page-0063");
    let rect = |k: &str| {
        h.rt.elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some(k))
            .map(|e| e.rect)
            .unwrap_or_else(|| panic!("{k} not mounted"))
    };
    let (row, port) = (rect(&format!("row-{sel}")), rect("list-viewport"));
    assert!(
        row[1] >= port[1] && row[1] + row[3] <= port[1] + port[3],
        "selected row {row:?} visible in viewport {port:?}"
    );
    assert!(mounted(&h) < 200);
}

/// Keys starting with `prefix` whose tree node reports `selected`.
fn selected(h: &Harness, prefix: &str) -> Vec<String> {
    h.tree()["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["selected"] == true)
        .filter_map(|n| n["key"].as_str())
        .filter(|k| k.starts_with(prefix))
        .map(str::to_string)
        .collect()
}

#[test]
fn only_the_active_rail_tab_reports_selected_in_the_tree() {
    let mut h = scene("canary");
    assert_eq!(selected(&h, "rail-"), ["rail-demo"]);
    h.script(&["click:rail-pages"]);
    assert_eq!(selected(&h, "rail-"), ["rail-pages"]);
}

#[test]
fn active_rail_tab_is_selected_in_the_accessibility_tree() {
    let h = scene("canary");
    let tree = h.rt.accessibility_tree();
    let tabs: Vec<(&str, Option<bool>)> = tree
        .nodes
        .iter()
        .filter_map(|(_, n)| Some((n.author_id()?, n.is_selected())))
        .filter(|(k, _)| k.starts_with("rail-") && !k.ends_with("-badge"))
        .collect();
    assert!(
        tabs.len() > 1,
        "rail tabs in the accessibility tree: {tabs:?}"
    );
    for (key, sel) in tabs {
        if key == "rail-demo" {
            assert_eq!(sel, Some(true), "{key}");
        } else {
            assert_ne!(sel, Some(true), "{key}");
        }
    }
}

#[test]
fn selected_nav_item_and_list_row_report_selected_in_the_tree() {
    let h = scene("pages-5k");
    assert_eq!(selected(&h, "nav-"), ["nav-pages"]);
    let row = format!("row-{}", h.state().selected_key().unwrap());
    assert_eq!(selected(&h, "row-"), [row]);
}

#[test]
fn palette_cursor_option_reports_selected_in_the_tree() {
    let h = scene("palette");
    let first = h.tree()["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|n| n["key"].as_str())
        .find(|k| k.starts_with("cmd-"))
        .expect("palette lists a command")
        .to_string();
    assert_eq!(selected(&h, "cmd-"), [first]);
}

#[test]
fn scripted_palette_flow_reaches_every_degraded_state() {
    let mut h = scene("canary");
    for (needle, banner) in [
        ("engine-absent", "Engine absent"),
        ("version-mismatch", "Engine version mismatch"),
        ("watch-dead", "Watch session died"),
        ("store-corrupt", "Store corrupt"),
    ] {
        h.script(&["key:mod+k", &format!("key:text:{needle}"), "key:enter"]);
        assert!(h.has("banner"), "{needle}");
        assert!(h.texts().iter().any(|t| t.starts_with(banner)), "{needle}");
    }
    h.script(&["cmd:shell.degraded.dismiss"]);
    assert!(!h.has("banner"));
}

#[test]
fn degraded_banner_dismisses_by_click_and_by_palette() {
    let mut h = scene("watch-dead");
    let alert = h.tree()["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|n| n["key"] == "banner" && n["role"] == "alert");
    assert!(alert, "banner keeps role alert");
    h.script(&["click:banner-dismiss"]);
    assert!(!h.has("banner"), "dismissed by click");

    let mut h = scene("store-corrupt");
    h.script(&["key:mod+k", "key:text:dismiss degraded", "key:enter"]);
    assert!(!h.has("banner"), "dismissed from the palette");
}

#[test]
fn status_bar_buttons_toggle_theme_and_motion() {
    let mut h = scene("canary");
    h.script(&["click:status-3"]);
    assert_eq!(h.state().theme.mode, cv_model::theme::Mode::Light);
    h.script(&["click:status-4"]);
    assert!(h.state().theme.motion.reduced);
}

#[test]
fn the_engine_chip_opens_doctor_off_the_rail() {
    let mut h = scene("canary");
    assert!(!h.has("rail-doctor"), "Doctor is not a rail module");
    h.script(&["click:status-0"]);
    let wb = h.state();
    let active = wb.registry.get(wb.active).unwrap().spec().id;
    assert_eq!(active, "doctor");
    assert_eq!(wb.selected_key(), Some("engine.status"));
    assert!(h.has("row-engine.status"), "{:?}", h.texts());
}

#[test]
fn transient_status_shows_as_a_closable_toast() {
    let mut h = scene("research-promote");
    assert!(h.has("status-msg"));
    assert!(h.has("close"), "toast has a close button");
    h.script(&["click:close"]);
    assert!(!h.has("status-msg"));
    assert!(h.state().status.is_empty());
}

#[test]
fn engine_absent_toast_summarizes_the_search() {
    let h = scene("engine-absent-toast");
    assert!(h.has("banner"));
    assert!(h.has("status-msg") && h.has("close"));
    let texts = h.texts();
    assert!(
        texts
            .iter()
            .any(|t| t == "/workspace/happy: boris not found (searched 24 locations)"),
        "{texts:?}"
    );
    assert!(!texts.iter().any(|t| t.contains("/opt/tool-")));
}

#[test]
fn a_huge_status_keeps_the_toast_short_and_closable() {
    let mut h = scene("engine-absent-toast");
    h.state_mut().status = "/very/long/searched/path/boris ".repeat(400);
    h.settle();
    let els = h.rt.elements();
    let rect = |k: &str| {
        els.iter()
            .find(|e| e.key.as_deref() == Some(k))
            .map(|e| e.rect)
            .unwrap_or_else(|| panic!("no {k}"))
    };
    let [_, y, _, height] = rect("status-msg");
    let (_, window_h) = cv_widgets::harness::SIZE;
    assert!(height <= window_h / 3.0, "toast is {height}pt tall");
    let [_, close_y, _, _] = rect("close");
    assert!(close_y >= y && close_y >= 0.0, "close at {close_y}");
    h.script(&["click:close"]);
    assert!(!h.has("status-msg"));
}

#[test]
fn effects_are_opt_in_and_off_under_reduced_motion() {
    let plain = scene("canary");
    assert!(!plain.has("fx-rail"), "no effect slots without the flag");
    let mut h = scene("effects");
    assert!(h.has("fx-rail"), "dark rail aurora slot");
    let builds = h.state().app.builds_finished;
    assert!(builds > 0);
    assert!(
        h.has(&format!("fx-build-{builds}")),
        "build glow on Activity"
    );
    h.script(&["cmd:shell.motion.toggle"]);
    assert!(h.state().theme.effects, "flag survives the motion toggle");
    assert!(!h.has("fx-rail") && !h.has(&format!("fx-build-{builds}")));
    h.script(&["cmd:shell.motion.toggle", "cmd:shell.theme.toggle"]);
    assert!(!h.has("fx-rail"), "aurora is dark-only");
    assert!(h.has(&format!("fx-build-{builds}")));
}

#[test]
fn click_and_keyboard_drive_selection_and_rail() {
    let mut h = scene("canary");
    h.script(&["click:nav-pages"]);
    assert_eq!(h.state().active_nav(), Some("pages"));
    h.script(&["click:row-page-0002"]);
    assert_eq!(h.state().selected_key(), Some("page-0002"));
    h.script(&["key:mod+i"]);
    assert!(!h.has("inspector"));
    h.script(&["key:enter"]);
    assert_eq!(
        h.state().windows.companions.len(),
        1,
        "companion window opened"
    );
}

#[test]
fn light_reduced_motion_theme_is_applied() {
    let h = scene("light");
    assert!(h.state().theme.motion.reduced);
    assert!(h.rt.system_reduced_motion());
    assert_ne!(scene("canary").tree(), h.tree());
}

#[test]
fn research_scenes_cover_empty_promote_and_degraded() {
    let has = |name: &str, want: &str| {
        let t = scene(name).texts();
        assert!(
            t.iter().any(|x| x.contains(want)),
            "{name}: no {want:?} in {t:?}"
        );
    };
    has("research-empty", "Inbox zero");
    has(
        "research-promote",
        "promoted → field-notes-graph-first-research",
    );
    has("research-degraded", "no content writer attached");
}

fn node(h: &Harness, key: &str) -> serde_json::Value {
    h.tree()["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["key"] == key)
        .cloned()
        .unwrap_or_else(|| panic!("no node keyed {key}"))
}

#[test]
fn hovered_list_row_shows_hover_in_the_tree() {
    let mut h = scene("pages");
    let row = "row-index.md";
    assert!(node(&h, row).get("hover").is_none(), "rest has no hover");
    assert!(h.hover(row));
    let hover = node(&h, row)["hover"].as_f64().unwrap_or(0.0);
    assert!(hover > 0.0, "hovered row reports hover {hover}");
}

#[test]
fn held_press_shows_press_in_the_tree_until_release() {
    for (name, item) in [("pages", "index.md"), ("light", "ev-stub-1")] {
        let row = format!("row-{item}");
        let mut h = scene(name);
        assert!(h.hold(&row), "{name}: no {row}");
        let held = node(&h, &row);
        assert!(
            held["press"].as_f64().unwrap_or(0.0) > 0.0,
            "{name}: {held}"
        );
        assert!(
            held.get("scale").is_none_or(|s| s.as_f64().unwrap() <= 1.0),
            "{name}: a press never grows the row: {held}"
        );
        if h.state().theme.motion.reduced {
            assert!(held.get("scale").is_none(), "reduced motion keeps scale 1");
        }
        let (x, y) = h.rt.find(&row).unwrap();
        h.rt.pointer_up(x, y);
        // The press track springs back over a few frames; `settle` lands layout, not paint.
        for _ in 0..60 {
            h.rt.frame(1.0 / 60.0);
        }
        let after = node(&h, &row);
        assert!(
            after.get("press").is_none(),
            "{name}: release clears press: {after}"
        );
        assert_eq!(h.state().selected_key(), Some(item));
    }
}

#[test]
fn held_buttons_shrink_until_the_motion_toggle_reduces_motion() {
    let mut h = scene("watch-dead");
    assert!(h.hold("banner-dismiss"));
    let held = node(&h, "banner-dismiss");
    let scale = held["scale"].as_f64().unwrap_or(1.0);
    assert!(scale < 1.0, "a held press shrinks: {held}");
    let (x, y) = h.rt.find("banner-dismiss").unwrap();
    h.rt.pointer_up(x, y);
    h.settle();
    assert!(!h.has("banner"), "the release dismissed it");
    h.script(&["cmd:shell.motion.toggle"]);
    assert!(h.state().theme.motion.reduced);
    assert!(!h.rt.system_reduced_motion(), "the OS setting stays off");
    h.script(&["key:mod+k", "key:text:watch-dead", "key:enter"]);
    assert!(h.hold("banner-dismiss"));
    let held = node(&h, "banner-dismiss");
    assert!(
        held.get("scale").is_none(),
        "reduced motion keeps scale 1: {held}"
    );
    assert!(held["press"].as_f64().unwrap_or(0.0) > 0.0, "{held}");
}

#[test]
fn watch_dead_banner_offers_a_restart_that_reports_when_nothing_is_open() {
    let mut h = scene("watch-dead");
    assert!(h.state().workspace.is_none());
    assert!(h.has("banner-restart"));
    h.script(&["click:banner-restart"]);
    assert!(h.state().requests.is_empty());
    assert!(
        h.state().status.contains("no workspace is open"),
        "{}",
        h.state().status
    );
    assert!(h.has("banner"), "restart does not dismiss the banner");
    assert!(
        !scene("store-corrupt").has("banner-restart"),
        "only a dead watch offers it"
    );
}

#[test]
fn content_only_banner_names_the_initial_build_failure_and_reveals_it() {
    let mut h = scene("content-only");
    assert_eq!(
        h.state().app.degraded,
        Some(DegradedReason::InitialBuildFailed {
            code: "ELAYOUT".into(),
            message: "failed to load layout themes/boris/layouts/main.html: FileNotFound".into(),
        })
    );
    let tree = h.tree().to_string();
    assert!(tree.contains("Initial build failed: ELAYOUT"), "{tree}");
    assert!(tree.contains("boris init"));
    assert!(!h.has("banner-restart"), "a restart cannot fix the folder");
    let ids: Vec<_> = h.state().commands().into_iter().map(|c| c.id).collect();
    assert!(!ids.iter().any(|id| id == "watch.restart"));
    assert!(ids.iter().any(|id| id == "shell.degraded.reveal"));

    h.script(&["click:banner-reveal"]);
    let wb = h.state();
    assert_eq!(wb.registry.get(wb.active).unwrap().spec().id, "activity");
    assert_eq!(
        wb.row(wb.windows.primary.row.as_ref().unwrap().0).title,
        "ELAYOUT: failed to load layout themes/boris/layouts/main.html: FileNotFound"
    );

    h.script(&["click:banner-reopen"]);
    assert!(h.state().status.contains("no workspace is open"));
    assert!(
        h.has("banner"),
        "reopening without a workspace keeps the cause up"
    );
}

#[test]
fn nothing_draws_under_the_macos_traffic_lights() {
    let lights = Harness::traffic_lights().expect("headless runs report the default chrome");
    let hit = |r: &[f64], l: &[f32; 4]| {
        let (x, y, w, h) = (l[0] as f64, l[1] as f64, l[2] as f64, l[3] as f64);
        r[0] < x + w && x < r[0] + r[2] && r[1] < y + h && y < r[1] + r[3]
    };
    for name in ["canary", "pages", "pages-watch-dead", "audit", "light"] {
        let tree = scene(name).tree();
        for n in tree["nodes"].as_array().expect("nodes") {
            let role = n["role"].as_str().unwrap_or_default();
            if !matches!(role, "text" | "icon" | "button" | "tab" | "treeitem") {
                continue;
            }
            let r: Vec<f64> = n["rect"]
                .as_array()
                .expect("rect")
                .iter()
                .filter_map(|v| v.as_f64())
                .collect();
            assert!(
                !lights.iter().any(|l| hit(&r, l)),
                "{name}: {} sits under the traffic lights",
                n["path"]
            );
        }
    }
}
