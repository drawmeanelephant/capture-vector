//! Generation-spanning terminal outcome delivery.

use cv_core::events::engine::{EngineEvent, VerbReport};
use cv_core::events::{AppEvent, Cursor, Envelope, EventId, SessionId, SnapshotEpoch};
use cv_core::id::WorkspaceId;
use cv_model::module::Registry;
use cv_model::modules::activity;
use cv_model::workbench::{Inbound, Workbench};
use cv_model::workspace_key::WorkspaceKey;

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

fn key(path: &str, file: u64) -> WorkspaceKey {
    WorkspaceKey::new(std::path::PathBuf::from(path), Some((7, file)))
}

fn install_key(wb: &mut Workbench, path: &str, file: u64) {
    wb.workspace_key = Some(key(path, file));
    wb.workspace = Some(std::path::PathBuf::from(path));
}

#[test]
fn generation_spanning_outcomes_survive_reset_and_dedupe_live_delivery() {
    let env = validate_outcome("r9-outcome");
    let mut wb = Workbench::new(Registry::with_all());
    install_key(&mut wb, "ws", 1);
    wb.inbox().send(Inbound::Envelope(env.clone()));
    wb.outcome_sink().send_for(key("ws", 1), env.clone());
    assert!(wb.drain());
    assert_eq!(activity_rows(&wb, "validate"), 1);

    let mut wb = Workbench::new(Registry::with_all());
    install_key(&mut wb, "ws", 1);
    wb.outcome_sink().send_for(key("ws", 1), env);
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
    install_key(&mut wb, "ws", 1);
    wb.outcome_sink().send_for(key("ws", 1), validate);
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
    wb.outcome_sink().send_for(key("ws", 1), env);
    assert!(!wb.drain());
    assert_eq!(activity_rows(&wb, "validate"), 0);
    install_key(&mut wb, "ws", 1);
    assert!(wb.drain());
    assert_eq!(activity_rows(&wb, "validate"), 1);
}

#[test]
fn causal_merge_allows_live_first_when_live_completed_first() {
    let build = build_outcome("build-r11", 1);
    let validate = validate_outcome("validate-r11");
    let mut wb = Workbench::new(Registry::with_all());
    install_key(&mut wb, "ws", 1);
    wb.inbox().send(Inbound::Envelope(build));
    wb.outcome_sink().send_for(key("ws", 1), validate);
    assert!(wb.drain());
    let titles = titles(&wb);
    let iv = titles
        .iter()
        .position(|t| t == "validate finished")
        .unwrap();
    let ib = titles.iter().position(|t| t == "build finished").unwrap();
    assert!(
        iv < ib,
        "later validate did not render newer than causally earlier live build: {titles:?}"
    );
}

#[test]
fn workspace_instance_identity_distinguishes_alias_and_recreation() {
    assert!(key("canonical", 10).same_instance(&key("firmlink-alias", 10)));
    assert!(!key("same-path", 10).same_instance(&key("same-path", 11)));
}

#[test]
fn cv_instance_evidence_follows_moves_and_rejects_inode_recycling() {
    let owner =
        WorkspaceKey::with_instance("old-path".into(), Some((7, 42)), "owner-instance".into());
    let moved =
        WorkspaceKey::with_instance("new-path".into(), Some((7, 42)), "owner-instance".into());
    let recycled = WorkspaceKey::with_instance(
        "brand-new".into(),
        Some((7, 42)),
        "recreated-instance".into(),
    );
    assert!(owner.same_instance(&moved));
    assert!(!owner.same_instance(&recycled));
    let legacy = WorkspaceKey::new("old-path".into(), Some((7, 42)));
    assert!(!owner.same_instance(&legacy));
}

#[cfg(unix)]
#[test]
fn workspace_identity_fallback_uses_exact_path_bytes_not_lossy_strings() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    let a = WorkspaceKey::new(
        std::path::PathBuf::from(OsString::from_vec(b"ws-\xff".to_vec())),
        None,
    );
    let b = WorkspaceKey::new(
        std::path::PathBuf::from(OsString::from_vec(b"ws-\xfe".to_vec())),
        None,
    );
    assert_eq!(a.path().to_string_lossy(), b.path().to_string_lossy());
    assert!(!a.same_instance(&b));
}

#[test]
fn held_foreign_outcomes_are_bounded_and_evict_oldest() {
    let mut wb = Workbench::new(Registry::with_all());
    install_key(&mut wb, "foreign-current", 99);
    let sink = wb.outcome_sink();
    for i in 0..65 {
        sink.send_for(key("owner", 1), validate_outcome(&format!("held-{i}")));
    }
    assert!(wb.drain(), "the retention disclosure is visible");
    install_key(&mut wb, "owner", 1);
    assert!(wb.drain());
    assert_eq!(activity_rows(&wb, "validate"), 64);
}

#[test]
fn mixed_workspace_identity_evidence_is_conservatively_unproven() {
    let proven = key("same-path", 10);
    let unproven = WorkspaceKey::new(std::path::PathBuf::from("same-path"), None);
    assert!(!proven.same_instance(&unproven));
}
