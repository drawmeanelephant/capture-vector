use crate::adoption::AdoptionJob;
use crate::command::ImportJob;
use crate::dialog::{FileFilter, PickPurpose};
use crate::new_workspace::WorkspaceCreation;
use std::path::PathBuf;

/// Requests only the windowing layer can carry out (native dialogs) or
/// the composition root owns (opening a workspace).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellRequest {
    PickWorkspace,
    /// Native file picker; the answer lands in `Workbench::picked`.
    PickFile {
        filters: Vec<FileFilter>,
        purpose: PickPurpose,
    },
    PickFolder {
        purpose: PickPurpose,
    },
    OpenWorkspace(PathBuf),
    /// Run `boris init` for the New workspace form; the outcome comes
    /// back as `ShellNotice::Created`.
    CreateWorkspace(WorkspaceCreation),
    /// Adopt a migration run into the open workspace or a new one.
    AdoptRun(AdoptionJob),
    /// Run a migration inventory or import in the background for Migrate.
    RunImport(ImportJob),
    /// Open a page (relative to the content input) in an editor window.
    EditPage(String),
    /// Put text on the system clipboard.
    Copy(String),
}

/// Toolkit-neutral key input; `cv-widgets` maps rune keys onto it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyInput {
    Char(char),
    /// Primary-modifier chord (⌘ on macOS, Ctrl elsewhere), lowercased.
    Command(char),
    Up,
    Down,
    Enter,
    Escape,
    Backspace,
    Tab,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyPress {
    pub key: KeyInput,
    pub shift: bool,
}

impl KeyPress {
    pub fn new(key: KeyInput) -> Self {
        Self { key, shift: false }
    }
}
