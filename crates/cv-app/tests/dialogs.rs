//! The shared dialog layer through the window: modal over a scrim, named
//! for its title, one at a time, keys and buttons answering the asker.

use cv_app::{SCENES, init};
use cv_model::dialog::{DialogKind, DialogSpec};
use cv_model::workbench::{KeyInput, KeyPress, ShellRequest};
use cv_widgets::harness::Harness;
use serde_json::Value;
use std::path::PathBuf;

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

fn open_id(h: &Harness) -> Option<String> {
    h.state().dialogs.open().map(|d| d.spec.id.clone())
}

const KINDS: &[&str] = &[
    "dialog-confirm",
    "dialog-prompt",
    "dialog-form",
    "dialog-choose",
    "dialog-secret",
    "dialog-progress",
];

#[test]
fn every_kind_opens_as_a_dialog_named_for_its_title_over_a_full_window_scrim() {
    for name in KINDS {
        let h = scene(name);
        let title = h.state().dialogs.open().expect(name).spec.title.clone();
        let panel = node(&h, "dialog-dialog");
        assert_eq!(panel["role"], "dialog", "{name}");
        assert_eq!(panel["label"], title.as_str(), "{name}");
        let scrim = node(&h, "dialog-scrim");
        let [w, hh] = [scrim["rect"][2].as_f64(), scrim["rect"][3].as_f64()];
        assert_eq!((w, hh), (Some(1280.0), Some(800.0)), "{name}");
    }
}

#[test]
fn enter_runs_the_primary_action_of_a_confirm() {
    let mut h = scene("dialog-confirm");
    h.script(&["key:enter"]);
    assert_eq!(open_id(&h), None);
    let wb = h.state();
    assert!(wb.status.contains("archived"), "{}", wb.status);
}

#[test]
fn escape_cancels_only_the_open_dialog_and_the_queued_one_follows() {
    let mut h = scene("dialog-progress");
    h.state_mut()
        .dialogs
        .push(DialogSpec::new("demo.next", "Next question", DialogKind::Confirm).cancel("Close"));
    h.settle();
    assert_eq!(open_id(&h).as_deref(), Some("publish.export"));
    h.script(&["key:esc"]);
    assert_eq!(open_id(&h).as_deref(), Some("demo.next"));
    assert!(h.has("dialog-dialog"));
    h.script(&["key:esc"]);
    assert_eq!(open_id(&h), None);
    assert!(!h.has("dialog-dialog") && !h.has("dialog-scrim"));
}

#[test]
fn one_enter_answers_one_dialog_of_a_queue() {
    let mut h = scene("dialog-confirm");
    h.state_mut().dialogs.push(
        DialogSpec::new("demo.next", "Next question", DialogKind::Confirm).action(
            "ok",
            "OK",
            cv_model::dialog::ActionTone::Primary,
        ),
    );
    h.settle();
    h.script(&["key:enter"]);
    assert_eq!(open_id(&h).as_deref(), Some("demo.next"));
}

#[test]
fn the_cancel_button_dismisses_and_the_note_stays_in_the_inbox() {
    let mut h = scene("dialog-confirm");
    h.script(&["click:dialog-cancel"]);
    assert_eq!(open_id(&h), None);
    assert!(!h.state().status.contains("archived"));
}

#[test]
fn a_press_on_the_scrim_cancels() {
    let mut h = scene("dialog-prompt");
    h.rt.pointer_down(20.0, 760.0);
    h.rt.pointer_up(20.0, 760.0);
    h.settle();
    assert_eq!(open_id(&h), None);
}

#[test]
fn typing_in_the_window_fills_the_prompt_and_enter_captures_it() {
    let mut h = scene("dialog-prompt");
    let before = h.state().list_len();
    h.script(&["key:text:!", "key:enter"]);
    assert_eq!(open_id(&h), None);
    assert_eq!(h.state().list_len(), before + 1, "{}", h.state().status);
}

#[test]
fn keys_stay_in_the_dialog_while_it_is_open() {
    let mut h = scene("dialog-form");
    let selected = h.state().selected_key().map(str::to_string);
    let focus = h.state().dialogs.open().unwrap().focus;
    h.script(&["key:tab"]);
    assert_eq!(h.state().dialogs.open().unwrap().focus, focus + 1);
    h.script(&["key:shift+tab", "key:down", "key:j"]);
    assert_eq!(h.state().dialogs.open().unwrap().focus, focus);
    assert_eq!(h.state().selected_key().map(str::to_string), selected);
}

#[test]
fn choose_filters_and_enter_opens_the_highlighted_workspace() {
    let mut h = scene("dialog-choose");
    assert!(h.has("dialog-choice-1"), "handbook matches");
    assert!(!h.has("dialog-choice-2"), "dogfood is filtered out");
    h.script(&["key:enter"]);
    assert_eq!(open_id(&h), None);
    // The dialog's keymap serves shell requests on the keypress's frame;
    // the harness has no workspace opener, so the shell says so.
    assert!(h.state().requests.is_empty());
    assert!(
        h.state().status.contains("cannot open workspaces"),
        "{}",
        h.state().status
    );
}

#[test]
fn the_choose_scene_uses_platform_neutral_recent_path_text() {
    let h = scene("dialog-choose");
    assert_eq!(
        node(&h, "dialog-choice-1")["label"],
        "/Users/demo/sites/handbook"
    );
    assert_eq!(
        node(&h, "dialog-choice-0")["label"],
        "/Users/demo/sites/field-notes"
    );
}

#[test]
fn the_choice_answer_names_the_highlighted_workspace() {
    let mut wb = init();
    (SCENES
        .iter()
        .find(|(n, _)| *n == "dialog-choose")
        .unwrap()
        .1)(&mut wb);
    wb.key(&KeyPress::new(KeyInput::Enter));
    assert_eq!(
        wb.requests,
        vec![ShellRequest::OpenWorkspace(PathBuf::from(
            "/Users/demo/sites/handbook"
        ))]
    );
}
