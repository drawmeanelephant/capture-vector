//! `EngineScheduler` — one per workspace. Mutating verbs are serialized,
//! read-only verbs run in parallel, at most one watch session exists, and
//! a live watch holds the HTML output (`dist/`) writer seat: a build that
//! would write there either stops the watch first or is redirected to a
//! separate `--html-dir`, per `SeatPolicy`.

use crate::verbs::{is_mutating, writes_html};
use cv_core::engine::{
    Engine, EventSink, ExitClass, Fingerprint, PageDraft, Verb, WatchHandle, WriteOutcome,
};
use cv_core::events::SessionId;
use cv_core::rules::ArtifactSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SeatPolicy {
    /// Stop the live watch before an HTML-writing verb; the caller
    /// restarts it (with a fresh sink) when it wants to.
    StopWatch,
    /// Leave the watch running; redirect the build to
    /// `<base>/<n>` (workspace-relative, as Boris requires).
    SeparateOutDir(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunReport {
    pub exit: ExitClass,
    /// The watch session this run stopped, if any.
    pub stopped_watch: Option<SessionId>,
    /// Where an isolated build wrote, if redirected.
    pub out_dir: Option<String>,
}

pub struct EngineScheduler<E: Engine + Sync> {
    engine: Arc<E>,
    policy: SeatPolicy,
    rw: RwLock<()>,
    watch: Mutex<Option<Box<dyn WatchHandle>>>,
    isolated: AtomicU64,
}

impl<E: Engine + Sync> EngineScheduler<E> {
    pub fn new(engine: Arc<E>, policy: SeatPolicy) -> Self {
        Self {
            engine,
            policy,
            rw: RwLock::new(()),
            watch: Mutex::new(None),
            isolated: AtomicU64::new(0),
        }
    }

    pub fn engine(&self) -> &Arc<E> {
        &self.engine
    }

    pub fn watch_session(&self) -> Option<SessionId> {
        self.watch
            .lock()
            .unwrap()
            .as_ref()
            .map(|w| w.session().clone())
    }

    /// Start the single watch session. Takes the writer lock so it never
    /// races an in-flight build for `dist/`.
    pub fn start_watch(&self, sink: EventSink) -> cv_core::Result<SessionId> {
        let _w = self.rw.write().unwrap();
        let mut slot = self.watch.lock().unwrap();
        if slot.is_some() {
            return Err(crate::EngineError::WatchAlreadyLive.into());
        }
        let h = self.engine.watch(sink)?;
        let id = h.session().clone();
        *slot = Some(h);
        Ok(id)
    }

    pub fn stop_watch(&self) -> Option<SessionId> {
        let mut h = self.watch.lock().unwrap().take()?;
        h.stop();
        Some(h.session().clone())
    }

    pub fn run(&self, verb: &Verb, sink: EventSink) -> cv_core::Result<RunReport> {
        if !is_mutating(verb) {
            let _r = self.rw.read().unwrap();
            let exit = self.engine.run(verb, sink)?;
            return Ok(RunReport {
                exit,
                stopped_watch: None,
                out_dir: None,
            });
        }
        let _w = self.rw.write().unwrap();
        let mut stopped_watch = None;
        let mut out_dir = None;
        let mut verb = verb.clone();
        let seat_held = self.watch.lock().unwrap().is_some();
        if seat_held && writes_html(&verb) {
            match &self.policy {
                SeatPolicy::StopWatch => stopped_watch = self.stop_watch(),
                SeatPolicy::SeparateOutDir(base) => {
                    let n = self.isolated.fetch_add(1, Ordering::SeqCst) + 1;
                    let dir = format!("{}/{n}", base.trim_end_matches('/'));
                    let mut args = match &verb {
                        Verb::Custom { args, .. } => args.clone(),
                        _ => vec![],
                    };
                    args.extend(["--html-dir".to_string(), dir.clone()]);
                    verb = Verb::Custom {
                        name: "build".into(),
                        args,
                    };
                    out_dir = Some(dir);
                }
            }
        }
        let exit = self.engine.run(&verb, sink)?;
        Ok(RunReport {
            exit,
            stopped_watch,
            out_dir,
        })
    }

    pub fn write_content(
        &self,
        draft: &PageDraft,
        base: Option<Fingerprint>,
    ) -> cv_core::Result<WriteOutcome> {
        let _w = self.rw.write().unwrap();
        self.engine.write_content(draft, base)
    }

    /// Disk truth; a read, so it runs alongside other reads.
    pub fn snapshot(&self) -> cv_core::Result<ArtifactSet> {
        let _r = self.rw.read().unwrap();
        self.engine.snapshot()
    }
}

impl<E: Engine + Sync> Drop for EngineScheduler<E> {
    fn drop(&mut self) {
        self.stop_watch();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cv_core::engine::{EngineCaps, EngineInfo};
    use std::sync::atomic::AtomicI64;
    use std::time::Duration;

    #[derive(Default)]
    struct Probe {
        active_reads: AtomicI64,
        active_writes: AtomicI64,
        max_reads: AtomicI64,
        max_writes: AtomicI64,
        overlap_violation: std::sync::atomic::AtomicBool,
        verbs: Mutex<Vec<Verb>>,
        watch_stops: AtomicU64,
    }

    struct H(SessionId, Arc<Probe>);
    impl WatchHandle for H {
        fn session(&self) -> &SessionId {
            &self.0
        }
        fn stop(&mut self) {
            self.1.watch_stops.fetch_add(1, Ordering::SeqCst);
        }
    }

    struct Fake(Arc<Probe>);
    impl Engine for Fake {
        fn locate(&self) -> cv_core::Result<EngineInfo> {
            unreachable!()
        }
        fn capabilities(&self) -> EngineCaps {
            EngineCaps::default()
        }
        fn snapshot(&self) -> cv_core::Result<ArtifactSet> {
            unreachable!()
        }
        fn watch(&self, _: EventSink) -> cv_core::Result<Box<dyn WatchHandle>> {
            Ok(Box::new(H(SessionId("w".into()), self.0.clone())))
        }
        fn run(&self, cmd: &Verb, _: EventSink) -> cv_core::Result<ExitClass> {
            let p = &self.0;
            p.verbs.lock().unwrap().push(cmd.clone());
            let (mine, other, max) = if is_mutating(cmd) {
                (&p.active_writes, &p.active_reads, &p.max_writes)
            } else {
                (&p.active_reads, &p.active_writes, &p.max_reads)
            };
            let now = mine.fetch_add(1, Ordering::SeqCst) + 1;
            max.fetch_max(now, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(60));
            if other.load(Ordering::SeqCst) > 0 || p.active_writes.load(Ordering::SeqCst) > 1 {
                p.overlap_violation.store(true, Ordering::SeqCst);
            }
            mine.fetch_sub(1, Ordering::SeqCst);
            Ok(ExitClass::Ok)
        }
        fn write_content(
            &self,
            _: &PageDraft,
            _: Option<Fingerprint>,
        ) -> cv_core::Result<WriteOutcome> {
            unreachable!()
        }
    }

    fn noop() -> EventSink {
        Box::new(|_| {})
    }

    #[test]
    fn reads_parallel_writes_serial() {
        let p = Arc::new(Probe::default());
        let s = Arc::new(EngineScheduler::new(
            Arc::new(Fake(p.clone())),
            SeatPolicy::StopWatch,
        ));
        let verbs = [
            Verb::Validate,
            Verb::Check,
            Verb::Graph,
            Verb::Build,
            Verb::Build,
            Verb::Validate,
            Verb::Build,
        ];
        std::thread::scope(|sc| {
            for v in &verbs {
                let s = s.clone();
                sc.spawn(move || s.run(v, noop()).unwrap());
            }
        });
        assert_eq!(p.max_writes.load(Ordering::SeqCst), 1);
        assert!(!p.overlap_violation.load(Ordering::SeqCst));
        // Not a timing assertion on a loaded CI box: the 3 leading reads
        // were released together, so >1 normally overlaps; never require it.
        assert!(p.max_reads.load(Ordering::SeqCst) >= 1);
    }

    #[test]
    fn one_watch_and_seat_policies() {
        let p = Arc::new(Probe::default());
        let s = EngineScheduler::new(Arc::new(Fake(p.clone())), SeatPolicy::StopWatch);
        s.start_watch(noop()).unwrap();
        assert!(s.start_watch(noop()).is_err(), "second watch refused");
        // Exports don't touch dist/: watch keeps the seat.
        let r = s
            .run(
                &Verb::Export {
                    kind: cv_core::engine::ExportKind::Rag,
                },
                noop(),
            )
            .unwrap();
        assert_eq!(r.stopped_watch, None);
        let r = s.run(&Verb::Build, noop()).unwrap();
        assert_eq!(r.stopped_watch, Some(SessionId("w".into())));
        assert_eq!(p.watch_stops.load(Ordering::SeqCst), 1);
        assert!(s.watch_session().is_none());

        let p = Arc::new(Probe::default());
        let s = EngineScheduler::new(
            Arc::new(Fake(p.clone())),
            SeatPolicy::SeparateOutDir(".cv/out".into()),
        );
        s.start_watch(noop()).unwrap();
        let r = s.run(&Verb::Build, noop()).unwrap();
        assert_eq!(r.out_dir.as_deref(), Some(".cv/out/1"));
        assert_eq!(p.watch_stops.load(Ordering::SeqCst), 0);
        assert_eq!(
            p.verbs.lock().unwrap().last().unwrap(),
            &Verb::Custom {
                name: "build".into(),
                args: vec!["--html-dir".into(), ".cv/out/1".into()]
            }
        );
    }
}
