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

/// Branded noun kind — the field is private so producers cannot mint ad
/// hoc strings: `register` only accepts `&'static str` constants, which
/// means every mintable kind is declared as a `noun_kind` const and
/// listed in `noun_kind::ALL`. The cv-model noun-view registry must
/// render every registered kind and always falls back to a generic view.
/// Kinds read back from stored data need not be registered — they
/// render through that fallback.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct NounKind(String);

impl NounKind {
    /// Mint a registered kind. `kind` must be a `noun_kind` constant —
    /// `noun_kind::ALL` is the closed minting set the noun-view
    /// registry's comprehensiveness test covers.
    ///
    /// # Panics
    /// On an unregistered kind: declare a `noun_kind::` constant and
    /// register a view for it instead of minting ad hoc.
    pub fn register(kind: &'static str) -> Self {
        assert!(
            noun_kind::ALL.contains(&kind),
            "unregistered noun kind {kind:?}: declare a noun_kind:: constant"
        );
        Self(kind.into())
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
    pub const EVENT: &str = "event";
    pub const ARTIFACT: &str = "artifact";
    pub const CHECK: &str = "check";
    pub const CLAIM: &str = "claim";
    pub const DELIVERY: &str = "delivery";
    /// A `boris-migration-lab` run under `.cv/imports/`.
    pub const IMPORT_RUN: &str = "import_run";
    /// A lab `human_review` / `unsupported` entry, triaged in Migrate.
    pub const REVIEW_ITEM: &str = "review_item";

    /// The closed set every producer mints from — `NounKind::register`
    /// rejects anything outside it, and the noun-view registry asserts
    /// a view for each member.
    pub const ALL: &[&str] = &[
        PAGE,
        FINDING,
        PLAN,
        RECEIPT,
        NOTE,
        OUTPUT,
        DIAGNOSTIC,
        EVENT,
        ARTIFACT,
        CHECK,
        CLAIM,
        DELIVERY,
        IMPORT_RUN,
        REVIEW_ITEM,
    ];
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
            kind: NounKind::register(noun_kind::PAGE),
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
