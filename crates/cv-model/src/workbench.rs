//! Workbench — the shell's whole state (the rune `S`), pure and testable.
//! Canary pipeline: engine `EventSink` → channel → `drain` (on wake) →
//! commit (dedupe by event id, journal to the `Store` port, cascade,
//! module ingest) → redraw.

use crate::adoption::AdoptionJob;
use crate::app_state::AppState;
use crate::cascade;
use crate::command::{CommandSpec, Intent, Palette};
use crate::dialog::{Dialogs, PickPurpose, SecretPort};
use crate::module::{Link, ProjectSnapshot, Registry};
use crate::modules::activity;
use crate::new_workspace::{self, Archetypes, Notices, ShellNotice};
use crate::status::{StatusKind, StatusNotice};
use crate::theme::{Mode, Theme};
use crate::windows::{CloseOutcome, PRIMARY, WindowId, Windows};
use crate::workspace_key::WorkspaceKey;
mod disclosure;
mod input;
mod navigation;
mod outcome_retention;
mod viewport;
use cv_core::engine::{CancelToken, EventSink, WatchHandle};
use cv_core::events::Envelope;
use cv_core::events::shell::{DegradedReason, ShellEvent};
use cv_core::events::{AppEvent, Cursor, EventId, SessionId, SnapshotEpoch};
use cv_core::id::{StableId, WorkspaceId};
use cv_core::store::{Namespace, Store};
pub use disclosure::ValueExpansionKey;
pub use input::{KeyInput, KeyPress, ShellRequest};
use outcome_retention::{
    MAX_PENDING_OUTCOME_BYTES, MAX_PENDING_OUTCOMES, dedupe_outcome_copies, digest,
    enforce_outcome_retention,
};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
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
    Status(StatusNotice),
}

/// What the queue carries. A snapshot waits in the `SnapshotSlot`; the
/// queue holds a marker where the first undrained one was sent.
#[allow(clippy::large_enum_variant)] // every message was an unboxed `Inbound` before the marker existed
enum Queued {
    Msg { seq: u64, msg: Inbound },
    Snapshot { seq: u64 },
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
    causal: Arc<AtomicU64>,
}

impl Inbox {
    /// False once the workbench moved on (reset or dropped).
    ///
    /// A snapshot sent while an earlier one is still undrained replaces it
    /// and takes its place in the queue. Every other message is queued
    /// FIFO as sent, so their relative order is untouched, and whatever
    /// was sent after a snapshot drains after one at least that new.
    pub fn send(&self, m: Inbound) -> bool {
        self.try_send(m).is_ok()
    }

    /// Send like [`Inbox::send`], but return the message on failure. This
    /// is load-bearing for values whose drop performs cleanup: the caller
    /// can move that drop outside whatever lock it was publishing under.
    pub fn try_send(&self, m: Inbound) -> Result<(), Box<Inbound>> {
        let ok = match m {
            Inbound::Snapshot(snap) => {
                let mut slot = lock(&self.snapshot);
                if slot.closed {
                    return Err(Box::new(Inbound::Snapshot(snap)));
                }
                if slot.latest.replace(snap).is_some() {
                    // The pending marker is queued and its wake delivered.
                    return Ok(());
                }
                // Queued under the slot lock, so no other sender can fill
                // the slot and queue messages ahead of this marker.
                let seq = self.causal.fetch_add(1, Ordering::SeqCst) + 1;
                match self.tx.send(Queued::Snapshot { seq }) {
                    Ok(()) => true,
                    Err(_) => {
                        let snap = slot.latest.take().expect("stored above");
                        return Err(Box::new(Inbound::Snapshot(snap)));
                    }
                }
            }
            m => {
                let seq = self.causal.fetch_add(1, Ordering::SeqCst) + 1;
                match self.tx.send(Queued::Msg { seq, msg: m }) {
                    Ok(()) => true,
                    Err(e) => {
                        let Queued::Msg { msg, .. } = e.0 else {
                            unreachable!("watch messages are never snapshot markers")
                        };
                        return Err(Box::new(msg));
                    }
                }
            }
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
        Ok(())
    }
    pub fn sink(&self) -> EventSink {
        let me = self.clone();
        Box::new(move |env| {
            me.send(Inbound::Envelope(env));
        })
    }
}

/// Generation-independent terminal engine outcomes. Unlike [`Inbox`], this
/// queue deliberately survives `reset_project`: workspace-stable operations
/// such as migration adoption can finish while the shell is between
/// generations, and their outcomes are committed by whichever generation of
/// the owning workspace instance the user ends in. The sequence is causal
/// data shared with live delivery, not a drain-order policy; replays reuse
/// the first minted sequence. Delivery of the same envelope through a live
/// inbox is deduplicated by the normal commit idempotency key.
#[derive(Clone)]
pub struct OutcomeSink {
    tx: Sender<(u64, WorkspaceKey, Envelope)>,
    waker: Arc<Mutex<Option<Waker>>>,
    causal: Arc<AtomicU64>,
    send_lock: Arc<Mutex<()>>,
}

impl OutcomeSink {
    /// Send an outcome and return its causal sequence. Journal replays must
    /// reuse that returned stamp with [`OutcomeSink::send_stamped_for`]:
    /// replay time is not the event's causal time.
    pub fn send_for(&self, workspace: WorkspaceKey, env: Envelope) -> u64 {
        let (seq, sent) = {
            let _send = self.send_lock.lock().unwrap_or_else(|e| e.into_inner());
            let seq = self.causal.fetch_add(1, Ordering::SeqCst) + 1;
            (seq, self.publish(seq, workspace, env))
        };
        self.wake(sent);
        seq
    }

    /// Republish an outcome with the causal sequence minted when its terminal
    /// fact first occurred. This is for journal replay only; fresh outcomes
    /// must use [`OutcomeSink::send_for`].
    pub fn send_stamped_for(&self, seq: u64, workspace: WorkspaceKey, env: Envelope) {
        let sent = {
            let _send = self.send_lock.lock().unwrap_or_else(|e| e.into_inner());
            self.publish(seq, workspace, env)
        };
        self.wake(sent);
    }

    fn publish(&self, seq: u64, workspace: WorkspaceKey, env: Envelope) -> bool {
        self.tx.send((seq, workspace, env)).is_ok()
    }

    fn wake(&self, sent: bool) {
        if sent
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
    /// Inspector values the user expanded inline, scoped by workspace,
    /// module, mailbox, row and stable noun evidence so one record's
    /// expansion never leaks into another's. Long values (paths, argv,
    /// hashes) truncate in the pane's width; expansion is how their full
    /// text shows without leaving the app.
    values_expanded: BTreeSet<ValueExpansionKey>,
    /// Shows the window bookkeeping (dirty state per window) in the
    /// inspector; a debugging aid, off by default.
    pub debug_windows: bool,
    pub status: String,
    /// Classification of `status`, used to keep ambient refresh/engine
    /// notices from overwriting foreground operation outcomes and progress.
    status_kind: Option<StatusKind>,
    /// Whether a foreground operation has started and not yet produced its
    /// first terminal/evidence notice. This is the shell's smallest causal
    /// operation boundary: it lets evidence for a newly started operation
    /// supersede an older operation's outcome without letting ambient
    /// evidence erase every fresh outcome.
    status_operation_open: bool,
    /// Root of the open workspace, if any.
    pub workspace: Option<PathBuf>,
    /// Instance identity of `workspace`, shared with scheduler registration.
    pub workspace_key: Option<WorkspaceKey>,
    /// Identity evidence errors already surfaced for this shell, by path/error.
    identity_disclosures: BTreeSet<String>,
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
    outcome_tx: Sender<(u64, WorkspaceKey, Envelope)>,
    outcome_rx: Receiver<(u64, WorkspaceKey, Envelope)>,
    outcome_send_lock: Arc<Mutex<()>>,
    /// Outcomes whose owning workspace is not currently open; retained so a
    /// foreign workspace never receives them as unattributed facts. Retention
    /// is bounded (64 outcomes and about 1 MiB serialized); oldest foreign
    /// outcomes are evicted with a status disclosure when either bound is hit.
    pending_outcomes: Vec<(u64, WorkspaceKey, Envelope)>,
    /// `(event_id, envelope digest)` — the contract's idempotency key.
    seen: BTreeSet<(String, u64)>,
    /// Applied but not yet journaled (store write failed); retried on replay.
    unjournaled: BTreeSet<String>,
    tx: Sender<Queued>,
    rx: Receiver<Queued>,
    causal: Arc<AtomicU64>,
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
        let (outcome_tx, outcome_rx) = channel();
        let causal = Arc::new(AtomicU64::new(1));
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
            values_expanded: BTreeSet::new(),
            debug_windows: false,
            status: String::new(),
            status_kind: None,
            status_operation_open: false,
            workspace: None,
            workspace_key: None,
            identity_disclosures: BTreeSet::new(),
            recent: Vec::new(),
            requests: Vec::new(),
            archetypes: Err(new_workspace::LOCATING.into()),
            new_parent: None,
            create_cancel: None,
            notice_tx,
            notice_rx,
            outcome_tx,
            outcome_rx,
            outcome_send_lock: Arc::new(Mutex::new(())),
            pending_outcomes: Vec::new(),
            seen: BTreeSet::new(),
            unjournaled: BTreeSet::new(),
            tx,
            rx,
            causal,
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

    /// Generation-spanning terminal outcome queue for workspace-stable workers.
    pub fn outcome_sink(&self) -> OutcomeSink {
        OutcomeSink {
            tx: self.outcome_tx.clone(),
            waker: self.waker.clone(),
            causal: self.causal.clone(),
            send_lock: self.outcome_send_lock.clone(),
        }
    }

    pub fn inbox(&self) -> Inbox {
        Inbox {
            tx: self.tx.clone(),
            snapshot: self.snapshot.clone(),
            waker: self.waker.clone(),
            causal: self.causal.clone(),
        }
    }

    /// Classification of the current status line.
    pub fn status_kind(&self) -> Option<StatusKind> {
        self.status_kind
    }

    /// Mark that a foreground operation is starting even when its first
    /// visible notice will be engine evidence rather than progress.
    pub fn begin_status_operation(&mut self) {
        self.status_operation_open = true;
    }

    /// Present a classified status notice. Progress and outcomes are the
    /// foreground operation lifecycle: progress starts or continues an
    /// operation and may supersede an older outcome; an outcome completes
    /// whatever is visible. While progress is live, ambient bookkeeping and
    /// engine evidence cannot displace it. A completed outcome holds against
    /// both ambient classes until a new operation starts; evidence belonging
    /// to that new operation may supersede the older outcome. Returns whether
    /// the visible status changed.
    pub fn set_status_notice(&mut self, notice: StatusNotice) -> bool {
        self.set_status_kind(notice.kind, notice.text)
    }

    /// [`set_status_notice`] with the kind and text separate.
    pub fn set_status_kind(&mut self, kind: StatusKind, text: impl Into<String>) -> bool {
        let text = text.into();
        let replace = match kind {
            StatusKind::Outcome | StatusKind::Progress => true,
            StatusKind::Bookkeeping => !matches!(
                self.status_kind,
                Some(StatusKind::Outcome | StatusKind::Progress)
            ),
            StatusKind::EngineEvidence => {
                !matches!(self.status_kind, Some(StatusKind::Progress))
                    && (self.status_operation_open
                        || !matches!(self.status_kind, Some(StatusKind::Outcome)))
            }
        };
        if !replace {
            return false;
        }
        match kind {
            StatusKind::Progress => self.status_operation_open = true,
            StatusKind::Outcome | StatusKind::EngineEvidence => self.status_operation_open = false,
            StatusKind::Bookkeeping => {}
        }
        if self.status == text && self.status_kind == Some(kind) {
            return false;
        }
        self.status = text;
        self.status_kind = Some(kind);
        true
    }

    /// Convenience for a completed user-requested operation.
    pub fn outcome_status(&mut self, text: impl Into<String>) -> bool {
        self.set_status_kind(StatusKind::Outcome, text)
    }

    /// Convenience for foreground operation progress.
    pub fn progress_status(&mut self, text: impl Into<String>) -> bool {
        self.set_status_kind(StatusKind::Progress, text)
    }

    /// Convenience for ambient refresh/presentation bookkeeping.
    pub fn bookkeeping_status(&mut self, text: impl Into<String>) -> bool {
        self.set_status_kind(StatusKind::Bookkeeping, text)
    }

    /// Convenience for machinery evidence that Activity also carries.
    pub fn evidence_status(&mut self, text: impl Into<String>) -> bool {
        self.set_status_kind(StatusKind::EngineEvidence, text)
    }

    /// Dismiss the status line; the next ambient notice may show again.
    pub fn clear_status(&mut self) -> bool {
        let changed = !self.status.is_empty();
        self.status.clear();
        self.status_kind = None;
        self.status_operation_open = false;
        changed
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
        self.sections_flipped.clear();
        self.values_expanded.clear();
        self.palette = Palette::default();
        // Degraded identity evidence belongs to the workspace, not one
        // module generation; disclose it afresh after each rebuild so the
        // persistent degraded channel in the new AppState represents it.
        self.identity_disclosures.clear();
        // Their askers were just replaced.
        self.dialogs.clear();
    }

    /// Drain pending envelopes. Returns true when visible state changed.
    pub fn drain(&mut self) -> bool {
        enum Item {
            Inbox(Queued),
            Outcome(Envelope),
        }

        fn queued_seq(q: &Queued) -> u64 {
            match q {
                Queued::Msg { seq, .. } | Queued::Snapshot { seq } => *seq,
            }
        }

        let mut changed = false;
        while let Ok(n) = self.notice_rx.try_recv() {
            self.notice(n);
            changed = true;
        }

        // Live and generation-spanning outcome paths share one logical
        // enqueue clock. Merge by that sequence rather than applying a fixed
        // queue-first/live-first policy: either side may be causally first,
        // and the sequence is the data that says which. Foreign outcomes
        // stay held for their owning workspace instead of entering the merge.
        let mut pending = std::mem::take(&mut self.pending_outcomes);
        while let Ok(env) = self.outcome_rx.try_recv() {
            pending.push(env);
        }
        dedupe_outcome_copies(&mut pending);
        let mut keep = Vec::new();
        let mut ordered: Vec<(u64, Item)> = Vec::new();
        for (seq, key, env) in pending {
            let current_key = self.workspace_key.clone();
            let current = current_key
                .as_ref()
                .is_some_and(|current| current.same_instance(&key));
            if current {
                ordered.push((seq, Item::Outcome(env)));
            } else {
                keep.push((seq, key, env));
            }
        }
        let evicted = enforce_outcome_retention(&mut keep);
        let mut disclosures = Vec::new();
        let identity_error = self.workspace_key.as_ref().and_then(|key| {
            key.instance_error()
                .map(|error| (key.path().display().to_string(), error.to_string()))
        });
        let mut identity_degraded = None;
        if let Some((path, error)) = identity_error {
            let disclosure_key = format!("{path}::{error}");
            if self.identity_disclosures.insert(disclosure_key) {
                disclosures.push(format!(
                    "workspace instance evidence for {path} is unusable; held outcomes stay unowned: {error}"
                ));
                let seq = self.causal.fetch_add(1, Ordering::SeqCst) + 1;
                identity_degraded =
                    Some((seq, self.degraded_envelope(DegradedReason::StoreCorrupt)));
            }
        }
        if evicted > 0 {
            disclosures.push(format!(
                "discarded {evicted} held outcome(s): retention is bounded to {MAX_PENDING_OUTCOMES} outcomes or {MAX_PENDING_OUTCOME_BYTES} bytes"
            ));
        }
        self.pending_outcomes = keep;
        while let Ok(q) = self.rx.try_recv() {
            ordered.push((queued_seq(&q), Item::Inbox(q)));
        }
        ordered.sort_by_key(|(seq, _)| *seq);

        for (seq, item) in ordered {
            match item {
                Item::Outcome(env) => changed |= self.commit(&env, seq),
                Item::Inbox(q) => {
                    let m = match q {
                        Queued::Msg { msg, .. } => msg,
                        Queued::Snapshot { .. } => match lock(&self.snapshot).latest.take() {
                            Some(snap) => Inbound::Snapshot(snap),
                            None => continue,
                        },
                    };
                    changed |= match m {
                        Inbound::Envelope(env) => self.commit(&env, seq),
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
                            self.commit(&env, seq)
                        }
                        Inbound::Status(s) => self.set_status_notice(s),
                    };
                }
            }
        }
        if let Some((seq, env)) = identity_degraded {
            changed |= self.commit(&env, seq);
        }
        if !disclosures.is_empty() {
            changed |= self.outcome_status(disclosures.join(" · "));
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

    fn commit(&mut self, env: &Envelope, causal: u64) -> bool {
        if !self.seen.insert((env.event_id.0.clone(), digest(env))) {
            if self.unjournaled.contains(&env.event_id.0) {
                self.journal(env);
            }
            return false;
        }
        self.journal(env);
        let mut changed = cascade::dispatch(&mut self.app, env);
        for m in self.registry.iter_mut() {
            changed |= m.ingest_stamped(env, causal);
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
        let mut changed = self.app.graph.update(snap.ir_graph.clone()).changed;
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
                nav: None,
            }),
            None => {
                self.outcome_status("nothing to open");
            }
        }
    }

    /// Disclose a degraded state through the same commit path as engine events.
    pub fn disclose(&mut self, reason: DegradedReason) {
        let env = self.degraded_envelope(reason);
        let seq = self.causal.fetch_add(1, Ordering::SeqCst) + 1;
        let _ = self.tx.send(Queued::Msg {
            seq,
            msg: Inbound::Envelope(env),
        });
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
                None => {
                    self.outcome_status("no workspace open");
                }
            },
            "shell.workspace.open" => self.requests.push(ShellRequest::PickWorkspace),
            new_workspace::NEW => self.new_workspace(),
            crate::dialog::RECENT => self.choose_recent(),
            // Reopening the open workspace starts a new watch and rearms recovery.
            "shell.watch.restart" => match self.workspace.clone() {
                Some(root) => self.requests.push(ShellRequest::OpenWorkspace(root)),
                None => {
                    self.outcome_status("no workspace is open; open one to start a watch");
                }
            },
            _ => {
                if let Some(i) = id.strip_prefix("shell.workspace.recent.") {
                    match i.parse::<usize>().ok().and_then(|i| self.recent.get(i)) {
                        Some(p) => self.requests.push(ShellRequest::OpenWorkspace(p.clone())),
                        None => {
                            self.outcome_status(format!("unknown command {id}"));
                        }
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
                    self.outcome_status(format!("unknown command {id}"));
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
            self.outcome_status("no failed build to show");
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
                nav: None,
            }),
            None => {
                self.outcome_status(format!("{code} is not in Activity"));
            }
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
                let causal = self.causal.fetch_add(1, Ordering::SeqCst) + 1;
                self.commit(&env, causal);
            }
            Intent::Recover => self.app.degraded = None,
            Intent::Outcome(s) => {
                self.outcome_status(s);
            }
            Intent::Progress(s) => {
                self.progress_status(s);
            }
            Intent::Bookkeeping(s) => {
                self.bookkeeping_status(s);
            }
            Intent::EngineEvidence(s) => {
                self.evidence_status(s);
            }
            Intent::RebuildIndexes => match self.workspace.clone() {
                Some(root) => {
                    self.index_rebuild_requested = true;
                    self.requests.push(ShellRequest::OpenWorkspace(root));
                }
                None => {
                    self.outcome_status("no workspace open; nothing to rebuild");
                }
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
                self.bookkeeping_status(format!("companion: {title}"));
            }
            Intent::Reveal { module, key, nav } => {
                let Some(i) = self.registry.index_of(&module) else {
                    self.outcome_status(format!("no module {module}"));
                    return;
                };
                self.activate(i);
                let items = self
                    .registry
                    .get(i)
                    .map(|m| m.nav().items)
                    .unwrap_or_default();
                let chosen = nav
                    .filter(|wanted| items.iter().any(|item| &item.key == wanted))
                    .or_else(|| items.first().map(|item| item.key.clone()));
                if let Some(nav) = chosen {
                    self.set_nav(&nav);
                }
                self.select_key(&key);
                if self.selected_key() != Some(key.as_str()) {
                    self.outcome_status(format!("{key} is not in {module}"));
                }
            }
            Intent::Dialog(spec) => self.dialogs.push(*spec),
            Intent::CloseDialog(id) => self.dialogs.close(&id),
            Intent::Pick { id, pick } => self
                .requests
                .push(crate::dialog::pick_request(pick, PickPurpose::Command(id))),
            Intent::PrepareAdoptNew {
                run_id,
                suggested_archetype,
            } => self.adopt_new_workspace(&run_id, suggested_archetype.as_deref()),
            Intent::AdoptRun(request) => match self.workspace.clone() {
                Some(source_root) => {
                    let cancel = match &request.target {
                        crate::adoption::AdoptionTarget::NewWorkspace(creation) => {
                            creation.cancel.clone()
                        }
                        crate::adoption::AdoptionTarget::ExistingWorkspace { .. }
                        | crate::adoption::AdoptionTarget::ExistingWorkspaceInPlace => {
                            CancelToken::new()
                        }
                    };
                    self.requests.push(ShellRequest::AdoptRun(AdoptionJob {
                        source_root,
                        request,
                        cancel,
                    }));
                }
                None => {
                    self.outcome_status("open the workspace that owns this import run first");
                }
            },
            Intent::RunImport(mut job) => {
                if job.workspace.as_os_str().is_empty() {
                    let Some(root) = self.workspace.clone() else {
                        self.outcome_status("no workspace open; the import cannot start");
                        return;
                    };
                    job.workspace = root;
                }
                self.requests.push(ShellRequest::RunImport(job));
            }
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
            None => {
                self.outcome_status("Doctor is not registered in this build");
            }
        }
    }

    pub fn close_window(&mut self, id: WindowId, force: bool) -> CloseOutcome {
        let out = self.windows.close(id, force);
        if let CloseOutcome::NeedsConfirm(ws) = &out {
            self.outcome_status(format!("{} window(s) have unsaved changes", ws.len()));
        }
        out
    }
}

impl Drop for Workbench {
    fn drop(&mut self) {
        close(&self.snapshot);
        self.stop_watch();
    }
}

#[cfg(test)]
mod tests;
