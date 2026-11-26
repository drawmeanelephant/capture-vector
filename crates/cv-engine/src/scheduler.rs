//! `EngineScheduler` — one per workspace. Mutating verbs are serialized,
//! read-only verbs run in parallel, at most one watch session exists, and
//! a live watch holds the HTML output (`dist/`) writer seat: a build that
//! would write there either stops the watch first or is redirected to a
//! separate `--html-dir`, per `SeatPolicy`. A verb with its own output
//! dir (IR mode, `--out`) runs beside the watch, and is refused if that
//! dir is or nests with the seat.

use crate::EngineError;
use crate::verbs::{is_mutating, own_out_dir, writes_html};
use cv_core::engine::{
    CancelToken, Engine, EventSink, ExitClass, Fingerprint, MoveOutcome, PageDraft, PageMove,
    RemoveOutcome, Verb, WatchHandle, WriteOutcome,
};
use cv_core::events::{EventId, SessionId};
use cv_core::rules::ArtifactSet;
use std::path::{Component, Path};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

/// Lock poisoning is tolerated here deliberately: this lock serializes
/// access — it does not guard invariants a panic could violate. The
/// state inside (session ids, seat bookkeeping) is plain data whose
/// writes complete or don't run at all, and the filesystem truth a
/// panicking mutation batch might strand is recovered by the adoption
/// journal, not by this lock. Refusing all future access because one
/// batch panicked would brick the workspace for no consistency gain.
fn write<T>(lock: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(|e| e.into_inner())
}

fn read<T>(lock: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(|e| e.into_inner())
}

fn mutex<T>(lock: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    lock.lock().unwrap_or_else(|e| e.into_inner())
}

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
    /// Where a build wrote instead of the seat: its own output dir, or
    /// the isolated dir it was redirected to.
    pub out_dir: Option<String>,
}

/// Outcome of a fatal-watch diagnostic build. Unlike ordinary builds,
/// recovery never stops the watch currently occupying the output seat:
/// that watch is necessarily a replacement, not the fatal session whose
/// recovery was already retired.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryBuild {
    /// The diagnostic ran while no watch owned the output seat.
    Ran(ExitClass),
    /// A replacement watch was already installed; recovery is abandoned.
    WatchReplaced(SessionId),
}

/// Is `dir` a workspace-relative path that neither is, contains, nor
/// sits inside `seat`? Components compare case-insensitively, since
/// `Dist` is `dist` on the default macOS and Windows file systems.
fn clear_of_seat(dir: &str, seat: &str) -> bool {
    fn parts(p: &str) -> Option<Vec<String>> {
        Path::new(p)
            .components()
            .filter(|c| *c != Component::CurDir)
            .map(|c| match c {
                Component::Normal(n) => Some(n.to_string_lossy().to_lowercase()),
                _ => None,
            })
            .collect()
    }
    let (Some(dir), Some(seat)) = (parts(dir), parts(seat)) else {
        return false;
    };
    !dir.is_empty() && !dir.starts_with(&seat) && !seat.starts_with(&dir)
}

/// Result of an atomic watch start/publication attempt. `artifact` is
/// whatever the publication closure chose to return; it is handed back so
/// a rejected publication object can be dropped after the scheduler
/// writer lock and watch-slot mutex have been released.
pub struct PublicationOutcome<P> {
    pub result: cv_core::Result<Option<SessionId>>,
    pub artifact: Option<P>,
}

pub struct EngineScheduler<E: Engine + Sync> {
    /// Current engine incarnation. The scheduler — especially `rw` — is
    /// the workspace's durable mutation identity; the engine behind it may
    /// be refreshed when a workspace is reopened with changed config.
    engine: RwLock<Arc<E>>,
    /// Last gate/probe outcome for the installed incarnation. This is
    /// evidence state owned by the serialization identity, never a
    /// re-probe: a failed refresh closes every engine route until a later
    /// serialized refresh succeeds.
    readiness: RwLock<Result<(), String>>,
    policy: SeatPolicy,
    /// The HTML output tree a live watch writes (Boris default `dist`).
    seat: RwLock<String>,
    rw: RwLock<()>,
    watch: Mutex<Option<Box<dyn WatchHandle>>>,
    isolated: AtomicU64,
}

impl<E: Engine + Sync> EngineScheduler<E> {
    pub fn new(engine: Arc<E>, policy: SeatPolicy) -> Self {
        Self {
            engine: RwLock::new(engine),
            readiness: RwLock::new(Ok(())),
            policy,
            seat: RwLock::new("dist".into()),
            rw: RwLock::new(()),
            watch: Mutex::new(None),
            isolated: AtomicU64::new(0),
        }
    }

    /// Name the watch's HTML output tree when it is not `dist`.
    pub fn with_seat(self, html_dir: impl Into<String>) -> Self {
        *write(&self.seat) = html_dir.into();
        self
    }

    /// The current engine incarnation. Callers needing stable identity
    /// must go through a serialized scheduler operation; this snapshot is
    /// for read-only metadata/delivery wiring and may be superseded by a
    /// later config refresh.
    pub fn engine(&self) -> Arc<E> {
        read(&self.engine).clone()
    }

    /// Readiness evidence for the current scheduler identity. `Err` means
    /// the latest serialized refresh failed; operations must refuse even
    /// though the previous engine object is still retained for identity.
    pub fn readiness(&self) -> Result<(), String> {
        read(&self.readiness).clone()
    }

    fn ready(&self) -> cv_core::Result<()> {
        self.readiness().map_err(|e| {
            EngineError::Refused(format!(
                "the workspace engine is not ready after its latest refresh: {e}"
            ))
            .into()
        })
    }

    /// Construct and install a refreshed engine as one serialized mutation.
    /// Unlike constructing first and swapping later, there is no gap in
    /// which another batch can use the old incarnation while the probe's
    /// write-capability evidence is already being created; the old-config
    /// watch is stopped before the constructor can touch the input tree.
    /// A constructor error leaves the old engine installed and disclosed
    /// to the caller, with the watch stopped as reopen's restart contract.
    pub fn replace_engine_with<X: std::fmt::Display>(
        &self,
        make: impl FnOnce() -> Result<(Arc<E>, String), X>,
    ) -> Result<Arc<E>, X> {
        let _w = write(&self.rw);
        self.stop_watch();
        match make() {
            Ok((engine, html_dir)) => {
                *write(&self.engine) = engine.clone();
                *write(&self.seat) = html_dir;
                *write(&self.readiness) = Ok(());
                Ok(engine)
            }
            Err(e) => {
                *write(&self.readiness) = Err(e.to_string());
                Err(e)
            }
        }
    }

    fn seat(&self) -> String {
        read(&self.seat).clone()
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
        self.start_watch_with_publication(sink, |_| (true, ()))
            .result
            .map(|session| session.expect("an accepted start publishes"))
    }

    /// Start a watch only if its owner can publish the session handle. The
    /// publication decision is made while both the scheduler writer and the
    /// watch-slot mutex are held. An unpublished child is stopped completely
    /// before either lock is released, so it is never observable as an
    /// already-live session and cannot consume a concurrent boot's start.
    /// `Ok(None)` means the engine start succeeded but publication refused
    /// it (normally because the shell generation is dead); `Ok(Some)` means
    /// the slot contains a published session.
    ///
    /// The closure returns `(accepted, artifact)`. The artifact is carried
    /// out of the critical section untouched: if it owns a publication
    /// message whose drop can acquire scheduler/watch locks, the caller can
    /// drop that message only after this method has returned. Never drop
    /// such an object inside the closure itself.
    pub fn start_watch_with_publication<P>(
        &self,
        sink: EventSink,
        publish: impl FnOnce(&SessionId) -> (bool, P),
    ) -> PublicationOutcome<P> {
        let early = |result| PublicationOutcome {
            result,
            artifact: None,
        };
        let _w = write(&self.rw);
        if let Err(e) = self.ready() {
            return early(Err(e));
        }
        let mut slot = mutex(&self.watch);
        if slot.is_some() {
            return early(Err(crate::EngineError::WatchAlreadyLive.into()));
        }
        let engine = self.engine();
        let mut h = match engine.watch(sink) {
            Ok(h) => h,
            Err(e) => return early(Err(e)),
        };
        let id = h.session().clone();
        let (accepted, artifact) = publish(&id);
        let result = if accepted {
            *slot = Some(h);
            Ok(Some(id))
        } else {
            h.stop();
            Ok(None)
        };
        PublicationOutcome {
            result,
            artifact: Some(artifact),
        }
    }

    /// Stop the current watch atomically with respect to the slot. The
    /// handle remains occupied while its process tree is killed and its
    /// pump joined; only then may observers see the slot as empty. This
    /// closes the half-stopped window in which a start could lose to the
    /// engine incarnation's still-live watch bit.
    pub fn stop_watch(&self) -> Option<SessionId> {
        let mut slot = mutex(&self.watch);
        let h = slot.as_mut()?;
        let session = h.session().clone();
        h.stop();
        *slot = None;
        Some(session)
    }

    /// Stop the watch only while it is still `session`: a handle that
    /// outlived its session must not stop the watch that replaced it.
    /// Like `stop_watch`, the slot is not released until the process stop
    /// has converged.
    pub fn stop_watch_session(&self, session: &SessionId) -> bool {
        let mut slot = mutex(&self.watch);
        if slot.as_ref().is_none_or(|w| w.session() != session) {
            return false;
        }
        let h = slot.as_mut().expect("checked above");
        h.stop();
        *slot = None;
        true
    }

    pub fn run(&self, verb: &Verb, sink: EventSink) -> cv_core::Result<RunReport> {
        self.run_caused(verb, sink, None)
    }

    pub fn run_caused(
        &self,
        verb: &Verb,
        sink: EventSink,
        cause: Option<&EventId>,
    ) -> cv_core::Result<RunReport> {
        self.dispatch(verb, |e, v| e.run_caused(v, sink, cause))
    }

    /// A user-requested run (`Engine::run_reported`) under the same seat
    /// rules as `run`: a build stops the watch or is redirected.
    pub fn run_reported(
        &self,
        verb: &Verb,
        sink: EventSink,
        cancel: &CancelToken,
    ) -> cv_core::Result<RunReport> {
        self.dispatch(verb, |e, v| e.run_reported(v, sink, cancel))
    }

    /// Run the fatal-watch diagnostic without ordinary stop-current-watch
    /// seat policy. The writer lock is held for the whole build, so
    /// `start_watch` cannot interleave once the empty-seat check passes.
    /// The watch mutex is only inspected and is never held across a stop
    /// or join, preserving the existing writer→watch lock order.
    pub fn run_recovery_build_caused(
        &self,
        sink: EventSink,
        cause: Option<&EventId>,
    ) -> cv_core::Result<RecoveryBuild> {
        let _w = write(&self.rw);
        self.ready()?;
        if let Some(session) = self
            .watch
            .lock()
            .unwrap()
            .as_ref()
            .map(|w| w.session().clone())
        {
            return Ok(RecoveryBuild::WatchReplaced(session));
        }
        let engine = self.engine();
        let exit = engine.run_caused(&Verb::Build, sink, cause)?;
        Ok(RecoveryBuild::Ran(exit))
    }

    fn dispatch(
        &self,
        verb: &Verb,
        run: impl FnOnce(&E, &Verb) -> cv_core::Result<ExitClass>,
    ) -> cv_core::Result<RunReport> {
        let own = own_out_dir(verb);
        let check_seat = |seat: &str| -> cv_core::Result<()> {
            if let Some(dir) = &own
                && !clear_of_seat(dir, seat)
            {
                return Err(EngineError::Refused(format!(
                    "{} must write a workspace-relative dir clear of the watch's HTML output {:?}, not {dir:?}",
                    verb.name(),
                    seat
                ))
                .into());
            }
            Ok(())
        };
        if !is_mutating(verb) {
            let _r = read(&self.rw);
            self.ready()?;
            check_seat(&self.seat())?;
            let engine = self.engine();
            let exit = run(engine.as_ref(), verb)?;
            return Ok(RunReport {
                exit,
                stopped_watch: None,
                out_dir: None,
            });
        }
        let _w = write(&self.rw);
        self.ready()?;
        check_seat(&self.seat())?;
        let mut stopped_watch = None;
        let mut out_dir = own;
        let mut verb = verb.clone();
        let seat_held = mutex(&self.watch).is_some();
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
        let engine = self.engine();
        let exit = run(engine.as_ref(), &verb)?;
        Ok(RunReport {
            exit,
            stopped_watch,
            out_dir,
        })
    }

    /// Run one logical mutation batch under the writer lock, so a
    /// multi-write operation (migration adoption) cannot interleave with
    /// page edits, moves, deletes, builds, or watch starts instead of
    /// interleaving per file. The closure must not call back into the
    /// scheduler — the lock is not reentrant and doing so deadlocks.
    ///
    /// Scope, honestly: this serializes scheduler-mediated mutations
    /// only. An already-running watch is a separate process whose
    /// filesystem reactions do not take this lock; callers that need
    /// the batch unobserved (migration adoption) stop the watch first
    /// and restore it afterwards — see the composition root's
    /// `batch_watch_safe`.
    pub fn mutation_batch<R>(&self, f: impl FnOnce() -> R) -> R {
        let _w = write(&self.rw);
        f()
    }

    pub fn write_content(
        &self,
        draft: &PageDraft,
        base: Option<Fingerprint>,
    ) -> cv_core::Result<WriteOutcome> {
        let _w = write(&self.rw);
        self.ready()?;
        self.engine().write_content(draft, base)
    }

    /// Serialized fingerprint-checked delete.
    pub fn remove_content(&self, path: &str, base: Fingerprint) -> cv_core::Result<RemoveOutcome> {
        let _w = write(&self.rw);
        self.ready()?;
        self.engine().remove_content(path, base)
    }

    /// Serialized move: the destination write and source removal are
    /// covered by one writer lock against every other engine mutation.
    pub fn move_content(&self, mv: &PageMove) -> cv_core::Result<MoveOutcome> {
        let _w = write(&self.rw);
        self.ready()?;
        self.engine().move_content(mv)
    }

    /// Disk truth; a read, so it runs alongside other reads.
    pub fn snapshot(&self) -> cv_core::Result<ArtifactSet> {
        let _r = read(&self.rw);
        self.ready()?;
        self.engine().snapshot()
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
    use cv_core::events::engine::EngineEvent;
    use cv_core::events::{Cursor, Envelope, EventId, SnapshotEpoch};
    use std::sync::Condvar;
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
        fn run_caused(
            &self,
            cmd: &Verb,
            sink: EventSink,
            _: Option<&EventId>,
        ) -> cv_core::Result<ExitClass> {
            self.run(cmd, sink)
        }
        fn run_reported(
            &self,
            cmd: &Verb,
            mut sink: EventSink,
            _: &CancelToken,
        ) -> cv_core::Result<ExitClass> {
            let exit = self.run(cmd, Box::new(|_| {}))?;
            sink(Envelope {
                cause: None,
                workspace: cv_core::id::WorkspaceId("scheduler-test".into()),
                snapshot_epoch: SnapshotEpoch(0),
                build_id: None,
                cursor: Cursor {
                    session: SessionId("reported".into()),
                    seq: 0,
                },
                event_id: EventId(format!("reported-{}", cmd.name())),
                payload: cv_core::events::AppEvent::Engine(EngineEvent::VerbReported(
                    cv_core::events::engine::VerbReport {
                        verb: cmd.name(),
                        argv: vec![cmd.name()],
                        exit,
                        code: Some(0),
                        duration_ms: 0,
                        stdout: String::new(),
                        stderr: String::new(),
                    },
                )),
            });
            Ok(exit)
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

    /// Blocker-2 probe: an adoption-style multi-file batch holds the
    /// writer lock for its whole duration, so a concurrent mutating verb
    /// can never interleave between two of its writes.
    #[test]
    fn mutation_batch_is_serialized_against_verb_writes() {
        let p = Arc::new(Probe::default());
        let s = Arc::new(EngineScheduler::new(
            Arc::new(Fake(p.clone())),
            SeatPolicy::StopWatch,
        ));

        let batch_probe = p.clone();
        let batch_sched = s.clone();
        let batch = std::thread::spawn(move || {
            batch_sched.mutation_batch(|| {
                // Three slow writes inside one batch, each indistinguishable
                // from engine writes as far as the overlap detector cares.
                for _ in 0..3 {
                    let now = batch_probe.active_writes.fetch_add(1, Ordering::SeqCst) + 1;
                    batch_probe.max_writes.fetch_max(now, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(50));
                    if batch_probe.active_writes.load(Ordering::SeqCst) > 1 {
                        batch_probe.overlap_violation.store(true, Ordering::SeqCst);
                    }
                    batch_probe.active_writes.fetch_sub(1, Ordering::SeqCst);
                }
            });
        });
        std::thread::sleep(Duration::from_millis(10));
        let mut verbs = vec![];
        for _ in 0..2 {
            let s = s.clone();
            verbs.push(std::thread::spawn(move || {
                s.run(&Verb::Build, noop()).unwrap();
            }));
        }

        batch.join().unwrap();
        for v in verbs {
            v.join().unwrap();
        }
        assert!(
            !p.overlap_violation.load(Ordering::SeqCst),
            "a verb write interleaved with the in-flight mutation batch"
        );
        assert_eq!(p.max_writes.load(Ordering::SeqCst), 1);
    }

    struct StopGate {
        state: Mutex<(bool, bool)>,
        wake: Condvar,
    }

    fn released_race_probe() -> Arc<RaceProbe> {
        let probe = Arc::new(RaceProbe::default());
        probe.gate.state.lock().unwrap().1 = true;
        probe
    }

    struct RaceProbe {
        live: std::sync::atomic::AtomicBool,
        attempts: AtomicU64,
        gate: StopGate,
        stopped: std::sync::atomic::AtomicBool,
    }

    impl Default for RaceProbe {
        fn default() -> Self {
            Self {
                live: std::sync::atomic::AtomicBool::new(false),
                attempts: AtomicU64::new(0),
                gate: StopGate {
                    state: Mutex::new((false, false)),
                    wake: Condvar::new(),
                },
                stopped: std::sync::atomic::AtomicBool::new(false),
            }
        }
    }

    struct RaceWatch {
        session: SessionId,
        probe: Arc<RaceProbe>,
    }

    impl WatchHandle for RaceWatch {
        fn session(&self) -> &SessionId {
            &self.session
        }
        fn stop(&mut self) {
            {
                let mut state = self.probe.gate.state.lock().unwrap();
                state.0 = true;
                self.probe.gate.wake.notify_all();
                while !state.1 {
                    state = self.probe.gate.wake.wait(state).unwrap();
                }
            }
            self.probe.live.store(false, Ordering::SeqCst);
            self.probe.stopped.store(true, Ordering::SeqCst);
        }
    }

    struct RaceEngine(Arc<RaceProbe>);
    impl Engine for RaceEngine {
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
            self.probe().attempts.fetch_add(1, Ordering::SeqCst);
            if self.probe().live.swap(true, Ordering::SeqCst) {
                return Err(EngineError::WatchAlreadyLive.into());
            }
            Ok(Box::new(RaceWatch {
                session: SessionId("race".into()),
                probe: self.probe().clone(),
            }))
        }
        fn run(&self, _: &Verb, _: EventSink) -> cv_core::Result<ExitClass> {
            Ok(ExitClass::Ok)
        }
        fn run_caused(
            &self,
            cmd: &Verb,
            sink: EventSink,
            _: Option<&EventId>,
        ) -> cv_core::Result<ExitClass> {
            self.run(cmd, sink)
        }
        fn write_content(
            &self,
            _: &PageDraft,
            _: Option<Fingerprint>,
        ) -> cv_core::Result<WriteOutcome> {
            unreachable!()
        }
    }

    impl RaceEngine {
        fn probe(&self) -> &Arc<RaceProbe> {
            &self.0
        }
    }

    /// Publication refusal must be atomic too: the child is stopped before
    /// the start lock opens, so a replacement can never observe the
    /// unpublished transient as already-live.
    #[test]
    fn an_unpublished_watch_is_stopped_before_the_next_start() {
        let probe = released_race_probe();
        let sched =
            EngineScheduler::new(Arc::new(RaceEngine(probe.clone())), SeatPolicy::StopWatch);
        let first = sched
            .start_watch_with_publication(noop(), |_| (false, ()))
            .result
            .unwrap();
        assert_eq!(first, None);
        assert!(probe.stopped.load(Ordering::SeqCst));
        assert!(!probe.live.load(Ordering::SeqCst));
        assert!(sched.watch_session().is_none());

        let published = sched
            .start_watch_with_publication(noop(), |_| (true, ()))
            .result
            .unwrap();
        assert_eq!(published, Some(SessionId("race".into())));
        assert_eq!(sched.watch_session(), Some(SessionId("race".into())));
    }

    /// Concurrent restores serialize through the one watch seat: exactly
    /// one start publishes and the loser sees already-live, never a second
    /// engine watch or a zero-watch terminal state.
    #[test]
    fn double_restore_never_duplicates_a_watch_session() {
        for rep in 0..20 {
            let probe = released_race_probe();
            let sched = Arc::new(EngineScheduler::new(
                Arc::new(RaceEngine(probe.clone())),
                SeatPolicy::StopWatch,
            ));
            let barrier = Arc::new(std::sync::Barrier::new(2));
            let mut starts = Vec::new();
            for _ in 0..2 {
                let sched = sched.clone();
                let barrier = barrier.clone();
                starts.push(std::thread::spawn(move || {
                    barrier.wait();
                    sched.start_watch(noop())
                }));
            }
            let results: Vec<_> = starts.into_iter().map(|t| t.join().unwrap()).collect();
            assert_eq!(
                results.iter().filter(|r| r.is_ok()).count(),
                1,
                "rep {rep}: {results:?}"
            );
            assert_eq!(
                results
                    .iter()
                    .filter(|r| r
                        .as_ref()
                        .is_err_and(|e| e.to_string().contains("already live")))
                    .count(),
                1,
                "rep {rep}: {results:?}"
            );
            assert!(sched.watch_session().is_some());
            let live = sched.watch_session().unwrap();
            assert!(sched.stop_watch_session(&live));
        }
    }

    /// Round-5 concurrent stop/start shape: while a process stop is still
    /// joined inside the scheduler slot, a restore must not observe an
    /// empty slot and lose to the engine's still-live watch bit. The start
    /// waits for the stop to converge, then installs exactly one session.
    #[test]
    fn concurrent_stop_and_start_cannot_observe_a_half_stopped_watch() {
        for rep in 0..20 {
            let probe = Arc::new(RaceProbe::default());
            let sched = Arc::new(EngineScheduler::new(
                Arc::new(RaceEngine(probe.clone())),
                SeatPolicy::StopWatch,
            ));
            let first = sched.start_watch(noop()).unwrap();
            let stopper = {
                let sched = sched.clone();
                std::thread::spawn(move || sched.stop_watch_session(&first))
            };
            {
                let mut state = probe.gate.state.lock().unwrap();
                while !state.0 {
                    state = probe.gate.wake.wait(state).unwrap();
                }
            }
            let (started_tx, started_rx) = std::sync::mpsc::channel();
            let starter = {
                let sched = sched.clone();
                std::thread::spawn(move || {
                    started_tx.send(()).unwrap();
                    sched.start_watch(noop())
                })
            };
            started_rx.recv().unwrap();
            probe.gate.state.lock().unwrap().1 = true;
            probe.gate.wake.notify_all();
            assert!(stopper.join().unwrap(), "the original session stopped");
            let started = starter.join().unwrap();
            assert!(
                started.is_ok(),
                "rep {rep}: restore lost to a half-stopped watch: {started:?}"
            );
            assert!(probe.stopped.load(Ordering::SeqCst));
            assert!(sched.watch_session().is_some());
            let live = sched.watch_session().unwrap();
            assert!(sched.stop_watch_session(&live));
        }
    }

    /// A failed refresh is closed evidence owned by the scheduler: even
    /// though the old engine object remains installed for identity, no
    /// engine route may use it until a later serialized replacement passes.
    #[test]
    fn failed_engine_replacement_closes_routes_until_a_serialized_retry_succeeds() {
        let sched = EngineScheduler::new(
            Arc::new(Fake(Arc::new(Probe::default()))),
            SeatPolicy::StopWatch,
        );
        assert!(sched.readiness().is_ok());

        let err = sched
            .replace_engine_with(|| -> Result<(Arc<Fake>, String), &'static str> {
                Err("replacement probe failed")
            })
            .err()
            .expect("the replacement probe fails");
        assert_eq!(err, "replacement probe failed");
        assert!(sched.readiness().is_err());
        assert!(
            sched.run(&Verb::Build, noop()).is_err(),
            "a route must not keep using the retained engine after a failed refresh"
        );

        let retry = sched
            .replace_engine_with(|| -> Result<(Arc<Fake>, String), &'static str> {
                Ok((Arc::new(Fake(Arc::new(Probe::default()))), "dist".into()))
            })
            .expect("serialized retry installs a healthy incarnation");
        assert!(Arc::ptr_eq(&retry, &sched.engine()));
        assert!(sched.readiness().is_ok());
    }

    /// Config refresh must reuse the scheduler's serialization identity,
    /// not race it: replacement cannot enter a held mutation batch, and it
    /// retires the old-config watch before the new incarnation is visible.
    #[test]
    fn engine_replacement_is_serialized_and_stops_the_old_watch() {
        let old_probe = Arc::new(Probe::default());
        let old = Arc::new(Fake(old_probe.clone()));
        let new_probe = Arc::new(Probe::default());
        let new = Arc::new(Fake(new_probe));
        let s = Arc::new(EngineScheduler::new(old, SeatPolicy::StopWatch));
        s.start_watch(noop()).unwrap();
        assert_eq!(old_probe.watch_stops.load(Ordering::SeqCst), 0);

        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let batch_sched = s.clone();
        let batch = std::thread::spawn(move || {
            batch_sched.mutation_batch(|| {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            });
        });
        entered_rx.recv().unwrap();

        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let replace_sched = s.clone();
        let replacement = new.clone();
        let replace = std::thread::spawn(move || {
            replace_sched
                .replace_engine_with(|| {
                    Ok::<_, std::convert::Infallible>((replacement, "dist".to_string()))
                })
                .unwrap();
            done_tx.send(()).unwrap();
        });
        std::thread::sleep(Duration::from_millis(50));
        assert!(
            done_rx.try_recv().is_err(),
            "replacement entered the in-flight mutation batch"
        );
        release_tx.send(()).unwrap();
        batch.join().unwrap();
        done_rx.recv().unwrap();
        replace.join().unwrap();

        assert_eq!(
            old_probe.watch_stops.load(Ordering::SeqCst),
            1,
            "the old incarnation's watch was retired before the swap"
        );
        assert!(s.watch_session().is_none());
        assert!(Arc::ptr_eq(&s.engine(), &new));
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

    #[test]
    fn a_reported_build_takes_the_seat_like_any_build() {
        let p = Arc::new(Probe::default());
        let s = EngineScheduler::new(Arc::new(Fake(p.clone())), SeatPolicy::StopWatch);
        s.start_watch(noop()).unwrap();
        let cancel = CancelToken::new();
        let r = s
            .run_reported(&Verb::Impact { id: "index".into() }, noop(), &cancel)
            .unwrap();
        assert_eq!(r.stopped_watch, None, "a read leaves the watch alone");
        let r = s.run_reported(&Verb::Build, noop(), &cancel).unwrap();
        assert_eq!(r.stopped_watch, Some(SessionId("w".into())));
        assert_eq!(p.watch_stops.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_stale_handle_cannot_stop_the_watch_that_replaced_it() {
        let p = Arc::new(Probe::default());
        let s = EngineScheduler::new(Arc::new(Fake(p.clone())), SeatPolicy::StopWatch);
        s.start_watch(noop()).unwrap();
        assert!(!s.stop_watch_session(&SessionId("earlier".into())));
        assert_eq!(s.watch_session(), Some(SessionId("w".into())));
        assert!(s.stop_watch_session(&SessionId("w".into())));
        assert!(s.watch_session().is_none());
        assert_eq!(p.watch_stops.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn recovery_build_abandons_when_a_watch_replaces_the_fatal_session() {
        let p = Arc::new(Probe::default());
        let s = EngineScheduler::new(Arc::new(Fake(p.clone())), SeatPolicy::StopWatch);
        let fatal = s.start_watch(noop()).unwrap();
        assert!(s.stop_watch_session(&fatal));
        // A palette/manual build takes the empty writer seat first. The
        // replacement it installs is the exact interleaving delayed
        // recovery must not stop when it reaches its own writer seat.
        s.run(&Verb::Build, noop()).unwrap();
        let replacement = s.start_watch(noop()).unwrap();

        let outcome = s.run_recovery_build_caused(noop(), None).unwrap();
        assert_eq!(outcome, RecoveryBuild::WatchReplaced(replacement.clone()));
        assert_eq!(s.watch_session(), Some(replacement));
        assert_eq!(
            p.watch_stops.load(Ordering::SeqCst),
            1,
            "only the fatal session was stopped"
        );
        assert_eq!(
            p.verbs.lock().unwrap().as_slice(),
            [Verb::Build],
            "only the manual build ran; the abandoned diagnostic did not"
        );
    }

    #[test]
    fn recovery_build_runs_after_the_fatal_session_when_no_watch_intervenes() {
        let p = Arc::new(Probe::default());
        let s = EngineScheduler::new(Arc::new(Fake(p.clone())), SeatPolicy::StopWatch);
        let fatal = s.start_watch(noop()).unwrap();
        assert!(s.stop_watch_session(&fatal));

        let outcome = s.run_recovery_build_caused(noop(), None).unwrap();
        assert_eq!(outcome, RecoveryBuild::Ran(ExitClass::Ok));
        assert_eq!(p.verbs.lock().unwrap().as_slice(), [Verb::Build]);
        assert!(s.watch_session().is_none());
    }

    fn ir(out: &str) -> Verb {
        Verb::Ir {
            out: out.into(),
            input: None,
        }
    }

    #[test]
    fn an_ir_projection_runs_beside_the_watch_under_either_policy() {
        for policy in [
            SeatPolicy::StopWatch,
            SeatPolicy::SeparateOutDir(".cv/out".into()),
        ] {
            let p = Arc::new(Probe::default());
            let s = EngineScheduler::new(Arc::new(Fake(p.clone())), policy.clone());
            s.start_watch(noop()).unwrap();
            let r = s.run(&ir(crate::verbs::IR_DIR), noop()).unwrap();
            assert_eq!(r.stopped_watch, None, "{policy:?}");
            assert_eq!(r.out_dir.as_deref(), Some(".cv/ir"), "{policy:?}");
            assert_eq!(p.watch_stops.load(Ordering::SeqCst), 0, "{policy:?}");
            assert_eq!(s.watch_session(), Some(SessionId("w".into())));
            // Passed through as is, not redirected to an isolated --html-dir.
            assert_eq!(p.verbs.lock().unwrap().as_slice(), [ir(".cv/ir")]);
        }
    }

    #[test]
    fn an_own_output_verb_is_refused_the_watch_seat() {
        let p = Arc::new(Probe::default());
        let s = EngineScheduler::new(Arc::new(Fake(p.clone())), SeatPolicy::StopWatch);
        s.start_watch(noop()).unwrap();
        let custom = |out: &str| Verb::Custom {
            name: "build".into(),
            args: vec!["--out".into(), out.into()],
        };
        for verb in [
            ir("dist"),
            ir("./dist/"),
            ir("Dist"),
            ir("dist/ir"),
            ir("."),
            ir(""),
            ir("../elsewhere"),
            ir("/abs/ir"),
            custom("dist"),
        ] {
            let err = s.run(&verb, noop()).unwrap_err();
            assert!(err.to_string().contains("clear of"), "{verb:?}: {err}");
        }
        assert!(p.verbs.lock().unwrap().is_empty(), "nothing reached Boris");
        assert_eq!(p.watch_stops.load(Ordering::SeqCst), 0);

        // The seat is the configured HTML dir, not a hard-coded name.
        let p = Arc::new(Probe::default());
        let s = EngineScheduler::new(Arc::new(Fake(p.clone())), SeatPolicy::StopWatch)
            .with_seat("site");
        assert!(s.run(&ir("site/ir"), noop()).is_err());
        assert!(s.run(&ir("dist"), noop()).is_ok());
    }
}
