//! Inspector disclosure state: which sections and long values the user
//! opened, keyed so one record's expansion never leaks into another's.

use super::Workbench;

impl Workbench {
    /// Whether the inspector section `title` shows its items: open when
    /// it has any, unless the user flipped it.
    pub fn section_open(&self, title: &str, empty: bool) -> bool {
        empty == self.sections_flipped.contains(title)
    }

    pub fn toggle_section(&mut self, title: &str) {
        if !self.sections_flipped.remove(title) {
            self.sections_flipped.insert(title.to_string());
        }
    }

    /// Whether the inspector value `key` (`<noun>\u{1f}<element key>`)
    /// shows its full text inline instead of the truncated preview.
    pub fn value_expanded(&self, key: &str) -> bool {
        self.values_expanded.contains(key)
    }

    pub fn toggle_value(&mut self, key: &str) {
        if !self.values_expanded.remove(key) {
            self.values_expanded.insert(key.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Workbench;

    #[test]
    fn value_expansion_toggles_independently_per_key() {
        let mut wb = Workbench::new(crate::module::Registry::default());
        assert!(!wb.value_expanded("run-a\u{1f}field-argv"));
        wb.toggle_value("run-a\u{1f}field-argv");
        assert!(wb.value_expanded("run-a\u{1f}field-argv"));
        assert!(!wb.value_expanded("run-b\u{1f}field-argv"));
        wb.toggle_value("run-a\u{1f}field-argv");
        assert!(!wb.value_expanded("run-a\u{1f}field-argv"));
    }
}
