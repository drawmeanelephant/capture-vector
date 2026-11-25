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
    // Ownership is the write capability for every run-record mutation.
    // Re-acquiring here is cheap (cached per active workspace) and re-proves
    // the capability at the moment of writing: a foreign-held or unsafe
    // workspace writes nothing at all. Unchecked platforms keep their
    // honestly-unproven best-effort behavior.
    let ownership = cv_engine::wlock::ownership(&job.workspace);
    use cv_engine::wlock::Ownership;
    if matches!(ownership, Ownership::Foreign | Ownership::Unsafe) {
        return Err(format!(
            "workspace {} is not owned by this process ({ownership:?}); the import cannot run here",
            job.workspace.display()
        ));
    }
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
    std::thread::Builder::new()
        .name(format!("migrate-import-{}", job.run_id))
        .spawn(move || {
            let result = execute(&job, lab_override);
            notices.send(ShellNotice::ImportFinished {
                job: notice_job,
                result,
            });
        })
        .ok()
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
    let dir = store::run_dir(root, run_id).map_err(|e| e.to_string())?;
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
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    store::write(root, &rec).map_err(|e| e.to_string())?;
    let done = match lab.run(mode_name, &run.out) {
        Ok(done) => done,
        Err(cv_fixtures::lab::StubLabError::NoCapture(mode)) => {
            return Err(format!("no captured migration-lab mode {mode}"));
        }
        Err(e) => {
            let text = e.to_string();
            rec.stderr = Some(text.clone());
            rec.refused = Some(super::plain_lab_refusal(&text, &prepared.out));
            rec.finished = Some(store::Finished {
                exit_code: Some(2),
                exit: "usage".into(),
                outcome: "stub replay refused".into(),
                duration_ms: 0,
            });
            store::write(root, &rec).map_err(|e| e.to_string())?;
            return Ok(rec);
        }
    };
    rec.stderr = Some(done.stderr.clone());
    if done.exit_code == 2 {
        rec.refused = Some(super::plain_lab_refusal(&done.stderr, &prepared.out));
    }
    let exit = match done.exit_code {
        0 => "ok",
        2 => "usage",
        _ => "io",
    };
    rec.finished = Some(store::Finished {
        exit_code: Some(done.exit_code),
        exit: exit.into(),
        outcome: format!("exited {}", done.exit_code),
        duration_ms: 0,
    });
    store::write(root, &rec).map_err(|e| e.to_string())?;
    Ok(rec)
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    #[test]
    fn execute_refuses_an_unsafe_workspace_before_writing_anything() {
        // A symlinked `.cv` makes the workspace Unsafe without needing a
        // second process: the same matches!(Foreign | Unsafe) branch covers
        // foreign-held workspaces, whose cross-process exclusion is proven
        // in cv-engine's wlock tests.
        let t = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(t.path()).unwrap();
        let outside = t.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, root.join(".cv")).unwrap();
        let job = cv_model::command::ImportJob {
            workspace: root.clone(),
            run_id: "r1".into(),
            mode: "hugo".into(),
            phase: cv_model::command::ImportPhase::Emit,
            inputs: vec![("--root".into(), root.clone())],
            extras: Vec::new(),
            cancel: cv_engine::proc::CancelToken::new(),
        };
        let err = super::execute(&job, None).unwrap_err();
        assert!(err.contains("not owned"), "{err}");
        assert!(
            !root.join(".cv").join("imports").exists()
                || std::fs::read_dir(root.join(".cv").join("imports"))
                    .map(|mut d| d.next().is_none())
                    .unwrap_or(true),
            "nothing was written"
        );
    }
}
