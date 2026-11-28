//! Adoption target dialogs for finished migration runs. Kept separate from
//! the mailbox projection so the module file stays under the repository's
//! size budget; this is presentation flow only, with filesystem authority in
//! cv-engine/cv-store.

use super::*;
use crate::adoption::{AdoptionRequest, AdoptionTarget};
use crate::dialog::{ActionTone, CHOICE, Choice, DialogKind, DialogSpec, Field};

impl MigrateModule {
    pub(super) fn selected_run(&self) -> Option<&ImportRun> {
        self.selected
            .as_deref()
            .and_then(|key| key.strip_prefix("run:"))
            .and_then(|id| self.imports.runs.iter().find(|r| r.id == id))
    }

    pub(super) fn adopt_unavailable(run: Option<&ImportRun>) -> Option<String> {
        let Some(run) = run else {
            return Some("select a finished import run first".into());
        };
        if !matches!(run.status, RunStatus::Ok | RunStatus::Adopted) {
            return Some(format!("run {} has not finished successfully", run.id));
        }
        if run.mode.ends_with("-inventory") {
            return Some(format!(
                "run {} is inventory-only and emits no content",
                run.id
            ));
        }
        if run.problem.is_some() {
            return Some(format!("run {} is not readable", run.id));
        }
        None
    }

    pub(super) fn adopt(&mut self) -> Vec<Intent> {
        let Some(run) = self.selected_run().cloned() else {
            return vec![Intent::Outcome("select a finished import run first".into())];
        };
        if let Some(why) = Self::adopt_unavailable(Some(&run)) {
            return vec![Intent::Outcome(format!("Adopt: {why}"))];
        }
        let mut choices = vec![Choice {
            id: "new".into(),
            label: "New workspace".into(),
        }];
        choices.push(Choice {
            id: "existing".into(),
            label: if run.mode == "asset-filename" {
                "This workspace (rename assets in place)".into()
            } else {
                "Existing workspace (merge into a subfolder)".into()
            },
        });
        let body = if run.mode == "asset-filename" {
            "Choose where the utility result should land. In-place adoption applies only the accepted asset renames and Markdown reference rewrites to this workspace; rejected or colliding assets stay untouched. Provenance stays in .cv/imports/ and never enters frontmatter."
        } else {
            "Choose where the candidate content should become a Boris workspace. Provenance stays in .cv/imports/ and never enters frontmatter."
        };
        vec![Intent::dialog(
            DialogSpec::new(
                ADOPT,
                format!("Adopt {}", run.id),
                DialogKind::Choose(choices),
            )
            .body(body)
            .context(run.id)
            .cancel("Cancel")
            .action("continue", "Continue", ActionTone::Primary),
        )]
    }

    pub(super) fn adopt_answer(&mut self, a: &DialogAnswer) -> Vec<Intent> {
        if a.action.is_none() {
            return Vec::new();
        }
        let Some(run_id) = a.context.clone() else {
            return vec![Intent::Outcome("adoption lost its run context".into())];
        };
        let run = self.imports.runs.iter().find(|r| r.id == run_id).cloned();
        let Some(run) = run else {
            return vec![Intent::Outcome(format!(
                "import run {run_id} is no longer present"
            ))];
        };
        match a.text(CHOICE) {
            Some("new") => vec![Intent::PrepareAdoptNew {
                run_id,
                suggested_archetype: run.suggested_archetype,
            }],
            Some("existing") if run.mode == "asset-filename" => {
                vec![Intent::AdoptRun(AdoptionRequest {
                    run_id,
                    target: AdoptionTarget::ExistingWorkspaceInPlace,
                })]
            }
            Some("existing") => vec![Intent::dialog(
                DialogSpec::new(
                    ADOPT_EXISTING,
                    format!("Merge {}", run.id),
                    DialogKind::Form(vec![
                        Field::text("subfolder", "Content subfolder", "imported/site")
                            .hint("The candidate tree is copied into <subfolder> inside the workspace content input; existing paths or page ids refuse the whole merge before anything is written."),
                    ]),
                )
                .body("Choose a new subfolder inside this workspace's content input. Nothing is overwritten silently.")
                .context(run_id)
                .cancel("Cancel")
                .action("adopt", "Adopt", ActionTone::Primary),
            )],
            _ => vec![Intent::Outcome("choose New workspace or Existing workspace".into())],
        }
    }

    pub(super) fn adopt_existing_answer(&mut self, a: &DialogAnswer) -> Vec<Intent> {
        if a.action.is_none() {
            return Vec::new();
        }
        let Some(run_id) = a.context.clone() else {
            return vec![Intent::Outcome("adoption lost its run context".into())];
        };
        let typed = a.text("subfolder").unwrap_or_default();
        // Trailing slashes are the same folder; leading ones make it
        // absolute and must be refused, never silently rewritten.
        let subfolder = typed.trim_end_matches('/');
        let bad = typed != typed.trim()
            || typed.starts_with(['/', '\\'])
            || subfolder.is_empty()
            || subfolder.split('/').any(|part| {
                part.is_empty()
                    || part == "."
                    || part == ".."
                    || part.contains(['\\', ':', '<', '>', '"', '|', '?', '*'])
                    || part.ends_with(['.', ' '])
            });
        if bad {
            return vec![Intent::dialog(
                DialogSpec::new(
                    ADOPT_EXISTING,
                    format!("Merge {}", run_id),
                    DialogKind::Form(vec![
                        Field::text("subfolder", "Content subfolder", subfolder),
                    ]),
                )
                .error("Choose a portable relative subfolder without '..' or path separators in a name.")
                .context(run_id)
                .cancel("Cancel")
                .action("adopt", "Adopt", ActionTone::Primary),
            )];
        }
        vec![Intent::AdoptRun(AdoptionRequest {
            run_id,
            target: AdoptionTarget::ExistingWorkspace {
                subfolder: subfolder.to_string(),
            },
        })]
    }
}
