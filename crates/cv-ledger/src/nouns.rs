//! Evidence nouns. Ids are content-derived digests — never a filesystem
//! path (an artifact's path is an attribute, not its identity).

use cv_core::id::{AccountId, AttrValue, NounAttrRow, NounId, NounKind, StableId};
use cv_core::sys::Hash;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub mod kind {
    pub const ARTIFACT: &str = "artifact";
    pub const CHECK: &str = "check";
    pub const CLAIM: &str = "claim";
    pub const DELIVERY: &str = "delivery";
}

/// `prefix-<first 20 hex of sha256(parts joined by NUL)>`.
pub fn derive_id(prefix: &str, parts: &[&str]) -> StableId {
    let mut h = Sha256::new();
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            h.update([0u8]);
        }
        h.update(p.as_bytes());
    }
    let hex = format!("{:x}", h.finalize());
    StableId(format!("{prefix}-{}", &hex[..20]))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactNoun {
    pub id: StableId,
    pub run: StableId,
    pub target: String,
    pub path: String,
    pub kind: String,
    pub status: String,
    pub required: bool,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckNoun {
    pub id: StableId,
    pub run: StableId,
    pub target: String,
    pub check_id: String,
    pub status: String,
    pub ran: bool,
    pub coverage: Option<String>,
    pub checked: u64,
    pub findings: u64,
    /// sha256 of the artifact inventory this check ran against.
    pub inventory_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimNoun {
    pub id: StableId,
    pub run: StableId,
    pub target: String,
    pub claim_id: String,
    pub status: String,
    pub statement: String,
    /// The check this claim rests on (claims → checks edge).
    pub check_id: Option<String>,
    pub checks_report_sha256: Option<String>,
}

/// One dispatch of a sealed plan. Recorded **before** the transport is
/// touched so a crash leaves the delivery visible.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryNoun {
    pub id: StableId,
    pub run: StableId,
    pub target: String,
    pub adapter: String,
    pub plan_hash: Hash,
    pub files: u64,
    pub bytes: u64,
}

fn row(account: &AccountId, kind: &str, id: &StableId, f: Vec<(&str, AttrValue)>) -> NounAttrRow {
    NounAttrRow {
        noun: NounId {
            account: account.clone(),
            kind: NounKind(kind.into()),
            stable: id.clone(),
        },
        fields: f
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect::<BTreeMap<_, _>>(),
    }
}

fn t(s: &str) -> AttrValue {
    AttrValue::Text(s.to_string())
}

impl ArtifactNoun {
    pub fn attr_row(&self, account: &AccountId) -> NounAttrRow {
        row(
            account,
            kind::ARTIFACT,
            &self.id,
            vec![
                ("run", t(&self.run.0)),
                ("target", t(&self.target)),
                ("path", t(&self.path)),
                ("kind", t(&self.kind)),
                ("status", t(&self.status)),
                ("required", AttrValue::Bool(self.required)),
                ("bytes", AttrValue::Int(self.bytes as i64)),
            ],
        )
    }
}

impl CheckNoun {
    pub fn attr_row(&self, account: &AccountId) -> NounAttrRow {
        row(
            account,
            kind::CHECK,
            &self.id,
            vec![
                ("run", t(&self.run.0)),
                ("target", t(&self.target)),
                ("check_id", t(&self.check_id)),
                ("status", t(&self.status)),
                ("ran", AttrValue::Bool(self.ran)),
                ("findings", AttrValue::Int(self.findings as i64)),
            ],
        )
    }
}

impl ClaimNoun {
    pub fn attr_row(&self, account: &AccountId) -> NounAttrRow {
        let mut f = vec![
            ("run", t(&self.run.0)),
            ("target", t(&self.target)),
            ("claim_id", t(&self.claim_id)),
            ("status", t(&self.status)),
        ];
        if let Some(c) = &self.check_id {
            f.push(("check_id", t(c)));
        }
        row(account, kind::CLAIM, &self.id, f)
    }
}

impl DeliveryNoun {
    pub fn attr_row(&self, account: &AccountId) -> NounAttrRow {
        row(
            account,
            kind::DELIVERY,
            &self.id,
            vec![
                ("run", t(&self.run.0)),
                ("target", t(&self.target)),
                ("adapter", t(&self.adapter)),
                ("plan_hash", t(&self.plan_hash.to_hex())),
                ("files", AttrValue::Int(self.files as i64)),
            ],
        )
    }
}
