//! Watch lifecycle: kill mid-session → restart → cursor/snapshot
//! convergence with no ghost diagnostics; lag ⇒ resync; one-shot
//! fallback when the probe says watch is unsupported; scheduler seat.

mod common;

use common::{FakeBoris, collector, payloads, wait_until};
use cv_core::diagnostic::Diagnostic;
use cv_core::engine::{Engine, Verb};
use cv_core::events::engine::{BuildPhase, EngineEvent, WatchEndReason};
use cv_core::events::{AppEvent, Envelope};
use cv_engine::cursor::{CursorTracker, Observation, channel_sink};
use cv_engine::probe::WatchMode;
use cv_engine::ring::Resume;
use cv_engine::{EngineScheduler, SeatPolicy};
use std::sync::Arc;
use std::time::Duration;

const T: Duration = Duration::from_secs(30);

fn ended(v: &[Envelope]) -> Option<WatchEndReason> {
    v.iter().find_map(|e| match &e.payload {
        AppEvent::Engine(EngineEvent::WatchSessionEnded { reason }) => Some(reason.clone()),
        _ => None,
    })
}

/// A minimal consumer: derived diagnostics + the CV cursor.
#[derive(Default)]
struct Consumer {
    tracker: CursorTracker,
    diagnostics: Vec<Diagnostic>,
    resyncs: u32,
    snapshot: Option<cv_core::rules::ArtifactSet>,
}

impl Consumer {
    fn apply(&mut self, engine: &dyn Engine, env: &Envelope) {
        match self.tracker.observe(&env.cursor) {
            Observation::Duplicate => return,
            Observation::Resync(_) => {
                // Disk is truth: drop everything derived, reread.
                self.resyncs += 1;
                self.diagnostics.clear();
                self.snapshot = Some(engine.snapshot().unwrap());
            }
            Observation::Next => {}
        }
        if let AppEvent::Engine(ev) = &env.payload {
            match ev {
                EngineEvent::BuildFailed { diagnostics, .. } => {
                    self.diagnostics = diagnostics.clone()
                }
                EngineEvent::BuildFinished { diagnostics, .. } => {
                    self.diagnostics = diagnostics.clone()
                }
                _ => {}
            }
        }
    }
}

#[test]
fn kill_mid_session_then_restart_converges_without_ghosts() {
    let fake = FakeBoris::corpus("broken-frontmatter");
    let engine = fake.engine();
    let mut consumer = Consumer::default();

    // Session 1: a failing one-shot build leaves EFRONTMATTER derived state,
    // then a watch that is killed mid-session (child still running).
    let (sink, got) = collector();
    engine.run(&Verb::Build, sink).unwrap();
    fake.set("watch-hold", "");
    let (wsink, wgot) = collector();
    let mut h = engine.watch(wsink).unwrap();
    wait_until("failed build event", T, || wgot.lock().unwrap().len() >= 2);
    for e in got.lock().unwrap().iter() {
        consumer.apply(&engine, e);
    }
    assert_eq!(consumer.diagnostics[0].code, "EFRONTMATTER");
    h.stop();
    let s1: Vec<Envelope> = wgot.lock().unwrap().clone();
    assert_eq!(ended(&s1), Some(WatchEndReason::Killed));
    for e in &s1 {
        consumer.apply(&engine, e);
    }
    let resyncs_before = consumer.resyncs;
    let old_cursor = consumer.tracker.last().unwrap().clone();

    // Author fixes the page; Boris now builds green. Restart watch.
    fake.load_corpus("happy");
    std::fs::create_dir_all(fake.root().join("dist")).unwrap();
    std::fs::write(fake.root().join("dist/index.html"), "<html></html>").unwrap();
    let (wsink, wgot) = collector();
    let mut h = engine.watch(wsink).unwrap();
    wait_until("rebuild", T, || {
        payloads(&wgot.lock().unwrap()).iter().any(|e| {
            matches!(
                e,
                EngineEvent::BuildFinished {
                    phase: BuildPhase::Rebuild,
                    ..
                }
            )
        })
    });
    h.stop();
    let s2: Vec<Envelope> = wgot.lock().unwrap().clone();
    assert_ne!(
        s2[0].cursor.session, s1[0].cursor.session,
        "new child, new session"
    );
    for e in &s2 {
        consumer.apply(&engine, e);
    }

    assert!(
        consumer.resyncs > resyncs_before,
        "session change forced a resync"
    );
    assert!(
        consumer.diagnostics.is_empty(),
        "ghost diagnostics: {:?}",
        consumer.diagnostics
    );
    assert!(
        consumer.snapshot.as_ref().unwrap().html.is_some(),
        "snapshot reread disk"
    );
    // Cursor convergence: consumer is exactly at the engine's head.
    let last = consumer.tracker.last().unwrap();
    let head = engine.cursor();
    assert_eq!(
        (last.session.clone(), last.seq + 1),
        (head.session, head.seq)
    );
    // The old session's cursor can't be replayed — only resynced.
    assert!(matches!(engine.resume(&old_cursor), Resume::Resync));
    assert!(matches!(engine.resume(last), Resume::Replay(v) if v.is_empty()));
}

#[test]
fn lag_and_ring_eviction_force_resync() {
    let fake = FakeBoris::corpus("happy");
    let mut cfg = fake.config();
    cfg.ring_capacity = 2;
    let engine = cv_engine::ProcessEngine::start(cfg).unwrap();
    let (sink, rx, lag) = channel_sink(1);
    let mut h = engine.watch(sink).unwrap();
    wait_until("watch drained", T, || !engine.watch_live());
    h.stop();
    assert!(lag.dropped() > 0, "bounded sink overflowed");
    let delivered: Vec<Envelope> = rx.try_iter().collect();
    let mut t = CursorTracker::new();
    let mut obs: Vec<Observation> = delivered.iter().map(|e| t.observe(&e.cursor)).collect();
    obs.retain(|o| matches!(o, Observation::Resync(_)));
    assert!(!obs.is_empty());
    // Ring never exceeds its bound; a cursor older than it must resync.
    assert!(engine.ring_capacity() == 2);
    let first = &delivered[0].cursor;
    assert!(matches!(engine.resume(first), Resume::Resync));
}

#[test]
fn probe_drives_one_shot_fallback() {
    let fake = FakeBoris::corpus("happy");
    fake.set(
        "watch-refuse.txt",
        "error: watch is not supported on this platform\n",
    );
    let engine = fake.engine();
    assert_eq!(engine.report().watch_mode, WatchMode::OneShotFallback);
    assert!(!engine.capabilities().watch);
    assert!(engine.report().evidence.iter().any(|e| e.contains("watch")));
    let content = fake.root().join("content");
    std::fs::create_dir_all(&content).unwrap();
    std::fs::write(content.join("index.md"), "a").unwrap();

    let (sink, got) = collector();
    let mut h = engine.watch(sink).unwrap();
    wait_until("initial", T, || got.lock().unwrap().len() >= 2);
    // Output writes never retrigger; a content edit does.
    std::fs::create_dir_all(fake.root().join("dist")).unwrap();
    std::fs::write(fake.root().join("dist/index.html"), "x").unwrap();
    std::fs::write(content.join("index.md"), "abc").unwrap();
    wait_until("rebuild", T, || got.lock().unwrap().len() >= 3);
    h.stop();
    let p = payloads(&got.lock().unwrap());
    assert!(
        matches!(&p[0], EngineEvent::WatchSessionStarted { schema: 1, compiler } if compiler.contains("fallback"))
    );
    assert!(matches!(
        &p[1],
        EngineEvent::BuildFinished {
            phase: BuildPhase::Initial,
            pages_written: 3,
            ..
        }
    ));
    assert!(matches!(
        &p[2],
        EngineEvent::BuildFinished {
            phase: BuildPhase::Rebuild,
            ..
        }
    ));
    assert!(matches!(
        p.last(),
        Some(EngineEvent::WatchSessionEnded {
            reason: WatchEndReason::Killed
        })
    ));
    let builds = fake
        .read("argv.log")
        .lines()
        .filter(|l| *l == "build")
        .count();
    assert_eq!(builds, 2, "exactly initial + one rebuild");
}

#[test]
fn scheduler_stops_watch_before_dist_writer() {
    let fake = FakeBoris::corpus("happy");
    fake.set("watch-hold", "");
    let engine = Arc::new(fake.engine());
    let sched = EngineScheduler::new(engine.clone(), SeatPolicy::StopWatch);
    let (sink, got) = collector();
    let sid = sched.start_watch(sink).unwrap();
    assert!(sched.start_watch(Box::new(|_| {})).is_err());
    let r = sched.run(&Verb::Build, Box::new(|_| {})).unwrap();
    assert_eq!(r.stopped_watch, Some(sid));
    assert_eq!(ended(&got.lock().unwrap()), Some(WatchEndReason::Killed));
    assert!(!engine.watch_live());
}

#[test]
fn content_writes_are_fingerprinted() {
    let fake = FakeBoris::new();
    let engine = fake.engine();
    let draft = cv_core::engine::PageDraft {
        path: "content/new.md".into(),
        body: "---\ntitle: x\n---\n".into(),
        base: None,
    };
    let cv_core::engine::WriteOutcome::Written { fingerprint } =
        engine.write_content(&draft, None).unwrap()
    else {
        panic!()
    };
    let again = engine.write_content(&draft, None).unwrap();
    assert!(
        matches!(again, cv_core::engine::WriteOutcome::StaleBase { found } if found == fingerprint)
    );
    let esc = cv_core::engine::PageDraft {
        path: "../escape.md".into(),
        ..draft
    };
    assert!(engine.write_content(&esc, None).is_err());
}
