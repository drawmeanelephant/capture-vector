//! Generation-spanning terminal outcome delivery.

use cv_core::events::engine::{EngineEvent, VerbReport};
use cv_core::events::{AppEvent, Cursor, Envelope, EventId, SessionId, SnapshotEpoch};
use cv_core::id::WorkspaceId;
use cv_model::module::Registry;
use cv_model::modules::activity;
use cv_model::workbench::{Inbound, Workbench};

fn validate_outcome(id: &str) -> Envelope {
    Envelope {
        workspace: WorkspaceId("ws".into()),
        snapshot_epoch: SnapshotEpoch(0),
        build_id: None,
        cursor: Cursor {
            session: SessionId("adoption-gate".into()),
            seq: 1,
        },
        event_id: EventId(id.into()),
        cause: None,
        payload: AppEvent::Engine(EngineEvent::VerbReported(VerbReport {
            verb: "validate".into(),
            argv: vec!["validate".into()],
            exit: cv_core::engine::ExitClass::Ok,
            code: Some(0),
            duration_ms: 1,
            stdout: String::new(),
            stderr: String::new(),
        })),
    }
}

fn activity_rows(wb: &Workbench, prefix: &str) -> usize {
    let i = wb.registry.index_of(activity::ID).expect("activity module");
    let m = wb.registry.get(i).expect("activity module");
    (0..m.list_len(None))
        .filter(|r| m.row(None, *r).title.starts_with(prefix))
        .count()
}

#[test]
fn generation_spanning_outcomes_survive_reset_and_dedupe_live_delivery() {
    let env = validate_outcome("r9-outcome");
    let mut wb = Workbench::new(Registry::with_all());
    wb.workspace = Some(std::path::PathBuf::from("ws"));
    wb.inbox().send(Inbound::Envelope(env.clone()));
    wb.outcome_sink().send(env.clone());
    assert!(wb.drain());
    assert_eq!(activity_rows(&wb, "validate"), 1);

    let mut wb = Workbench::new(Registry::with_all());
    wb.workspace = Some(std::path::PathBuf::from("ws"));
    wb.outcome_sink().send(env);
    wb.reset_project(Registry::with_all());
    assert!(wb.drain());
    assert_eq!(activity_rows(&wb, "validate"), 1);
}

fn build_outcome(id: &str, seq: u64) -> Envelope {
    let mut env = validate_outcome(id);
    env.cursor.seq = seq;
    if let AppEvent::Engine(EngineEvent::VerbReported(r)) = &mut env.payload {
        r.verb = "build".into();
        r.argv = vec!["build".into()];
    }
    env
}

fn titles(wb: &Workbench) -> Vec<String> {
    let i = wb.registry.index_of(activity::ID).expect("activity module");
    let m = wb.registry.get(i).expect("activity module");
    (0..m.list_len(None))
        .map(|r| m.row(None, r).title)
        .collect()
}

#[test]
fn outcome_replays_precede_live_copies_in_completion_order() {
    let validate = validate_outcome("validate-r10");
    let build = build_outcome("build-r10", 2);
    let mut wb = Workbench::new(Registry::with_all());
    wb.workspace = Some(std::path::PathBuf::from("ws"));
    wb.outcome_sink().send(validate);
    wb.inbox().send(Inbound::Envelope(build));
    assert!(wb.drain());
    let titles = titles(&wb);
    let iv = titles
        .iter()
        .position(|t| t == "validate finished")
        .unwrap();
    let ib = titles.iter().position(|t| t == "build finished").unwrap();
    assert!(
        iv > ib,
        "newest-first Activity inverted completion order: {titles:?}"
    );
}

#[test]
fn outcomes_wait_for_their_owning_workspace() {
    let env = validate_outcome("foreign-r10");
    let mut wb = Workbench::new(Registry::with_all());
    wb.workspace = Some(std::path::PathBuf::from("other"));
    wb.outcome_sink().send(env);
    assert!(!wb.drain());
    assert_eq!(activity_rows(&wb, "validate"), 0);
    wb.workspace = Some(std::path::PathBuf::from("ws"));
    assert!(wb.drain());
    assert_eq!(activity_rows(&wb, "validate"), 1);
}
