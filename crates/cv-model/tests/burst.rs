//! Workbench queue and selection under rebuild bursts: undrained
//! snapshots coalesce without reordering other messages, and the
//! selection follows its key as rows move.

use cv_core::events::engine::{BuildPhase, EngineEvent};
use cv_core::events::shell::DegradedReason;
use cv_core::events::{AppEvent, BuildId, Cursor, Envelope, EventId, SessionId, SnapshotEpoch};
use cv_core::id::WorkspaceId;
use cv_model::command::{CommandSpec, Intent};
use cv_model::module::{
    Module, ModuleSpec, NavModel, PageRecord, ProjectSnapshot, ReadingModel, Registry, RowModel,
    Slot,
};
use cv_model::status::StatusNotice;
use cv_model::workbench::{Inbound, Workbench};
use std::sync::{Arc, Mutex};

type Log = Arc<Mutex<Vec<String>>>;

/// Logs every load and ingest; lists the snapshot's page paths. Uses the
/// default `index_of_key`.
struct Recorder {
    log: Log,
    rows: Vec<String>,
}

impl Module for Recorder {
    fn spec(&self) -> ModuleSpec {
        ModuleSpec {
            id: "recorder",
            title: "Recorder",
            slot: Slot::Extra,
        }
    }
    fn ingest(&mut self, env: &Envelope) -> bool {
        self.log.lock().unwrap().push(env.event_id.0.clone());
        true
    }
    fn load(&mut self, snap: &ProjectSnapshot) -> bool {
        self.log
            .lock()
            .unwrap()
            .push(format!("load {}", snap.epoch));
        self.rows = snap.pages.iter().map(|p| p.path.clone()).collect();
        true
    }
    fn nav(&self) -> NavModel {
        NavModel::default()
    }
    fn list_len(&self, _: Option<&str>) -> usize {
        self.rows.len()
    }
    fn row(&self, _: Option<&str>, i: usize) -> RowModel {
        RowModel {
            key: self.rows.get(i).cloned().unwrap_or_default(),
            ..Default::default()
        }
    }
    fn reading(&self, key: &str) -> ReadingModel {
        ReadingModel {
            title: key.into(),
            ..Default::default()
        }
    }
    fn commands(&self) -> Vec<CommandSpec> {
        vec![
            CommandSpec::new("recorder.prepend", "Prepend a row"),
            CommandSpec::new("recorder.drop-last", "Drop the last row"),
        ]
    }
    fn run(&mut self, command: &str) -> Vec<Intent> {
        match command {
            "recorder.prepend" => self.rows.insert(0, "new.md".into()),
            "recorder.drop-last" => drop(self.rows.pop()),
            _ => {}
        }
        vec![]
    }
}

fn recorder() -> (Workbench, Log) {
    let log = Log::default();
    let mut r = Registry::default();
    r.register(Box::new(Recorder {
        log: log.clone(),
        rows: vec![],
    }))
    .unwrap();
    (Workbench::new(r), log)
}

fn snapshot(epoch: u64, paths: &[&str]) -> Inbound {
    Inbound::Snapshot(ProjectSnapshot {
        epoch,
        pages: paths
            .iter()
            .map(|p| PageRecord {
                path: (*p).into(),
                id: p.trim_end_matches(".md").into(),
                title: p.trim_end_matches(".md").into(),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    })
}

fn built(seq: u64) -> Inbound {
    Inbound::Envelope(Envelope {
        cause: None,
        workspace: WorkspaceId("t".into()),
        snapshot_epoch: SnapshotEpoch(0),
        build_id: Some(BuildId(format!("b{seq}"))),
        cursor: Cursor {
            session: SessionId("t".into()),
            seq,
        },
        event_id: EventId(format!("e{seq}")),
        payload: AppEvent::Engine(EngineEvent::BuildFinished {
            phase: BuildPhase::Rebuild,
            pages_written: 0,
            diagnostics: vec![],
            duration_ms: 0,
        }),
    })
}

fn take(log: &Log) -> Vec<String> {
    std::mem::take(&mut *log.lock().unwrap())
}

#[test]
fn undrained_snapshots_coalesce_to_the_latest_in_the_first_ones_place() {
    let (mut wb, log) = recorder();
    let inbox = wb.inbox();
    for m in [
        snapshot(1, &["a.md"]),
        built(1),
        snapshot(2, &["a.md", "b.md"]),
        built(2),
        Inbound::Degraded(DegradedReason::BuildFailed),
        snapshot(3, &["a.md", "b.md", "c.md"]),
        built(3),
    ] {
        assert!(inbox.send(m));
    }
    assert!(wb.drain());
    assert_eq!(take(&log), ["load 3", "e1", "e2", "shell-1", "e3"]);
    assert_eq!(wb.app.snapshot_epoch, 3);
    assert_eq!(wb.app.committed, 4);
    assert_eq!(wb.list_len(), 3);
    assert!(!wb.drain(), "nothing left behind");
}

#[test]
fn a_snapshot_sent_after_a_drain_keeps_its_place() {
    let (mut wb, log) = recorder();
    let inbox = wb.inbox();
    inbox.send(snapshot(1, &["a.md"]));
    inbox.send(built(1));
    wb.drain();
    assert_eq!(take(&log), ["load 1", "e1"]);
    inbox.send(built(2));
    inbox.send(snapshot(2, &["a.md", "b.md"]));
    inbox.send(built(3));
    inbox.send(Inbound::Status(StatusNotice::engine_evidence("done")));
    wb.drain();
    assert_eq!(take(&log), ["e2", "load 2", "e3"]);
    assert_eq!(wb.status, "done");
}

#[test]
fn a_reset_drops_the_pending_snapshot_and_refuses_later_sends() {
    let (mut wb, log) = recorder();
    let inbox = wb.inbox();
    assert!(inbox.send(snapshot(1, &["a.md"])));
    wb.reset_project(Registry::default());
    assert!(!inbox.send(snapshot(2, &["a.md"])));
    assert!(!inbox.send(Inbound::Status(StatusNotice::engine_evidence("late"))));
    assert!(!wb.drain());
    assert!(take(&log).is_empty());
    assert_eq!(wb.app.snapshot_epoch, 0);
}

#[test]
fn reclamp_follows_the_selected_key_as_rows_move_and_clears_when_it_goes() {
    let (mut wb, _) = recorder();
    let inbox = wb.inbox();
    inbox.send(snapshot(1, &["a.md", "b.md", "c.md"]));
    wb.drain();
    wb.select_key("b.md");
    assert_eq!(wb.windows.primary.row, Some((1, "b.md".into())));
    let noun = wb.app.selection.clone();
    inbox.send(snapshot(2, &["x.md", "y.md", "a.md", "b.md"]));
    wb.drain();
    assert_eq!(wb.windows.primary.row, Some((3, "b.md".into())));
    assert_eq!(wb.app.selection, noun, "reclamp moves the index only");
    inbox.send(snapshot(3, &["b.md"]));
    wb.drain();
    assert_eq!(wb.windows.primary.row, Some((0, "b.md".into())));
    inbox.send(snapshot(4, &["a.md", "c.md"]));
    wb.drain();
    assert_eq!(wb.windows.primary.row, None);
    assert_eq!(wb.selected_key(), None);
}

#[test]
fn revalidation_after_a_command_reselects_the_moved_key_or_clears_it() {
    let (mut wb, _) = recorder();
    wb.inbox().send(snapshot(1, &["a.md", "b.md"]));
    wb.drain();
    wb.select_row(1);
    wb.run_command("recorder.prepend");
    assert_eq!(wb.windows.primary.row, Some((2, "b.md".into())));
    assert_eq!(wb.app.selection.mailbox, None);
    wb.run_command("recorder.drop-last");
    assert_eq!(wb.windows.primary.row, None);
    assert_eq!(wb.selected_key(), None);
}

#[test]
fn reclamp_tracks_a_newest_first_activity_row_across_commits() {
    let mut wb = Workbench::new(Registry::with_all());
    wb.activate(wb.registry.index_of("activity").unwrap());
    wb.set_nav("all");
    let inbox = wb.inbox();
    inbox.send(built(1));
    inbox.send(Inbound::Degraded(DegradedReason::BuildFailed));
    inbox.send(Inbound::Degraded(DegradedReason::WatchDead));
    wb.drain();
    assert_eq!(wb.list_len(), 3);
    let key = wb.row(1).key;
    wb.select_row(1);
    inbox.send(Inbound::Degraded(DegradedReason::BuildFailed));
    wb.drain();
    assert_eq!(wb.windows.primary.row, Some((2, key.clone())));
    assert_eq!(wb.row(2).key, key);
}
