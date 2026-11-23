//! Command palette — verbs live here, not in the rail. Lanes contribute
//! `CommandSpec`s; the shell adds its own `shell.*` verbs.

use cv_core::events::shell::DegradedReason;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommandSpec {
    /// `<module id>.<verb>`; `shell.*` is reserved for chrome.
    pub id: String,
    pub title: String,
    /// Display hint only (e.g. "mod+k"); binding lives in the shell keymap.
    pub chord: Option<String>,
}

impl CommandSpec {
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            chord: None,
        }
    }
    pub fn chord(mut self, c: &str) -> Self {
        self.chord = Some(c.into());
        self
    }
}

/// What a module asks the shell to do. Modules emit intents; the shell
/// performs them (UI never spawns, reads stores, or imports lanes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Intent {
    Degrade(DegradedReason),
    Recover,
    Status(String),
    OpenCompanion { key: String },
    MarkDirty,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Palette {
    pub open: bool,
    pub query: String,
    pub cursor: usize,
}

/// Subsequence fuzzy score over `title` and `id`; lower is better.
fn score(query: &str, c: &CommandSpec) -> Option<usize> {
    if query.is_empty() {
        return Some(0);
    }
    let q = query.to_lowercase();
    [c.title.to_lowercase(), c.id.to_lowercase()]
        .iter()
        .filter_map(|hay| {
            let mut pos = 0usize;
            let mut gaps = 0usize;
            let mut first = None;
            for qc in q.chars() {
                let found = hay[pos..].find(qc)?;
                first.get_or_insert(pos + found);
                gaps += found;
                pos += found + qc.len_utf8();
            }
            Some(first.unwrap_or(0) + gaps * 2)
        })
        .min()
}

impl Palette {
    pub fn toggle(&mut self) {
        self.open = !self.open;
        self.query.clear();
        self.cursor = 0;
    }
    pub fn close(&mut self) {
        *self = Self::default();
    }
    /// Ranked matches: by score, then id — deterministic.
    pub fn matches<'a>(&self, cmds: &'a [CommandSpec]) -> Vec<&'a CommandSpec> {
        let mut hits: Vec<_> = cmds
            .iter()
            .filter_map(|c| score(&self.query, c).map(|s| (s, c)))
            .collect();
        hits.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.id.cmp(&b.1.id)));
        hits.into_iter().map(|(_, c)| c).collect()
    }
    pub fn type_char(&mut self, ch: char) {
        self.query.push(ch);
        self.cursor = 0;
    }
    pub fn backspace(&mut self) {
        self.query.pop();
        self.cursor = 0;
    }
    pub fn move_cursor(&mut self, delta: isize, len: usize) {
        if len == 0 {
            self.cursor = 0;
            return;
        }
        self.cursor = (self.cursor as isize + delta).rem_euclid(len as isize) as usize;
    }
    pub fn selected<'a>(&self, cmds: &'a [CommandSpec]) -> Option<&'a CommandSpec> {
        self.matches(cmds).get(self.cursor).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmds() -> Vec<CommandSpec> {
        vec![
            CommandSpec::new("shell.theme.toggle", "Toggle light/dark theme"),
            CommandSpec::new("demo.replay", "Replay captured watch session"),
            CommandSpec::new("shell.motion.toggle", "Toggle reduced motion"),
        ]
    }

    #[test]
    fn fuzzy_ranks_and_filters() {
        let c = cmds();
        let mut p = Palette::default();
        p.toggle();
        assert_eq!(p.matches(&c).len(), 3);
        "replay".chars().for_each(|ch| p.type_char(ch));
        let m = p.matches(&c);
        assert_eq!(m[0].id, "demo.replay");
        p.query = "zzz".into();
        assert!(p.matches(&c).is_empty());
    }

    #[test]
    fn cursor_wraps() {
        let mut p = Palette::default();
        p.move_cursor(-1, 3);
        assert_eq!(p.cursor, 2);
        p.move_cursor(1, 3);
        assert_eq!(p.cursor, 0);
    }
}
