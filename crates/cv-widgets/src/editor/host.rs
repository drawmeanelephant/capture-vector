//! Native editor companions. All windows share the event-loop thread, so
//! the session and per-window bookkeeping are thread-local. Window open /
//! close reconciliation stays in `app::sync_companions`; this module only
//! builds editor windows and reports their dirty state and closes.
//!
//! rune's `on_close` cannot veto a title-bar close (the native window is
//! destroyed), so a dirty editor closed that way is opened again over the
//! same session buffer, asking to save or discard ([`on_closed`]); the model-level close path
//! (`Windows::close(id, false)`) answers `NeedsConfirm` because
//! [`sync_dirty`] mirrors buffer dirtiness.

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
    static REOPEN: RefCell<Vec<WindowId>> = const { RefCell::new(Vec::new()) };
    static ASK: RefCell<Vec<WindowId>> = const { RefCell::new(Vec::new()) };
    static OPENS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
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
        .unwrap_or_else(unqueued)
}

/// `open` queues exactly one editor per native window, so this is never
/// expected; if it happens the window says so instead of panicking. It
/// gets its own empty session so it cannot touch the user's buffers.
fn unqueued() -> EditorWindow {
    let ses = Rc::new(RefCell::new(EditorSession::new()));
    let mut w = EditorWindow::new(
        WindowId(u32::MAX),
        ses,
        cv_ui_editor::BufferId(u32::MAX),
        Theme::dark(),
    );
    w.fault = Some(super::EditorError::NotQueued);
    w
}

fn closed(s: &mut EditorWindow, _tx: &mut Tx) {
    on_closed(s);
}

/// A window closed (title bar, shortcut, `Tx::close_window`). Over unsaved
/// edits no other window shows, it is queued to reopen asking to confirm;
/// otherwise it is queued as closed.
pub fn on_closed(s: &mut EditorWindow) {
    s.wait_save();
    let shown_elsewhere = BOUND.with(|b| {
        b.borrow()
            .iter()
            .any(|(w, buf)| *w != s.id && *buf == s.buffer)
    });
    if s.is_dirty() && !shown_elsewhere && !rune::quitting() {
        s.closing = super::Closing::Confirm { stuck: false };
        REOPEN.with(|r| r.borrow_mut().push(s.id));
        ASK.with(|a| a.borrow_mut().push(s.id));
    } else {
        s.closing = super::Closing::Open;
        CLOSED.with(|c| c.borrow_mut().push(s.id));
    }
    wake();
}

/// Ids of editor windows closed over unsaved edits. Their native window is
/// gone; the next [`open`] for the id asks to save or discard.
pub fn take_reopen() -> Vec<WindowId> {
    REOPEN.with(|r| std::mem::take(&mut *r.borrow_mut()))
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
    if ASK.with(|a| {
        let mut a = a.borrow_mut();
        let ask = a.contains(&id);
        a.retain(|x| *x != id);
        ask
    }) {
        w.closing = super::Closing::Confirm { stuck: false };
    }
    // A closed window's host may still be going; never reuse its key.
    let n = OPENS.with(|c| {
        c.set(c.get() + 1);
        c.get()
    });
    bind(id, buffer);
    PENDING.with(|p| p.borrow_mut().push_back(w));
    Some(rune::open_window(
        WindowSpec::new(
            format!("editor-{}-{n}", id.0),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unqueued_native_editor_window_reports_it_instead_of_panicking() {
        let w = init();
        assert_eq!(w.error(), Some(super::super::EditorError::NotQueued));
        assert!(w.text().is_empty());
    }
}
