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
    wb.inbox().send(Inbound::Envelope(env.clone()));
    wb.outcome_sink().send(env.clone());
    assert!(wb.drain());
    assert_eq!(activity_rows(&wb, "validate"), 1);

    let mut wb = Workbench::new(Registry::with_all());
    wb.outcome_sink().send(env);
    wb.reset_project(Registry::with_all());
    assert!(wb.drain());
    assert_eq!(activity_rows(&wb, "validate"), 1);
}
