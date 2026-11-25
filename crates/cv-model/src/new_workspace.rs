//! `shell.workspace.new`: a fresh workspace from one of the starter
//! archetypes the located Boris lists. The shell asks with a Form; the
//! composition root runs `boris init` through cv-engine on a worker, and
//! the result opens as the workspace or returns to the form as its error.
//! What a starter holds, and which folders may take one, is Boris's.

use crate::command::Intent;
use crate::dialog::{ActionTone, DialogAnswer, DialogKind, DialogSpec, Field, Pick, Progress};
use crate::workbench::{ShellRequest, Waker, Workbench};
use cv_core::engine::CancelToken;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

pub const NEW: &str = "shell.workspace.new";
pub const CREATING: &str = "shell.workspace.creating";
/// Why New workspace waits before the engine has been asked.
pub const LOCATING: &str = "Boris has not been located yet";

/// The archetype names `boris init --help` lists, in its order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Archetypes {
    pub names: Vec<String>,
    /// Index of the one `--type` defaults to.
    pub default: usize,
}

/// What the form asked for: `parent/name`, written by `archetype`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewWorkspace {
    pub parent: PathBuf,
    pub name: String,
    pub archetype: String,
}

impl NewWorkspace {
    pub fn target(&self) -> PathBuf {
        self.parent.join(&self.name)
    }

    /// The form's own checks: one plain folder name under an absolute
    /// location. Whether that folder may take a starter is Boris's call.
    fn check(&self) -> Result<(), String> {
        let n = self.name.as_str();
        if n.is_empty() {
            return Err("Name the new folder.".into());
        }
        if n == "." || n == ".." || n.contains(['/', '\\']) || n.starts_with('-') {
            return Err("The folder name must be one plain name, not a path or a flag.".into());
        }
        if !self.parent.is_absolute() {
            return Err("Choose a location for the new folder.".into());
        }
        Ok(())
    }
}

/// One creation request and the token that cancels it. The token is
/// shared verbatim with cv-engine, so dismissing progress kills the child
/// process tree rather than merely hiding the dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceCreation {
    pub draft: NewWorkspace,
    pub cancel: CancelToken,
}

impl WorkspaceCreation {
    pub fn new(draft: NewWorkspace) -> Self {
        Self {
            draft,
            cancel: CancelToken::new(),
        }
    }
}

/// Shell-level results from worker threads. Unlike `Inbound`, these
/// outlive a project switch: they are about which project comes next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellNotice {
    /// What New workspace can offer; `Err` is why it is unavailable.
    Archetypes(Result<Archetypes, String>),
    Created {
        creation: WorkspaceCreation,
        result: Result<PathBuf, String>,
    },
}

/// `Send` handle for [`ShellNotice`]s: enqueue and wake the UI loop.
#[derive(Clone)]
pub struct Notices {
    pub(crate) tx: Sender<ShellNotice>,
    pub(crate) waker: Arc<Mutex<Option<Waker>>>,
}

impl Notices {
    pub fn send(&self, n: ShellNotice) {
        if self.tx.send(n).is_ok()
            && let Some(w) = self
                .waker
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
        {
            w();
        }
    }
}

impl Workbench {
    /// `shell.workspace.new`: ask with the form, or say why it can't.
    pub(crate) fn new_workspace(&mut self) {
        if self.create_cancel.is_some() {
            self.status = "A workspace is already being created.".into();
            return;
        }
        match self.archetypes.clone() {
            Ok(a) => {
                let spec = self.new_workspace_form(&a, None, None);
                self.apply(Intent::dialog(spec));
            }
            Err(why) => self.status = format!("New workspace is unavailable: {why}"),
        }
    }

    fn new_workspace_form(
        &self,
        a: &Archetypes,
        draft: Option<&NewWorkspace>,
        error: Option<String>,
    ) -> DialogSpec {
        let names: Vec<&str> = a.names.iter().map(String::as_str).collect();
        let selected = draft
            .and_then(|d| a.names.iter().position(|n| *n == d.archetype))
            .unwrap_or(a.default);
        let parent = draft
            .map(|d| d.parent.clone())
            .or_else(|| {
                self.workspace
                    .as_deref()
                    .and_then(Path::parent)
                    .map(Path::to_path_buf)
            })
            .or_else(|| self.new_parent.clone())
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let name = draft.map(|d| d.name.as_str()).unwrap_or_default();
        let mut spec = DialogSpec::new(
            NEW,
            "New workspace",
            DialogKind::Form(vec![
                Field::choice("archetype", "Starter", &names, selected)
                    .hint("The starter boris init writes, from its own list."),
                Field::path("parent", "Location", &parent, Pick::Folder),
                Field::text("name", "Folder name", name)
                    .placeholder("my-site")
                    .hint("A new folder; Boris refuses one that is not empty."),
            ]),
        )
        .body("Boris writes a starter site into a new folder and checks that it builds.")
        .cancel("Cancel")
        .action("create", "Create", ActionTone::Primary);
        if let Some(e) = error {
            spec = spec.error(e);
        }
        spec
    }

    pub(crate) fn new_workspace_answer(&mut self, answer: &DialogAnswer) {
        if answer.action.is_none() {
            return;
        }
        if self.create_cancel.is_some() {
            self.status = "A workspace is already being created.".into();
            return;
        }
        let Ok(a) = self.archetypes.clone() else {
            return;
        };
        let text = |k: &str| answer.text(k).unwrap_or_default().trim().to_string();
        let draft = NewWorkspace {
            parent: PathBuf::from(text("parent")),
            name: text("name"),
            archetype: text("archetype"),
        };
        let invalid = (!a.names.contains(&draft.archetype))
            .then(|| "Choose one of the starters Boris lists.".to_string());
        if let Err(e) = invalid.map_or(draft.check(), Err) {
            let spec = self.new_workspace_form(&a, Some(&draft), Some(e));
            self.dialogs.push(spec);
            return;
        }
        self.new_parent = Some(draft.parent.clone());
        let creation = WorkspaceCreation::new(draft);
        self.create_cancel = Some(creation.cancel.clone());
        self.dialogs.push(
            DialogSpec::new(
                CREATING,
                "Creating workspace",
                DialogKind::Progress(Progress {
                    label: format!(
                        "boris init {} --type {}",
                        creation.draft.name, creation.draft.archetype
                    ),
                    permille: None,
                }),
            )
            .cancel("Cancel"),
        );
        self.requests.push(ShellRequest::CreateWorkspace(creation));
    }

    /// Dismissing creation progress cancels the operation. The worker
    /// token is shared with cv-engine; the completion path also checks it
    /// because a fast init can finish before the kill lands.
    pub(crate) fn cancel_workspace_creation(&mut self) {
        if let Some(cancel) = &self.create_cancel {
            cancel.cancel();
            self.status = "Cancelling workspace creation…".into();
        }
    }

    pub(crate) fn notice(&mut self, n: ShellNotice) {
        match n {
            ShellNotice::Archetypes(a) => self.archetypes = a,
            ShellNotice::Created { creation, result } => {
                let cancelled = creation.cancel.is_cancelled();
                if self
                    .create_cancel
                    .as_ref()
                    .is_some_and(|c| *c == creation.cancel)
                {
                    self.create_cancel = None;
                }
                self.dialogs.close(CREATING);
                if cancelled {
                    self.status = "Workspace creation cancelled.".into();
                    return;
                }
                let draft = creation.draft;
                match (result, self.archetypes.clone()) {
                    (Ok(root), _) => self.requests.push(ShellRequest::OpenWorkspace(root)),
                    (Err(e), Ok(a)) => {
                        let spec = self.new_workspace_form(&a, Some(&draft), Some(e));
                        self.dialogs.push(spec);
                    }
                    (Err(e), Err(_)) => self.status = e,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialog::{DialogKind, Value};
    use crate::module::Registry;
    use crate::workbench::{KeyInput, KeyPress};

    /// A parent path valid for the host running the test. The form accepts
    /// native absolute paths; rooted-but-prefixless Unix syntax is not an
    /// absolute Windows path and must stay invalid there.
    fn parent() -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(r"C:\home\me")
        } else {
            PathBuf::from("/home/me")
        }
    }

    fn wb() -> Workbench {
        let mut wb = Workbench::new(Registry::default());
        wb.archetypes = Ok(Archetypes {
            names: vec!["docs".into(), "garden".into()],
            default: 0,
        });
        wb.new_parent = Some(parent());
        wb
    }

    fn take_creation(wb: &mut Workbench) -> WorkspaceCreation {
        match wb.requests.pop() {
            Some(ShellRequest::CreateWorkspace(creation)) => creation,
            other => panic!(
                "expected one create-workspace request, got {other:?}; remaining {:?}",
                wb.requests
            ),
        }
    }

    fn typed(wb: &mut Workbench, s: &str) {
        for c in s.chars() {
            wb.key(&KeyPress::new(KeyInput::Char(c)));
        }
    }

    /// Open the form, pick garden, tab to the name and type it.
    fn fill(wb: &mut Workbench, name: &str) {
        wb.key(&KeyPress {
            key: KeyInput::Command('n'),
            shift: true,
        });
        wb.key(&KeyPress::new(KeyInput::Down));
        wb.key(&KeyPress::new(KeyInput::Tab));
        wb.key(&KeyPress::new(KeyInput::Tab));
        typed(wb, name);
    }

    #[test]
    fn the_chord_opens_a_form_with_the_listed_archetypes_and_a_default_location() {
        let mut wb = wb();
        wb.key(&KeyPress {
            key: KeyInput::Command('n'),
            shift: true,
        });
        let d = wb.dialogs.open().expect("form open");
        assert_eq!(d.spec.id, NEW);
        let DialogKind::Form(fields) = &d.spec.kind else {
            panic!("{:?}", d.spec.kind)
        };
        assert_eq!(fields[0].value(), Value::Choice("docs".into()));
        assert_eq!(
            fields[1].value(),
            Value::Path(parent().display().to_string())
        );
    }

    #[test]
    fn create_asks_the_root_to_run_init_and_shows_progress() {
        let mut wb = wb();
        fill(&mut wb, "notes");
        wb.key(&KeyPress::new(KeyInput::Enter));
        let draft = NewWorkspace {
            parent: parent(),
            name: "notes".into(),
            archetype: "garden".into(),
        };
        let creation = take_creation(&mut wb);
        assert_eq!(creation.draft, draft);
        assert!(!creation.cancel.is_cancelled());
        assert_eq!(wb.dialogs.open().unwrap().spec.id, CREATING);

        wb.notice(ShellNotice::Created {
            creation,
            result: Ok(draft.target()),
        });
        assert!(!wb.dialogs.is_open());
        assert_eq!(
            wb.requests.last(),
            Some(&ShellRequest::OpenWorkspace(parent().join("notes")))
        );
    }

    #[test]
    fn dismissing_progress_cancels_init_and_success_cannot_force_open() {
        let mut wb = wb();
        fill(&mut wb, "notes");
        wb.key(&KeyPress::new(KeyInput::Enter));
        let creation = take_creation(&mut wb);
        assert!(wb.dialogs.open().is_some_and(|d| d.spec.id == CREATING));

        wb.dialog_cancel();
        assert!(creation.cancel.is_cancelled());
        assert_eq!(wb.status, "Cancelling workspace creation…");

        wb.notice(ShellNotice::Created {
            creation,
            result: Ok(parent().join("notes")),
        });
        assert!(
            !wb.requests
                .iter()
                .any(|r| matches!(r, ShellRequest::OpenWorkspace(_))),
            "success after dismissal must not force-open: {:?}",
            wb.requests
        );
        assert!(!wb.dialogs.is_open());
        assert_eq!(wb.status, "Workspace creation cancelled.");
        assert!(wb.create_cancel.is_none());
    }

    #[test]
    fn a_refusal_returns_to_the_filled_form_with_its_reason() {
        let mut wb = wb();
        fill(&mut wb, "notes");
        wb.key(&KeyPress::new(KeyInput::Enter));
        let creation = take_creation(&mut wb);
        wb.notice(ShellNotice::Created {
            creation,
            result: Err("target directory is not empty".into()),
        });
        let d = wb.dialogs.open().unwrap();
        assert_eq!(d.spec.id, NEW);
        assert_eq!(
            d.spec.error.as_deref(),
            Some("target directory is not empty")
        );
        assert_eq!(d.fields()[0].value(), Value::Choice("garden".into()));
        assert_eq!(d.fields()[2].value(), Value::Text("notes".into()));
        assert!(wb.requests.is_empty());
    }

    #[test]
    fn a_path_or_empty_name_is_asked_again_without_running_init() {
        for name in ["", "a/b", "..", "-x"] {
            let mut wb = wb();
            fill(&mut wb, name);
            wb.key(&KeyPress::new(KeyInput::Enter));
            assert!(wb.requests.is_empty(), "{name:?}");
            let d = wb.dialogs.open().unwrap();
            assert!(d.spec.error.is_some(), "{name:?}");
        }
    }

    #[test]
    fn without_an_engine_the_command_is_disabled_and_says_why() {
        let mut wb = wb();
        wb.notice(ShellNotice::Archetypes(Err("boris not found".into())));
        let cmd = wb.commands().into_iter().find(|c| c.id == NEW).unwrap();
        assert_eq!(cmd.disabled.as_deref(), Some("boris not found"));
        wb.run_command(NEW);
        assert!(!wb.dialogs.is_open());
        assert_eq!(wb.status, "New workspace is unavailable: boris not found");
    }

    #[test]
    fn notices_survive_a_project_switch() {
        let mut wb = wb();
        let n = wb.notices();
        wb.reset_project(Registry::default());
        n.send(ShellNotice::Archetypes(Err("gone".into())));
        wb.drain();
        assert_eq!(wb.archetypes, Err("gone".into()));
    }
}
