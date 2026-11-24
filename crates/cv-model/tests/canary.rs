//! Canary slice, model side: StubEngine replays a captured Boris watch
//! session → sink → wake → drain → journal + cascade + module ingest.

use cv_core::engine::Engine;
use cv_core::events::shell::DegradedReason;
use cv_core::id::StableId;
use cv_core::store::{Namespace, Predicate, Store};
use cv_fixtures::stub_engine::StubEngine;
use cv_fixtures::stub_store::StubStore;
use cv_model::module::{Registry, Slot};
use cv_model::windows::{CloseOutcome, PRIMARY};
use cv_model::workbench::{JOURNAL, KeyInput, KeyPress, Workbench};
use serde_json::Value;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

fn press(wb: &mut Workbench, k: KeyInput) -> bool {
    wb.key(&KeyPress::new(k))
}

fn replayed(corpus: &str) -> (Workbench, Arc<AtomicUsize>) {
    let mut wb = Workbench::new(Registry::with_all()).with_store(Box::new(StubStore::new()));
    wb.activate(wb.registry.index_of("demo").expect("demo registered"));
    let wakes = Arc::new(AtomicUsize::new(0));
    let w = wakes.clone();
    wb.set_waker(Arc::new(move || {
        w.fetch_add(1, Ordering::SeqCst);
    }));
    let engine = StubEngine::new(corpus);
    let h = engine.watch(wb.sink()).expect("stub watch");
    wb.attach_watch(h);
    wb.run_command("shell.goto.demo");
    (wb, wakes)
}

#[test]
fn happy_session_wakes_commits_and_renders() {
    let (mut wb, wakes) = replayed("happy");
    assert!(
        wakes.load(Ordering::SeqCst) > 0,
        "sink must wake the UI loop"
    );
    assert!(wb.drain());
    assert!(!wb.drain(), "drain is idle once empty");
    assert_eq!(wb.app.engine.as_deref(), Some("boris/0.9.0"));
    assert_eq!(wb.app.degraded, None);
    assert_eq!(wb.app.committed, 3, "hello + initial + rebuild");
    assert_eq!(wb.list_len() as u64, wb.app.committed);
    assert!(wb.row(0).title.contains("Watch session started"));
}

#[test]
fn replay_is_idempotent_and_journaled() {
    let store = Arc::new(StubStore::new());
    struct Shared(Arc<StubStore>);
    impl Store for Shared {
        fn put(&self, ns: &Namespace, k: &StableId, d: &Value) -> cv_core::Result<()> {
            self.0.put(ns, k, d)
        }
        fn get(&self, ns: &Namespace, k: &StableId) -> cv_core::Result<Option<Value>> {
            self.0.get(ns, k)
        }
        fn query(&self, ns: &Namespace, p: &Predicate) -> cv_core::Result<Vec<Value>> {
            self.0.query(ns, p)
        }
    }
    let mut wb = Workbench::new(Registry::with_all()).with_store(Box::new(Shared(store.clone())));
    let captured = Arc::new(std::sync::Mutex::new(Vec::new()));
    let c = captured.clone();
    let _h = StubEngine::new("happy").replay_watch(Box::new(move |e| c.lock().unwrap().push(e)));
    let envs = captured.lock().unwrap().clone();
    for _ in 0..2 {
        let mut sink = wb.sink();
        envs.iter().cloned().for_each(&mut sink);
        wb.drain();
    }
    assert_eq!(
        wb.app.committed,
        envs.len() as u64,
        "same event ids: replay is a no-op"
    );
    let first = store
        .get(&Namespace(JOURNAL.into()), &StableId("stub-0".into()))
        .unwrap();
    assert!(first.is_some(), "envelope journaled through the Store port");
}

#[test]
fn a_reused_event_id_with_a_new_payload_is_not_a_replay() {
    let mut wb = Workbench::new(Registry::with_all());
    let captured = Arc::new(std::sync::Mutex::new(Vec::new()));
    let c = captured.clone();
    let _h = StubEngine::new("happy").replay_watch(Box::new(move |e| c.lock().unwrap().push(e)));
    let envs = captured.lock().unwrap().clone();
    let mut sink = wb.sink();
    envs.iter().cloned().for_each(&mut sink);
    wb.drain();
    let committed = wb.app.committed;
    // Re-send the session-start envelope with the same event_id but a
    // new session: it is a new event, not a replay.
    let mut again = envs[0].clone();
    again.cursor.session = cv_core::events::SessionId("other".into());
    let mut sink = wb.sink();
    sink(again);
    wb.drain();
    assert_eq!(wb.app.committed, committed + 1);
}

#[test]
fn rail_slots_and_keyboard_navigation() {
    let (mut wb, _) = replayed("happy");
    wb.drain();
    let demo = wb.registry.index_of("demo").unwrap();
    assert_eq!(wb.registry.get(demo).unwrap().spec().slot, Slot::Extra);
    assert!(press(&mut wb, KeyInput::Down));
    assert_eq!(wb.windows.primary.row.as_ref().map(|r| r.0), Some(0));
    press(&mut wb, KeyInput::Char('j'));
    press(&mut wb, KeyInput::Char('j'));
    assert_eq!(wb.windows.primary.row.as_ref().map(|r| r.0), Some(2));
    assert_eq!(wb.app.selection, wb.windows.primary.selection);
    press(&mut wb, KeyInput::Tab);
    assert_eq!(wb.active_nav(), Some("pages"));
    assert_eq!(wb.list_len(), 5_000);
    assert!(
        wb.windows.primary.row.is_none(),
        "nav change clears selection"
    );
    wb.select_key("page-4999");
    assert_eq!(wb.windows.primary.row.as_ref().map(|r| r.0), Some(4998));
}

#[test]
fn palette_dispatches_lane_commands() {
    let mut wb = Workbench::new(Registry::with_all());
    assert!(press(&mut wb, KeyInput::Command('k')));
    assert!(wb.palette.open);
    for c in "store-corrupt".chars() {
        press(&mut wb, KeyInput::Char(c));
    }
    press(&mut wb, KeyInput::Enter);
    assert!(!wb.palette.open);
    assert_eq!(wb.app.degraded, Some(DegradedReason::StoreCorrupt));
    wb.run_command("shell.degraded.dismiss");
    assert_eq!(wb.app.degraded, None);
    wb.run_command("shell.theme.toggle");
    assert_eq!(wb.theme.mode, cv_model::theme::Mode::Light);
    wb.run_command("shell.motion.toggle");
    assert!(wb.theme.motion.reduced);
}

#[test]
fn demo_exercises_every_degraded_state() {
    let mut wb = Workbench::new(Registry::with_all());
    let cases = [
        ("demo.degrade.engine-absent", DegradedReason::EngineAbsent),
        (
            "demo.degrade.version-mismatch",
            DegradedReason::EngineVersionMismatch {
                found: "0.8.2".into(),
                want: "0.9.0".into(),
            },
        ),
        ("demo.degrade.watch-dead", DegradedReason::WatchDead),
        ("demo.degrade.store-corrupt", DegradedReason::StoreCorrupt),
    ];
    for (cmd, want) in cases {
        wb.run_command(cmd);
        assert_eq!(wb.app.degraded, Some(want), "{cmd}");
    }
}

#[test]
fn corrupt_store_is_disclosed() {
    struct Broken;
    impl Store for Broken {
        fn put(&self, _: &Namespace, _: &StableId, _: &Value) -> cv_core::Result<()> {
            Err(cv_core::Error::Store("checksum mismatch".into()))
        }
        fn get(&self, _: &Namespace, _: &StableId) -> cv_core::Result<Option<Value>> {
            Ok(None)
        }
        fn query(&self, _: &Namespace, _: &Predicate) -> cv_core::Result<Vec<Value>> {
            Ok(vec![])
        }
    }
    let mut wb = Workbench::new(Registry::with_all()).with_store(Box::new(Broken));
    let _h = StubEngine::new("happy").replay_watch(wb.sink());
    wb.drain();
    assert_eq!(wb.app.degraded, Some(DegradedReason::StoreCorrupt));
}

#[test]
fn companion_window_and_dirty_close() {
    let (mut wb, _) = replayed("happy");
    wb.drain();
    wb.select_row(1);
    press(&mut wb, KeyInput::Enter);
    assert_eq!(wb.windows.companions.len(), 1);
    let comp = wb.windows.companions[0].id;
    wb.select_row(0);
    assert_ne!(
        wb.windows.companions[0].state.selection, wb.windows.primary.selection,
        "selection is per window"
    );
    wb.windows.set_dirty(comp, true);
    assert_eq!(
        wb.close_window(PRIMARY, false),
        CloseOutcome::NeedsConfirm(vec![comp])
    );
    assert!(matches!(
        wb.close_window(PRIMARY, true),
        CloseOutcome::Closed(_)
    ));
}

#[test]
fn journal_writes_that_failed_are_retried_on_replay() {
    struct Flaky(Arc<StubStore>, Arc<std::sync::atomic::AtomicBool>);
    impl Store for Flaky {
        fn put(&self, ns: &Namespace, k: &StableId, d: &Value) -> cv_core::Result<()> {
            if self.1.load(Ordering::SeqCst) {
                return Err(cv_core::Error::Store("disk full".into()));
            }
            self.0.put(ns, k, d)
        }
        fn get(&self, ns: &Namespace, k: &StableId) -> cv_core::Result<Option<Value>> {
            self.0.get(ns, k)
        }
        fn query(&self, ns: &Namespace, p: &Predicate) -> cv_core::Result<Vec<Value>> {
            self.0.query(ns, p)
        }
    }
    let store = Arc::new(StubStore::new());
    let failing = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let mut wb = Workbench::new(Registry::with_all())
        .with_store(Box::new(Flaky(store.clone(), failing.clone())));
    let captured = Arc::new(std::sync::Mutex::new(Vec::new()));
    let c = captured.clone();
    let _h = StubEngine::new("happy").replay_watch(Box::new(move |e| c.lock().unwrap().push(e)));
    let envs = captured.lock().unwrap().clone();
    let journaled = || {
        envs.iter()
            .filter(|e| {
                store
                    .get(&Namespace(JOURNAL.into()), &StableId(e.event_id.0.clone()))
                    .unwrap()
                    .is_some()
            })
            .count()
    };
    let mut sink = wb.sink();
    envs.iter().cloned().for_each(&mut sink);
    wb.drain();
    assert_eq!(wb.app.degraded, Some(DegradedReason::StoreCorrupt));
    assert_eq!(journaled(), 0);
    failing.store(false, Ordering::SeqCst);
    envs.iter().cloned().for_each(&mut sink);
    wb.drain();
    assert_eq!(
        wb.app.committed,
        envs.len() as u64,
        "replay applies nothing twice"
    );
    assert_eq!(
        journaled(),
        envs.len(),
        "recovered store gets every envelope"
    );
}
