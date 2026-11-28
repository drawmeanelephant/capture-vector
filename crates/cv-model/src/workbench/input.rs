use super::Workbench;
use crate::adoption::AdoptionJob;
use crate::command::{CommandSpec, ImportJob};
use crate::dialog::{FileFilter, PickPurpose};
use crate::new_workspace::{self, WorkspaceCreation};
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
    Left,
    Right,
    Enter,
    Escape,
    Backspace,
    Tab,
}

/// Which physical key is the platform's primary shortcut modifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrimaryKey {
    /// Control on Windows and Linux.
    Ctrl,
    /// Command (the logo key) on macOS.
    Logo,
}

impl PrimaryKey {
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::Logo
        } else {
            Self::Ctrl
        }
    }
}

/// Toolkit-neutral modifier state. `ctrl` and `logo` are physical keys;
/// `primary_key` says which of them is the platform's shortcut modifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
    pub logo: bool,
    pub primary_key: PrimaryKey,
}

impl Default for Modifiers {
    fn default() -> Self {
        Self {
            shift: false,
            alt: false,
            ctrl: false,
            logo: false,
            primary_key: PrimaryKey::current(),
        }
    }
}

impl Modifiers {
    pub fn primary_held(&self) -> bool {
        match self.primary_key {
            PrimaryKey::Ctrl => self.ctrl,
            PrimaryKey::Logo => self.logo,
        }
    }

    pub fn set_primary(&mut self, held: bool) {
        match self.primary_key {
            PrimaryKey::Ctrl => self.ctrl = held,
            PrimaryKey::Logo => self.logo = held,
        }
    }

    pub fn with_primary(mut self, held: bool) -> Self {
        self.set_primary(held);
        self
    }

    pub fn non_primary_command_held(&self) -> bool {
        match self.primary_key {
            PrimaryKey::Ctrl => self.logo,
            PrimaryKey::Logo => self.ctrl,
        }
    }

    /// The primary shortcut key with no Shift, Alt, or counterpart command
    /// key. Rollover typing uses exactly this state; anything richer is a
    /// deliberate chord, not plain text.
    pub fn primary_alone(&self) -> bool {
        self.primary_held() && !self.shift && !self.alt && !self.non_primary_command_held()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyPress {
    pub key: KeyInput,
    pub modifiers: Modifiers,
}

impl KeyPress {
    pub fn new(key: KeyInput) -> Self {
        let mut modifiers = Modifiers::default();
        if matches!(key, KeyInput::Command(_)) {
            modifiers.set_primary(true);
        }
        Self { key, modifiers }
    }

    pub fn with_modifiers(key: KeyInput, modifiers: Modifiers) -> Self {
        Self { key, modifiers }
    }
}

/// Whether the current primary-modifier hold is the continuous gesture that
/// opened the palette. Releasing the primary key ends the hold even if no
/// printable key arrived in between.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) enum PrimaryHold {
    #[default]
    Up,
    Held {
        palette_rollover: bool,
    },
}

impl PrimaryHold {
    fn note(&mut self, modifiers: Modifiers) {
        if modifiers.primary_held() {
            if matches!(self, Self::Up) {
                *self = Self::Held {
                    palette_rollover: false,
                };
            }
        } else {
            *self = Self::Up;
        }
    }

    fn palette_opened(&mut self, rollover: bool) {
        if let Self::Held { palette_rollover } = self {
            *palette_rollover = rollover;
        }
    }

    fn rollover(&self) -> bool {
        matches!(
            self,
            Self::Held {
                palette_rollover: true
            }
        )
    }
}

impl Workbench {
    /// Latest modifier state from the windowing layer. Only the primary
    /// key's up/down edges participate in palette rollover; the richer
    /// modifier detail on each keypress decides whether a chord is plain
    /// rollover text or a deliberate modified chord.
    pub fn modifiers(&mut self, modifiers: Modifiers) {
        self.primary_hold.note(modifiers);
    }

    /// Keyboard-first shell keymap. Returns true when consumed.
    pub fn key(&mut self, k: &KeyPress) -> bool {
        self.modifiers(k.modifiers);
        if self.dialogs.is_open() {
            return self.dialog_key(k);
        }
        if self.palette.open {
            return self.palette_key(k);
        }
        if let Some(command) = self.active_module_command_for_key(k) {
            if let Some(why) = command.disabled {
                self.outcome_status(format!("{} is unavailable: {why}", command.title));
            } else {
                self.run_command(&command.id);
            }
            return true;
        }
        match &k.key {
            KeyInput::Command('k') => {
                self.palette.toggle();
                if self.palette.open {
                    self.primary_hold
                        .palette_opened(k.modifiers.primary_alone());
                }
            }
            KeyInput::Command('i') => self.run_command("shell.inspector.toggle"),
            KeyInput::Command('o') if k.modifiers.shift => self.run_command("shell.workspace.open"),
            KeyInput::Command('n') if k.modifiers.shift => self.run_command(new_workspace::NEW),
            KeyInput::Command('o') => self.follow_link(),
            KeyInput::Command(c) if c.is_ascii_digit() && *c != '0' => {
                self.activate(*c as usize - '1' as usize)
            }
            KeyInput::Down | KeyInput::Char('j') => self.move_row(1),
            KeyInput::Up | KeyInput::Char('k') => self.move_row(-1),
            KeyInput::Tab => self.cycle_nav(if k.modifiers.shift { -1 } else { 1 }),
            KeyInput::Enter => self.run_command("shell.window.companion"),
            KeyInput::Escape => self.clear_selection(),
            _ => return false,
        }
        true
    }

    fn active_module_command_for_key(&self, k: &KeyPress) -> Option<CommandSpec> {
        self.registry
            .get(self.active)?
            .commands()
            .into_iter()
            .find(|command| {
                command
                    .chord
                    .as_deref()
                    .is_some_and(|chord| chord_matches(chord, k))
            })
    }

    fn palette_key(&mut self, k: &KeyPress) -> bool {
        let cmds = self.commands();
        match &k.key {
            KeyInput::Escape | KeyInput::Command('k') => self.palette.close(),
            KeyInput::Char(c) => self.palette.type_char(*c),
            KeyInput::Command(c) if self.primary_hold.rollover() && k.modifiers.primary_alone() => {
                self.palette.type_char(*c)
            }
            KeyInput::Command(_) => return false,
            KeyInput::Backspace => self.palette.backspace(),
            KeyInput::Down | KeyInput::Tab => self
                .palette
                .move_cursor(1, self.palette.matches(&cmds).len()),
            KeyInput::Up => self
                .palette
                .move_cursor(-1, self.palette.matches(&cmds).len()),
            KeyInput::Enter => {
                let selected = self.palette.selected(&cmds).cloned();
                self.palette.close();
                if let Some(c) = selected {
                    if let Some(why) = c.disabled {
                        self.outcome_status(format!("{} is unavailable: {why}", c.title));
                    } else {
                        self.run_command(&c.id);
                    }
                }
            }
            _ => return false,
        }
        true
    }
}

/// Match a display chord against the toolkit-neutral key input. `mod` is
/// already folded into `KeyInput::Command` by the widget layer; only `shift`
/// remains significant there for existing chord matching.
fn chord_matches(chord: &str, press: &KeyPress) -> bool {
    let mut parts = chord.split('+');
    let key = parts.next_back().unwrap_or(chord).to_lowercase();
    let mut shift = false;
    let mut command = false;
    for part in parts {
        match part {
            "shift" => shift = true,
            "mod" | "cmd" | "command" | "ctrl" => command = true,
            _ => return false,
        }
    }
    if press.modifiers.shift != shift {
        return false;
    }
    match (&press.key, command) {
        (KeyInput::Command(c), true) => key.len() == c.len_utf8() && key.starts_with(*c),
        (KeyInput::Command(_), false) => false,
        (_, true) => false,
        (KeyInput::Char(c), false) => key.len() == c.len_utf8() && key.starts_with(*c),
        (KeyInput::Up, false) => key == "up",
        (KeyInput::Down, false) => key == "down",
        (KeyInput::Left, false) => key == "left",
        (KeyInput::Right, false) => key == "right",
        (KeyInput::Enter, false) => key == "enter",
        (KeyInput::Escape, false) => matches!(key.as_str(), "esc" | "escape"),
        (KeyInput::Backspace, false) => matches!(key.as_str(), "bksp" | "backspace"),
        (KeyInput::Tab, false) => key == "tab",
    }
}
