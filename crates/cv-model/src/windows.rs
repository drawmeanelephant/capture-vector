//! Multi-window scope (L2 decision):
//! - one primary window (rail + panes) plus companion windows, each a
//!   pinned single-noun host for editor popouts (L3 mounts its editor in it);
//! - `Selection` is PER WINDOW: moving the primary selection never
//!   retargets a companion; a companion's selection is its pinned noun;
//! - dirty-close: closing a dirty window asks first; closing the primary
//!   with dirty companions asks once for all of them; `force` discards.

use cv_core::id::Selection;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct WindowId(pub u32);

pub const PRIMARY: WindowId = WindowId(0);

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct WindowState {
    pub selection: Selection,
    /// The list row key backing `selection` and its index in the current list.
    pub row: Option<(usize, String)>,
    pub dirty: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Companion {
    pub id: WindowId,
    /// Module id + row key the companion is pinned to.
    pub module: String,
    pub key: String,
    pub title: String,
    pub state: WindowState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum CloseOutcome {
    Closed(Vec<WindowId>),
    /// Unsaved changes: re-issue with `force` after the user confirms.
    NeedsConfirm(Vec<WindowId>),
    NoSuchWindow,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Windows {
    pub primary: WindowState,
    pub companions: Vec<Companion>,
    pub primary_open: bool,
    next: u32,
}

impl Windows {
    pub fn new() -> Self {
        Self {
            primary_open: true,
            next: 1,
            ..Self::default()
        }
    }

    /// Opens (or focuses) the companion for `module/key`. One per noun.
    pub fn open_companion(&mut self, module: &str, key: &str, title: &str) -> WindowId {
        if let Some(c) = self
            .companions
            .iter()
            .find(|c| c.module == module && c.key == key)
        {
            return c.id;
        }
        let id = WindowId(self.next.max(1));
        self.next = id.0 + 1;
        let mut state = WindowState {
            selection: self.primary.selection.clone(),
            ..WindowState::default()
        };
        state.row = self.primary.row.clone();
        self.companions.push(Companion {
            id,
            module: module.into(),
            key: key.into(),
            title: title.into(),
            state,
        });
        id
    }

    pub fn state_mut(&mut self, id: WindowId) -> Option<&mut WindowState> {
        if id == PRIMARY {
            return Some(&mut self.primary);
        }
        self.companions
            .iter_mut()
            .find(|c| c.id == id)
            .map(|c| &mut c.state)
    }

    pub fn set_dirty(&mut self, id: WindowId, dirty: bool) {
        if let Some(s) = self.state_mut(id) {
            s.dirty = dirty;
        }
    }

    pub fn close(&mut self, id: WindowId, force: bool) -> CloseOutcome {
        let targets: Vec<WindowId> = if id == PRIMARY {
            std::iter::once(PRIMARY)
                .chain(self.companions.iter().map(|c| c.id))
                .collect()
        } else if self.companions.iter().any(|c| c.id == id) {
            vec![id]
        } else {
            return CloseOutcome::NoSuchWindow;
        };
        let dirty: Vec<WindowId> = targets
            .iter()
            .copied()
            .filter(|w| match *w {
                PRIMARY => self.primary.dirty,
                w => self.companions.iter().any(|c| c.id == w && c.state.dirty),
            })
            .collect();
        if !dirty.is_empty() && !force {
            return CloseOutcome::NeedsConfirm(dirty);
        }
        self.companions.retain(|c| !targets.contains(&c.id));
        if id == PRIMARY {
            self.primary_open = false;
        }
        CloseOutcome::Closed(targets)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cv_core::id::{AccountId, NounId, NounKind, StableId, noun_kind};

    fn noun(s: &str) -> NounId {
        NounId {
            account: AccountId("a".into()),
            kind: NounKind::register(noun_kind::PAGE),
            stable: StableId(s.into()),
        }
    }

    #[test]
    fn selection_is_per_window() {
        let mut w = Windows::new();
        w.primary.selection.noun = Some(noun("p1"));
        let c = w.open_companion("demo", "p1", "p1");
        w.primary.selection.noun = Some(noun("p2"));
        let comp = w.state_mut(c).unwrap();
        assert_eq!(comp.selection.noun, Some(noun("p1")));
        assert_eq!(w.open_companion("demo", "p1", "p1"), c, "one per noun");
    }

    #[test]
    fn dirty_close_requires_confirm() {
        let mut w = Windows::new();
        let a = w.open_companion("demo", "a", "a");
        let b = w.open_companion("demo", "b", "b");
        w.set_dirty(b, true);
        assert_eq!(w.close(a, false), CloseOutcome::Closed(vec![a]));
        assert_eq!(w.close(PRIMARY, false), CloseOutcome::NeedsConfirm(vec![b]));
        assert!(w.primary_open);
        assert_eq!(
            w.close(PRIMARY, true),
            CloseOutcome::Closed(vec![PRIMARY, b])
        );
        assert!(!w.primary_open && w.companions.is_empty());
        assert_eq!(w.close(a, false), CloseOutcome::NoSuchWindow);
    }
}
