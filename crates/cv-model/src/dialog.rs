//! Modal dialogs a module asks for with `Intent::Dialog`. The shell keeps
//! one open dialog and a FIFO of the rest, edits the open one's fields
//! from keys, and hands the answer back to the asking module as its
//! `<module>.<verb>` command (`Module::answer`). Pure: no rune, no IO.
//!
//! A `Secret` dialog's typed value lives only in [`SecretValue`] (no
//! `Clone`, no `Serialize`, redacted `Debug`, zeroed on drop) and goes
//! straight to the [`SecretPort`]; the module's answer says only whether
//! it was stored.

use crate::command::{CommandSpec, Intent, Palette};
use crate::workbench::{KeyInput, KeyPress, ShellRequest, Workbench};
use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use zeroize::Zeroizing;

/// The action id that answers like Escape: the dialog is cancelled.
pub const CANCEL: &str = "cancel";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogSpec {
    /// The answer command, `<module>.<verb>`; also identifies the dialog
    /// (a spec with the open dialog's id replaces it in place).
    pub id: String,
    /// Also the dialog's accessible name.
    pub title: String,
    pub body: Option<String>,
    pub kind: DialogKind,
    /// Buttons in reading order. The first `Primary` one is what Enter runs.
    pub actions: Vec<DialogAction>,
    /// Echoed back in the answer: what the question is about (a note id).
    pub context: Option<String>,
    /// Validation text the module supplies when it asks again.
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialogKind {
    Confirm,
    Prompt(Field),
    Form(Vec<Field>),
    /// A filterable list, matched like the command palette.
    Choose(Vec<Choice>),
    Secret(SecretField),
    Progress(Progress),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    /// Key of this field's value in the answer.
    pub id: String,
    pub label: String,
    pub hint: Option<String>,
    pub input: Input,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    Text {
        value: String,
        placeholder: String,
    },
    Choice {
        options: Vec<String>,
        selected: usize,
    },
    Toggle(bool),
    /// Typed, or filled from the native picker.
    Path {
        value: String,
        pick: Pick,
    },
}

/// What the native picker opens for a path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pick {
    File(Vec<FileFilter>),
    Folder,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileFilter {
    pub name: String,
    /// Without the dot: `enex`, `xml`.
    pub extensions: Vec<String>,
}

impl FileFilter {
    pub fn new(name: &str, extensions: &[&str]) -> Self {
        Self {
            name: name.into(),
            extensions: extensions.iter().map(|e| e.to_string()).collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretField {
    pub label: String,
    /// Keychain item the value is stored under — never the value.
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Progress {
    pub label: String,
    /// Done, in thousandths; `None` is indeterminate.
    pub permille: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogAction {
    pub id: String,
    pub label: String,
    pub tone: ActionTone,
}

/// How loud a dialog button is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionTone {
    Primary,
    Ghost,
    /// Destroys work.
    Danger,
}

impl Field {
    fn new(id: &str, label: &str, input: Input) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            hint: None,
            input,
        }
    }
    pub fn text(id: &str, label: &str, value: &str) -> Self {
        Self::new(
            id,
            label,
            Input::Text {
                value: value.into(),
                placeholder: String::new(),
            },
        )
    }
    pub fn choice(id: &str, label: &str, options: &[&str], selected: usize) -> Self {
        let options: Vec<String> = options.iter().map(|o| o.to_string()).collect();
        let selected = selected.min(options.len().saturating_sub(1));
        Self::new(id, label, Input::Choice { options, selected })
    }
    pub fn toggle(id: &str, label: &str, on: bool) -> Self {
        Self::new(id, label, Input::Toggle(on))
    }
    pub fn path(id: &str, label: &str, value: &str, pick: Pick) -> Self {
        Self::new(
            id,
            label,
            Input::Path {
                value: value.into(),
                pick,
            },
        )
    }
    pub fn hint(mut self, hint: &str) -> Self {
        self.hint = Some(hint.into());
        self
    }
    pub fn placeholder(mut self, p: &str) -> Self {
        if let Input::Text { placeholder, .. } = &mut self.input {
            *placeholder = p.into();
        }
        self
    }

    pub fn value(&self) -> Value {
        match &self.input {
            Input::Text { value, .. } => Value::Text(value.clone()),
            Input::Choice { options, selected } => {
                Value::Choice(options.get(*selected).cloned().unwrap_or_default())
            }
            Input::Toggle(on) => Value::Toggle(*on),
            Input::Path { value, .. } => Value::Path(value.clone()),
        }
    }
}

impl DialogSpec {
    pub fn new(id: impl Into<String>, title: impl Into<String>, kind: DialogKind) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            body: None,
            kind,
            actions: Vec::new(),
            context: None,
            error: None,
        }
    }
    pub fn body(mut self, b: impl Into<String>) -> Self {
        self.body = Some(b.into());
        self
    }
    pub fn action(mut self, id: &str, label: &str, tone: ActionTone) -> Self {
        self.actions.push(DialogAction {
            id: id.into(),
            label: label.into(),
            tone,
        });
        self
    }
    /// A Ghost `Cancel` button.
    pub fn cancel(self, label: &str) -> Self {
        self.action(CANCEL, label, ActionTone::Ghost)
    }
    pub fn context(mut self, c: impl Into<String>) -> Self {
        self.context = Some(c.into());
        self
    }
    pub fn error(mut self, e: impl Into<String>) -> Self {
        self.error = Some(e.into());
        self
    }
    /// What Enter runs.
    pub fn primary(&self) -> Option<&DialogAction> {
        self.actions.iter().find(|a| a.tone == ActionTone::Primary)
    }
}

/// A field's answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Text(String),
    /// The chosen option's label (`Input::Choice`), or the chosen row's id
    /// (`DialogKind::Choose`, under [`CHOICE`]).
    Choice(String),
    Toggle(bool),
    Path(String),
}

/// Where a `Choose` dialog's pick is in [`DialogAnswer::values`].
pub const CHOICE: &str = "choice";
/// Where a picked path is when a module asked for one (`Intent::Pick`).
pub const PATH: &str = "path";

/// What came back. Holds no secret: a `Secret` dialog's value went to
/// the secret port, and `secret` says how that went.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DialogAnswer {
    /// The action pressed; `None` when cancelled (Escape, the close
    /// button, a press outside, or a [`CANCEL`] action).
    pub action: Option<String>,
    pub context: Option<String>,
    pub values: BTreeMap<String, Value>,
    pub secret: Option<Result<(), String>>,
}

impl DialogAnswer {
    pub fn text(&self, field: &str) -> Option<&str> {
        match self.values.get(field)? {
            Value::Text(s) | Value::Choice(s) | Value::Path(s) => Some(s),
            Value::Toggle(_) => None,
        }
    }
    pub fn is(&self, action: &str) -> bool {
        self.action.as_deref() == Some(action)
    }
}

/// A typed secret. It cannot be cloned, serialized or printed, and its
/// bytes are zeroed when it drops.
#[derive(Default)]
pub struct SecretValue(Zeroizing<String>);

impl SecretValue {
    pub fn new(s: impl Into<String>) -> Self {
        Self(Zeroizing::new(s.into()))
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    /// Characters typed, for the mask.
    pub fn chars(&self) -> usize {
        self.0.chars().count()
    }
    fn push(&mut self, c: char) {
        // Growing reallocates, leaving the old bytes behind unzeroed; a
        // reserve up front keeps ordinary secrets in one allocation.
        if self.0.capacity() == 0 {
            self.0.reserve(256);
        }
        self.0.push(c);
    }
    fn pop(&mut self) {
        self.0.pop();
    }
}

impl std::fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretValue(<redacted>)")
    }
}

/// Where a `Secret` dialog's value goes. Implemented over the system
/// keychain by the composition root; tests use a mock.
pub trait SecretPort {
    fn store(&mut self, name: &str, value: SecretValue) -> Result<(), String>;
}

/// The open dialog and what has been typed into it.
pub struct OpenDialog {
    pub spec: DialogSpec,
    /// The field keys edit (`Form`).
    pub focus: usize,
    /// Query and cursor of a `Choose` list.
    pub choose: Palette,
    secret: SecretValue,
}

impl std::fmt::Debug for OpenDialog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenDialog")
            .field("spec", &self.spec)
            .field("focus", &self.focus)
            .field("choose", &self.choose)
            .field("secret", &self.secret)
            .finish()
    }
}

/// What a key did to the open dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Edited,
    Act(String),
    Cancel,
    /// Swallowed: the dialog is modal.
    Ignored,
}

impl OpenDialog {
    pub fn new(spec: DialogSpec) -> Self {
        Self {
            spec,
            focus: 0,
            choose: Palette::default(),
            secret: SecretValue::default(),
        }
    }

    pub fn fields(&self) -> &[Field] {
        match &self.spec.kind {
            DialogKind::Prompt(f) => std::slice::from_ref(f),
            DialogKind::Form(fs) => fs,
            _ => &[],
        }
    }

    fn fields_mut(&mut self) -> &mut [Field] {
        match &mut self.spec.kind {
            DialogKind::Prompt(f) => std::slice::from_mut(f),
            DialogKind::Form(fs) => fs,
            _ => &mut [],
        }
    }

    /// How many mask characters a `Secret` dialog shows.
    pub fn secret_chars(&self) -> usize {
        self.secret.chars()
    }

    fn choice_specs(&self) -> Vec<CommandSpec> {
        match &self.spec.kind {
            DialogKind::Choose(cs) => cs
                .iter()
                .map(|c| CommandSpec::new(c.id.clone(), c.label.clone()))
                .collect(),
            _ => Vec::new(),
        }
    }

    /// The `Choose` rows shown, in order, as `(choice id, label)`.
    pub fn visible_choices(&self) -> Vec<(String, String)> {
        let specs = self.choice_specs();
        self.choose
            .visible(&specs)
            .into_iter()
            .map(|c| (c.id.clone(), c.title.clone()))
            .collect()
    }

    /// Focus `field`; with `option`, also pick it (a choice) or flip it
    /// (a toggle).
    pub fn press_field(&mut self, field: usize, option: Option<usize>) {
        if field >= self.fields().len() {
            return;
        }
        self.focus = field;
        if let Some(o) = option {
            match &mut self.fields_mut()[field].input {
                Input::Choice { options, selected } if o < options.len() => *selected = o,
                Input::Toggle(on) => *on = !*on,
                _ => {}
            }
        }
    }

    /// Fill a path field (the native picker answered).
    pub fn fill_path(&mut self, field: usize, path: &str) {
        if let Some(Field {
            input: Input::Path { value, .. },
            ..
        }) = self.fields_mut().get_mut(field)
        {
            *value = path.to_string();
        }
    }

    /// The modal keymap: every key is the dialog's while it is open.
    pub fn key(&mut self, k: &KeyPress) -> Step {
        match &k.key {
            KeyInput::Escape => return Step::Cancel,
            KeyInput::Enter => {
                return match self.spec.primary() {
                    Some(a) if a.id == CANCEL => Step::Cancel,
                    Some(a) => Step::Act(a.id.clone()),
                    None => Step::Ignored,
                };
            }
            KeyInput::Command(_) => return Step::Ignored,
            _ => {}
        }
        let edited = match &self.spec.kind {
            DialogKind::Secret(_) => match &k.key {
                KeyInput::Char(c) => {
                    self.secret.push(*c);
                    true
                }
                KeyInput::Backspace => {
                    self.secret.pop();
                    true
                }
                _ => false,
            },
            DialogKind::Choose(_) => {
                let len = self.visible_choices().len();
                let down = match &k.key {
                    KeyInput::Down => true,
                    KeyInput::Up => false,
                    KeyInput::Tab => !k.modifiers.shift,
                    KeyInput::Char(c) => {
                        self.choose.type_char(*c);
                        return Step::Edited;
                    }
                    KeyInput::Backspace => {
                        self.choose.backspace();
                        return Step::Edited;
                    }
                    _ => return Step::Ignored,
                };
                self.choose.move_cursor(if down { 1 } else { -1 }, len);
                true
            }
            DialogKind::Prompt(_) | DialogKind::Form(_) => self.field_key(k),
            DialogKind::Confirm | DialogKind::Progress(_) => false,
        };
        if edited { Step::Edited } else { Step::Ignored }
    }

    fn field_key(&mut self, k: &KeyPress) -> bool {
        let n = self.fields().len();
        if n == 0 {
            return false;
        }
        if k.key == KeyInput::Tab {
            let d = if k.modifiers.shift { -1 } else { 1 };
            self.focus = (self.focus as isize + d).rem_euclid(n as isize) as usize;
            return true;
        }
        let i = self.focus.min(n - 1);
        match (&mut self.fields_mut()[i].input, &k.key) {
            (Input::Text { value, .. } | Input::Path { value, .. }, KeyInput::Char(c)) => {
                value.push(*c)
            }
            (Input::Text { value, .. } | Input::Path { value, .. }, KeyInput::Backspace) => {
                value.pop();
            }
            (Input::Choice { options, selected }, KeyInput::Down | KeyInput::Char(' ')) => {
                *selected = (*selected + 1) % options.len().max(1)
            }
            (Input::Choice { options, selected }, KeyInput::Up) => {
                *selected = (*selected + options.len().max(1) - 1) % options.len().max(1)
            }
            (Input::Toggle(on), KeyInput::Char(' ')) => *on = !*on,
            _ => return false,
        }
        true
    }

    /// The answer for `action` (`None`: cancelled), and the typed secret
    /// with the keychain item it belongs to.
    fn into_answer(self, action: Option<String>) -> Finished {
        let mut values: BTreeMap<String, Value> = self
            .fields()
            .iter()
            .map(|f| (f.id.clone(), f.value()))
            .collect();
        if action.is_some()
            && let Some((id, _)) = self.visible_choices().get(self.choose.cursor)
        {
            values.insert(CHOICE.into(), Value::Choice(id.clone()));
        }
        let secret = match (&self.spec.kind, &action) {
            (DialogKind::Secret(s), Some(_)) => Some((s.name.clone(), self.secret)),
            _ => None,
        };
        let values = if action.is_some() {
            values
        } else {
            BTreeMap::new()
        };
        Finished {
            id: self.spec.id,
            answer: DialogAnswer {
                action,
                context: self.spec.context,
                values,
                secret: None,
            },
            secret,
        }
    }
}

/// A closed dialog's answer, before the shell routes it.
#[derive(Debug)]
pub struct Finished {
    pub id: String,
    pub answer: DialogAnswer,
    /// `(keychain item, value)` from a `Secret` dialog that was not cancelled.
    pub secret: Option<(String, SecretValue)>,
}

/// One dialog at a time; the rest wait in order.
#[derive(Debug, Default)]
pub struct Dialogs {
    open: Option<OpenDialog>,
    queue: VecDeque<DialogSpec>,
}

impl Dialogs {
    pub fn open(&self) -> Option<&OpenDialog> {
        self.open.as_ref()
    }
    pub fn open_mut(&mut self) -> Option<&mut OpenDialog> {
        self.open.as_mut()
    }
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }
    pub fn queued(&self) -> usize {
        self.queue.len()
    }

    /// Show `spec`, or queue it behind the open dialog. A spec whose id is
    /// already open or queued replaces that one where it stands (a module
    /// asking again with validation text, a progress update).
    pub fn push(&mut self, spec: DialogSpec) {
        match &mut self.open {
            None => self.open = Some(OpenDialog::new(spec)),
            Some(o) if o.spec.id == spec.id => {
                let focus = o.focus.min(match &spec.kind {
                    DialogKind::Form(fs) => fs.len().saturating_sub(1),
                    _ => 0,
                });
                o.spec = spec;
                o.focus = focus;
            }
            Some(_) => match self.queue.iter_mut().find(|q| q.id == spec.id) {
                Some(q) => *q = spec,
                None => self.queue.push_back(spec),
            },
        }
    }

    /// Withdraw dialog `id` without an answer (the job it tracked ended).
    pub fn close(&mut self, id: &str) {
        self.queue.retain(|q| q.id != id);
        if self.open.as_ref().is_some_and(|o| o.spec.id == id) {
            self.open = self.queue.pop_front().map(OpenDialog::new);
        }
    }

    /// Close the open dialog with `action` (`None` or [`CANCEL`]:
    /// cancelled) and open the next.
    pub fn finish(&mut self, action: Option<&str>) -> Option<Finished> {
        let open = self.open.take()?;
        self.open = self.queue.pop_front().map(OpenDialog::new);
        let action = action.filter(|a| *a != CANCEL).map(str::to_string);
        Some(open.into_answer(action))
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

/// Where a native picker's answer goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickPurpose {
    /// Fill path field `field` of dialog `dialog`, if it is still open.
    DialogField { dialog: String, field: usize },
    /// Answer command `<module>.<verb>` with the path under [`PATH`].
    Command(String),
}

impl Workbench {
    /// The open dialog's keymap: every key is the dialog's while it is up.
    pub(crate) fn dialog_key(&mut self, k: &KeyPress) -> bool {
        let Some(d) = self.dialogs.open_mut() else {
            return false;
        };
        match d.key(k) {
            Step::Act(a) => self.dialog_act(&a),
            Step::Cancel => self.dialog_cancel(),
            Step::Edited | Step::Ignored => {}
        }
        true
    }

    /// Press action `action` of the open dialog.
    pub fn dialog_act(&mut self, action: &str) {
        let Some(f) = self.dialogs.finish(Some(action)) else {
            return;
        };
        let Finished {
            id,
            mut answer,
            secret,
        } = f;
        if let Some((name, value)) = secret {
            answer.secret = Some(match self.secrets.as_mut() {
                Some(port) => port.store(&name, value),
                None => Err("no secret store is attached".into()),
            });
        }
        self.deliver(&id, answer);
    }

    /// Escape, the close button, a press outside: the asker hears it.
    pub fn dialog_cancel(&mut self) {
        if let Some(f) = self.dialogs.finish(None) {
            self.deliver(&f.id, f.answer);
        }
    }

    pub fn dialog_field(&mut self, field: usize, option: Option<usize>) {
        if let Some(d) = self.dialogs.open_mut() {
            d.press_field(field, option);
        }
    }

    /// A `Choose` row pressed: pick it and run the primary action.
    pub fn dialog_choose(&mut self, row: usize) {
        let Some(d) = self.dialogs.open_mut() else {
            return;
        };
        d.choose.cursor = row;
        if let Some(a) = d.spec.primary().map(|a| a.id.clone()) {
            self.dialog_act(&a);
        }
    }

    /// Ask the windowing layer's picker to fill path field `field`.
    pub fn dialog_browse(&mut self, field: usize) {
        let Some(d) = self.dialogs.open() else {
            return;
        };
        let Some(Input::Path { pick, .. }) = d.fields().get(field).map(|f| &f.input) else {
            return;
        };
        let purpose = PickPurpose::DialogField {
            dialog: d.spec.id.clone(),
            field,
        };
        self.requests.push(pick_request(pick.clone(), purpose));
    }

    /// The native picker answered (`None`: dismissed).
    pub fn picked(&mut self, purpose: PickPurpose, path: Option<PathBuf>) {
        let Some(path) = path else {
            return;
        };
        let path = path.display().to_string();
        match purpose {
            PickPurpose::DialogField { dialog, field } => {
                if let Some(d) = self.dialogs.open_mut().filter(|d| d.spec.id == dialog) {
                    d.fill_path(field, &path);
                }
            }
            PickPurpose::Command(id) => {
                let mut answer = DialogAnswer {
                    action: Some(PATH.into()),
                    ..Default::default()
                };
                answer.values.insert(PATH.into(), Value::Path(path));
                self.deliver(&id, answer);
            }
        }
    }

    /// Hand `answer` to the module owning `id`'s namespace.
    fn deliver(&mut self, id: &str, answer: DialogAnswer) {
        let module = id.split('.').next().unwrap_or_default();
        if module == "shell" {
            self.shell_answer(id, &answer);
            return;
        }
        let Some(i) = self.registry.index_of(module) else {
            self.status = format!("no module answers {id}");
            return;
        };
        let intents = self
            .registry
            .get_mut(i)
            .map(|m| m.answer(id, &answer))
            .unwrap_or_default();
        for it in intents {
            self.apply(it);
        }
        if i == self.active {
            self.revalidate_selection();
        }
    }

    fn shell_answer(&mut self, id: &str, answer: &DialogAnswer) {
        if id == crate::new_workspace::CREATING {
            self.cancel_workspace_creation();
            return;
        }
        if id == crate::new_workspace::NEW {
            self.new_workspace_answer(answer);
            return;
        }
        if id == crate::adoption::ADOPT_NEW_DIALOG {
            self.adopt_new_workspace_answer(answer);
            return;
        }
        if id == RECENT && answer.action.is_some() {
            let pick = answer.text(CHOICE).and_then(|i| i.parse::<usize>().ok());
            if let Some(p) = pick.and_then(|i| self.recent.get(i)) {
                self.requests.push(ShellRequest::OpenWorkspace(p.clone()));
            }
        }
    }

    /// `shell.workspace.recent`: choose among recent workspaces.
    pub(crate) fn choose_recent(&mut self) {
        if self.recent.is_empty() {
            self.status = "no recent workspaces".into();
            return;
        }
        let rows = self
            .recent
            .iter()
            .enumerate()
            .map(|(i, p)| Choice {
                id: i.to_string(),
                label: p.display().to_string(),
            })
            .collect();
        self.apply(Intent::dialog(
            DialogSpec::new(RECENT, "Open recent workspace", DialogKind::Choose(rows))
                .cancel("Cancel")
                .action("open", "Open", ActionTone::Primary),
        ));
    }
}

pub(crate) const RECENT: &str = "shell.workspace.recent";

pub(crate) fn pick_request(pick: Pick, purpose: PickPurpose) -> ShellRequest {
    match pick {
        Pick::File(filters) => ShellRequest::PickFile { filters, purpose },
        Pick::Folder => ShellRequest::PickFolder { purpose },
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn press(d: &mut OpenDialog, k: KeyInput) -> Step {
        d.key(&KeyPress::new(k))
    }

    fn typed(d: &mut OpenDialog, s: &str) {
        for c in s.chars() {
            press(d, KeyInput::Char(c));
        }
    }

    fn prompt(id: &str) -> DialogSpec {
        DialogSpec::new(
            id,
            "Capture idea",
            DialogKind::Prompt(Field::text("text", "Idea", "")),
        )
        .cancel("Cancel")
        .action("capture", "Capture", ActionTone::Primary)
    }

    #[test]
    fn enter_runs_the_primary_action_and_escape_cancels() {
        let mut d = OpenDialog::new(prompt("m.a"));
        typed(&mut d, "hi");
        assert_eq!(press(&mut d, KeyInput::Enter), Step::Act("capture".into()));
        assert_eq!(press(&mut d, KeyInput::Escape), Step::Cancel);
        assert_eq!(d.fields()[0].value(), Value::Text("hi".into()));
        press(&mut d, KeyInput::Backspace);
        assert_eq!(d.fields()[0].value(), Value::Text("h".into()));
    }

    #[test]
    fn one_dialog_opens_at_a_time_and_the_rest_wait_in_order() {
        let mut ds = Dialogs::default();
        ds.push(prompt("m.a"));
        ds.push(prompt("m.b"));
        ds.push(prompt("m.c"));
        assert_eq!(ds.open().unwrap().spec.id, "m.a");
        assert_eq!(ds.queued(), 2);
        let f = ds.finish(Some(CANCEL)).unwrap();
        assert_eq!((f.id.as_str(), f.answer.action.as_deref()), ("m.a", None));
        assert_eq!(ds.open().unwrap().spec.id, "m.b");
        ds.close("m.c");
        ds.finish(Some("capture"));
        assert!(!ds.is_open());
    }

    #[test]
    fn asking_again_replaces_the_open_dialog_in_place() {
        let mut ds = Dialogs::default();
        ds.push(prompt("m.a"));
        ds.push(prompt("m.b"));
        ds.push(prompt("m.a").error("Type something first"));
        assert_eq!(ds.queued(), 1);
        let open = ds.open().unwrap();
        assert_eq!(open.spec.error.as_deref(), Some("Type something first"));
    }

    #[test]
    fn answers_carry_field_values_and_context() {
        let spec = DialogSpec::new(
            "m.form",
            "Form",
            DialogKind::Form(vec![
                Field::text("name", "Name", ""),
                Field::choice("op", "Operator", &["=", "≠"], 0),
                Field::toggle("save", "Save", false),
            ]),
        )
        .context("note-1")
        .action("add", "Add", ActionTone::Primary);
        let mut ds = Dialogs::default();
        ds.push(spec);
        let d = ds.open_mut().unwrap();
        typed(d, "x");
        press(d, KeyInput::Tab);
        press(d, KeyInput::Down);
        press(d, KeyInput::Tab);
        press(d, KeyInput::Char(' '));
        let f = ds.finish(Some("add")).unwrap();
        let a = f.answer;
        assert!(a.is("add"));
        assert_eq!(a.context.as_deref(), Some("note-1"));
        assert_eq!(a.text("name"), Some("x"));
        assert_eq!(a.text("op"), Some("≠"));
        assert_eq!(a.values["save"], Value::Toggle(true));
    }

    #[test]
    fn choose_filters_like_the_palette_and_answers_the_row_id() {
        let spec = DialogSpec::new(
            "shell.pick",
            "Pick",
            DialogKind::Choose(vec![
                Choice {
                    id: "a".into(),
                    label: "Alpha".into(),
                },
                Choice {
                    id: "b".into(),
                    label: "Beta".into(),
                },
            ]),
        )
        .action("open", "Open", ActionTone::Primary);
        let mut ds = Dialogs::default();
        ds.push(spec);
        typed(ds.open_mut().unwrap(), "bet");
        assert_eq!(ds.open().unwrap().visible_choices().len(), 1);
        let a = ds.finish(Some("open")).unwrap().answer;
        assert_eq!(a.values[CHOICE], Value::Choice("b".into()));
    }

    #[test]
    fn a_cancelled_dialog_answers_no_values() {
        let mut ds = Dialogs::default();
        ds.push(prompt("m.a"));
        typed(ds.open_mut().unwrap(), "draft");
        let a = ds.finish(None).unwrap().answer;
        assert_eq!(a.action, None);
        assert!(a.values.is_empty());
    }

    #[test]
    fn a_typed_secret_never_shows_in_debug_output_and_leaves_only_with_the_answer() {
        let spec = DialogSpec::new(
            "publish.secret",
            "App password",
            DialogKind::Secret(SecretField {
                label: "App password".into(),
                name: "bsky".into(),
            }),
        )
        .action("store", "Store", ActionTone::Primary);
        let mut ds = Dialogs::default();
        ds.push(spec.clone());
        typed(ds.open_mut().unwrap(), "hunter2-xyzzy");
        assert_eq!(ds.open().unwrap().secret_chars(), 13);
        assert!(!format!("{ds:?}").contains("hunter2"), "{ds:?}");
        let f = ds.finish(Some("store")).unwrap();
        assert!(!format!("{f:?}").contains("hunter2"), "{f:?}");
        let (name, value) = f.secret.expect("the secret leaves with the answer");
        assert_eq!((name.as_str(), value.expose()), ("bsky", "hunter2-xyzzy"));

        ds.push(spec);
        typed(ds.open_mut().unwrap(), "hunter2");
        assert!(ds.finish(None).unwrap().secret.is_none(), "cancel drops it");
    }
}
