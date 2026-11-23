//! Rope-backed text buffer: byte-addressed edits, grouped undo/redo, and
//! dirty tracking against the last saved revision.
//!
//! Offsets are UTF-8 byte offsets into the exact text (CRLF counts as two
//! bytes) — the same unit as `cv_core::diagnostic::Span`. An edit whose
//! range does not fall on char boundaries is refused, never rounded.

use cv_core::engine::Fingerprint;
use ropey::Rope;
use std::ops::Range;

/// One replacement, recorded with enough data to invert it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub start: usize,
    pub removed: String,
    pub inserted: String,
}

impl Edit {
    fn inverse(&self) -> Edit {
        Edit {
            start: self.start,
            removed: self.inserted.clone(),
            inserted: self.removed.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditError {
    OutOfBounds { end: usize, len: usize },
    NotCharBoundary(usize),
}

impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EditError::OutOfBounds { end, len } => {
                write!(f, "edit end {end} past buffer length {len}")
            }
            EditError::NotCharBoundary(b) => write!(f, "byte {b} is not a char boundary"),
        }
    }
}

impl std::error::Error for EditError {}

/// An undo step: the edits of one group, and the revisions on either side.
#[derive(Debug, Clone)]
struct Step {
    edits: Vec<Edit>,
    before: u64,
    after: u64,
    /// Typing coalesces into the open step until a boundary.
    open: bool,
}

#[derive(Debug, Clone)]
pub struct Buffer {
    path: String,
    rope: Rope,
    revision: u64,
    next_revision: u64,
    saved_revision: u64,
    base: Option<Fingerprint>,
    undo: Vec<Step>,
    redo: Vec<Step>,
}

impl Buffer {
    /// A buffer over the file's current bytes; `base` is the fingerprint
    /// those bytes were read under (`None` for a file not yet on disk).
    pub fn open(path: impl Into<String>, text: &str, base: Option<Fingerprint>) -> Self {
        Self {
            path: path.into(),
            rope: Rope::from_str(text),
            revision: 0,
            next_revision: 1,
            saved_revision: 0,
            base,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    pub fn len_bytes(&self) -> usize {
        self.rope.len_bytes()
    }

    pub fn slice(&self, range: Range<usize>) -> String {
        self.rope
            .byte_slice(range.start.min(self.len_bytes())..range.end.min(self.len_bytes()))
            .to_string()
    }

    /// Monotonic id of the current text state. Undo returns to an earlier id.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn saved_revision(&self) -> u64 {
        self.saved_revision
    }

    pub fn base(&self) -> Option<Fingerprint> {
        self.base
    }

    pub fn is_dirty(&self) -> bool {
        self.revision != self.saved_revision
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// True if `byte` is a char boundary inside (or at the end of) the text.
    pub fn is_boundary(&self, byte: usize) -> bool {
        byte <= self.len_bytes() && self.rope.char_to_byte(self.rope.byte_to_char(byte)) == byte
    }

    fn check(&self, range: &Range<usize>) -> Result<(), EditError> {
        let len = self.len_bytes();
        if range.end > len || range.start > range.end {
            return Err(EditError::OutOfBounds {
                end: range.end,
                len,
            });
        }
        for b in [range.start, range.end] {
            if !self.is_boundary(b) {
                return Err(EditError::NotCharBoundary(b));
            }
        }
        Ok(())
    }

    fn apply_raw(&mut self, edit: &Edit) {
        let start = self.rope.byte_to_char(edit.start);
        let end = self.rope.byte_to_char(edit.start + edit.removed.len());
        self.rope.remove(start..end);
        self.rope.insert(start, &edit.inserted);
    }

    /// Replace `range` with `text` as one undoable edit. Consecutive plain
    /// typing (single-line inserts and backspaces at the caret) coalesces
    /// into one undo step until [`Buffer::break_undo_group`] or a save.
    pub fn replace(&mut self, range: Range<usize>, text: &str) -> Result<(), EditError> {
        self.check(&range)?;
        if range.is_empty() && text.is_empty() {
            return Ok(());
        }
        let edit = Edit {
            start: range.start,
            removed: self.slice(range.clone()),
            inserted: text.to_string(),
        };
        if edit.removed == edit.inserted {
            return Ok(());
        }
        self.apply_raw(&edit);
        let before = self.revision;
        self.revision = self.next_revision;
        self.next_revision += 1;
        self.redo.clear();
        let typing = !edit.inserted.contains('\n') && !edit.removed.contains('\n');
        match self.undo.last_mut() {
            Some(step)
                if step.open && typing && step.after == before && before != self.saved_revision =>
            {
                step.edits.push(edit);
                step.after = self.revision;
            }
            _ => self.undo.push(Step {
                edits: vec![edit],
                before,
                after: self.revision,
                open: typing,
            }),
        }
        Ok(())
    }

    pub fn insert(&mut self, at: usize, text: &str) -> Result<(), EditError> {
        self.replace(at..at, text)
    }

    /// Replace the whole text with `next` as the minimal single edit
    /// (common prefix/suffix trimmed). Used to fold a widget's edited
    /// string back into the rope.
    pub fn set_text(&mut self, next: &str) -> Result<(), EditError> {
        let cur = self.text();
        let (start, end_cur, end_next) = diff_range(&cur, next);
        self.replace(start..end_cur, &next[start..end_next])
    }

    /// Close the open typing group so the next edit starts a new step.
    pub fn break_undo_group(&mut self) {
        if let Some(step) = self.undo.last_mut() {
            step.open = false;
        }
    }

    /// Walk history back, then forward, to the saved revision. False (and
    /// back at the newest text) when an edit after an undo replaced it.
    pub fn revert_to_saved(&mut self) -> bool {
        while self.is_dirty() && self.undo() {}
        while self.is_dirty() && self.redo() {}
        !self.is_dirty()
    }

    pub fn undo(&mut self) -> bool {
        let Some(mut step) = self.undo.pop() else {
            return false;
        };
        for e in step.edits.iter().rev() {
            self.apply_raw(&e.inverse());
        }
        self.revision = step.before;
        step.open = false;
        self.redo.push(step);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(step) = self.redo.pop() else {
            return false;
        };
        for e in &step.edits {
            self.apply_raw(e);
        }
        self.revision = step.after;
        self.undo.push(step);
        true
    }

    /// Record that `revision` is now what disk holds under `fingerprint`.
    pub fn mark_saved(&mut self, revision: u64, fingerprint: Fingerprint) {
        self.saved_revision = revision;
        self.base = Some(fingerprint);
        self.break_undo_group();
    }

    /// Replace the content with what disk now holds (reload after a
    /// conflict). Clears history: the old steps describe other bytes.
    pub fn reload(&mut self, text: &str, base: Option<Fingerprint>) {
        self.rope = Rope::from_str(text);
        self.revision = self.next_revision;
        self.next_revision += 1;
        self.saved_revision = self.revision;
        self.base = base;
        self.undo.clear();
        self.redo.clear();
    }

    /// 0-based line containing `byte` (clamped to the text).
    pub fn line_of_byte(&self, byte: usize) -> usize {
        self.rope.byte_to_line(byte.min(self.len_bytes()))
    }

    pub fn line_count(&self) -> usize {
        self.rope.len_lines()
    }

    /// Byte range of 0-based `line`, excluding its line ending.
    pub fn line_range(&self, line: usize) -> Option<Range<usize>> {
        if line >= self.rope.len_lines() {
            return None;
        }
        let start = self.rope.line_to_byte(line);
        let next = if line + 1 < self.rope.len_lines() {
            self.rope.line_to_byte(line + 1)
        } else {
            self.len_bytes()
        };
        let text = self.slice(start..next);
        let trimmed = text.trim_end_matches(['\n', '\r']).len();
        Some(start..start + trimmed)
    }
}

/// Byte range that differs between `a` and `b`: `(start, end_in_a, end_in_b)`.
/// Boundaries are snapped to chars so the result slices both strings.
pub fn diff_range(a: &str, b: &str) -> (usize, usize, usize) {
    let mut start = a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count();
    while !a.is_char_boundary(start) || !b.is_char_boundary(start) {
        start -= 1;
    }
    let max_suffix = (a.len() - start).min(b.len() - start);
    let mut suffix = a
        .bytes()
        .rev()
        .zip(b.bytes().rev())
        .take(max_suffix)
        .take_while(|(x, y)| x == y)
        .count();
    while !a.is_char_boundary(a.len() - suffix) || !b.is_char_boundary(b.len() - suffix) {
        suffix -= 1;
    }
    (start, a.len() - suffix, b.len() - suffix)
}

#[cfg(test)]
#[path = "buffer_tests.rs"]
mod tests;
