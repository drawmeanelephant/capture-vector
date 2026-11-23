//! Identity vocabulary. `StableId` is **never a filesystem path** —
//! renames and reparents must not break selection, backlinks, or snoozes.

use serde::{Deserialize, Serialize};

/// A Boris publication opened as an account in the app.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AccountId(pub String);

/// A workspace = one open account instance (a window onto a project).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct WorkspaceId(pub String);

/// Content-stable identifier. Produced by `IdGen`, never derived from a
/// filesystem path.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct StableId(pub String);

/// Branded open string — lanes may mint kinds, well-knowns are constants.
/// The noun-view registry must be able to render every produced kind and
/// always falls back to a generic view.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct NounKind(pub String);

impl NounKind {
    pub const fn new(s: String) -> Self {
        Self(s)
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

pub mod noun_kind {
    pub const PAGE: &str = "page";
    pub const FINDING: &str = "finding";
    pub const PLAN: &str = "plan";
    pub const RECEIPT: &str = "receipt";
    pub const NOTE: &str = "note";
    pub const OUTPUT: &str = "output";
    pub const DIAGNOSTIC: &str = "diagnostic";
}

/// Every artifact in the app is a noun: page, diagnostic, finding, plan,
/// receipt, note, output file.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct NounId {
    pub account: AccountId,
    pub kind: NounKind,
    pub stable: StableId,
}

/// One `Selection` value type; every surface reads it. **Per-window**
/// (wave-0 pin): companion windows carry their own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Selection {
    pub account: Option<AccountId>,
    pub mailbox: Option<String>,
    pub noun: Option<NounId>,
}

/// Attribute value for `NounAttrRow` projections — closed set so smart
/// mailboxes stay grammar-free (field op value conjunctions only).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AttrValue {
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
}

/// Uniform attribute projection of a noun. Every lane projects its nouns
/// to these rows; smart-mailbox conjunctions evaluate them — never a
/// central `match` on kind.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NounAttrRow {
    pub noun: NounId,
    pub fields: std::collections::BTreeMap<String, AttrValue>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noun_id_is_path_free() {
        let id = NounId {
            account: AccountId("acct".into()),
            kind: NounKind(noun_kind::PAGE.into()),
            stable: StableId("01JAAA".into()),
        };
        let json = serde_json::to_string(&id).unwrap();
        assert!(
            !json.contains('/'),
            "stable ids must not embed paths: {json}"
        );
        let back: NounId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }

    #[test]
    fn selection_defaults_empty() {
        let sel = Selection::default();
        assert!(sel.noun.is_none() && sel.mailbox.is_none());
    }
}
