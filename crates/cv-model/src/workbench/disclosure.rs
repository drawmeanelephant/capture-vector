//! Inspector disclosure state: which sections and long values the user
//! opened, scoped so one record's expansion never leaks into another's.

use super::Workbench;
use std::path::PathBuf;

/// Structured identity of one inspector disclosure. The UI passes these
/// opaque keys back to the workbench; keeping every scope component as its
/// own field avoids separator-collision bugs that a concatenated string
/// would invite.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ValueExpansionKey {
    workspace: WorkspaceScope,
    module: String,
    mailbox: Option<String>,
    row: String,
    noun: Option<NounParts>,
    element: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord)]
struct WorkspaceScope {
    instance_token: Option<String>,
    instance_error: Option<String>,
    file: Option<(u64, u64)>,
    path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct NounParts {
    account: String,
    kind: String,
    stable: String,
}

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

    /// Scope `element` (the keyed inspector element) to the workspace,
    /// module, mailbox and record being inspected. `row` remains part of
    /// the key as a fallback for modules without stable nouns; when the
    /// row does have a noun, its account/kind/stable id is included too,
    /// so colliding row keys cannot share expansion state.
    pub fn expansion_key(&self, row: &str, element: &str) -> ValueExpansionKey {
        let workspace = match (&self.workspace_key, &self.workspace) {
            (Some(key), _) => WorkspaceScope {
                instance_token: key.instance_token().map(str::to_string),
                instance_error: key.instance_error().map(str::to_string),
                file: key.file_identity(),
                path: Some(key.path().to_path_buf()),
            },
            (None, Some(path)) => WorkspaceScope {
                path: Some(path.clone()),
                ..WorkspaceScope::default()
            },
            (None, None) => WorkspaceScope::default(),
        };
        let noun = (self.selected_key() == Some(row))
            .then(|| self.windows.primary.selection.noun.clone())
            .flatten()
            .map(|noun| NounParts {
                account: noun.account.0,
                kind: noun.kind.as_str().to_string(),
                stable: noun.stable.0,
            });
        ValueExpansionKey {
            workspace,
            module: self
                .registry
                .get(self.active)
                .map(|m| m.spec().id.to_string())
                .unwrap_or_default(),
            mailbox: self.active_nav().map(str::to_string),
            row: row.to_string(),
            noun,
            element: element.to_string(),
        }
    }

    /// Whether the inspector value/disclosure `key` shows its full content
    /// inline instead of the truncated preview.
    pub fn value_expanded(&self, key: &ValueExpansionKey) -> bool {
        self.values_expanded.contains(key)
    }

    pub fn toggle_value(&mut self, key: ValueExpansionKey) {
        if !self.values_expanded.remove(&key) {
            self.values_expanded.insert(key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Workbench;
    use crate::workspace_key::WorkspaceKey;
    use cv_core::id::{AccountId, NounId, NounKind, StableId, noun_kind};
    use std::path::PathBuf;

    #[test]
    fn value_expansion_toggles_independently_per_key() {
        let mut wb = Workbench::new(crate::module::Registry::default());
        let a = wb.expansion_key("run-a", "field-argv");
        let b = wb.expansion_key("run-b", "field-argv");
        assert!(!wb.value_expanded(&a));
        wb.toggle_value(a.clone());
        assert!(wb.value_expanded(&a));
        assert!(!wb.value_expanded(&b));
        wb.toggle_value(a.clone());
        assert!(!wb.value_expanded(&a));
    }

    #[test]
    fn expansion_keys_include_workspace_record_and_stable_noun_identity() {
        let mut wb = Workbench::new(crate::module::Registry::default());
        wb.workspace_key = Some(WorkspaceKey::with_instance(
            PathBuf::from("/tmp/workspace-a"),
            Some((7, 42)),
            "instance-a".into(),
        ));
        wb.windows.primary.row = Some((0, "same-row".into()));
        wb.windows.primary.selection.noun = Some(NounId {
            account: AccountId("local".into()),
            kind: NounKind::register(noun_kind::IMPORT_RUN),
            stable: StableId("run-a".into()),
        });
        let base = wb.expansion_key("same-row", "field-argv");

        wb.workspace_key = Some(WorkspaceKey::with_instance(
            PathBuf::from("/tmp/workspace-a"),
            Some((7, 42)),
            "instance-b".into(),
        ));
        assert_ne!(base, wb.expansion_key("same-row", "field-argv"));

        wb.workspace_key = Some(WorkspaceKey::with_instance(
            PathBuf::from("/tmp/workspace-a"),
            Some((7, 42)),
            "instance-a".into(),
        ));
        assert_eq!(base, wb.expansion_key("same-row", "field-argv"));
        assert_ne!(base, wb.expansion_key("other-row", "field-argv"));

        if let Some(noun) = &mut wb.windows.primary.selection.noun {
            noun.stable = StableId("run-b".into());
        }
        assert_ne!(base, wb.expansion_key("same-row", "field-argv"));
    }

    #[test]
    fn project_reset_clears_section_and_value_expansion_state() {
        let mut wb = Workbench::new(crate::module::Registry::default());
        let key = wb.expansion_key("run-a", "field-argv");
        wb.toggle_value(key);
        wb.toggle_section("Diagnostics");
        assert!(!wb.values_expanded.is_empty() && !wb.sections_flipped.is_empty());

        wb.reset_project(crate::module::Registry::default());

        assert!(wb.values_expanded.is_empty());
        assert!(wb.sections_flipped.is_empty());
    }
}
