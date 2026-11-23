//! Rendered reading view. Textile/Cooklang (and Markdown) render only
//! through Oliver via the engine's `OliverRender` command — this crate
//! never interprets markup. Rendering is on demand (open/read), cached by
//! note id + modified stamp, never in the typing loop.

use crate::model::{Format, Note};
use cv_core::id::StableId;
use cv_core::sys::Timestamp;
use std::collections::BTreeMap;

/// `oliver render --from <format>`: stdin source → stdout HTML.
/// Implemented by the engine adapter (the only place processes spawn)
/// and by fixture replay in tests.
pub trait OliverRender {
    fn render(&self, format: Format, source: &str) -> Result<String, String>;
}

/// Bounded cache of rendered bodies keyed by `(note, modified)`.
#[derive(Debug, Default)]
pub struct RenderCache {
    entries: BTreeMap<StableId, (Timestamp, Result<String, String>)>,
}

impl RenderCache {
    pub const MAX_ENTRIES: usize = 256;

    pub fn get(&self, note: &Note) -> Option<&Result<String, String>> {
        self.entries
            .get(&note.id)
            .filter(|(at, _)| *at == note.modified)
            .map(|(_, r)| r)
    }

    pub fn render(&mut self, note: &Note, oliver: &dyn OliverRender) -> &Result<String, String> {
        if self.get(note).is_none() {
            if self.entries.len() >= Self::MAX_ENTRIES {
                let oldest = self
                    .entries
                    .iter()
                    .min_by_key(|(_, (at, _))| *at)
                    .map(|(k, _)| k.clone());
                if let Some(k) = oldest {
                    self.entries.remove(&k);
                }
            }
            let out = oliver.render(note.format, &note.body_md);
            self.entries.insert(note.id.clone(), (note.modified, out));
        }
        &self.entries[&note.id].1
    }
}
