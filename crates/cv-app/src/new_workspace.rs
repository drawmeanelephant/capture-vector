//! Shell-side workers for New workspace. The workers only hand results to
//! `Workbench`; Boris is still spawned exclusively by `cv-engine`.

use crate::WorkspaceEnv;
use cv_engine::init::Initializer;
use cv_engine::locate::{Locator, PINNED};
use cv_engine::proc::CancelToken;
use cv_model::new_workspace::{Archetypes, NewWorkspace, ShellNotice};
use cv_model::workbench::Workbench;
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;

fn locator(env: &WorkspaceEnv, cwd: &Path) -> Locator {
    env.locator
        .clone()
        .unwrap_or_else(|| Locator::from_env(cwd))
}

fn spawn_notice(
    name: &'static str,
    notices: &cv_model::new_workspace::Notices,
    spawn_failed: impl FnOnce(std::io::Error) -> ShellNotice,
    work: impl FnOnce() + Send + 'static,
) -> Option<JoinHandle<()>> {
    match std::thread::Builder::new()
        .name(name.to_string())
        .spawn(work)
    {
        Ok(join) => Some(join),
        Err(e) => {
            notices.send(spawn_failed(e));
            None
        }
    }
}

/// Discover what this Boris can initialize. Runs in the background and
/// arrives as `ShellNotice::Archetypes`, so shell state stays synchronous.
pub fn probe_archetypes(wb: &mut Workbench, env: &WorkspaceEnv) -> Option<JoinHandle<()>> {
    let notices = wb.notices();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let locator = locator(env, &cwd);
    let worker_notices = notices.clone();
    spawn_notice(
        "cv-new-workspace-inventory",
        &notices,
        |e| ShellNotice::Archetypes(Err(format!("could not start the inventory worker: {e}"))),
        move || {
            let result = (|| {
                let initializer =
                    Initializer::locate(&locator, &cwd, PINNED).map_err(|e| e.to_string())?;
                let starters = initializer
                    .starters(&cwd, &CancelToken::new())
                    .map_err(|e| e.to_string())?;
                let default = starters
                    .default
                    .as_deref()
                    .and_then(|d| starters.archetypes.iter().position(|n| n == d))
                    .unwrap_or(0);
                Ok::<_, String>(Archetypes {
                    names: starters.archetypes,
                    default,
                })
            })();
            worker_notices.send(ShellNotice::Archetypes(result));
        },
    )
}

fn failure_text(output: &cv_engine::verbs::VerbOutput) -> String {
    let mut lines = output
        .stderr
        .lines()
        .chain(output.stdout.lines())
        .map(str::trim)
        .filter(|l| !l.is_empty());
    let first = lines
        .clone()
        .find(|l| l.to_ascii_lowercase().contains("error"))
        .or_else(|| lines.next())
        .unwrap_or("boris init failed");
    first.chars().take(240).collect()
}

/// Run `boris init` in the background. Success opens through the ordinary
/// workspace-open path when `ShellNotice::Created` reaches the workbench.
pub fn create_workspace(
    wb: &mut Workbench,
    draft: NewWorkspace,
    env: &WorkspaceEnv,
) -> Option<JoinHandle<()>> {
    let notices = wb.notices();
    let cwd = draft.parent.clone();
    let locator = locator(env, &cwd);
    let worker_notices = notices.clone();
    spawn_notice(
        "cv-new-workspace-init",
        &notices,
        {
            let draft = draft.clone();
            move |e| ShellNotice::Created {
                draft,
                result: Err(format!("could not start the workspace worker: {e}")),
            }
        },
        move || {
            let target = draft.target();
            let result = (|| {
                let initializer =
                    Initializer::locate(&locator, &cwd, PINNED).map_err(|e| e.to_string())?;
                let out = initializer
                    .create(&target, &draft.archetype, &CancelToken::new())
                    .map_err(|e| e.to_string())?;
                if out.output.outcome.class() == Some(cv_core::engine::ExitClass::Ok) {
                    Ok(out.target)
                } else {
                    Err(failure_text(&out.output))
                }
            })();
            worker_notices.send(ShellNotice::Created { draft, result });
        },
    )
}

pub fn create_native(wb: &mut Workbench, draft: NewWorkspace) {
    let _ = create_workspace(wb, draft, &WorkspaceEnv::native());
}
