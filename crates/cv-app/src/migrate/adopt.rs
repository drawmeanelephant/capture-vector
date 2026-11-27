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
use cv_core::events::AppEvent;
use cv_core::events::engine::EngineEvent;
use cv_core::events::shell::DegradedReason;
use cv_engine::adopt::{self, TxReport};
use cv_engine::init::Initializer;
use cv_engine::locate::{Locator, PINNED};
use cv_engine::{EngineConfig, EngineScheduler, ProcessEngine, SeatPolicy};
use cv_model::adoption::{AdoptionJob, AdoptionReport, AdoptionTarget};
use cv_model::new_workspace::ShellNotice;
use cv_model::workbench::{Inbound, Inbox, OutcomeSink, Workbench};
use cv_model::workspace_key::WorkspaceKey;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
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

fn terminal_outcome(env: &cv_core::events::Envelope) -> bool {
    matches!(
        env.payload,
        AppEvent::Engine(EngineEvent::VerbReported(_) | EngineEvent::VerbFailed { .. })
    )
}

type SharedOutcomes = Arc<Mutex<Vec<(u64, cv_core::events::Envelope)>>>;

/// Terminal outcomes recorded by one verb's sink. The generation-spanning
/// copy is published when the terminal envelope itself is emitted, because
/// that emission is the causal fact Activity must order. The shared gate
/// journal republishes the same envelopes once more when the gate ends;
/// workbench idempotency collapses those replays in an uninterrupted
/// generation, while a retirement during the gate makes a surviving copy
/// reach the replacement generation.
struct OutcomeRecorder {
    outcomes: OutcomeSink,
    workspace: WorkspaceKey,
    journal: SharedOutcomes,
}

impl OutcomeRecorder {
    fn record(&mut self, env: &cv_core::events::Envelope) {
        let seq = self.outcomes.send_for(self.workspace.clone(), env.clone());
        self.journal
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((seq, env.clone()));
    }
}

fn recording_sink(
    inner: EventSink,
    outcomes: &OutcomeSink,
    workspace: &WorkspaceKey,
    journal: SharedOutcomes,
) -> EventSink {
    let mut inner = inner;
    let mut recorder = OutcomeRecorder {
        outcomes: outcomes.clone(),
        workspace: workspace.clone(),
        journal,
    };
    Box::new(move |env| {
        if terminal_outcome(&env) {
            recorder.record(&env);
        }
        inner(env)
    })
}

fn publish_journal(outcomes: &OutcomeSink, workspace: &WorkspaceKey, journal: &SharedOutcomes) {
    let journal: Vec<_> = journal
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .drain(..)
        .collect();
    for (seq, env) in journal {
        outcomes.send_stamped_for(seq, workspace.clone(), env);
    }
}

/// Validate then build through the workspace's live scheduler, with the
/// live delivery pipeline's sink and the palette verb's watch-restore
/// contract. The seat policy is `StopWatch`, so a build that takes the
/// seat must end with the watch restarted — never silently dead.
fn run_gate_live(
    slot: &crate::live::EngineSlot,
    cancel: &cv_core::engine::CancelToken,
    outcomes: &OutcomeSink,
    workspace: &WorkspaceKey,
    input: &str,
) -> Result<(ExitClass, Option<ExitClass>), String> {
    let sched = slot
        .get()
        .cloned()
        .ok_or_else(|| "the workspace engine is not ready".to_string())?;
    // Shell-generation retirement is not adoption cancellation. The gate
    // is a workspace operation serialized by the stable scheduler; while a
    // reopen is between generations its streaming sink can be absent, so
    // terminal truth is owned independently of shell-generation delivery.
    let watching = sched.watch_session();
    let run = |verb: Verb, sink: EventSink| sched.run_reported(&verb, sink, cancel).map(|r| r.exit);
    // Record terminal outcomes independently of the shell generation that
    // happens to receive streaming delivery. Replays deduplicate in an
    // uninterrupted generation; the generation-spanning outcome queue
    // preserves them when retirement drops the inbox that owned the sink.
    let journal: SharedOutcomes = Arc::new(Mutex::new(Vec::new()));
    let sink = || {
        let inner = slot
            .live_sink()
            .unwrap_or_else(|| Box::new(|_: cv_core::events::Envelope| {}));
        recording_sink(inner, outcomes, workspace, journal.clone())
    };
    let result =
        adopt::validate_then_build_with_input(&run, &sink, Some(input)).map_err(|e| e.to_string());
    publish_journal(outcomes, workspace, &journal);
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
    outcomes: &OutcomeSink,
    workspace: &WorkspaceKey,
    input: &str,
) -> Result<(ExitClass, Option<ExitClass>), String> {
    let mut cfg = EngineConfig::new(root);
    cfg.locator = locator.clone();
    cfg.content_input = input.into();
    let engine = ProcessEngine::start(cfg).map_err(|e| e.to_string())?;
    let scheduler = EngineScheduler::new(Arc::new(engine), SeatPolicy::StopWatch);
    let run =
        |verb: Verb, sink: EventSink| scheduler.run_reported(&verb, sink, cancel).map(|r| r.exit);
    let journal: SharedOutcomes = Arc::new(Mutex::new(Vec::new()));
    let sink = || recording_sink(inbox.sink(), outcomes, workspace, journal.clone());
    let result =
        adopt::validate_then_build_with_input(&run, &sink, Some(input)).map_err(|e| e.to_string());
    publish_journal(outcomes, workspace, &journal);
    result
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
/// half-copied tree. The writer lock is therefore taken first and the
/// live watch stopped inside it: no queued watch start can win the gap
/// and observe the batch. Restoration happens after the writer lock is
/// released because starting a watch reacquires it; panics cross that
/// boundary under `catch_unwind` and are resumed only after the restore
/// guard exists.
#[cfg(test)]
pub(crate) fn arm_batch_entry_probe(
    workspace: std::path::PathBuf,
    tx: std::sync::mpsc::Sender<()>,
) {
    batch_entry_probes()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push((workspace, tx));
}

#[cfg(test)]
fn batch_entry_probes()
-> &'static std::sync::Mutex<Vec<(std::path::PathBuf, std::sync::mpsc::Sender<()>)>> {
    static PROBES: std::sync::Mutex<Vec<(std::path::PathBuf, std::sync::mpsc::Sender<()>)>> =
        std::sync::Mutex::new(Vec::new());
    &PROBES
}

#[cfg(test)]
fn notify_batch_entry_probe(workspace: &std::path::Path) {
    if std::thread::current().name() != Some("cv-migrate-adopt") {
        return;
    }
    let mut probes = batch_entry_probes()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let mut index = 0;
    while index < probes.len() {
        if probes[index].0 == workspace {
            let (_, tx) = probes.remove(index);
            let _ = tx.send(());
        } else {
            index += 1;
        }
    }
}

pub(crate) fn batch_watch_safe<R>(
    slot: &crate::live::EngineSlot,
    sched: &Arc<crate::live::Scheduler>,
    batch: impl FnOnce() -> R,
) -> R {
    // Take the writer lock first. Only then may the live watch stop: a
    // `start_watch` queued ahead of this batch cannot enter afterwards
    // because watch starts are serialized by the same writer lock. The
    // batch itself runs inside the lock; restoration happens after the
    // lock is released because `start_watch` reacquires it. Panics are
    // caught only across the lock boundary, then resumed after restore,
    // so unwinding cannot strand the workspace unobserved or deadlock in
    // the guard.
    #[cfg(test)]
    notify_batch_entry_probe(sched.engine().root());
    let mut watching = None;
    let outcome = sched.mutation_batch(|| {
        watching = sched.watch_session();
        if let Some(session) = &watching {
            sched.stop_watch_session(session);
        }
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(batch))
    });
    let _restore = WatchRestore { slot, watching };
    match outcome {
        Ok(value) => value,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

fn destination_display(content_input: &str, subfolder: Option<&str>) -> String {
    let mut parts = Vec::new();
    let input = content_input.trim_matches('/');
    if !input.is_empty() && input != "." {
        parts.push(input);
    }
    if let Some(subfolder) = subfolder {
        parts.push(subfolder.trim_matches('/'));
    }
    let out = parts.join("/");
    if out.is_empty() { ".".into() } else { out }
}

fn finish_report(
    workspace: &Path,
    tx: &TxReport,
    gate: (ExitClass, Option<ExitClass>),
    new_workspace: bool,
    pages: usize,
    assets: usize,
    destination: String,
) -> AdoptionReport {
    AdoptionReport {
        workspace: workspace.to_path_buf(),
        files_written: tx.written + tx.resumed,
        pages,
        assets,
        destination,
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
    outcomes: &OutcomeSink,
) -> Result<AdoptionReport, String> {
    let err = |e: adopt::AdoptError| e.to_string();
    let src = adopt::source(&job.source_root, &job.request.run_id).map_err(err)?;
    let pages = src.pages.len();
    let assets = src.files.len().saturating_sub(pages);
    let at_ms = now_ms();
    match &job.request.target {
        AdoptionTarget::ExistingWorkspace { subfolder } => {
            let root = job.source_root.clone();
            // Diagnose malformed identity evidence before interpreting a
            // necessarily failed registry lookup as a missing engine slot.
            // The valid pre-corruption key cannot be trusted for mutation,
            // so this stays fail-closed while entering the persistent
            // degraded channel in the live generation.
            let workspace_key = slots::workspace_key_lookup(&root);
            if let Some(identity_error) = workspace_key.instance_error() {
                inbox.send(Inbound::Degraded(DegradedReason::StoreCorrupt));
                return Err(format!(
                    "adoption refused: workspace identity evidence is malformed ({identity_error}); nothing was written"
                ));
            }
            let slot = slots::slot_for(&root)
                .ok_or_else(|| "the workspace engine slot is not registered".to_string())?;
            let sched = slot.get().cloned().ok_or_else(|| {
                "the workspace engine is not ready; nothing was written".to_string()
            })?;
            // Capture the already-open workspace's identity before mutation.
            // This lookup must not mint or recreate the root if the workspace
            // moves mid-adoption; the captured evidence is the owner outcomes
            // are held for at the surviving path.
            let instance_token = workspace_key.instance_token().ok_or_else(|| {
                "adoption refused: workspace identity evidence is unavailable or degraded; nothing was written".to_string()
            })?;
            let mutation_lock_identity = cv_store::workspace_mutation::current_identity(&root)
                .map_err(|e| {
                    format!(
                        "adoption refused: workspace mutation-lock evidence is unavailable ({e}); nothing was written"
                    )
                })?;
            let expected_instance = adopt::ExpectedWorkspaceInstance::new(
                instance_token,
                workspace_key.file_identity(),
                mutation_lock_identity,
            );
            // Preflight through copy, provenance, and sidecars as one
            // serialized, journaled transaction: no page edit, move,
            // build, or watch start can interleave with the batch. The
            // content input and existing-id snapshot are read only after
            // that serialization point is entered; a pre-lock snapshot
            // would be exactly the stale state the batch exists to exclude.
            let mut adopted_input = String::new();
            let tx = batch_watch_safe(&slot, &sched, || {
                if let Err(e) = sched.readiness() {
                    let _ = e;
                    return Err(adopt::AdoptError::Run(
                        "the workspace engine is not ready; nothing was written".into(),
                    ));
                }
                let input = crate::snapshot::content_input(&root);
                adopted_input = input.clone();
                let scan_root = root.clone();
                adopt::transact_existing_with_identity(adopt::ExistingWorkspaceTransaction {
                    target_root: &root,
                    src: &src,
                    content_input: &input,
                    merge_subfolder: Some(subfolder),
                    scan_existing_ids: Box::new(move || existing_ids(&scan_root)),
                    at_ms,
                    expected_instance: &expected_instance,
                })
            })
            .map_err(err)?;
            // Content writes emit no engine events of their own; queue
            // disk truth explicitly so Pages reflects the merge even
            // before the watch's own rebuild lands.
            slot.request_resync();
            let gate = run_gate_live(
                &slot,
                &cv_core::engine::CancelToken::new(),
                outcomes,
                &workspace_key,
                &adopted_input,
            )?;
            Ok(finish_report(
                &root,
                &tx,
                gate,
                false,
                pages,
                assets,
                destination_display(&adopted_input, Some(subfolder)),
            ))
        }
        AdoptionTarget::ExistingWorkspaceInPlace => {
            let root = job.source_root.clone();
            let workspace_key = slots::workspace_key_lookup(&root);
            if let Some(identity_error) = workspace_key.instance_error() {
                inbox.send(Inbound::Degraded(DegradedReason::StoreCorrupt));
                return Err(format!(
                    "asset transformation refused: workspace identity evidence is malformed ({identity_error}); nothing was written"
                ));
            }
            let slot = slots::slot_for(&root)
                .ok_or_else(|| "the workspace engine slot is not registered".to_string())?;
            let sched = slot.get().cloned().ok_or_else(|| {
                "the workspace engine is not ready; nothing was written".to_string()
            })?;
            let instance_token = workspace_key.instance_token().ok_or_else(|| {
                "asset transformation refused: workspace identity evidence is unavailable or degraded; nothing was written".to_string()
            })?;
            let mutation_lock_identity = cv_store::workspace_mutation::current_identity(&root)
                .map_err(|e| {
                    format!(
                        "asset transformation refused: workspace mutation-lock evidence is unavailable ({e}); nothing was written"
                    )
                })?;
            let expected_instance = adopt::ExpectedWorkspaceInstance::new(
                instance_token,
                workspace_key.file_identity(),
                mutation_lock_identity,
            );
            let mut adopted_input = String::new();
            let tx = batch_watch_safe(&slot, &sched, || {
                if sched.readiness().is_err() {
                    return Err(adopt::AdoptError::Run(
                        "the workspace engine is not ready; nothing was written".into(),
                    ));
                }
                let input = crate::snapshot::content_input(&root);
                adopted_input = input.clone();
                adopt::asset_filename::transact_existing_with_identity(
                    adopt::asset_filename::AssetFilenameTransaction {
                        target_root: &root,
                        src: &src,
                        content_input: &input,
                        at_ms,
                        expected_instance: &expected_instance,
                    },
                )
            })
            .map_err(err)?;
            slot.request_resync();
            let gate = run_gate_live(
                &slot,
                &cv_core::engine::CancelToken::new(),
                outcomes,
                &workspace_key,
                &adopted_input,
            )?;
            Ok(finish_report(
                &root,
                &tx,
                gate,
                false,
                0,
                tx.asset_moves,
                destination_display(&adopted_input, None),
            ))
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
            let workspace_key = slots::workspace_key(&root);
            let gate = run_gate_fresh(
                &root,
                &locator,
                inbox,
                &creation.cancel,
                outcomes,
                &workspace_key,
                &input,
            )?;
            if creation.cancel.is_cancelled() {
                return Err(
                    "adoption cancelled during validation; the workspace is adopted but was not opened"
                        .into(),
                );
            }
            Ok(finish_report(
                &root,
                &tx,
                gate,
                true,
                pages,
                assets,
                destination_display(&input, None),
            ))
        }
    }
}

/// Run the adoption in the background; the outcome arrives as
/// `ShellNotice::Adopted`, and engine events stream through the live
/// delivery pipeline like any other engine run.
pub fn adopt(wb: &mut Workbench, job: AdoptionJob, env: &WorkspaceEnv) -> Option<JoinHandle<()>> {
    let notices = wb.notices();
    let inbox = wb.inbox();
    let outcomes = wb.outcome_sink();
    let run_id = job.request.run_id.clone();
    let worker_notices = notices.clone();
    let env_locator = env.locator.clone();
    let worker_run = run_id.clone();
    let cancel = job.cancel.clone();
    let worker_cancel = cancel.clone();
    match std::thread::Builder::new()
        .name("cv-migrate-adopt".into())
        .spawn(move || {
            // A panic inside the transaction is a defect, not a supported
            // path — but it must never die silently here, with the wizard
            // waiting forever. The watch is already restored by the
            // drop-guard during unwinding; what remains is surfacing the
            // failure as loudly as the channel allows.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                execute(&job, env_locator, &inbox, &outcomes)
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
                cancel: worker_cancel,
                result,
            });
        }) {
        Ok(join) => Some(join),
        Err(e) => {
            notices.send(ShellNotice::Adopted {
                run_id,
                cancel,
                result: Err(format!("could not start the adoption worker: {e}")),
            });
            None
        }
    }
}

pub fn adopt_native(wb: &mut Workbench, job: AdoptionJob) {
    let _ = adopt(wb, job, &WorkspaceEnv::native());
}
