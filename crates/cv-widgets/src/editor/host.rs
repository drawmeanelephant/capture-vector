//! Native editor companions. All windows share the event-loop thread, so
//! the session and per-window bookkeeping are thread-local. Window open /
//! close reconciliation stays in `app::sync_companions`; this module only
//! builds editor windows and reports their dirty state and closes.
//!
//! rune's `on_close` cannot veto a title-bar close, so a dirty editor
//! closed that way keeps its buffer in the session (nothing is dropped);
//! the model-level close path (`Windows::close(id, false)`) still answers
//! `NeedsConfirm` because [`sync_dirty`] mirrors buffer dirtiness.

use super::EditorWindow;
use cv_core::diagnostic::Diagnostic;
use cv_core::engine::EventSink;
use cv_core::events::engine::EngineEvent;
use cv_core::events::{AppEvent, BuildId};
use cv_model::theme::Theme;
use cv_model::windows::{WindowId, Windows};
use cv_ui_editor::EditorSession;
use cv_ui_editor::save::ContentWriter;
use rune::{AppSpec, Tx, Waker, WindowHandle, WindowSpec};
use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;
use std::sync::{Arc, Mutex, OnceLock};

thread_local! {
    static SESSION: Rc<RefCell<EditorSession>> = Rc::new(RefCell::new(EditorSession::new()));
    static WRITER: RefCell<Option<Arc<dyn ContentWriter>>> = const { RefCell::new(None) };
    static PENDING: RefCell<VecDeque<EditorWindow>> = const { RefCell::new(VecDeque::new()) };
    static CLOSED: RefCell<Vec<WindowId>> = const { RefCell::new(Vec::new()) };
    static BOUND: RefCell<BTreeMap<WindowId, cv_ui_editor::BufferId>> = const { RefCell::new(BTreeMap::new()) };
}

static WAKE: OnceLock<Waker> = OnceLock::new();
/// Finished builds from the engine thread, drained on the UI thread.
static BUILDS: Mutex<Vec<(BuildId, Vec<Diagnostic>)>> = Mutex::new(Vec::new());

/// The process-wide (UI-thread) editor session.
pub fn session() -> Rc<RefCell<EditorSession>> {
    SESSION.with(Rc::clone)
}

pub fn set_writer(w: Arc<dyn ContentWriter>) {
    WRITER.with(|c| *c.borrow_mut() = Some(w));
}

pub fn set_waker(w: Waker) {
    let _ = WAKE.set(w);
}

pub(crate) fn wake() {
    if let Some(w) = WAKE.get() {
        w();
    }
}

fn init() -> EditorWindow {
    PENDING
        .with(|p| p.borrow_mut().pop_front())
        .expect("editor window queued before open")
}

fn closed(s: &mut EditorWindow, _tx: &mut Tx) {
    s.wait_save();
    CLOSED.with(|c| c.borrow_mut().push(s.id));
    wake();
}

/// Open a native editor window for companion `id` on buffer `path`.
/// `None` when no buffer is open on that path.
pub fn open(id: WindowId, path: &str, theme: &Theme) -> Option<WindowHandle> {
    let ses = session();
    let buffer = ses.borrow().find(path)?;
    let mut w = EditorWindow::new(id, ses, buffer, theme.clone());
    if let Some(writer) = WRITER.with(|c| c.borrow().clone()) {
        w = w.with_writer(writer);
    }
    bind(id, buffer);
    PENDING.with(|p| p.borrow_mut().push_back(w));
    Some(rune::open_window(
        WindowSpec::new(
            format!("editor-{}", id.0),
            AppSpec {
                poll: Some(super::poll),
                on_key: Some(super::on_key),
                ..AppSpec::new("Capture Vector — editor", super::SIZE, init, super::view)
            },
        )
        .on_close(closed),
    ))
}

/// Record that window `id` edits `buffer` (dirty mirroring, close).
pub fn bind(id: WindowId, buffer: cv_ui_editor::BufferId) {
    BOUND.with(|b| b.borrow_mut().insert(id, buffer));
}

/// Ids of editor windows the user closed since the last call.
pub fn take_closed() -> Vec<WindowId> {
    let ids = CLOSED.with(|c| std::mem::take(&mut *c.borrow_mut()));
    BOUND.with(|b| {
        let mut b = b.borrow_mut();
        for id in &ids {
            b.remove(id);
        }
    });
    ids
}

/// Mirror each bound buffer's dirtiness onto its window state.
pub fn sync_dirty(windows: &mut Windows) {
    sync_dirty_from(windows, &session().borrow());
}

pub fn sync_dirty_from(windows: &mut Windows, ses: &EditorSession) {
    BOUND.with(|b| {
        for (id, buf) in b.borrow().iter() {
            windows.set_dirty(*id, ses.is_dirty(*buf));
        }
    });
}

/// Wrap the engine sink so finished builds also reach the editor session
/// (handed over on the UI thread by [`ingest_builds`]).
pub fn tee_builds(mut inner: EventSink) -> EventSink {
    Box::new(move |env| {
        if let (
            Some(build),
            AppEvent::Engine(
                EngineEvent::BuildFinished { diagnostics, .. }
                | EngineEvent::BuildFailed { diagnostics, .. },
            ),
        ) = (&env.build_id, &env.payload)
        {
            BUILDS
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push((build.clone(), diagnostics.clone()));
        }
        inner(env);
    })
}

/// Give every open buffer the diagnostics of builds that finished since
/// the last call. True when any arrived.
pub fn ingest_builds() -> bool {
    let builds = std::mem::take(&mut *BUILDS.lock().unwrap_or_else(|e| e.into_inner()));
    let ses = session();
    for (build, diags) in &builds {
        ses.borrow_mut().ingest_build(build.clone(), diags);
    }
    !builds.is_empty()
}
