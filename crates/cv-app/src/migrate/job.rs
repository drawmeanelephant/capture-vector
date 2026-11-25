//! Shell-owned migration workers. The worker only locates the gated lab and
//! calls [`run_lab`]; validation, argv construction, spawning, cancellation
//! and process-group handling remain inside `cv-engine`.

use super::{now_ms, run_lab};
use cv_engine::lab::{self as engine_lab, LabIdentity};
use cv_model::command::ImportJob;
use cv_model::new_workspace::ShellNotice;
use cv_model::workbench::Workbench;
use cv_store::imports::{self as store, RunRecord};
use std::thread::JoinHandle;

fn execute(job: &ImportJob, lab_override: Option<LabIdentity>) -> Result<(), String> {
    let mode = engine_lab::mode(&job.mode)
        .ok_or_else(|| format!("migration mode {} is not registered", job.mode))?;
    let lab = match lab_override {
        Some(lab) => lab,
        None => {
            let loc = engine_lab::locator_from_env(&job.workspace);
            engine_lab::locate(&loc, &job.workspace, engine_lab::PINNED)
                .map_err(|e| e.to_string())?
        }
    };
    run_lab(
        &job.workspace,
        &job.run_id,
        &lab,
        mode,
        job.inputs.clone(),
        job.extras.clone(),
        &job.cancel,
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

/// Start `job` on a named worker and report it as a shell notice. The
/// optional identity is a test seam for a fake lab; production passes `None`
/// and uses the ordinary pinned-lab discovery/gate.
pub fn spawn_import_with(
    wb: &mut Workbench,
    job: ImportJob,
    lab_override: Option<LabIdentity>,
) -> Option<JoinHandle<()>> {
    let notices = wb.notices();
    let notice_job = job.clone();
    let spawnfail_job = job.clone();
    let worker = std::thread::Builder::new()
        .name(format!("migrate-import-{}", job.run_id))
        .spawn(move || {
            // The wizard blocks on this notice: a panic inside execute must
            // never strand it in Running, so the terminal notice is sent
            // from behind a catch boundary either way.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                execute(&job, lab_override)
            }))
            .unwrap_or_else(|_| {
                Err("the import worker panicked; the run record shows what was written".into())
            });
            notices.send(ShellNotice::ImportFinished {
                job: notice_job,
                result,
            });
        });
    match worker {
        Ok(handle) => Some(handle),
        Err(e) => {
            // A failed spawn must not wedge the wizard either: deliver the
            // terminal notice it is waiting for, as an error.
            let notices = wb.notices();
            notices.send(ShellNotice::ImportFinished {
                job: spawnfail_job,
                result: Err(format!("the import worker could not start: {e}")),
            });
            None
        }
    }
}

/// Production worker startup: discover and gate the pinned lab inside the
/// worker so locating it never blocks the UI loop.
pub fn spawn_import(wb: &mut Workbench, job: ImportJob) -> Option<JoinHandle<()>> {
    spawn_import_with(wb, job, None)
}

/// Replay one captured mode through the checked-in `StubLab` fixture. This
/// is not a production execution path: scenes and headless tests use it to
/// exercise the same run-record/projection boundary without spawning a lab.
pub fn run_stub_lab(
    root: &std::path::Path,
    run_id: &str,
    mode_name: &str,
    inputs: Vec<(String, std::path::PathBuf)>,
    extras: Vec<(String, String)>,
) -> Result<RunRecord, String> {
    let mode = engine_lab::mode(mode_name)
        .ok_or_else(|| format!("migration mode {mode_name} is not registered"))?;
    let lock = store::claim_run(root, run_id).map_err(|e| e.to_string())?;
    let dir = lock.dir().to_path_buf();
    let run = engine_lab::LabRun {
        mode,
        inputs,
        extras,
        out: dir.join(store::OUT),
    };
    let lab = cv_fixtures::lab::StubLab::new();
    let mut rec = RunRecord::started(
        run_id,
        mode_name,
        store::RecordedLab {
            tool_id: lab.tool_id(),
            source: "fixture capture".into(),
            path: "fixtures/captures/lab".into(),
        },
        now_ms(),
    );
    let prepared = run
        .prepare()
        .map_err(|e| super::plain_prepare_refusal(&e))?;
    rec.inputs = prepared
        .inputs
        .iter()
        .map(|(flag, path)| store::input(flag, &path.display().to_string()))
        .collect();
    rec.argv = prepared
        .argv
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    store::write(&lock, &rec).map_err(|e| e.to_string())?;
    let done = match lab.run(mode_name, &run.out) {
        Ok(done) => done,
        Err(cv_fixtures::lab::StubLabError::NoCapture(mode)) => {
            return Err(format!("no captured migration-lab mode {mode}"));
        }
        Err(e) => {
            let text = e.to_string();
            let (len, sha256) =
                store::write_stderr(&lock, text.as_bytes()).map_err(|e| e.to_string())?;
            rec.stderr_bytes = Some(len);
            rec.stderr_sha256 = Some(sha256);
            rec.refused = Some(super::plain_lab_refusal(&text, &prepared.out));
            rec.finished = Some(store::Finished {
                exit_code: Some(2),
                exit: store::FinishedExit::Usage,
                outcome: "stub replay refused".into(),
                duration_ms: 0,
            });
            store::write(&lock, &rec).map_err(|e| e.to_string())?;
            return Ok(rec);
        }
    };
    let (len, sha256) =
        store::write_stderr(&lock, done.stderr.as_bytes()).map_err(|e| e.to_string())?;
    rec.stderr_bytes = Some(len);
    rec.stderr_sha256 = Some(sha256);
    if done.exit_code == 2 {
        rec.refused = Some(super::plain_lab_refusal(&done.stderr, &prepared.out));
    }
    let exit = match done.exit_code {
        0 => store::FinishedExit::Ok,
        2 => store::FinishedExit::Usage,
        _ => store::FinishedExit::Io,
    };
    rec.finished = Some(store::Finished {
        exit_code: Some(done.exit_code),
        exit,
        outcome: format!("exited {}", done.exit_code),
        duration_ms: 0,
    });
    store::write(&lock, &rec).map_err(|e| e.to_string())?;
    Ok(rec)
}
