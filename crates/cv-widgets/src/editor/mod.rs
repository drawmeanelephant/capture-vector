//! L7 editor surface: one window per buffer. The model is
//! `cv_ui_editor`; this module only renders it and routes input back.
//!
//! Layout: header (path, dirty, save status) · frontmatter form · gutter +
//! plain body editor (wikilink decorations + completion) · Problems rail ·
//! draft sketch (`rune_kit::Markdown`, labelled advisory — never a site
//! preview; Boris output in the user's browser is authoritative).

pub mod host;
pub mod parity;
mod view;

use cv_core::engine::WriteOutcome;
use cv_model::theme::Theme;
use cv_model::windows::WindowId;
use cv_ui_editor::completion::{self, WikilinkContext};
use cv_ui_editor::frontmatter::{self, SCHEMA};
use cv_ui_editor::problems::Problem;
use cv_ui_editor::save::{ContentWriter, SaveStatus, SaveTicket, save_blocking};
use cv_ui_editor::sketch::Sketch;
use cv_ui_editor::{BufferId, EditorSession};
use rune::{Env, Key, KeyEvent, Runtime, TextEdit, Tx};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, mpsc};

pub use view::view;

/// `cv_model::windows::Companion::module` for editor companions; the
/// companion's `key` is the buffer's repo-relative path.
pub const MODULE: &str = "editor";

/// Rich-layer gate (see `crates/cv-ui-editor/RICH-PROBE.md`): decorations
/// over the plain field plus wikilink completion. `TextEdit::rich` (marks /
/// HTML) stays off — it would hide Markdown syntax the author must edit.
pub const DECORATIONS: bool = true;

type SaveRx = mpsc::Receiver<cv_core::Result<WriteOutcome>>;

/// Closing a window over unsaved edits. A title-bar close cannot be
/// vetoed, so the host reopens the window in `Confirm`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Closing {
    #[default]
    Open,
    /// Asking: save and close, discard and close, or keep editing.
    /// `stuck`: discard failed (the saved text left undo history).
    Confirm { stuck: bool },
    /// Saving; the window closes once the save lands clean.
    SaveThenClose,
}

pub struct EditorWindow {
    pub id: WindowId,
    pub session: Rc<RefCell<EditorSession>>,
    pub buffer: BufferId,
    pub theme: Theme,
    pub body: TextEdit,
    /// One field per `frontmatter::SCHEMA` key, in schema order.
    pub fields: Vec<TextEdit>,
    pub decorations: bool,
    pub writer: Option<Arc<dyn ContentWriter>>,
    pub sketch: Sketch,
    pub closing: Closing,
    /// `closing` as last drawn: a host-side change needs a rebuild.
    drawn_closing: Closing,
    pending: Option<(SaveTicket, SaveRx)>,
    seen: (u64, u64),
}

impl EditorWindow {
    pub fn new(
        id: WindowId,
        session: Rc<RefCell<EditorSession>>,
        buffer: BufferId,
        theme: Theme,
    ) -> Self {
        let sketch = {
            let s = session.borrow();
            Sketch::of(&s.get(buffer).expect("buffer is open").buffer)
        };
        let mut w = Self {
            id,
            session,
            buffer,
            theme,
            body: TextEdit::new(""),
            fields: SCHEMA.iter().map(|_| TextEdit::new("")).collect(),
            decorations: DECORATIONS,
            writer: None,
            sketch,
            closing: Closing::Open,
            drawn_closing: Closing::Open,
            pending: None,
            seen: (u64::MAX, u64::MAX),
        };
        w.refresh();
        w
    }

    pub fn with_writer(mut self, writer: Arc<dyn ContentWriter>) -> Self {
        self.writer = Some(writer);
        self
    }

    fn text(&self) -> String {
        self.session
            .borrow()
            .get(self.buffer)
            .map(|e| e.buffer.text())
            .unwrap_or_default()
    }

    pub fn is_dirty(&self) -> bool {
        self.session.borrow().is_dirty(self.buffer)
    }

    pub fn save_status(&self) -> SaveStatus {
        self.session
            .borrow()
            .get(self.buffer)
            .map(|e| e.save.clone())
            .unwrap_or(SaveStatus::Idle)
    }

    pub fn problems(&self) -> Vec<Problem> {
        let s = self.session.borrow();
        s.get(self.buffer)
            .map(|e| e.problems.rail(&e.buffer))
            .unwrap_or_default()
    }

    fn body_offset(&self) -> usize {
        frontmatter::read(&self.text()).body_offset
    }

    fn stamp(&self) -> (u64, u64) {
        let s = self.session.borrow();
        (
            s.generation(),
            s.get(self.buffer).map(|e| e.buffer.revision()).unwrap_or(0),
        )
    }

    /// Re-read the session if it changed outside this window (a build
    /// landed, another window edited the same buffer); true when it did.
    pub fn sync(&mut self) -> bool {
        if self.stamp() == self.seen {
            return false;
        }
        self.refresh();
        true
    }

    /// Push buffer text into the widgets (after undo, form edits, reloads).
    pub fn refresh(&mut self) {
        self.seen = self.stamp();
        let text = self.text();
        let form = frontmatter::read(&text);
        let body = &text[form.body_offset..];
        if self.body.text != body {
            let caret = self.body.range().1;
            self.body.text = body.to_string();
            self.body.set_caret(caret.min(body.len()));
        }
        for (spec, edit) in SCHEMA.iter().zip(self.fields.iter_mut()) {
            let v = form.field(spec.key).map(|f| f.value.as_str()).unwrap_or("");
            // `read` trims; keep the field's own edge whitespace while typing.
            if edit.text.trim_matches([' ', '\t']) != v {
                edit.text = v.to_string();
                edit.set_caret(v.len());
            }
        }
        let s = self.session.borrow();
        if let Some(e) = s.get(self.buffer)
            && e.buffer.revision() != self.sketch.revision
        {
            self.sketch = Sketch::of(&e.buffer);
        }
    }

    /// Fold the body field back into the buffer.
    pub fn body_changed(&mut self) {
        let text = self.text();
        let form = frontmatter::read(&text);
        let mut next = text[..form.body_offset].to_string();
        if !self.body.text.is_empty() {
            let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
            // A fence at end of file needs its line ending, and an unclosed
            // block its closing fence, or the typed body would vanish into it.
            if !next.is_empty() && !next.ends_with('\n') {
                next.push_str(nl);
            }
            if form
                .notes
                .iter()
                .any(|n| matches!(n, frontmatter::Note::Unclosed))
            {
                next.push_str("---");
                next.push_str(nl);
            }
        }
        next.push_str(&self.body.text);
        if let Some(e) = self.session.borrow_mut().get_mut(self.buffer) {
            let _ = e.buffer.set_text(&next);
        }
        self.refresh();
    }

    pub fn field_changed(&mut self, i: usize) {
        let (Some(spec), Some(edit)) = (SCHEMA.get(i), self.fields.get(i)) else {
            return;
        };
        let value = edit.text.clone();
        if let Some(e) = self.session.borrow_mut().get_mut(self.buffer) {
            let _ = frontmatter::set_field(&mut e.buffer, spec.key, &value);
        }
        self.refresh();
    }

    pub fn undo(&mut self) -> bool {
        let done = self
            .session
            .borrow_mut()
            .get_mut(self.buffer)
            .is_some_and(|e| e.buffer.undo());
        self.refresh();
        done
    }

    pub fn redo(&mut self) -> bool {
        let done = self
            .session
            .borrow_mut()
            .get_mut(self.buffer)
            .is_some_and(|e| e.buffer.redo());
        self.refresh();
        done
    }

    /// Undo back to what disk holds, then close.
    pub fn discard_and_close(&mut self, tx: &mut Tx) {
        let reverted = self
            .session
            .borrow_mut()
            .get_mut(self.buffer)
            .is_some_and(|e| e.buffer.revert_to_saved());
        self.refresh();
        if reverted {
            self.closing = Closing::Open;
            tx.close_window();
        } else {
            self.closing = Closing::Confirm { stuck: true };
        }
    }

    pub fn save_and_close(&mut self, tx: &mut Tx) {
        self.save();
        self.closing = Closing::SaveThenClose;
        self.step_close(tx);
    }

    /// Close once a save-to-close landed clean; ask again if it did not.
    fn step_close(&mut self, tx: &mut Tx) {
        if self.closing != Closing::SaveThenClose || self.pending.is_some() {
            return;
        }
        if self.is_dirty() {
            self.closing = Closing::Confirm { stuck: false };
        } else {
            self.closing = Closing::Open;
            tx.close_window();
        }
    }

    /// Start a save on a worker thread (never on the UI thread).
    pub fn save(&mut self) {
        if self.pending.is_some() {
            return;
        }
        let Some(ticket) = self.session.borrow_mut().begin_save(self.buffer) else {
            return;
        };
        let Some(writer) = self.writer.clone() else {
            let err = Err(cv_core::Error::Other(
                "no content writer for this buffer".into(),
            ));
            self.session
                .borrow_mut()
                .finish_save(self.buffer, &ticket, err);
            return;
        };
        let (tx, rx) = mpsc::channel();
        let t = ticket.clone();
        std::thread::spawn(move || {
            let _ = tx.send(save_blocking(writer.as_ref(), &t));
            host::wake();
        });
        self.pending = Some((ticket, rx));
    }

    /// Fold a finished save back; true when visible state changed.
    pub fn poll_save(&mut self) -> bool {
        let Some((ticket, rx)) = &self.pending else {
            return false;
        };
        let result = match rx.try_recv() {
            Ok(r) => r,
            Err(mpsc::TryRecvError::Empty) => return false,
            Err(mpsc::TryRecvError::Disconnected) => {
                Err(cv_core::Error::Other("save worker vanished".into()))
            }
        };
        let ticket = ticket.clone();
        self.pending = None;
        self.session
            .borrow_mut()
            .finish_save(self.buffer, &ticket, result);
        true
    }

    /// Block until an in-flight save lands (tests and quit paths).
    pub fn wait_save(&mut self) {
        if let Some((ticket, rx)) = self.pending.take() {
            let r = rx
                .recv()
                .unwrap_or_else(|_| Err(cv_core::Error::Other("save worker vanished".into())));
            self.session
                .borrow_mut()
                .finish_save(self.buffer, &ticket, r);
        }
    }

    /// Move the caret to problem `i`'s target (fresh problems only).
    pub fn jump(&mut self, i: usize) -> bool {
        let Some(target) = self.problems().get(i).and_then(|p| p.target.clone()) else {
            return false;
        };
        let off = self.body_offset();
        if target.start < off {
            return false;
        }
        self.body.anchor = target.start - off;
        self.body.focus = target.end - off;
        true
    }

    /// The open `[[…` at the body caret, in full-buffer coordinates.
    pub fn wikilink(&self) -> Option<WikilinkContext> {
        let off = self.body_offset();
        let mut ctx = completion::wikilink_context(&self.body.text, self.body.range().1)?;
        ctx.range = ctx.range.start + off..ctx.range.end + off;
        Some(ctx)
    }

    /// `(id, title)` candidates for the open wikilink.
    pub fn completions(&self) -> Vec<(String, String)> {
        let Some(ctx) = self.wikilink() else {
            return Vec::new();
        };
        let s = self.session.borrow();
        let Some(idx) = s.completion.as_ref() else {
            return Vec::new();
        };
        idx.complete(&ctx.prefix)
            .into_iter()
            .map(|e| (e.id.clone(), e.title.clone().unwrap_or_default()))
            .collect()
    }

    pub fn accept_completion(&mut self, n: usize) -> bool {
        let (Some(ctx), Some((id, _))) = (self.wikilink(), self.completions().into_iter().nth(n))
        else {
            return false;
        };
        let caret = self
            .session
            .borrow_mut()
            .get_mut(self.buffer)
            .and_then(|e| completion::accept(&mut e.buffer, &ctx, &id).ok());
        self.refresh();
        if let Some(c) = caret {
            let off = self.body_offset();
            self.body.set_caret(c.saturating_sub(off));
        }
        true
    }

    /// Editor shortcuts; true when handled.
    pub fn key(&mut self, ev: &KeyEvent) -> bool {
        match (&ev.key, ev.mods.shift) {
            (Key::Command('z'), false) => self.undo(),
            (Key::Command('z'), true) | (Key::Command('y'), _) => self.redo(),
            (Key::Command('s'), _) => {
                self.save();
                true
            }
            (Key::Tab, false) if !self.completions().is_empty() => self.accept_completion(0),
            _ => false,
        }
    }
}

pub fn on_key(s: &mut EditorWindow, ev: &KeyEvent, _tx: &mut Tx) -> bool {
    s.key(ev)
}

pub fn poll(s: &mut EditorWindow, _t: f64, tx: &mut Tx) -> bool {
    let saved = s.poll_save();
    s.step_close(tx);
    let prompt = std::mem::replace(&mut s.drawn_closing, s.closing) != s.closing;
    s.sync() || saved || prompt
}

pub const SIZE: (f32, f32) = (1100.0, 760.0);

/// Headless runtime over one editor window (goldens, scripted sessions).
pub fn runtime(w: EditorWindow) -> Runtime<EditorWindow> {
    let env = Env {
        system_dark: w.theme.mode == cv_model::theme::Mode::Dark,
        scale: 1.0,
    };
    let mut rt = Runtime::new(w, view, SIZE, env);
    rt.poll = Some(poll);
    rt.on_key = Some(on_key);
    rt.frame(0.0);
    rt
}
