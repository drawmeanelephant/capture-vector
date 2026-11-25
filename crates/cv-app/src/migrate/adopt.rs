//! Shell-side migration adoption worker. The pure Migrate module produces
//! an [`AdoptionJob`]; this composition-root worker confines every write
//! through `cv-engine::adopt` (staged, collision-checked) and runs the
//! post-adoption `validate`/`build` gate through cv-engine — for the open
//! workspace through its live scheduler, so events flow into Activity and
//! Pages like any other engine run.

use crate::WorkspaceEnv;
use crate::slots;
use cv_core::engine::{EventSink, ExitClass, Verb};
use cv_engine::adopt;
use cv_engine::init::Initializer;
use cv_engine::locate::{Locator, PINNED};
use cv_engine::proc::CancelToken;
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

fn run_gate(root: &Path, inbox: &Inbox) -> Result<(ExitClass, Option<ExitClass>), String> {
    let scheduler = slots::scheduler_for(root).ok_or_else(|| {
        "the workspace engine is not ready; adoption is refused before writing".to_string()
    })?;
    let run = |verb: Verb, sink: EventSink| scheduler.run(&verb, sink).map(|r| r.exit);
    let sink = || inbox.sink();
    adopt::validate_then_build(&run, &sink).map_err(|e| e.to_string())
}

fn run_gate_fresh(
    root: &Path,
    locator: &Locator,
    inbox: &Inbox,
) -> Result<(ExitClass, Option<ExitClass>), String> {
    let mut cfg = EngineConfig::new(root);
    cfg.locator = locator.clone();
    let engine = ProcessEngine::start(cfg).map_err(|e| e.to_string())?;
    let scheduler = EngineScheduler::new(Arc::new(engine), SeatPolicy::StopWatch);
    let run = |verb: Verb, sink: EventSink| scheduler.run(&verb, sink).map(|r| r.exit);
    let sink = || inbox.sink();
    adopt::validate_then_build(&run, &sink).map_err(|e| e.to_string())
}

fn gate_text(exit: &ExitClass) -> String {
    format!("{exit:?}").to_lowercase()
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
            let input = crate::snapshot::content_input(&root);
            let planned = adopt::plan(&src, &input, Some(subfolder)).map_err(err)?;
            adopt::collisions(&root, &src, &planned, &existing_ids(&root)).map_err(err)?;
            // Refuse before the first write when the gate cannot run:
            // content must never land without its validate/build truth.
            if slots::scheduler_for(&root).is_none() {
                return Err("the workspace engine is not ready; nothing was written".into());
            }
            let written = adopt::copy_content(&root, &planned).map_err(err)?;
            adopt::copy_provenance(&src, &root).map_err(err)?;
            adopt::record_adoption(&src, &root, at_ms, "existing", Some(subfolder), &planned)
                .map_err(err)?;
            let (validate, build) = run_gate(&root, inbox)?;
            Ok(AdoptionReport {
                workspace: root,
                files_written: written,
                validate: gate_text(&validate),
                build: build
                    .as_ref()
                    .map(gate_text)
                    .unwrap_or_else(|| "skipped".into()),
                new_workspace: false,
            })
        }
        AdoptionTarget::NewWorkspace(draft) => {
            let target = draft.target();
            let locator = env_locator
                .clone()
                .unwrap_or_else(|| Locator::from_env(&draft.parent));
            let initializer =
                Initializer::locate(&locator, &draft.parent, PINNED).map_err(|e| e.to_string())?;
            let out = initializer
                .create(&target, &draft.archetype, &CancelToken::new())
                .map_err(|e| e.to_string())?;
            if out.output.outcome.class() != Some(ExitClass::Ok) {
                return Err(format!(
                    "boris init failed: {}",
                    out.output.stderr.lines().next().unwrap_or("unknown error")
                ));
            }
            let root = out.target;
            let input = crate::snapshot::content_input(&root);
            adopt::clear_new_workspace_content(&root, &input).map_err(err)?;
            let planned = adopt::plan(&src, &input, None).map_err(err)?;
            let written = adopt::copy_content(&root, &planned).map_err(err)?;
            adopt::copy_provenance(&src, &root).map_err(err)?;
            adopt::record_adoption(&src, &root, at_ms, "new", None, &planned).map_err(err)?;
            let (validate, build) = run_gate_fresh(&root, &locator, inbox)?;
            Ok(AdoptionReport {
                workspace: root,
                files_written: written,
                validate: gate_text(&validate),
                build: build
                    .as_ref()
                    .map(gate_text)
                    .unwrap_or_else(|| "skipped".into()),
                new_workspace: true,
            })
        }
    }
}

/// Run the adoption in the background; the outcome arrives as
/// `ShellNotice::Adopted`, and engine events stream through the inbox.
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
            let result = execute(&job, env_locator, &inbox);
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
