//! Acceptance gate (PROMPT-v3 §Acceptance gates, L4 §6): one
//! `BuildFinished` → findings in the Audit mailbox → snooze one → redraw
//! shows it snoozed and the rail badge updated. Scripted and headless; the
//! snooze persists in `.cv/audit/` and survives the next build and a
//! fresh shell over the same project.

use cv_app::audit::DiskPort;
use cv_model::module::Registry;
use cv_model::modules::audit;
use cv_model::workbench::Workbench;
use cv_widgets::harness::Harness;
use serde_json::Value;
use std::path::Path;

fn shell(project: &Path) -> Harness {
    let mut wb: Workbench = cv_app::init();
    audit::install(Box::new(DiskPort::open(project).expect("sidecar opens")));
    wb.reset_project(Registry::with_all());
    let mut h = Harness::new(wb);
    h.settle();
    h
}

/// One captured `boris watch` session: hello, build started, BuildFinished.
fn build(h: &mut Harness) {
    cv_app::open_corpus(h.state_mut(), cv_app::CANARY_CORPUS);
    h.settle();
}

fn text_of(h: &Harness, key: &str) -> Option<String> {
    let Value::Array(els) = h.tree()["nodes"].take() else {
        panic!("tree has nodes")
    };
    els.iter()
        .find(|e| e["key"] == key)
        .and_then(|e| e["text"].as_str().map(str::to_string))
}

/// The row's lifecycle badge as drawn (last text under `row-<key>`).
fn row_badge(h: &Harness, row: &str) -> Option<String> {
    let Value::Array(els) = h.tree()["nodes"].take() else {
        panic!("tree has nodes")
    };
    let path = els.iter().find(|e| e["key"] == row)?["path"]
        .as_str()?
        .to_string();
    els.iter()
        .filter(|e| {
            e["path"]
                .as_str()
                .is_some_and(|p| p.starts_with(&format!("{path}/")))
        })
        .filter_map(|e| e["text"].as_str())
        .next_back()
        .map(str::to_string)
}

#[test]
fn build_finished_findings_snooze_redraw() {
    let project = tempfile::tempdir().unwrap();
    let mut h = shell(project.path());
    h.script(&["click:rail-audit"]);
    let wb = h.state();
    assert_eq!(
        wb.registry.get(wb.active).map(|m| m.spec().id),
        Some("audit")
    );
    assert!(!h.has("rail-audit-badge"), "no findings before a build");
    assert!(h.has("list-empty"), "the empty mailbox says so");

    build(&mut h);
    let open = text_of(&h, "rail-audit-badge")
        .expect("BuildFinished put a count on the rail")
        .parse::<usize>()
        .unwrap();
    assert!(open > 0);
    assert_eq!(
        h.state().list_len(),
        open,
        "Open mailbox lists every open finding"
    );
    let key = h.state().row(0).key;
    let row = format!("row-{key}");
    assert_eq!(row_badge(&h, &row).as_deref(), Some("open"));

    h.script(&[&format!("click:{row}"), "cmd:audit.snooze"]);
    assert!(
        h.state().status.ends_with("snoozed"),
        "{}",
        h.state().status
    );
    assert_eq!(
        text_of(&h, "rail-audit-badge"),
        Some((open - 1).to_string()),
        "rail badge drops by one"
    );
    assert!(!h.has(&row), "snoozed finding leaves the Open view");
    h.script(&["click:nav-snoozed"]);
    assert!(h.has(&row));
    assert_eq!(row_badge(&h, &row).as_deref(), Some("snoozed"));

    let journal = project.path().join(".cv/audit/lifecycle.jsonl");
    let line = std::fs::read_to_string(&journal).unwrap();
    assert!(
        line.contains("\"schema_version\":1") && line.contains("\"to\":\"Ignored\""),
        "{line}"
    );

    // The finding re-emits on the next build and keeps its snooze.
    build(&mut h);
    assert_eq!(
        text_of(&h, "rail-audit-badge"),
        Some((open - 1).to_string())
    );
    assert_eq!(row_badge(&h, &row).as_deref(), Some("snoozed"));

    // A fresh shell over the same project reads the sidecar.
    drop(h);
    let mut h = shell(project.path());
    build(&mut h);
    h.script(&["click:rail-audit", "click:nav-snoozed"]);
    assert_eq!(
        text_of(&h, "rail-audit-badge"),
        Some((open - 1).to_string())
    );
    assert_eq!(row_badge(&h, &row).as_deref(), Some("snoozed"));
}
