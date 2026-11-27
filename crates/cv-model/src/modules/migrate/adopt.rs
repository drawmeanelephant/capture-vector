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
            return vec![Intent::Status("select a finished import run first".into())];
        };
        if let Some(why) = Self::adopt_unavailable(Some(&run)) {
            return vec![Intent::Status(format!("Adopt: {why}"))];
        }
        vec![Intent::dialog(
            DialogSpec::new(
                ADOPT,
                format!("Adopt {}", run.id),
                DialogKind::Choose(vec![
                    Choice {
                        id: "new".into(),
                        label: "New workspace".into(),
                    },
                    Choice {
                        id: "existing".into(),
                        label: "Existing workspace (merge into a subfolder)".into(),
                    },
                ]),
            )
            .body("Choose where the candidate content should become a Boris workspace. Provenance stays in .cv/imports/ and never enters frontmatter.")
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
            return vec![Intent::Status("adoption lost its run context".into())];
        };
        let run = self.imports.runs.iter().find(|r| r.id == run_id).cloned();
        let Some(run) = run else {
            return vec![Intent::Status(format!(
                "import run {run_id} is no longer present"
            ))];
        };
        match a.text(CHOICE) {
            Some("new") => vec![Intent::PrepareAdoptNew {
                run_id,
                suggested_archetype: run.suggested_archetype,
            }],
            Some("existing") => vec![Intent::dialog(
                DialogSpec::new(
                    ADOPT_EXISTING,
                    format!("Merge {}", run.id),
                    DialogKind::Form(vec![
                        Field::text("subfolder", "Content subfolder", "")
                            .placeholder("imported/site")
                            .hint("The candidate tree is copied below content/<subfolder>; existing paths or page ids refuse the whole merge before anything is written."),
                    ]),
                )
                .body("Choose a new subfolder inside this workspace's content input. Nothing is overwritten silently.")
                .context(run_id)
                .cancel("Cancel")
                .action("adopt", "Adopt", ActionTone::Primary),
            )],
            _ => vec![Intent::Status("choose New workspace or Existing workspace".into())],
        }
    }

    pub(super) fn adopt_existing_answer(&mut self, a: &DialogAnswer) -> Vec<Intent> {
        if a.action.is_none() {
            return Vec::new();
        }
        let Some(run_id) = a.context.clone() else {
            return vec![Intent::Status("adoption lost its run context".into())];
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
                        Field::text("subfolder", "Content subfolder", subfolder)
                            .placeholder("imported/site"),
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
