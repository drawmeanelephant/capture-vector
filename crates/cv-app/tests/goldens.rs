//! Cross-OS UI gate: element-tree + layout JSON goldens per named scene,
//! driven headless in-process (no GPU). Bless: `CV_BLESS=1 cargo test -p cv-app`.

use cv_app::{SCENES, init};
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
    "empty",
    "mid-watch",
    "pages-watch-dead",
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
