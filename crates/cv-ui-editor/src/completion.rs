//! Wikilink completion from Boris's `.boris/completion.json`
//! (`format = boris-completion-index`, `schema_version = 1`).
//!
//! The index is Boris's; this module only filters it by what the author
//! has typed after `[[`. An index with another format or version degrades
//! to "no completion" rather than being guessed at.

use crate::buffer::{Buffer, EditError};
use serde::Deserialize;
use std::ops::Range;

pub const FORMAT: &str = "boris-completion-index";
pub const SCHEMA_VERSION: u32 = 1;
const LIMIT: usize = 12;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Entity {
    pub id: String,
    pub title: Option<String>,
    pub role: String,
    pub status: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CompletionIndex {
    pub format: String,
    pub schema_version: u32,
    pub compiler_id: String,
    pub frozen: bool,
    pub entities: Vec<Entity>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionError {
    Malformed(String),
    Unsupported { format: String, schema_version: u32 },
}

impl CompletionIndex {
    pub fn parse(json: &str) -> Result<Self, CompletionError> {
        let idx: CompletionIndex =
            serde_json::from_str(json).map_err(|e| CompletionError::Malformed(e.to_string()))?;
        if idx.format != FORMAT || idx.schema_version != SCHEMA_VERSION {
            return Err(CompletionError::Unsupported {
                format: idx.format,
                schema_version: idx.schema_version,
            });
        }
        Ok(idx)
    }

    /// Entities matching `prefix`: id-prefix matches, then id/title
    /// substring matches (case-insensitive), each in index order.
    pub fn complete(&self, prefix: &str) -> Vec<&Entity> {
        let q = prefix.to_lowercase();
        let starts = self
            .entities
            .iter()
            .filter(|e| e.id.to_lowercase().starts_with(&q));
        let contains = self.entities.iter().filter(|e| {
            !e.id.to_lowercase().starts_with(&q)
                && (e.id.to_lowercase().contains(&q)
                    || e.title
                        .as_deref()
                        .is_some_and(|t| t.to_lowercase().contains(&q)))
        });
        starts.chain(contains).take(LIMIT).collect()
    }
}

/// An open `[[…` before the caret: the typed prefix and its byte range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WikilinkContext {
    pub prefix: String,
    pub range: Range<usize>,
}

pub fn wikilink_context(text: &str, caret: usize) -> Option<WikilinkContext> {
    let caret = caret.min(text.len());
    if !text.is_char_boundary(caret) {
        return None;
    }
    let line_start = text[..caret].rfind('\n').map_or(0, |i| i + 1);
    let open = text[line_start..caret].rfind("[[")? + line_start + 2;
    let prefix = &text[open..caret];
    if prefix.contains("]]") || prefix.contains(['|', '#']) {
        return None;
    }
    Some(WikilinkContext {
        prefix: prefix.to_string(),
        range: open..caret,
    })
}

/// Replace the typed prefix with `id` and close the link. Returns the caret.
pub fn accept(buf: &mut Buffer, ctx: &WikilinkContext, id: &str) -> Result<usize, EditError> {
    let text = buf.text();
    let closed = text[ctx.range.end..].starts_with("]]");
    let insert = if closed {
        id.to_string()
    } else {
        format!("{id}]]")
    };
    buf.break_undo_group();
    buf.replace(ctx.range.clone(), &insert)?;
    buf.break_undo_group();
    Ok(ctx.range.start + id.len() + 2)
}

#[cfg(test)]
#[path = "completion_tests.rs"]
mod tests;
