//! Workbench — the shell's whole state (the rune `S`), pure and testable.
//! Canary pipeline: engine `EventSink` → channel → `drain` (on wake) →
//! commit (dedupe by event id, journal to the `Store` port, cascade,
//! module ingest) → redraw.

use crate::app_state::AppState;
use crate::cascade;
use crate::command::{CommandSpec, Intent, Palette};
use crate::dialog::{Dialogs, FileFilter, PickPurpose, SecretPort};
use crate::module::{Link, ProjectSnapshot, Registry, RowModel};
use crate::modules::activity;
use crate::new_workspace::{self, Archetypes, Notices, ShellNotice, WorkspaceCreation};
use crate::theme::{Mode, Theme};
use crate::windows::{CloseOutcome, PRIMARY, WindowId, Windows};
use cv_core::engine::{CancelToken, EventSink, WatchHandle};
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
/// triggered it. Snapshots not yet drained coalesce to the latest: see
/// `Inbox::send`.
pub enum Inbound {
    Envelope(Envelope),
    Snapshot(ProjectSnapshot),
    Watch(Box<dyn WatchHandle>),
    /// The engine worker stopped this watch and released its writer seat.
    WatchStopped(SessionId),
    Degraded(DegradedReason),
    Status(String),
}

/// What the queue carries. A snapshot waits in the `SnapshotSlot`; the
/// queue holds a marker where the first undrained one was sent.
#[allow(clippy::large_enum_variant)] // every message was an unboxed `Inbound` before the marker existed
enum Queued {
    Msg(Inbound),
    Snapshot,
}

/// The latest snapshot not yet drained.
#[derive(Default)]
struct SnapshotSlot {
    latest: Option<ProjectSnapshot>,
    /// Set when the workbench resets or drops this queue.
    closed: bool,
}

type Slot = Arc<Mutex<SnapshotSlot>>;

fn lock(slot: &Slot) -> std::sync::MutexGuard<'_, SnapshotSlot> {
    slot.lock().unwrap_or_else(|e| e.into_inner())
}

fn close(slot: &Slot) {
    let mut s = lock(slot);
    s.closed = true;
    s.latest = None;
}

/// `Send` handle onto the workbench queue: enqueue + wake the UI loop.
/// Sends after `reset_project` land nowhere (the old queue is dropped).
#[derive(Clone)]
pub struct Inbox {
    tx: Sender<Queued>,
    snapshot: Slot,
    waker: Arc<Mutex<Option<Waker>>>,
}

impl Inbox {
    /// False once the workbench moved on (reset or dropped).
    ///
    /// A snapshot sent while an earlier one is still undrained replaces it
    /// and takes its place in the queue. Every other message is queued
    /// FIFO as sent, so their relative order is untouched, and whatever
    /// was sent after a snapshot drains after one at least that new.
    pub fn send(&self, m: Inbound) -> bool {
        let ok = match m {
            Inbound::Snapshot(snap) => {
                let mut slot = lock(&self.snapshot);
                if slot.closed {
                    return false;
                }
                if slot.latest.replace(snap).is_some() {
                    // The pending marker is queued and its wake delivered.
                    return true;
                }
                // Queued under the slot lock, so no other sender can fill
                // the slot and queue messages ahead of this marker.
                let ok = self.tx.send(Queued::Snapshot).is_ok();
                if !ok {
                    slot.latest = None;
                }
                ok
            }
            m => self.tx.send(Queued::Msg(m)).is_ok(),
        };
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

pub struct Workbench {
    pub app: AppState,
    pub theme: Theme,
    pub registry: Registry,
    pub active: usize,
    /// Per-module nav selection, indexed like `registry`.
    pub nav: Vec<Option<String>>,
    pub windows: Windows,
    pub palette: Palette,
    /// The open modal dialog and the ones queued behind it.
    pub dialogs: Dialogs,
    pub inspector_open: bool,
    /// Inspector sections whose open state the user flipped from the
    /// default (open when it has items), by title.
    pub sections_flipped: BTreeSet<String>,
    /// Shows the window bookkeeping (dirty state per window) in the
    /// inspector; a debugging aid, off by default.
    pub debug_windows: bool,
    pub status: String,
    /// Root of the open workspace, if any.
    pub workspace: Option<PathBuf>,
    /// Recently opened workspaces, newest first (palette entries).
    pub recent: Vec<PathBuf>,
    /// Pending requests for the windowing layer; drained by its poll.
    pub requests: Vec<ShellRequest>,
    /// What New workspace offers, or why it is unavailable.
    pub archetypes: Result<Archetypes, String>,
    /// New workspace's default location while no workspace is open.
    pub new_parent: Option<PathBuf>,
    /// Cancellation for the one workspace creation progress dialog.
    pub(crate) create_cancel: Option<CancelToken>,
    notice_tx: Sender<ShellNotice>,
    notice_rx: Receiver<ShellNotice>,
    /// `(event_id, envelope digest)` — the contract's idempotency key.
    seen: BTreeSet<(String, u64)>,
    /// Applied but not yet journaled (store write failed); retried on replay.
    unjournaled: BTreeSet<String>,
    tx: Sender<Queued>,
    rx: Receiver<Queued>,
    snapshot: Slot,
    waker: Arc<Mutex<Option<Waker>>>,
    store: Option<Box<dyn Store>>,
    pub(crate) secrets: Option<Box<dyn SecretPort>>,
    watch: Option<Box<dyn WatchHandle>>,
    index_rebuild_requested: bool,
    local_seq: u64,
}

impl Workbench {
    pub fn new(registry: Registry) -> Self {
        let (tx, rx) = channel();
        let (notice_tx, notice_rx) = channel();
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
            dialogs: Dialogs::default(),
            inspector_open: true,
            sections_flipped: BTreeSet::new(),
            debug_windows: false,
            status: String::new(),
            workspace: None,
            recent: Vec::new(),
            requests: Vec::new(),
            archetypes: Err(new_workspace::LOCATING.into()),
            new_parent: None,
            create_cancel: None,
            notice_tx,
            notice_rx,
            seen: BTreeSet::new(),
            unjournaled: BTreeSet::new(),
            tx,
            rx,
            snapshot: Slot::default(),
            waker: Arc::new(Mutex::new(None)),
            store: None,
            secrets: None,
            watch: None,
            index_rebuild_requested: false,
            local_seq: 0,
        }
    }

    pub fn with_store(mut self, store: Box<dyn Store>) -> Self {
        self.store = Some(store);
        self
    }

    /// Where `Secret` dialogs store what was typed.
    pub fn with_secrets(mut self, secrets: Box<dyn SecretPort>) -> Self {
        self.secrets = Some(secrets);
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

    /// Where shell-level workers report; survives `reset_project`.
    pub fn notices(&self) -> Notices {
        Notices {
            tx: self.notice_tx.clone(),
            waker: self.waker.clone(),
        }
    }

    pub fn inbox(&self) -> Inbox {
        Inbox {
            tx: self.tx.clone(),
            snapshot: self.snapshot.clone(),
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
        close(&self.snapshot);
        self.snapshot = Slot::default();
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
        // Their askers were just replaced.
        self.dialogs.clear();
    }

    /// Drain pending envelopes. Returns true when visible state changed.
    pub fn drain(&mut self) -> bool {
        let mut changed = false;
        while let Ok(n) = self.notice_rx.try_recv() {
            self.notice(n);
            changed = true;
        }
        while let Ok(q) = self.rx.try_recv() {
            let m = match q {
                Queued::Msg(m) => m,
                Queued::Snapshot => match lock(&self.snapshot).latest.take() {
                    Some(snap) => Inbound::Snapshot(snap),
                    None => continue,
                },
            };
            changed |= match m {
                Inbound::Envelope(env) => self.commit(&env),
                Inbound::Snapshot(snap) => self.load_snapshot(&snap),
                Inbound::Watch(h) => {
                    self.stop_watch();
                    self.watch = Some(h);
                    false
                }
                Inbound::WatchStopped(session) => {
                    if self.watch.as_ref().is_some_and(|h| *h.session() == session) {
                        self.watch = None;
                    }
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
        if !self.seen.insert((env.event_id.0.clone(), digest(env))) {
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
        let owed: Vec<Intent> = self
            .registry
            .iter_mut()
            .flat_map(|m| m.take_owed())
            .collect();
        for it in owed {
            self.apply(it);
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
        let _ = self.tx.send(Queued::Msg(Inbound::Envelope(env)));
        self.drain();
    }

    fn local_envelope(&mut self, payload: AppEvent) -> Envelope {
        self.local_seq += 1;
        Envelope {
            cause: None,
            workspace: WorkspaceId("local".into()),
            snapshot_epoch: SnapshotEpoch(self.app.snapshot_epoch),
            build_id: None,
            cursor: Cursor {
                session: SessionId("shell".into()),
                seq: self.local_seq,
            },
            event_id: EventId(format!("shell-{}", self.local_seq)),
            payload,
        }
    }

    fn degraded_envelope(&mut self, reason: DegradedReason) -> Envelope {
        self.local_envelope(AppEvent::Shell(ShellEvent::Degraded { reason }))
    }

    // ---- theme ----

    pub fn set_mode(&mut self, mode: Mode) {
        self.rebuild_theme(mode, self.theme.motion.reduced);
    }

    pub fn set_reduced_motion(&mut self, reduced: bool) {
        self.rebuild_theme(self.theme.mode, reduced);
    }

    /// Opt in to (or out of) the signature effects; kept across toggles.
    pub fn set_effects(&mut self, on: bool) {
        self.theme.effects = on;
    }

    fn rebuild_theme(&mut self, mode: Mode, reduced: bool) {
        let effects = self.theme.effects;
        self.theme = Theme::for_mode(mode, reduced);
        self.theme.effects = effects;
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
    pub(crate) fn revalidate_selection(&mut self) {
        let Some((i, key)) = self.windows.primary.row.clone() else {
            return;
        };
        if i < self.list_len() && self.row(i).key == key {
            return;
        }
        match self.row_index(&key) {
            Some(j) => self.select_row(j),
            None => self.clear_selection(),
        }
    }

    pub fn select_key(&mut self, key: &str) {
        if let Some(i) = self.row_index(key) {
            self.select_row(i);
        }
    }

    /// The first row of the active mailbox whose key is `key`.
    fn row_index(&self, key: &str) -> Option<usize> {
        self.registry
            .get(self.active)?
            .index_of_key(self.active_nav(), key)
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
        let Some((_, key)) = self.windows.primary.row.clone() else {
            return;
        };
        let key = match self.row_index(&key) {
            Some(i) => {
                if let Some(row) = &mut self.windows.primary.row {
                    row.0 = i;
                }
                return;
            }
            None => self
                .windows
                .primary
                .selection
                .noun
                .clone()
                .and_then(|noun| {
                    self.registry
                        .get(self.active)
                        .and_then(|m| m.key_for(&noun))
                }),
        };
        match key.and_then(|k| self.row_index(&k).map(|i| (i, k))) {
            Some((i, _)) => self.select_row(i),
            None => self.clear_selection(),
        }
    }

    // ---- commands ----

    pub fn commands(&self) -> Vec<CommandSpec> {
        let mut out = vec![
            CommandSpec::new("shell.palette", "Command palette").chord("mod+k"),
            CommandSpec::new("shell.theme.toggle", "Toggle light/dark theme"),
            CommandSpec::new("shell.motion.toggle", "Toggle reduced motion"),
            CommandSpec::new("shell.inspector.toggle", "Toggle inspector").chord("mod+i"),
            CommandSpec::new("shell.debug.windows", "Toggle window debug info"),
            CommandSpec::new(
                "shell.window.companion",
                "Open selection in companion window",
            )
            .chord("enter"),
            CommandSpec::new("shell.window.close-companions", "Close companion windows"),
            CommandSpec::new("shell.degraded.dismiss", "Dismiss degraded banner"),
            CommandSpec::new("shell.watch.restart", "Restart watch (reopen workspace)"),
            CommandSpec::new("shell.follow", "Open linked noun").chord("mod+o"),
            CommandSpec::new("shell.workspace.open", "Open workspace…").chord("mod+shift+o"),
            CommandSpec::new("shell.doctor", "Open Doctor"),
            CommandSpec::new(new_workspace::NEW, "New workspace…")
                .chord("mod+shift+n")
                .disabled(self.archetypes.as_ref().err().cloned()),
        ];
        // Offered only while the banner it backs is up; a live watch
        // needs no restart and the palette stays honest.
        if self.app.degraded == Some(DegradedReason::WatchDead) {
            out.push(CommandSpec::new("watch.restart", "Restart watch"));
        }
        if !self.recent.is_empty() {
            out.push(CommandSpec::new(
                crate::dialog::RECENT,
                "Open recent workspace…",
            ));
        }
        if matches!(
            self.app.degraded,
            Some(DegradedReason::InitialBuildFailed { .. })
        ) {
            out.push(CommandSpec::new(
                "shell.degraded.reveal",
                "Show the failed build's diagnostic",
            ));
        }
        for (i, p) in self.recent.iter().enumerate() {
            out.push(CommandSpec::new(
                format!("shell.workspace.recent.{i}"),
                format!("Open recent workspace: {}", p.display()),
            ));
        }
        for (i, m) in self.registry.iter().enumerate() {
            let s = m.spec();
            if s.slot == crate::module::Slot::Utility {
                out.extend(m.commands());
                continue;
            }
            let mut c =
                CommandSpec::new(format!("shell.goto.{}", s.id), format!("Go to {}", s.title));
            // Utilities stay off the number row so a workspace and the
            // fixture demo (which adds Demo) number the same modules.
            if i < 9 && s.slot != crate::module::Slot::Utilities {
                c = c.chord(&format!("mod+{}", i + 1));
            }
            out.push(c);
            out.extend(m.commands());
        }
        out
    }

    /// Whether the inspector section `title` shows its items: open when
    /// it has any, unless the user flipped it.
    pub fn section_open(&self, title: &str, empty: bool) -> bool {
        empty == self.sections_flipped.contains(title)
    }

    pub fn toggle_section(&mut self, title: &str) {
        if !self.sections_flipped.remove(title) {
            self.sections_flipped.insert(title.to_string());
        }
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
            "shell.debug.windows" => self.debug_windows = !self.debug_windows,
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
            "shell.degraded.reveal" => self.reveal_degraded_cause(),
            "shell.follow" => self.follow_link(),
            "shell.doctor" => self.open_doctor(),
            // Reopening the workspace is the restart: `open_workspace`
            // stops the dead handle and respawns the engine boot thread,
            // and the next `WatchSessionStarted` clears `WatchDead`.
            "watch.restart" => match &self.workspace {
                Some(p) => self.requests.push(ShellRequest::OpenWorkspace(p.clone())),
                None => self.status = "no workspace open".into(),
            },
            "shell.workspace.open" => self.requests.push(ShellRequest::PickWorkspace),
            new_workspace::NEW => self.new_workspace(),
            crate::dialog::RECENT => self.choose_recent(),
            // Reopening the open workspace starts a new watch and rearms recovery.
            "shell.watch.restart" => match self.workspace.clone() {
                Some(root) => self.requests.push(ShellRequest::OpenWorkspace(root)),
                None => self.status = "no workspace is open; open one to start a watch".into(),
            },
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
                // selected now, not on a noun it resolved earlier. A
                // utility module's verbs act on the active selection.
                let utility = self
                    .registry
                    .get(i)
                    .is_some_and(|m| m.spec().slot == crate::module::Slot::Utility);
                if i == self.active || utility {
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

    /// Select, in Activity, the diagnostic a failed initial build names.
    fn reveal_degraded_cause(&mut self) {
        let Some(DegradedReason::InitialBuildFailed { code, message }) = &self.app.degraded else {
            self.status = "no failed build to show".into();
            return;
        };
        let title = activity::diagnostic_title(code, message);
        let key = self
            .registry
            .index_of(activity::ID)
            .and_then(|i| self.registry.get(i))
            .and_then(|m| {
                (0..m.list_len(None))
                    .map(|i| m.row(None, i))
                    .find(|r| r.title == title)
            })
            .map(|r| r.key);
        match key {
            Some(key) => self.apply(Intent::Reveal {
                module: activity::ID.into(),
                key,
            }),
            None => self.status = format!("{code} is not in Activity"),
        }
    }

    /// Consume a queued index-rebuild maintenance request. The
    /// composition root uses it while reopening the same workspace.
    pub fn take_index_rebuild_request(&mut self) -> bool {
        std::mem::take(&mut self.index_rebuild_requested)
    }

    pub fn apply(&mut self, intent: Intent) {
        match intent {
            Intent::Degrade(reason) => self.disclose(reason),
            Intent::Pages(ev) => {
                let env = self.local_envelope(AppEvent::Pages(ev));
                self.commit(&env);
            }
            Intent::Recover => self.app.degraded = None,
            Intent::Status(s) => self.status = s,
            Intent::RebuildIndexes => match self.workspace.clone() {
                Some(root) => {
                    self.index_rebuild_requested = true;
                    self.requests.push(ShellRequest::OpenWorkspace(root));
                }
                None => self.status = "no workspace open; nothing to rebuild".into(),
            },
            Intent::MarkDirty => self.windows.set_dirty(PRIMARY, true),
            Intent::Copy(text) => self.requests.push(ShellRequest::Copy(text)),
            Intent::OpenCompanion { key } => {
                let Some(m) = self.registry.get(self.active) else {
                    return;
                };
                // Only a workspace's pages are edited: the fixture demo
                // reads the checkout's own corpora.
                if let Some(page) = m.source(&key).filter(|_| self.workspace.is_some()) {
                    self.requests.push(ShellRequest::EditPage(page));
                    return;
                }
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
            Intent::Dialog(spec) => self.dialogs.push(*spec),
            Intent::CloseDialog(id) => self.dialogs.close(&id),
            Intent::Pick { id, pick } => self
                .requests
                .push(crate::dialog::pick_request(pick, PickPurpose::Command(id))),
        }
    }

    /// Show the Doctor with its first row selected, so the reading pane
    /// opens on the engine verdict.
    pub fn open_doctor(&mut self) {
        match self.registry.index_of(crate::modules::doctor::ID) {
            Some(i) => {
                self.activate(i);
                if self.selected_key().is_none() {
                    self.select_row(0);
                }
            }
            None => self.status = "Doctor is not registered in this build".into(),
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
        if self.dialogs.is_open() {
            return self.dialog_key(k);
        }
        if self.palette.open {
            return self.palette_key(k);
        }
        match &k.key {
            KeyInput::Command('k') => self.palette.toggle(),
            KeyInput::Command('i') => self.run_command("shell.inspector.toggle"),
            KeyInput::Command('o') if k.shift => self.run_command("shell.workspace.open"),
            KeyInput::Command('n') if k.shift => self.run_command(new_workspace::NEW),
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
                let selected = self.palette.selected(&cmds).cloned();
                self.palette.close();
                if let Some(c) = selected {
                    if let Some(why) = c.disabled {
                        self.status = format!("{} is unavailable: {why}", c.title);
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

/// Idempotency digest: the serialized envelope (event_id is deduped
/// separately, so this stands in for the contract's `input_hash` — a
/// reused id with a different payload or session is a new event, not a
/// replay).
fn digest(env: &Envelope) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_vec(env).unwrap_or_default().hash(&mut h);
    h.finish()
}

impl Drop for Workbench {
    fn drop(&mut self) {
        close(&self.snapshot);
        self.stop_watch();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watch_restart_is_offered_only_while_the_watch_is_dead() {
        let mut wb = Workbench::new(Registry::with_all());
        assert!(wb.commands().iter().all(|c| c.id != "watch.restart"));
        wb.disclose(DegradedReason::WatchDead);
        assert!(wb.commands().iter().any(|c| c.id == "watch.restart"));
        wb.apply(Intent::Recover);
        assert!(wb.commands().iter().all(|c| c.id != "watch.restart"));
    }

    #[test]
    fn watch_restart_reopens_the_workspace() {
        let mut wb = Workbench::new(Registry::with_all());
        wb.workspace = Some(PathBuf::from("/ws"));
        wb.run_command("watch.restart");
        assert_eq!(
            wb.requests,
            vec![ShellRequest::OpenWorkspace(PathBuf::from("/ws"))]
        );
    }

    #[test]
    fn watch_restart_without_a_workspace_only_notes_the_status() {
        let mut wb = Workbench::new(Registry::with_all());
        wb.run_command("watch.restart");
        assert!(wb.requests.is_empty());
        assert_eq!(wb.status, "no workspace open");
    }
}
