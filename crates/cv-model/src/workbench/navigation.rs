//! Navigation, row selection, and shell-wide selection identity.

use super::Workbench;
use crate::module::RowModel;
use cv_core::id::Selection;

impl Workbench {
    // ---- navigation & selection ----

    pub fn active_nav(&self) -> Option<&str> {
        self.nav.get(self.active).and_then(|n| n.as_deref())
    }

    pub fn list_len(&self) -> usize {
        self.registry
            .get(self.active)
            .map_or(0, |m| m.list_len(self.active_nav()))
    }

    pub fn row(&self, i: usize) -> RowModel {
        self.registry
            .get(self.active)
            .map(|m| m.row(self.active_nav(), i))
            .unwrap_or_default()
    }

    pub fn selected_key(&self) -> Option<&str> {
        self.windows.primary.row.as_ref().map(|(_, k)| k.as_str())
    }

    pub fn activate(&mut self, index: usize) {
        if index >= self.registry.len() {
            return;
        }
        if index != self.active {
            // `Selection` is shell-wide identity, not row position. When the
            // newly active module can represent the same noun, reveal it
            // there instead of dropping the round-trip at the module boundary.
            let noun = self.app.selection.noun.clone();
            self.clear_selection();
            self.active = index;
            if let Some(m) = self.registry.get_mut(index) {
                m.activated();
            }
            if let Some(noun) = noun {
                let key = self.registry.get(index).and_then(|m| m.key_for(&noun));
                if let Some(key) = key {
                    let in_current_nav = self
                        .registry
                        .get(index)
                        .and_then(|m| m.index_of_key(self.active_nav(), &key))
                        .is_some();
                    if !in_current_nav {
                        let nav = self
                            .registry
                            .get(index)
                            .map(|m| m.nav().items)
                            .unwrap_or_default()
                            .into_iter()
                            .find(|item| {
                                self.registry
                                    .get(index)
                                    .and_then(|m| m.index_of_key(Some(&item.key), &key))
                                    .is_some()
                            })
                            .map(|item| item.key);
                        if let Some(nav) = nav {
                            self.set_nav(&nav);
                        }
                    }
                    self.select_key(&key);
                }
            }
        } else if let Some(m) = self.registry.get_mut(index) {
            m.activated();
        }
    }

    pub fn set_nav(&mut self, key: &str) {
        if let Some(slot) = self.nav.get_mut(self.active) {
            *slot = Some(key.to_string());
        }
        self.clear_selection();
    }

    pub(super) fn cycle_nav(&mut self, delta: isize) {
        let Some(m) = self.registry.get(self.active) else {
            return;
        };
        let items = m.nav().items;
        if items.is_empty() {
            return;
        }
        let cur = items
            .iter()
            .position(|i| Some(i.key.as_str()) == self.active_nav())
            .unwrap_or(0) as isize;
        let next = (cur + delta).rem_euclid(items.len() as isize) as usize;
        let key = items[next].key.clone();
        self.set_nav(&key);
    }

    fn sync_module_selection(&mut self, key: Option<&str>) {
        if let Some(m) = self.registry.get_mut(self.active) {
            m.select(key);
        }
    }

    pub fn clear_selection(&mut self) {
        self.windows.primary.row = None;
        self.windows.primary.selection = Selection::default();
        self.app.selection = Selection::default();
        self.sync_module_selection(None);
    }

    pub fn select_row(&mut self, i: usize) {
        if i >= self.list_len() {
            return;
        }
        let key = self.row(i).key;
        let noun = self.registry.get(self.active).and_then(|m| m.noun(&key));
        let sel = Selection {
            account: noun.as_ref().map(|n| n.account.clone()),
            mailbox: self.active_nav().map(str::to_string),
            noun,
        };
        let module_key = key.clone();
        self.windows.primary.row = Some((i, key));
        self.windows.primary.selection = sel.clone();
        self.app.selection = sel;
        self.sync_module_selection(Some(&module_key));
    }

    /// After a module mutates its list, keep the selected key selected at
    /// its new index, or clear the selection when the key left the list.
    pub(crate) fn revalidate_selection(&mut self) {
        let Some((i, key)) = self.windows.primary.row.clone() else {
            return;
        };
        if i < self.list_len() && self.row(i).key == key {
            return;
        }
        match self.row_index(&key) {
            Some(j) => self.select_row(j),
            None => self.clear_selection(),
        }
    }

    pub fn select_key(&mut self, key: &str) {
        if let Some(i) = self.row_index(key) {
            self.select_row(i);
        }
    }

    /// The first row of the active mailbox whose key is `key`.
    fn row_index(&self, key: &str) -> Option<usize> {
        self.registry
            .get(self.active)?
            .index_of_key(self.active_nav(), key)
    }

    pub(super) fn move_row(&mut self, delta: isize) {
        let len = self.list_len();
        if len == 0 {
            return;
        }
        let next = match &self.windows.primary.row {
            None if delta >= 0 => 0,
            None => len - 1,
            Some((i, _)) => (*i as isize + delta).clamp(0, len as isize - 1) as usize,
        };
        self.select_row(next);
    }

    pub(super) fn reclamp_rows(&mut self) {
        let Some((_, key)) = self.windows.primary.row.clone() else {
            return;
        };
        let key = match self.row_index(&key) {
            Some(i) => {
                if let Some(row) = &mut self.windows.primary.row {
                    row.0 = i;
                }
                return;
            }
            None => self
                .windows
                .primary
                .selection
                .noun
                .clone()
                .and_then(|noun| {
                    self.registry
                        .get(self.active)
                        .and_then(|m| m.key_for(&noun))
                }),
        };
        match key.and_then(|k| self.row_index(&k).map(|i| (i, k))) {
            Some((i, _)) => self.select_row(i),
            None => self.clear_selection(),
        }
    }
}
