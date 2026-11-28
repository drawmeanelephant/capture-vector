//! Shell-owned migration workers. The worker locates the gated lab, claims
//! the collision-resolved run, and executes it under that held capability;
//! validation, argv construction, spawning, cancellation and process-group
//! handling remain inside `cv-engine`.

use super::{claim_import_run, now_ms, run_lab_with_lock};
use cv_engine::lab::{self as engine_lab, LabIdentity};
use cv_model::command::{ImportJob, ImportPhase};
use cv_model::new_workspace::{Notices, ShellNotice};
use cv_model::workbench::Workbench;
use cv_store::imports::{self as store, RunRecord};
use std::thread::JoinHandle;

fn execute(
    job: &ImportJob,
    lab_override: Option<LabIdentity>,
    notices: &Notices,
) -> Result<String, String> {
    let mode = engine_lab::mode(&job.mode)
        .ok_or_else(|| format!("migration mode {} is not registered", job.mode))?;
    let mut inputs = job.inputs.clone();
    if job.phase == ImportPhase::Utility {
        if !inputs.is_empty() {
            return Err(format!(
                "workspace utility {} does not accept picked inputs",
                job.mode
            ));
        }
        let content = crate::snapshot::content_input(&job.workspace);
        let (flag, path) = match job.mode.as_str() {
            // The watch/build output seat is the workspace's built HTML tree.
            "link-audit" => ("--root", job.workspace.join("dist")),
            "frontmatter-review" => ("--content", job.workspace.join(&content)),
            "asset-filename" => ("--root", job.workspace.join(&content)),
            other => return Err(format!("{other} is not a workspace utility mode")),
        };
        inputs = vec![(flag.to_string(), path)];
    }
    let lab = match lab_override {
        Some(lab) => lab,
        None => {
            let loc = engine_lab::locator_from_env(&job.workspace);
            engine_lab::locate(&loc, &job.workspace, engine_lab::PINNED)
                .map_err(|e| e.to_string())?
        }
    };
    // Claim after lab discovery but before any lab process exists. The
    // wizard's id came from a possibly stale projection; collision
    // resolution happens here, then the shell is told the actual id before
    // the run can complete.
    let (lock, actual_id) =
        claim_import_run(&job.workspace, &job.run_id, &job.mode).map_err(|e| e.to_string())?;
    if actual_id != job.run_id {
        let mut actual = job.clone();
        actual.run_id = actual_id.clone();
        notices.send(ShellNotice::ImportStarted {
            requested: job.run_id.clone(),
            job: actual,
        });
    }
    run_lab_with_lock(
        lock,
        &actual_id,
        &lab,
        mode,
        inputs,
        job.extras.clone(),
        &job.cancel,
    )
    .map(|_| actual_id)
    .map_err(|e| e.to_string())
}

fn link_audit_evidence(
    workspace: &std::path::Path,
    run_id: &str,
) -> Option<Vec<cv_model::modules::audit::LinkAuditEvidence>> {
    let path = workspace
        .join(".cv/imports")
        .join(run_id)
        .join("out/link_audit.json");
    let bytes = std::fs::read(path).ok()?;
    let doc = cv_contracts::lab::Document::decode("link_audit.json", &bytes).ok()?;
    let cv_contracts::lab::Document::LinkAudit(audit) = doc else {
        return None;
    };
    Some(
        audit
            .findings
            .iter()
            .map(|f| cv_model::modules::audit::LinkAuditEvidence {
                source: f.source.clone(),
                target: f.target.clone(),
                line: f.line,
                kind: f.kind.clone(),
                reason: f.reason.clone(),
            })
            .collect(),
    )
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
    let doctor = cv_model::modules::doctor::installed();
    let notice_job = job.clone();
    let spawnfail_job = job.clone();
    let worker = std::thread::Builder::new()
        .name(format!("migrate-import-{}", job.run_id))
        .spawn(move || {
            // The wizard blocks on this notice: a panic inside execute must
            // never strand it in Running, so the terminal notice is sent
            // from behind a catch boundary either way.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                execute(&job, lab_override, &notices)
            }))
            .unwrap_or_else(|_| {
                Err("the import worker panicked; the run record shows what was written".into())
            });
            doctor.refresh_store();
            let evidence = if notice_job.mode == "link-audit" {
                result.as_ref().ok().and_then(|actual_id| {
                    link_audit_evidence(&notice_job.workspace, actual_id)
                        .map(|findings| (actual_id.clone(), findings))
                })
            } else {
                None
            };
            let workspace = notice_job.workspace.clone();
            notices.send(ShellNotice::ImportFinished {
                job: notice_job,
                result: result.map(|_| ()),
            });
            if let Some((run_id, findings)) = evidence {
                notices.send(ShellNotice::LinkAuditEvidence {
                    workspace,
                    run_id,
                    findings,
                });
            }
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
    // Mirror the production run path: an in-place asset-filename adoption
    // needs Capture Vector's own snapshot of the content input, taken before
    // the lab output exists.
    if mode_name == "asset-filename"
        && let Some((_, source_root)) = prepared.inputs.iter().find(|(f, _)| *f == "--root")
    {
        store::capture_input_inventory(&lock, source_root).map_err(|e| e.to_string())?;
    }
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
