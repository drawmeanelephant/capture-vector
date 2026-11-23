//! Frontmatter as a form over Boris's closed key set.
//!
//! This is form UI, not a grammar: it finds the `---` fences and the
//! `key: value` lines so each key can be edited as a field, and turns field
//! edits back into buffer edits. It never decides validity — unknown keys
//! and odd lines are shown as advisory notes and Boris's next build judges
//! (`EFRONTMATTER`). Keys and value hints mirror Boris 0.9.0
//! `docs/contracts/frontmatter.md` (closed set, v0.1).

use crate::buffer::{Buffer, EditError};
use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    Text,
    EntityId,
    /// One of a fixed set of spellings.
    Choice(&'static [&'static str]),
    /// Bracketed list, e.g. `[a, b]`.
    List,
    /// `YYYY-MM-DDTHH:MM:SSZ`.
    Timestamp,
    Count,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeySpec {
    pub key: &'static str,
    pub kind: FieldKind,
    pub hint: &'static str,
}

pub const STATUS_CHOICES: &[&str] = &["draft", "published", "archived"];

/// Boris 0.9.0's canonical author-facing keys, in form order.
pub const SCHEMA: &[KeySpec] = &[
    KeySpec {
        key: "id",
        kind: FieldKind::EntityId,
        hint: "entity id (defaults to the path)",
    },
    KeySpec {
        key: "title",
        kind: FieldKind::Text,
        hint: "one line",
    },
    KeySpec {
        key: "parent",
        kind: FieldKind::EntityId,
        hint: "parent page id — makes this a Satellite",
    },
    KeySpec {
        key: "status",
        kind: FieldKind::Choice(STATUS_CHOICES),
        hint: "draft | published | archived",
    },
    KeySpec {
        key: "tags",
        kind: FieldKind::List,
        hint: "[a, b]",
    },
    KeySpec {
        key: "relations",
        kind: FieldKind::List,
        hint: "[kind=target, …]",
    },
    KeySpec {
        key: "published_at",
        kind: FieldKind::Timestamp,
        hint: "YYYY-MM-DDTHH:MM:SSZ (needs summary)",
    },
    KeySpec {
        key: "summary",
        kind: FieldKind::Text,
        hint: "one line",
    },
    KeySpec {
        key: "servings",
        kind: FieldKind::Count,
        hint: "Cooklang count, e.g. 4 people",
    },
    KeySpec {
        key: "serves",
        kind: FieldKind::Count,
        hint: "Cooklang alias of servings",
    },
    KeySpec {
        key: "yield",
        kind: FieldKind::Count,
        hint: "Cooklang alias of servings",
    },
];

pub fn spec(key: &str) -> Option<&'static KeySpec> {
    SCHEMA.iter().find(|s| s.key == key)
}

/// One `key: value` line inside the fences.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub key: String,
    pub value: String,
    /// Bytes of the value text (after `:` and spacing, before the line end).
    pub value_range: Range<usize>,
    /// Bytes of the whole line including its line ending.
    pub line_range: Range<usize>,
    pub spec: Option<&'static KeySpec>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Note {
    /// A key outside the closed set; Boris will judge it.
    UnknownKey { key: String, line: usize },
    /// A line the form cannot show as a field (no `:` or indented).
    Unparsed { line: usize, text: String },
    /// Opening fence without a closing one.
    Unclosed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Form {
    /// Whether the file opens with a `---` fence.
    pub present: bool,
    /// Where field insertion goes (start of the closing fence line).
    pub close_at: usize,
    /// First body byte.
    pub body_offset: usize,
    pub fields: Vec<Field>,
    pub notes: Vec<Note>,
}

impl Form {
    pub fn field(&self, key: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.key == key)
    }

    /// Schema rows in order, each with its current field if set, then any
    /// unknown keys as extra rows.
    pub fn rows(&self) -> Vec<(Option<&'static KeySpec>, Option<&Field>, String)> {
        let mut rows: Vec<_> = SCHEMA
            .iter()
            .map(|s| (Some(s), self.field(s.key), s.key.to_string()))
            .collect();
        for f in self.fields.iter().filter(|f| f.spec.is_none()) {
            rows.push((None, Some(f), f.key.clone()));
        }
        rows
    }
}

fn is_fence(line: &str) -> bool {
    line.strip_suffix('\r').unwrap_or(line) == "---"
}

/// Split `text` into lines with byte offsets, each line keeping its `\n`.
fn lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut at = 0;
    text.split_inclusive('\n').map(move |l| {
        let start = at;
        at += l.len();
        (start, l)
    })
}

pub fn read(text: &str) -> Form {
    let mut it = lines(text).enumerate();
    let mut form = Form {
        present: false,
        close_at: 0,
        body_offset: 0,
        fields: Vec::new(),
        notes: Vec::new(),
    };
    match it.next() {
        Some((_, (_, first)))
            if is_fence(first.trim_end_matches('\n')) && first.ends_with('\n') => {}
        Some((_, (_, first))) if is_fence(first) => {
            form.present = true;
            form.notes.push(Note::Unclosed);
            form.close_at = text.len();
            form.body_offset = text.len();
            return form;
        }
        _ => return form,
    }
    form.present = true;
    for (n, (start, raw)) in it {
        let line = raw.trim_end_matches('\n').trim_end_matches('\r');
        if is_fence(line) {
            form.close_at = start;
            form.body_offset = start + raw.len();
            return form;
        }
        if line.trim().is_empty() {
            continue;
        }
        let indented = line.starts_with([' ', '\t']);
        match line.split_once(':') {
            Some((key, rest)) if !indented && !key.is_empty() => {
                let lead = rest.len() - rest.trim_start_matches([' ', '\t']).len();
                let vstart = start + key.len() + 1 + lead;
                let value = rest.trim_matches([' ', '\t']).to_string();
                let s = spec(key);
                if s.is_none() {
                    form.notes.push(Note::UnknownKey {
                        key: key.to_string(),
                        line: n + 1,
                    });
                }
                form.fields.push(Field {
                    key: key.to_string(),
                    value_range: vstart..vstart + value.len(),
                    value,
                    line_range: start..start + raw.len(),
                    spec: s,
                });
            }
            _ => form.notes.push(Note::Unparsed {
                line: n + 1,
                text: line.to_string(),
            }),
        }
    }
    form.notes.push(Note::Unclosed);
    form.close_at = text.len();
    form.body_offset = text.len();
    form
}

/// The line ending the file already uses (CRLF files stay CRLF).
fn eol(text: &str) -> &'static str {
    if text.contains("\r\n") { "\r\n" } else { "\n" }
}

/// Set `key` to `value` as one buffer edit; an empty value removes the line.
/// Values are written verbatim — quoting and validity are Boris's call.
pub fn set_field(buf: &mut Buffer, key: &str, value: &str) -> Result<(), EditError> {
    let text = buf.text();
    let form = read(&text);
    let nl = eol(&text);
    buf.break_undo_group();
    let r = match form.field(key) {
        Some(f) if value.is_empty() => buf.replace(f.line_range.clone(), ""),
        Some(f) => {
            // Through end of line, so whitespace typed in the form lands once.
            let line = text[f.line_range.clone()].trim_end_matches(['\n', '\r']);
            let end = (f.line_range.start + line.len()).max(f.value_range.end);
            buf.replace(f.value_range.start..end, value)
        }
        None if value.is_empty() => Ok(()),
        None if form.present => buf.insert(form.close_at, &format!("{key}: {value}{nl}")),
        None => buf.insert(0, &format!("---{nl}{key}: {value}{nl}---{nl}")),
    };
    buf.break_undo_group();
    r
}

/// Body text (everything after the closing fence).
pub fn body(text: &str) -> &str {
    &text[read(text).body_offset..]
}

#[cfg(test)]
#[path = "frontmatter_tests.rs"]
mod tests;
