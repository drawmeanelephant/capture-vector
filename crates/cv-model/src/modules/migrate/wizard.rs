//! Stepped New import wizard. The module owns only the dialog state and
//! the answer state machine; picking is a shell request and the run itself
//! is a shell-owned background job executed through `cv-engine`.

use super::{ImportExtraKind, ImportInputKind, ImportMode, Imports, MigrateModule, RunStatus};
use crate::command::{ImportJob, ImportPhase, Intent};
use crate::dialog::{
    ActionTone, CHOICE, Choice, DialogAnswer, DialogKind, DialogSpec, Field, FileFilter, Pick,
    Progress,
};
use cv_core::engine::CancelToken;
use std::collections::BTreeMap;
use std::path::PathBuf;

pub(crate) const FAMILY: &str = "migrate.new.family";
pub(crate) const MODE: &str = "migrate.new.mode";
pub(crate) const INPUT: &str = "migrate.new.input";
pub(crate) const INVENTORY: &str = "migrate.new.inventory";
pub(crate) const PROGRESS: &str = "migrate.new.progress";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum WizardStage {
    #[default]
    Idle,
    ChoosingFamily,
    ChoosingMode,
    Input,
    Running {
        phase: ImportPhase,
        run_id: String,
        cancel: CancelToken,
    },
    AwaitingEmit {
        inventory_run: String,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Wizard {
    pub family: Option<String>,
    pub mode: Option<String>,
    /// `(flag, typed-or-picked path)`, in answer order by flag.
    pub inputs: BTreeMap<String, String>,
    pub stage: WizardStage,
}

impl Wizard {
    fn reset(&mut self) {
        *self = Self::default();
    }
}

fn choice(id: &str, label: impl Into<String>) -> Choice {
    Choice {
        id: id.into(),
        label: label.into(),
    }
}

fn action_value(answer: &DialogAnswer, field: &str) -> String {
    // Never trim: whitespace is valid in POSIX paths, so `/tmp/export.xml `
    // and `/tmp/export.xml` are different files. Only zero-length means
    // absent; whitespace-only typed input is rejected at validation.
    answer.text(field).unwrap_or_default().to_string()
}

fn mode_ref<'a>(imports: &'a Imports, name: &str) -> Option<&'a ImportMode> {
    imports.modes.iter().find(|m| m.name == name)
}

impl MigrateModule {
    pub(crate) fn new_unavailable(&self) -> Option<String> {
        if !matches!(self.wizard.stage, WizardStage::Idle) {
            return Some("finish or cancel the import already running".into());
        }
        match &self.imports.lab {
            super::LabState::NoWorkspace => Some("open a workspace first".into()),
            super::LabState::Fixture { .. } | super::LabState::Located { .. } => {
                (self.imports.modes.is_empty())
                    .then(|| "no migration modes are registered in this build".into())
            }
            super::LabState::Absent { .. } => Some("boris-migration-lab was not found".into()),
            super::LabState::OverrideMissing { .. } => {
                Some("CV_MIGRATION_LAB_BIN is not an executable".into())
            }
            super::LabState::Mismatch { found, want, .. } => {
                Some(format!("lab is {found}; pinned {want}"))
            }
            super::LabState::Failed { error } => Some(format!("lab gate failed: {error}")),
        }
    }

    pub(crate) fn begin_import(&mut self) -> Vec<Intent> {
        if let Some(why) = self.new_unavailable() {
            return vec![Intent::Status(format!("New import is unavailable: {why}"))];
        }
        self.wizard.reset();
        self.wizard.stage = WizardStage::ChoosingFamily;
        vec![Intent::dialog(self.family_dialog(None))]
    }

    fn family_dialog(&self, error: Option<String>) -> DialogSpec {
        // A family with no offered emitting mode is a dead end; only
        // families the wizard can actually fulfill appear.
        let companions: std::collections::BTreeSet<&str> = self
            .imports
            .modes
            .iter()
            .filter_map(|m| m.inventory.as_deref())
            .collect();
        let choices = self
            .imports
            .families
            .iter()
            .filter(|f| {
                self.imports
                    .modes
                    .iter()
                    .any(|m| m.family == f.key && !companions.contains(m.name.as_str()))
            })
            .map(|f| choice(&f.key, f.label.clone()))
            .collect();
        let mut d = DialogSpec::new(
            FAMILY,
            "New import — choose a source",
            DialogKind::Choose(choices),
        )
        .body("Pick the source family first. The next step lists only the pinned lab modes for that family.")
        .cancel("Cancel")
        .action("choose", "Continue", ActionTone::Primary);
        d.error = error;
        d
    }

    fn mode_dialog(&self, family: &str, error: Option<String>) -> DialogSpec {
        let companions: std::collections::BTreeSet<&str> = self
            .imports
            .modes
            .iter()
            .filter_map(|m| m.inventory.as_deref())
            .collect();
        let choices = self
            .imports
            .modes
            .iter()
            .filter(|m| m.family == family && !companions.contains(m.name.as_str()))
            .map(|m| {
                let inventory = if m.inventory.is_some() {
                    " · inventory first"
                } else {
                    ""
                };
                choice(
                    &m.name,
                    format!("{} — {}{inventory}", m.name, m.description),
                )
            })
            .collect();
        let family_label = self
            .imports
            .families
            .iter()
            .find(|f| f.key == family)
            .map(|f| f.label.as_str())
            .unwrap_or(family);
        let mut d = DialogSpec::new(
            MODE,
            format!("New import — choose a {family_label} mode"),
            DialogKind::Choose(choices),
        )
        .body("Modes marked “inventory first” run the lab's read-only inventory companion before emitting. Output is staged under .cv/imports/<run-id>/out.")
        .cancel("Cancel")
        .action("back", "Back", ActionTone::Ghost)
        .action("choose", "Continue", ActionTone::Primary);
        d.error = error;
        d
    }

    fn input_dialog(&self, mode: &ImportMode, error: Option<String>) -> DialogSpec {
        let fields = mode
            .inputs
            .iter()
            .map(|input| {
                let id = input.flag.trim_start_matches('-');
                let value = self
                    .wizard
                    .inputs
                    .get(&input.flag)
                    .cloned()
                    .unwrap_or_default();
                let pick = match input.kind {
                    ImportInputKind::File => {
                        let filters = if input.extensions.is_empty() {
                            Vec::new()
                        } else {
                            vec![FileFilter::new(
                                &format!("{} files", input.extensions.join("/")),
                                &input
                                    .extensions
                                    .iter()
                                    .map(String::as_str)
                                    .collect::<Vec<_>>(),
                            )]
                        };
                        Pick::File(filters)
                    }
                    ImportInputKind::Folder => Pick::Folder,
                };
                let label = if input.required {
                    input.label.clone()
                } else {
                    format!("{} (optional)", input.label)
                };
                Field::path(id, &label, &value, pick)
                    .hint(&format!("{} — the original stays in place", input.flag))
            })
            .collect();
        let mut fields: Vec<Field> = fields;
        for extra in &mode.extras {
            let id = extra.flag.trim_start_matches('-');
            let value = self
                .wizard
                .inputs
                .get(&extra.flag)
                .cloned()
                .unwrap_or_default();
            fields.push(
                Field::text(id, &format!("{} (optional)", extra.label), &value).hint(&extra.hint),
            );
        }
        let mut body = vec![
            mode.description.clone(),
            String::new(),
            "Safety:".to_string(),
        ];
        body.extend(mode.safety.iter().map(|n| format!("• {n}")));
        body.push("• The source is never copied into the workspace or repository; the run records only its path and hash.".into());
        body.push(
            "• Output is staged under .cv/imports/<run-id>/out and the active run is held by its own narrow lock."
                .into(),
        );
        let mut d = DialogSpec::new(
            INPUT,
            format!("New import — {} input", mode.name),
            DialogKind::Form(fields),
        )
        .body(body.join("\n"))
        .cancel("Cancel")
        .action("back", "Back", ActionTone::Ghost)
        .action(
            "go",
            if mode.inventory.is_some() {
                "Run inventory"
            } else {
                "Emit import"
            },
            ActionTone::Primary,
        );
        d.error = error;
        d
    }

    fn progress_dialog(job: &ImportJob) -> DialogSpec {
        DialogSpec::new(
            PROGRESS,
            match job.phase {
                ImportPhase::Inventory => "Running inventory",
                ImportPhase::Emit => "Emitting import",
            },
            DialogKind::Progress(Progress {
                label: format!("{} · {}", job.phase.label(), job.mode),
                permille: None,
            }),
        )
        .body("The lab is running in the background. Its output is staged under .cv/imports/<run-id>/out; cancelling closes this dialog and kills the process group.")
        .cancel("Cancel")
    }

    fn inventory_dialog(&self, run_id: &str) -> DialogSpec {
        let counts = self
            .imports
            .runs
            .iter()
            .find(|r| r.id == run_id)
            .map(|r| {
                // Valid zero is explicit zero evidence: when the inventory
                // carries count evidence, every class row shows — including
                // zero rows — rather than collapsing into a bare
                // "inventory completed" implication.
                r.counts
                    .iter()
                    .filter(|(_, n)| *n > 0 || r.count_evidence)
                    .map(|(k, n)| format!("{n} {k}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "the inventory completed".into());
        DialogSpec::new(
            INVENTORY,
            "Inventory complete",
            DialogKind::Confirm,
        )
        .body(format!(
            "The read-only inventory finished ({counts}). Review it in Runs if needed, or continue to emit the import. The same source paths will be used; nothing is copied into the workspace."
        ))
        .cancel("Stop here")
        .action("emit", "Emit import", ActionTone::Primary)
    }

    fn next_run_id(&self, mode: &str) -> String {
        for n in 1..10_000u32 {
            let id = format!("import-{n}-{mode}");
            if !self.imports.runs.iter().any(|r| r.id == id) {
                return id;
            }
        }
        format!("import-{}-{mode}", self.imports.runs.len() + 10_000)
    }

    fn start_job(&mut self, phase: ImportPhase, mode_name: &str) -> Vec<Intent> {
        let Some(mode) = mode_ref(&self.imports, mode_name).cloned() else {
            self.wizard.reset();
            return vec![Intent::Status(format!(
                "migration mode {mode_name} is not registered"
            ))];
        };
        let run_id = self.next_run_id(mode_name);
        let inputs = mode
            .inputs
            .iter()
            .filter_map(|input| {
                let raw = self.wizard.inputs.get(&input.flag)?;
                (!raw.is_empty()).then(|| (input.flag.clone(), PathBuf::from(raw)))
            })
            .collect::<Vec<_>>();
        let extras = mode
            .extras
            .iter()
            .filter_map(|extra| {
                let raw = self.wizard.inputs.get(&extra.flag)?;
                (!raw.is_empty()).then(|| (extra.flag.clone(), raw.clone()))
            })
            .collect::<Vec<_>>();
        let cancel = CancelToken::new();
        let job = ImportJob {
            workspace: PathBuf::new(),
            run_id: run_id.clone(),
            mode: mode_name.into(),
            phase,
            inputs,
            extras,
            cancel: cancel.clone(),
        };
        self.wizard.stage = WizardStage::Running {
            phase,
            run_id,
            cancel,
        };
        vec![
            Intent::dialog(Self::progress_dialog(&job)),
            Intent::RunImport(job),
        ]
    }

    fn validate_inputs(
        &self,
        mode: &ImportMode,
        answer: &DialogAnswer,
    ) -> Result<Vec<(String, String)>, String> {
        let mut values = Vec::new();
        for extra in &mode.extras {
            let key = extra.flag.trim_start_matches('-');
            let value = action_value(answer, key);
            if !value.is_empty() && value.trim().is_empty() {
                return Err(format!(
                    "{} is only whitespace. Clear the field entirely to use the lab default.",
                    extra.flag
                ));
            }
            if !value.is_empty()
                && extra.kind == ImportExtraKind::Count
                && value.parse::<usize>().map_or(true, |n| n == 0)
            {
                return Err(format!(
                    "{} must be a positive integer, or empty for the lab default.",
                    extra.flag
                ));
            }
            values.push((extra.flag.clone(), value));
        }
        for input in &mode.inputs {
            let key = input.flag.trim_start_matches('-');
            let value = action_value(answer, key);
            if input.required && value.is_empty() {
                return Err(format!(
                    "{} is required: choose the {}.",
                    input.flag, input.label
                ));
            }

            if !value.is_empty() && value.trim().is_empty() {
                return Err(format!(
                    "{} is only whitespace, which is not a usable path here. Clear the field entirely to leave it out.",
                    input.flag
                ));
            }
            if !value.is_empty() && !std::path::Path::new(&value).is_absolute() {
                return Err(format!(
                    "{} must be an absolute path; relative paths and ~/ are not expanded here.",
                    input.flag
                ));
            }
            values.push((input.flag.clone(), value));
        }
        Ok(values)
    }

    pub(crate) fn answer_wizard(&mut self, command: &str, answer: &DialogAnswer) -> Vec<Intent> {
        match command {
            FAMILY => {
                if !matches!(self.wizard.stage, WizardStage::ChoosingFamily) {
                    return Vec::new();
                }
                if answer.action.is_none() {
                    self.wizard.reset();
                    return vec![Intent::Status("New import cancelled".into())];
                }
                let family = action_value(answer, CHOICE);
                if self.imports.families.iter().all(|f| f.key != family) {
                    return vec![Intent::dialog(self.family_dialog(Some(
                        "choose one of the listed source families".into(),
                    )))];
                }
                self.wizard.family = Some(family.clone());
                self.wizard.stage = WizardStage::ChoosingMode;
                vec![Intent::dialog(self.mode_dialog(&family, None))]
            }
            MODE => {
                if !matches!(self.wizard.stage, WizardStage::ChoosingMode) {
                    return Vec::new();
                }
                if answer.action.is_none() {
                    self.wizard.reset();
                    return vec![Intent::Status("New import cancelled".into())];
                }
                if answer.action.as_deref() == Some("back") {
                    self.wizard.stage = WizardStage::ChoosingFamily;
                    return vec![Intent::dialog(self.family_dialog(None))];
                }
                let name = action_value(answer, CHOICE);
                let family = self.wizard.family.clone().unwrap_or_default();
                let companions: std::collections::BTreeSet<&str> = self
                    .imports
                    .modes
                    .iter()
                    .filter_map(|m| m.inventory.as_deref())
                    .collect();
                let Some(mode) = self
                    .imports
                    .modes
                    .iter()
                    .find(|m| {
                        m.name == name
                            && m.family == family
                            && !companions.contains(m.name.as_str())
                    })
                    .cloned()
                else {
                    return vec![Intent::dialog(self.mode_dialog(
                        &family,
                        Some("choose one of the listed modes".into()),
                    ))];
                };
                self.wizard.mode = Some(name);
                self.wizard.stage = WizardStage::Input;
                vec![Intent::dialog(self.input_dialog(&mode, None))]
            }
            INPUT => {
                if !matches!(self.wizard.stage, WizardStage::Input) {
                    return Vec::new();
                }
                if answer.action.is_none() {
                    self.wizard.reset();
                    return vec![Intent::Status("New import cancelled".into())];
                }
                let Some(mode_name) = self.wizard.mode.clone() else {
                    self.wizard.reset();
                    return vec![Intent::Status(
                        "The import wizard lost its selected mode; start again".into(),
                    )];
                };
                let Some(mode) = mode_ref(&self.imports, &mode_name).cloned() else {
                    self.wizard.reset();
                    return vec![Intent::Status(format!(
                        "migration mode {mode_name} is not registered"
                    ))];
                };
                if answer.action.as_deref() == Some("back") {
                    let family = self
                        .wizard
                        .family
                        .clone()
                        .unwrap_or_else(|| mode.family.clone());
                    self.wizard.stage = WizardStage::ChoosingMode;
                    return vec![Intent::dialog(self.mode_dialog(&family, None))];
                }
                // Snapshot every field of this mode from the answer first:
                // a cleared field stays cleared even when validation fails,
                // so stale picks can never resurrect on a later submit.
                let flags = mode
                    .inputs
                    .iter()
                    .map(|i| i.flag.clone())
                    .chain(mode.extras.iter().map(|e| e.flag.clone()))
                    .collect::<Vec<_>>();
                for flag in flags {
                    let key = flag.trim_start_matches('-');
                    let value = action_value(answer, key);
                    if value.is_empty() {
                        self.wizard.inputs.remove(&flag);
                    } else {
                        self.wizard.inputs.insert(flag, value);
                    }
                }
                match self.validate_inputs(&mode, answer) {
                    Err(error) => {
                        vec![Intent::dialog(self.input_dialog(&mode, Some(error)))]
                    }
                    Ok(values) => {
                        self.wizard.inputs =
                            values.into_iter().filter(|(_, v)| !v.is_empty()).collect();
                        if let Some(inventory) = mode.inventory.clone() {
                            self.start_job(ImportPhase::Inventory, &inventory)
                        } else {
                            self.start_job(ImportPhase::Emit, &mode.name)
                        }
                    }
                }
            }
            INVENTORY => {
                if !matches!(self.wizard.stage, WizardStage::AwaitingEmit { .. }) {
                    return Vec::new();
                }
                if answer.action.as_deref() == Some("emit") {
                    let Some(mode) = self.wizard.mode.clone() else {
                        self.wizard.reset();
                        return vec![Intent::Status(
                            "The import wizard lost its selected mode; start again".into(),
                        )];
                    };
                    self.start_job(ImportPhase::Emit, &mode)
                } else {
                    self.wizard.reset();
                    vec![Intent::Status(
                        "Inventory kept in Runs; emit cancelled before anything was emitted".into(),
                    )]
                }
            }
            PROGRESS => {
                if let WizardStage::Running { cancel, phase, .. } = &self.wizard.stage {
                    cancel.cancel();
                    vec![Intent::Status(format!(
                        "Cancelling the {}; the run record will show what happened",
                        phase.label()
                    ))]
                } else {
                    Vec::new()
                }
            }
            _ => Vec::new(),
        }
    }

    pub(crate) fn finish_import_job(&mut self) -> Vec<Intent> {
        self.imports = self.port.load();
        let Some(stage) = (match &self.wizard.stage {
            WizardStage::Running { .. } => Some(self.wizard.stage.clone()),
            _ => None,
        }) else {
            return vec![Intent::Status(format!(
                "{} import run(s) loaded",
                self.imports.runs.len()
            ))];
        };
        let WizardStage::Running {
            phase,
            run_id,
            cancel,
        } = stage
        else {
            return Vec::new();
        };
        let Some(run) = self.imports.runs.iter().find(|r| r.id == run_id).cloned() else {
            self.wizard.reset();
            return vec![Intent::Status(format!(
                "The {} ended before its run record could be written",
                phase.label()
            ))];
        };
        if run.status == RunStatus::Running {
            self.wizard.reset();
            return vec![
                Intent::Status(format!(
                    "The {} worker ended before the lab reported an outcome, but the run lock is still held; inspect the run before retrying",
                    phase.label()
                )),
                Intent::Reveal {
                    module: super::ID.into(),
                    key: format!("run:{run_id}"),
                },
            ];
        }
        if run.status == RunStatus::Unfinished {
            self.wizard.reset();
            return vec![
                Intent::Status(format!(
                    "The {} stopped before the lab reported an outcome; the unfinished run's output may be partial, it cannot be adopted, and a new import is the retry",
                    phase.label()
                )),
                Intent::Reveal {
                    module: super::ID.into(),
                    key: format!("run:{run_id}"),
                },
            ];
        }
        // The recorded run status is the authority on what happened: a
        // cancel token set after the run already finished ok or refused
        // must not rewrite that outcome into a cancellation.
        if run.status == RunStatus::Cancelled
            || (cancel.is_cancelled() && matches!(run.status, RunStatus::Running))
        {
            self.wizard.reset();
            return vec![
                Intent::Status(format!(
                    "{} cancelled; partial output was left in the staged run",
                    phase.label()
                )),
                Intent::Reveal {
                    module: super::ID.into(),
                    key: format!("run:{run_id}"),
                },
            ];
        }
        match (phase, run.status) {
            (ImportPhase::Inventory, RunStatus::Ok) => {
                // Emit requires positive decoded-count evidence: projection
                // records whether the decoded document carries count data in
                // this family's shape (structural for Hugo, known-class rows
                // for the tabled families) — never inferred from the
                // pre-seeded aggregate. A decodable but row-less or
                // wrong-class inventory is not evidence, exactly like an
                // unreadable or absent report; all are blocked outright
                // rather than merely confirmed, and all are distinct from a
                // valid inventory that simply counted zero.
                let missing = run.problem.is_none() && !run.count_evidence;
                if run.problem.is_some() || missing {
                    self.wizard.reset();
                    let why = match run.problem.as_deref() {
                        Some(p) => format!("its report is unreadable — {p}"),
                        None => "no count data could be decoded from its output".to_string(),
                    };
                    return vec![
                        Intent::Status(format!(
                            "The inventory finished but {why}, so there are no counts to base an emit on; emit is blocked. Inspect the run in Runs."
                        )),
                        Intent::Reveal {
                            module: super::ID.into(),
                            key: format!("run:{run_id}"),
                        },
                    ];
                }
                self.wizard.stage = WizardStage::AwaitingEmit {
                    inventory_run: run_id.clone(),
                };
                vec![
                    Intent::Status("Inventory finished; review it or continue to emit".into()),
                    Intent::dialog(self.inventory_dialog(&run_id)),
                ]
            }
            (_, RunStatus::Ok) => {
                self.wizard.reset();
                // Zero exit is not verification: the lab may have exited 0
                // yet left no readable report or output evidence, which
                // projection records as a problem. That is a durable
                // could-not-verify state, never "finished".
                if let Some(problem) = &run.problem {
                    return vec![
                        Intent::Status(format!(
                            "The import exited zero but its output could not be verified — {problem}. Inspect the run in Runs."
                        )),
                        Intent::Reveal {
                            module: super::ID.into(),
                            key: format!("run:{run_id}"),
                        },
                    ];
                }
                vec![
                    Intent::Status("Import finished; the run is in Runs".into()),
                    Intent::Reveal {
                        module: super::ID.into(),
                        key: format!("run:{run_id}"),
                    },
                ]
            }
            (_, status) => {
                self.wizard.reset();
                let detail = run
                    .refused
                    .or(run.outcome)
                    .unwrap_or_else(|| format!("the lab ended as {}", status.label()));
                vec![
                    Intent::Status(format!("{} did not complete: {detail}", phase.label())),
                    Intent::Reveal {
                        module: super::ID.into(),
                        key: format!("run:{run_id}"),
                    },
                ]
            }
        }
    }
}
