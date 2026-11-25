//! Shell-side migration adoption worker. The pure Migrate module produces
//! an [`AdoptionJob`]; this composition-root worker runs the copy as one
//! crash-aware transaction inside the scheduler's mutation batch — so it
//! is atomic against page edits, moves, builds, and watch starts — and
//! runs the post-adoption validate/build gate through the live delivery
//! pipeline, restoring the workspace watch exactly like a palette verb
//! when the gate takes its seat.

use crate::WorkspaceEnv;
use crate::slots;
use cv_core::engine::{EventSink, ExitClass, Verb};
use cv_engine::adopt::{self, TxReport};
use cv_engine::init::Initializer;
use cv_engine::locate::{Locator, PINNED};
use cv_engine::{EngineConfig, EngineScheduler, ProcessEngine, SeatPolicy};
use cv_model::adoption::{AdoptionJob, AdoptionReport, AdoptionTarget};
use cv_model::new_workspace::ShellNotice;
use cv_model::workbench::{Inbox, Workbench};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{SystemTime, UNIX_EPOCH};

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Existing page ids (`id` → content-input-relative path) from the
/// composition root's current Boris-view snapshot: IR-graph fields where a
/// build produced them, the marked source scan elsewhere. This is a
/// preflight disclosure; `boris validate` after the copy stays the judge.
fn existing_ids(root: &Path) -> BTreeMap<String, String> {
    crate::snapshot::workspace_snapshot(root, "dist", None, 0)
        .pages
        .into_iter()
        .map(|p| (p.id, p.path))
        .collect()
}

fn gate_text(exit: &ExitClass) -> String {
    format!("{exit:?}").to_lowercase()
}

/// Validate then build through the workspace's live scheduler, with the
/// live delivery pipeline's sink and the palette verb's watch-restore
/// contract. The seat policy is `StopWatch`, so a build that takes the
/// seat must end with the watch restarted — never silently dead.
fn run_gate_live(
    slot: &crate::live::EngineSlot,
    cancel: &cv_core::engine::CancelToken,
) -> Result<(ExitClass, Option<ExitClass>), String> {
    let sched = slot
        .get()
        .cloned()
        .ok_or_else(|| "the workspace engine is not ready".to_string())?;
    if slot.live_sink().is_none() {
        return Err("the workspace engine is not ready".into());
    }
    let watching = sched.watch_session();
    let run = |verb: Verb, sink: EventSink| sched.run_reported(&verb, sink, cancel).map(|r| r.exit);
    let sink = || {
        slot.live_sink()
            .unwrap_or_else(|| Box::new(|_: cv_core::events::Envelope| {}))
    };
    let result = adopt::validate_then_build(&run, &sink).map_err(|e| e.to_string());
    let outcome = match &result {
        Ok((v, b)) => format!(
            "adoption gate: validate {}, build {}",
            gate_text(v),
            b.as_ref()
                .map(gate_text)
                .unwrap_or_else(|| "skipped".into())
        ),
        Err(e) => format!("adoption gate did not finish: {e}"),
    };
    slot.restore_watch(watching, outcome);
    result
}

/// The fresh workspace has no live session yet: validate/build through
/// its own gated engine. Once the workspace opens, its ordinary boot
/// starts the watch that keeps diagnostics flowing.
fn run_gate_fresh(
    root: &Path,
    locator: &Locator,
    inbox: &Inbox,
    cancel: &cv_core::engine::CancelToken,
) -> Result<(ExitClass, Option<ExitClass>), String> {
    let mut cfg = EngineConfig::new(root);
    cfg.locator = locator.clone();
    let engine = ProcessEngine::start(cfg).map_err(|e| e.to_string())?;
    let scheduler = EngineScheduler::new(Arc::new(engine), SeatPolicy::StopWatch);
    let run =
        |verb: Verb, sink: EventSink| scheduler.run_reported(&verb, sink, cancel).map(|r| r.exit);
    let sink = || inbox.sink();
    adopt::validate_then_build(&run, &sink).map_err(|e| e.to_string())
}

/// Restore-on-drop guard: the watch is put back even while a panic is
/// unwinding through the batch, so a panicking mutation can never strand
/// the workspace unobserved. Lock poisoning inside the scheduler is
/// tolerated there — see the scheduler — and the worker boundary
/// converts the panic into a loud failure notice. This protection covers
/// unwinding panics only: the workspace builds with the default unwind
/// strategy, but a `panic=abort` build is process-fatal and no guard or
/// catch_unwind can run — the journal's resume path is the only recovery
/// there, exactly as for a kill or a crash.
struct WatchRestore<'a> {
    slot: &'a crate::live::EngineSlot,
    watching: Option<cv_core::events::SessionId>,
}

impl Drop for WatchRestore<'_> {
    fn drop(&mut self) {
        self.slot
            .restore_watch(self.watching.take(), "adoption copy".into());
    }
}

/// Run one scheduler mutation batch with the workspace watch paused for
/// the batch's duration. `mutation_batch` alone serializes only
/// scheduler-mediated mutations and watch starts; an already-running
/// watch would still react to each file as it lands, firing builds of a
/// half-copied tree. Stopping the watch first means nothing observes the
/// intermediate state, the explicit post-batch resync delivers disk truth
/// once, and the watch is restored with the same contract the palette
/// verb route honors — in every outcome: refusal, and panic too, since
/// the guard's `Drop` rides the unwinding.
pub(crate) fn batch_watch_safe<R>(
    slot: &crate::live::EngineSlot,
    sched: &Arc<crate::live::Scheduler>,
    batch: impl FnOnce() -> R,
) -> R {
    let watching = sched.watch_session();
    if let Some(session) = &watching {
        sched.stop_watch_session(session);
    }
    let _restore = WatchRestore { slot, watching };
    sched.mutation_batch(batch)
}

fn finish_report(
    workspace: &Path,
    tx: &TxReport,
    gate: (ExitClass, Option<ExitClass>),
    new_workspace: bool,
) -> AdoptionReport {
    AdoptionReport {
        workspace: workspace.to_path_buf(),
        files_written: tx.written + tx.resumed,
        validate: gate_text(&gate.0),
        build: gate
            .1
            .as_ref()
            .map(gate_text)
            .unwrap_or_else(|| "skipped".into()),
        new_workspace,
    }
}

fn execute(
    job: &AdoptionJob,
    env_locator: Option<Locator>,
    inbox: &Inbox,
) -> Result<AdoptionReport, String> {
    let err = |e: adopt::AdoptError| e.to_string();
    let src = adopt::source(&job.source_root, &job.request.run_id).map_err(err)?;
    let at_ms = now_ms();
    match &job.request.target {
        AdoptionTarget::ExistingWorkspace { subfolder } => {
            let root = job.source_root.clone();
            let slot = slots::slot_for(&root)
                .ok_or_else(|| "the workspace engine slot is not registered".to_string())?;
            let sched = slot.get().cloned().ok_or_else(|| {
                "the workspace engine is not ready; nothing was written".to_string()
            })?;
            let input = crate::snapshot::content_input(&root);
            let ids = existing_ids(&root);
            // Preflight through copy, provenance, and sidecars as one
            // serialized, journaled transaction: no page edit, move,
            // build, or watch start can interleave with the batch.
            let tx = batch_watch_safe(&slot, &sched, || {
                adopt::transact(
                    &root,
                    &src,
                    &input,
                    Some(subfolder),
                    "existing",
                    &ids,
                    at_ms,
                )
            })
            .map_err(err)?;
            // Content writes emit no engine events of their own; queue
            // disk truth explicitly so Pages reflects the merge even
            // before the watch's own rebuild lands.
            slot.request_resync();
            let gate = run_gate_live(&slot, &cv_core::engine::CancelToken::new())?;
            Ok(finish_report(&root, &tx, gate, false))
        }
        AdoptionTarget::NewWorkspace(creation) => {
            let draft = &creation.draft;
            let target = draft.target();
            let locator = env_locator
                .clone()
                .unwrap_or_else(|| Locator::from_env(&draft.parent));
            let initializer =
                Initializer::locate(&locator, &draft.parent, PINNED).map_err(|e| e.to_string())?;
            let out = initializer
                .create(&target, &draft.archetype, &creation.cancel)
                .map_err(|e| e.to_string())?;
            if creation.cancel.is_cancelled() {
                return Err("adoption cancelled".into());
            }
            if out.output.outcome.class() != Some(ExitClass::Ok) {
                return Err(format!(
                    "boris init failed: {}",
                    out.output.stderr.lines().next().unwrap_or("unknown error")
                ));
            }
            let root = out.target;
            let input = crate::snapshot::content_input(&root);
            // Cancellation semantics: checked at phase boundaries. The
            // copy transaction itself is atomic — once started it runs to
            // convergence, because a half-written tree is exactly what the
            // journal exists to prevent; cancelling then decides whether
            // the gate runs and the workspace opens, never whether the
            // copy is half-done. Starter deletion is inside the journaled
            // transaction, so there is no unrecoverable window.
            // Nothing else can mutate a workspace that does not exist in
            // any live session yet, so the transaction needs no scheduler
            // batch here; the journal still makes a crash resumable.
            let tx = adopt::transact(&root, &src, &input, None, "new", &BTreeMap::new(), at_ms)
                .map_err(err)?;
            if creation.cancel.is_cancelled() {
                return Err(
                    "adoption cancelled after the content converged; the workspace is adopted but was not opened"
                        .into(),
                );
            }
            let gate = run_gate_fresh(&root, &locator, inbox, &creation.cancel)?;
            if creation.cancel.is_cancelled() {
                return Err(
                    "adoption cancelled during validation; the workspace is adopted but was not opened"
                        .into(),
                );
            }
            Ok(finish_report(&root, &tx, gate, true))
        }
    }
}

/// Run the adoption in the background; the outcome arrives as
/// `ShellNotice::Adopted`, and engine events stream through the live
/// delivery pipeline like any other engine run.
pub fn adopt(wb: &mut Workbench, job: AdoptionJob, env: &WorkspaceEnv) -> Option<JoinHandle<()>> {
    let notices = wb.notices();
    let inbox = wb.inbox();
    let run_id = job.request.run_id.clone();
    let worker_notices = notices.clone();
    let env_locator = env.locator.clone();
    let worker_run = run_id.clone();
    match std::thread::Builder::new()
        .name("cv-migrate-adopt".into())
        .spawn(move || {
            // A panic inside the transaction is a defect, not a supported
            // path — but it must never die silently here, with the wizard
            // waiting forever. The watch is already restored by the
            // drop-guard during unwinding; what remains is surfacing the
            // failure as loudly as the channel allows.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                execute(&job, env_locator, &inbox)
            }))
            .unwrap_or_else(|panic| {
                let detail = panic
                    .downcast_ref::<&str>()
                    .map(|s| (*s).to_string())
                    .or_else(|| panic.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "unknown panic".into());
                Err(format!(
                    "the adoption worker panicked — this is a defect, please report it. \
                     The watch was restored and any interrupted copy is resumable; detail: {detail}"
                ))
            });
            worker_notices.send(ShellNotice::Adopted {
                run_id: worker_run,
                result,
            });
        }) {
        Ok(join) => Some(join),
        Err(e) => {
            notices.send(ShellNotice::Adopted {
                run_id,
                result: Err(format!("could not start the adoption worker: {e}")),
            });
            None
        }
    }
}

pub fn adopt_native(wb: &mut Workbench, job: AdoptionJob) {
    let _ = adopt(wb, job, &WorkspaceEnv::native());
}
