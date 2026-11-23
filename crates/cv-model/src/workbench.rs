//! Workbench — the shell's whole state (the rune `S`), pure and testable.
//! Canary pipeline: engine `EventSink` → channel → `drain` (on wake) →
//! commit (dedupe by event id, journal to the `Store` port, cascade,
//! module ingest) → redraw.

use crate::app_state::AppState;
use crate::cascade;
use crate::command::{CommandSpec, Intent, Palette};
use crate::module::{Link, ProjectSnapshot, Registry, RowModel};
use crate::theme::{Mode, Theme};
use crate::windows::{CloseOutcome, PRIMARY, WindowId, Windows};
use cv_core::engine::{EventSink, WatchHandle};
use cv_core::events::Envelope;
use cv_core::events::shell::{DegradedReason, ShellEvent};
use cv_core::events::{AppEvent, Cursor, EventId, SessionId, SnapshotEpoch};
use cv_core::id::{Selection, StableId, WorkspaceId};
use cv_core::store::{Namespace, Store};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

/// Same shape as `rune::Waker`, without importing rune.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

pub const JOURNAL: &str = "shell.journal";

/// Something a background thread (engine boot, watch pump) hands the UI
/// loop. One queue so a resync snapshot lands before the envelope that
/// triggered it.
pub enum Inbound {
    Envelope(Envelope),
    Snapshot(ProjectSnapshot),
    Watch(Box<dyn WatchHandle>),
    Degraded(DegradedReason),
    Status(String),
}

/// `Send` handle onto the workbench queue: enqueue + wake the UI loop.
/// Sends after `reset_project` land nowhere (the old queue is dropped).
#[derive(Clone)]
pub struct Inbox {
    tx: Sender<Inbound>,
    waker: Arc<Mutex<Option<Waker>>>,
}

impl Inbox {
    /// False once the workbench moved on (reset or dropped).
    pub fn send(&self, m: Inbound) -> bool {
        let ok = self.tx.send(m).is_ok();
        if ok
            && let Some(w) = self
                .waker
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
        {
            w();
        }
        ok
    }
    pub fn sink(&self) -> EventSink {
        let me = self.clone();
        Box::new(move |env| {
            me.send(Inbound::Envelope(env));
        })
    }
}

/// Requests only the windowing layer can carry out (native dialogs) or
/// the composition root owns (opening a workspace).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellRequest {
    PickWorkspace,
    OpenWorkspace(PathBuf),
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

pub struct Workbench {
    pub app: AppState,
    pub theme: Theme,
    pub registry: Registry,
    pub active: usize,
    /// Per-module nav selection, indexed like `registry`.
    pub nav: Vec<Option<String>>,
    pub windows: Windows,
    pub palette: Palette,
    pub inspector_open: bool,
    pub status: String,
    /// Root of the open workspace, if any.
    pub workspace: Option<PathBuf>,
    /// Recently opened workspaces, newest first (palette entries).
    pub recent: Vec<PathBuf>,
    /// Pending requests for the windowing layer; drained by its poll.
    pub requests: Vec<ShellRequest>,
    seen: BTreeSet<String>,
    /// Applied but not yet journaled (store write failed); retried on replay.
    unjournaled: BTreeSet<String>,
    tx: Sender<Inbound>,
    rx: Receiver<Inbound>,
    waker: Arc<Mutex<Option<Waker>>>,
    store: Option<Box<dyn Store>>,
    watch: Option<Box<dyn WatchHandle>>,
    local_seq: u64,
}

impl Workbench {
    pub fn new(registry: Registry) -> Self {
        let (tx, rx) = channel();
        let nav = registry
            .iter()
            .map(|m| m.nav().items.first().map(|i| i.key.clone()))
            .collect();
        Self {
            app: AppState::default(),
            theme: Theme::dark(),
            registry,
            active: 0,
            nav,
            windows: Windows::new(),
            palette: Palette::default(),
            inspector_open: true,
            status: String::new(),
            workspace: None,
            recent: Vec::new(),
            requests: Vec::new(),
            seen: BTreeSet::new(),
            unjournaled: BTreeSet::new(),
            tx,
            rx,
            waker: Arc::new(Mutex::new(None)),
            store: None,
            watch: None,
            local_seq: 0,
        }
    }

    pub fn with_store(mut self, store: Box<dyn Store>) -> Self {
        self.store = Some(store);
        self
    }

    pub fn set_waker(&mut self, w: Waker) {
        *self.waker.lock().unwrap_or_else(|e| e.into_inner()) = Some(w);
    }

    pub fn attach_watch(&mut self, h: Box<dyn WatchHandle>) {
        self.watch = Some(h);
    }

    pub fn stop_watch(&mut self) {
        if let Some(mut h) = self.watch.take() {
            h.stop();
        }
    }

    /// An `EventSink` for the engine port: enqueue + wake the UI loop.
    pub fn sink(&self) -> EventSink {
        self.inbox().sink()
    }

    pub fn inbox(&self) -> Inbox {
        Inbox {
            tx: self.tx.clone(),
            waker: self.waker.clone(),
        }
    }

    /// Swap in a fresh project: stop the watch, drop the old queue (late
    /// sends from the previous workspace's threads go nowhere) and
    /// rebuild module state. Theme, windows, waker and store survive.
    pub fn reset_project(&mut self, registry: Registry) {
        self.stop_watch();
        let (tx, rx) = channel();
        self.tx = tx;
        self.rx = rx;
        self.nav = registry
            .iter()
            .map(|m| m.nav().items.first().map(|i| i.key.clone()))
            .collect();
        self.registry = registry;
        self.active = 0;
        self.app = AppState::default();
        self.seen.clear();
        self.unjournaled.clear();
        self.palette = Palette::default();
    }

    /// Drain pending envelopes. Returns true when visible state changed.
    pub fn drain(&mut self) -> bool {
        let mut changed = false;
        while let Ok(m) = self.rx.try_recv() {
            changed |= match m {
                Inbound::Envelope(env) => self.commit(&env),
                Inbound::Snapshot(snap) => self.load_snapshot(&snap),
                Inbound::Watch(h) => {
                    self.stop_watch();
                    self.watch = Some(h);
                    false
                }
                Inbound::Degraded(reason) => {
                    let env = self.degraded_envelope(reason);
                    self.commit(&env)
                }
                Inbound::Status(s) => {
                    self.status = s;
                    true
                }
            };
        }
        changed
    }

    fn journal(&mut self, env: &Envelope) {
        let Some(store) = &self.store else { return };
        let id = &env.event_id.0;
        let doc = serde_json::to_value(env).unwrap_or(serde_json::Value::Null);
        if store
            .put(&Namespace(JOURNAL.into()), &StableId(id.clone()), &doc)
            .is_ok()
        {
            self.unjournaled.remove(id);
        } else {
            self.unjournaled.insert(id.clone());
            self.app.degraded = Some(DegradedReason::StoreCorrupt);
        }
    }

    fn commit(&mut self, env: &Envelope) -> bool {
        if !self.seen.insert(env.event_id.0.clone()) {
            if self.unjournaled.contains(&env.event_id.0) {
                self.journal(env);
            }
            return false;
        }
        self.journal(env);
        let mut changed = cascade::dispatch(&mut self.app, env);
        for m in self.registry.iter_mut() {
            changed |= m.ingest(env);
        }
        self.reclamp_rows();
        changed
    }

    /// Fan a project snapshot out to every module (snapshot resync).
    pub fn load_snapshot(&mut self, snap: &ProjectSnapshot) -> bool {
        self.app.snapshot_epoch = snap.epoch;
        let mut changed = false;
        for m in self.registry.iter_mut() {
            changed |= m.load(snap);
        }
        self.reclamp_rows();
        changed
    }

    /// The row's click-through link, offered only when its target exists.
    pub fn link_for(&self, key: &str) -> Option<Link> {
        let l = self.registry.get(self.active)?.link(key)?;
        let target = self.registry.get(self.registry.index_of(l.module)?)?;
        target.contains(&l.key).then_some(l)
    }

    /// Follow the selected row's click-through link, if any.
    pub fn follow_link(&mut self) {
        let link = self.selected_key().and_then(|k| self.link_for(k));
        match link {
            Some(l) => self.apply(Intent::Reveal {
                module: l.module.to_string(),
                key: l.key,
            }),
            None => self.status = "nothing to open".into(),
        }
    }

    /// Disclose a degraded state through the same commit path as engine events.
    pub fn disclose(&mut self, reason: DegradedReason) {
        let env = self.degraded_envelope(reason);
        let _ = self.tx.send(Inbound::Envelope(env));
        self.drain();
    }

    fn degraded_envelope(&mut self, reason: DegradedReason) -> Envelope {
        self.local_seq += 1;
        Envelope {
            workspace: WorkspaceId("local".into()),
            snapshot_epoch: SnapshotEpoch(self.app.snapshot_epoch),
            build_id: None,
            cursor: Cursor {
                session: SessionId("shell".into()),
                seq: self.local_seq,
            },
            event_id: EventId(format!("shell-{}", self.local_seq)),
            payload: AppEvent::Shell(ShellEvent::Degraded { reason }),
        }
    }

    // ---- theme ----

    pub fn set_mode(&mut self, mode: Mode) {
        self.theme = Theme::for_mode(mode, self.theme.motion.reduced);
    }

    pub fn set_reduced_motion(&mut self, reduced: bool) {
        self.theme = Theme::for_mode(self.theme.mode, reduced);
    }

    // ---- navigation & selection ----

    pub fn active_nav(&self) -> Option<&str> {
        self.nav.get(self.active).and_then(|n| n.as_deref())
    }

    pub fn list_len(&self) -> usize {
        self.registry
            .get(self.active)
            .map_or(0, |m| m.list_len(self.active_nav()))
    }

    pub fn row(&self, i: usize) -> RowModel {
        self.registry
            .get(self.active)
            .map(|m| m.row(self.active_nav(), i))
            .unwrap_or_default()
    }

    pub fn selected_key(&self) -> Option<&str> {
        self.windows.primary.row.as_ref().map(|(_, k)| k.as_str())
    }

    pub fn activate(&mut self, index: usize) {
        if index < self.registry.len() && index != self.active {
            self.active = index;
            self.clear_selection();
            if let Some(m) = self.registry.get(index) {
                self.status = format!("{} — {}", m.spec().slot.title(), m.spec().title);
            }
        }
    }

    pub fn set_nav(&mut self, key: &str) {
        if let Some(slot) = self.nav.get_mut(self.active) {
            *slot = Some(key.to_string());
        }
        self.clear_selection();
    }

    fn cycle_nav(&mut self, delta: isize) {
        let Some(m) = self.registry.get(self.active) else {
            return;
        };
        let items = m.nav().items;
        if items.is_empty() {
            return;
        }
        let cur = items
            .iter()
            .position(|i| Some(i.key.as_str()) == self.active_nav())
            .unwrap_or(0) as isize;
        let next = (cur + delta).rem_euclid(items.len() as isize) as usize;
        let key = items[next].key.clone();
        self.set_nav(&key);
    }

    pub fn clear_selection(&mut self) {
        self.windows.primary.row = None;
        self.windows.primary.selection = Selection::default();
        self.app.selection = Selection::default();
    }

    pub fn select_row(&mut self, i: usize) {
        if i >= self.list_len() {
            return;
        }
        let key = self.row(i).key;
        let noun = self.registry.get(self.active).and_then(|m| m.noun(&key));
        let sel = Selection {
            account: noun.as_ref().map(|n| n.account.clone()),
            mailbox: self.active_nav().map(str::to_string),
            noun,
        };
        self.windows.primary.row = Some((i, key));
        self.windows.primary.selection = sel.clone();
        self.app.selection = sel;
    }

    /// After a module mutates its list, keep the selected key selected at
    /// its new index, or clear the selection when the key left the list.
    fn revalidate_selection(&mut self) {
        let Some((i, key)) = self.windows.primary.row.clone() else {
            return;
        };
        if i < self.list_len() && self.row(i).key == key {
            return;
        }
        match (0..self.list_len()).find(|&j| self.row(j).key == key) {
            Some(j) => self.select_row(j),
            None => self.clear_selection(),
        }
    }

    pub fn select_key(&mut self, key: &str) {
        if let Some(i) = (0..self.list_len()).find(|&i| self.row(i).key == key) {
            self.select_row(i);
        }
    }

    fn move_row(&mut self, delta: isize) {
        let len = self.list_len();
        if len == 0 {
            return;
        }
        let next = match &self.windows.primary.row {
            None if delta >= 0 => 0,
            None => len - 1,
            Some((i, _)) => (*i as isize + delta).clamp(0, len as isize - 1) as usize,
        };
        self.select_row(next);
    }

    fn reclamp_rows(&mut self) {
        if let Some((_, key)) = self.windows.primary.row.clone() {
            let len = self.list_len();
            match (0..len).find(|&i| self.row(i).key == key) {
                Some(i) => self.windows.primary.row = Some((i, key)),
                None => self.clear_selection(),
            }
        }
    }

    // ---- commands ----

    pub fn commands(&self) -> Vec<CommandSpec> {
        let mut out = vec![
            CommandSpec::new("shell.palette", "Command palette").chord("mod+k"),
            CommandSpec::new("shell.theme.toggle", "Toggle light/dark theme"),
            CommandSpec::new("shell.motion.toggle", "Toggle reduced motion"),
            CommandSpec::new("shell.inspector.toggle", "Toggle inspector").chord("mod+i"),
            CommandSpec::new(
                "shell.window.companion",
                "Open selection in companion window",
            )
            .chord("enter"),
            CommandSpec::new("shell.window.close-companions", "Close companion windows"),
            CommandSpec::new("shell.degraded.dismiss", "Dismiss degraded banner"),
            CommandSpec::new("shell.follow", "Open linked noun").chord("mod+o"),
            CommandSpec::new("shell.workspace.open", "Open workspace…").chord("mod+shift+o"),
        ];
        for (i, p) in self.recent.iter().enumerate() {
            out.push(CommandSpec::new(
                format!("shell.workspace.recent.{i}"),
                format!("Open recent workspace: {}", p.display()),
            ));
        }
        for (i, m) in self.registry.iter().enumerate() {
            let s = m.spec();
            let mut c =
                CommandSpec::new(format!("shell.goto.{}", s.id), format!("Go to {}", s.title));
            if i < 9 {
                c = c.chord(&format!("mod+{}", i + 1));
            }
            out.push(c);
            out.extend(m.commands());
        }
        out
    }

    pub fn run_command(&mut self, id: &str) {
        match id {
            "shell.palette" => self.palette.toggle(),
            "shell.theme.toggle" => self.set_mode(match self.theme.mode {
                Mode::Dark => Mode::Light,
                Mode::Light => Mode::Dark,
            }),
            "shell.motion.toggle" => self.set_reduced_motion(!self.theme.motion.reduced),
            "shell.inspector.toggle" => self.inspector_open = !self.inspector_open,
            "shell.window.companion" => {
                if let Some(key) = self.selected_key().map(str::to_string) {
                    self.apply(Intent::OpenCompanion { key });
                }
            }
            "shell.window.close-companions" => {
                let ids: Vec<WindowId> = self.windows.companions.iter().map(|c| c.id).collect();
                for w in ids {
                    self.close_window(w, false);
                }
            }
            "shell.degraded.dismiss" => self.apply(Intent::Recover),
            "shell.follow" => self.follow_link(),
            "shell.workspace.open" => self.requests.push(ShellRequest::PickWorkspace),
            _ => {
                if let Some(i) = id.strip_prefix("shell.workspace.recent.") {
                    match i.parse::<usize>().ok().and_then(|i| self.recent.get(i)) {
                        Some(p) => self.requests.push(ShellRequest::OpenWorkspace(p.clone())),
                        None => self.status = format!("unknown command {id}"),
                    }
                    return;
                }
                if let Some(target) = id.strip_prefix("shell.goto.") {
                    if let Some(i) = self.registry.index_of(target) {
                        self.activate(i);
                    }
                    return;
                }
                let module = id.split('.').next().unwrap_or_default();
                let Some(i) = self.registry.index_of(module) else {
                    self.status = format!("unknown command {id}");
                    return;
                };
                // Re-resolve the selection so the module acts on what is
                // selected now, not on a noun it resolved earlier.
                if i == self.active {
                    let key = self.selected_key().map(str::to_string);
                    if let Some(m) = self.registry.get_mut(i) {
                        m.select(key.as_deref());
                    }
                }
                let intents = self
                    .registry
                    .get_mut(i)
                    .map(|m| m.run(id))
                    .unwrap_or_default();
                for it in intents {
                    self.apply(it);
                }
                if i == self.active {
                    self.revalidate_selection();
                }
            }
        }
    }

    pub fn apply(&mut self, intent: Intent) {
        match intent {
            Intent::Degrade(reason) => self.disclose(reason),
            Intent::Recover => self.app.degraded = None,
            Intent::Status(s) => self.status = s,
            Intent::MarkDirty => self.windows.set_dirty(PRIMARY, true),
            Intent::OpenCompanion { key } => {
                let Some(m) = self.registry.get(self.active) else {
                    return;
                };
                let title = m.reading(&key).title;
                let module = m.spec().id;
                self.windows.open_companion(module, &key, &title);
                self.status = format!("companion: {title}");
            }
            Intent::Reveal { module, key } => {
                let Some(i) = self.registry.index_of(&module) else {
                    self.status = format!("no module {module}");
                    return;
                };
                self.activate(i);
                let first = self
                    .registry
                    .get(i)
                    .and_then(|m| m.nav().items.first().map(|n| n.key.clone()));
                if let Some(nav) = first {
                    self.set_nav(&nav);
                }
                self.select_key(&key);
                if self.selected_key() != Some(key.as_str()) {
                    self.status = format!("{key} is not in {module}");
                }
            }
        }
    }

    pub fn close_window(&mut self, id: WindowId, force: bool) -> CloseOutcome {
        let out = self.windows.close(id, force);
        if let CloseOutcome::NeedsConfirm(ws) = &out {
            self.status = format!("{} window(s) have unsaved changes", ws.len());
        }
        out
    }

    // ---- keyboard ----

    /// Keyboard-first shell keymap. Returns true when consumed.
    pub fn key(&mut self, k: &KeyPress) -> bool {
        if self.palette.open {
            return self.palette_key(k);
        }
        match &k.key {
            KeyInput::Command('k') => self.palette.toggle(),
            KeyInput::Command('i') => self.run_command("shell.inspector.toggle"),
            KeyInput::Command('o') if k.shift => self.run_command("shell.workspace.open"),
            KeyInput::Command('o') => self.follow_link(),
            KeyInput::Command(c) if c.is_ascii_digit() && *c != '0' => {
                self.activate(*c as usize - '1' as usize)
            }
            KeyInput::Down | KeyInput::Char('j') => self.move_row(1),
            KeyInput::Up | KeyInput::Char('k') => self.move_row(-1),
            KeyInput::Tab => self.cycle_nav(if k.shift { -1 } else { 1 }),
            KeyInput::Enter => self.run_command("shell.window.companion"),
            KeyInput::Escape => self.clear_selection(),
            _ => return false,
        }
        true
    }

    fn palette_key(&mut self, k: &KeyPress) -> bool {
        let cmds = self.commands();
        match &k.key {
            KeyInput::Escape | KeyInput::Command('k') => self.palette.close(),
            KeyInput::Char(c) => self.palette.type_char(*c),
            KeyInput::Backspace => self.palette.backspace(),
            KeyInput::Down | KeyInput::Tab => self
                .palette
                .move_cursor(1, self.palette.matches(&cmds).len()),
            KeyInput::Up => self
                .palette
                .move_cursor(-1, self.palette.matches(&cmds).len()),
            KeyInput::Enter => {
                let id = self.palette.selected(&cmds).map(|c| c.id.clone());
                self.palette.close();
                if let Some(id) = id {
                    self.run_command(&id);
                }
            }
            _ => return false,
        }
        true
    }
}

impl Drop for Workbench {
    fn drop(&mut self) {
        self.stop_watch();
    }
}
