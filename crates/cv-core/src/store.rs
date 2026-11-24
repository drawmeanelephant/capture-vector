//! Store port. JSONL in `.cv/` is the source of truth; SQLite in OS
//! app-data (keyed by project id) is a rebuildable index — `store-corrupt`
//! recovery is delete + rebuild. Per-namespace migration tracks
//! `(namespace, version)` so lanes migrate independently.
//!
//! Boris IR is **never** persisted — `Arc<GraphDocument>` lives in memory;
//! smart-mailbox conjunctions scan `NounAttrRow`s.

use crate::id::{AttrValue, StableId};
use serde::{Deserialize, Serialize};

pub type Value = serde_json::Value;

/// A lane-owned storage namespace (`research.*`, `publish.*`, …).
/// Migrations track `(namespace, version)` — no global integer.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Namespace(pub String);

/// Comparison operators for smart-mailbox terms — the whole grammar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Op {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Contains,
}

/// `field op value` — a single predicate term over `NounAttrRow` fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Term {
    pub field: String,
    pub op: Op,
    pub value: AttrValue,
}

/// Bounded conjunction only — no grammar, no joins, no regex.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Predicate {
    pub conj: Vec<Term>,
}

impl Predicate {
    /// Evaluate against one attribute row (in-memory scan semantics —
    /// the same conjunction the SQLite index implements).
    pub fn matches(&self, row: &crate::id::NounAttrRow) -> bool {
        self.conj.iter().all(|t| match row.fields.get(&t.field) {
            Some(v) => attr_cmp(v, &t.op, &t.value),
            None => false,
        })
    }

    /// Evaluate against a JSON document field-view — the exact same
    /// `attr_cmp` semantics as `matches`. Every store that projects
    /// JSON documents (the stub oracle, the future SQLite index) must
    /// agree with `matches`, so they delegate here instead of keeping
    /// their own comparator.
    pub fn matches_json(&self, doc: &serde_json::Value) -> bool {
        self.conj
            .iter()
            .all(|t| match doc.get(&t.field).and_then(json_attr) {
                Some(v) => attr_cmp(&v, &t.op, &t.value),
                None => false,
            })
    }
}

/// A JSON field as an `AttrValue`; non-scalar or non-matching values
/// are `None` (the term can't match, never coerced).
fn json_attr(v: &serde_json::Value) -> Option<AttrValue> {
    use serde_json::Value::*;
    match v {
        Bool(b) => Some(AttrValue::Bool(*b)),
        String(s) => Some(AttrValue::Text(s.clone())),
        Number(n) => n
            .as_i64()
            .or_else(|| n.as_u64().and_then(|u| i64::try_from(u).ok()))
            .map(AttrValue::Int)
            .or_else(|| n.as_f64().map(AttrValue::Float)),
        _ => None,
    }
}

fn attr_cmp(v: &AttrValue, op: &Op, want: &AttrValue) -> bool {
    use AttrValue::*;
    match (op, v, want) {
        (Op::Eq, a, b) => a == b,
        (Op::Ne, a, b) => a != b,
        (Op::Contains, Text(h), Text(n)) => h.contains(n.as_str()),
        (Op::Lt, Int(a), Int(b)) => a < b,
        (Op::Le, Int(a), Int(b)) => a <= b,
        (Op::Gt, Int(a), Int(b)) => a > b,
        (Op::Ge, Int(a), Int(b)) => a >= b,
        (Op::Lt, Float(a), Float(b)) => a < b,
        (Op::Le, Float(a), Float(b)) => a <= b,
        (Op::Gt, Float(a), Float(b)) => a > b,
        (Op::Ge, Float(a), Float(b)) => a >= b,
        _ => false,
    }
}

pub trait Store: Send {
    fn put(&self, ns: &Namespace, key: &StableId, doc: &Value) -> crate::Result<()>;
    fn get(&self, ns: &Namespace, key: &StableId) -> crate::Result<Option<Value>>;
    fn query(&self, ns: &Namespace, pred: &Predicate) -> crate::Result<Vec<Value>>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::{AccountId, AttrValue, NounId, NounKind};
    use std::collections::BTreeMap;

    fn row(fields: &[(&str, AttrValue)]) -> crate::id::NounAttrRow {
        crate::id::NounAttrRow {
            noun: NounId {
                account: AccountId("a".into()),
                kind: NounKind("page".into()),
                stable: StableId("s".into()),
            },
            fields: fields
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect::<BTreeMap<_, _>>(),
        }
    }

    #[test]
    fn conjunction_semantics() {
        let r = row(&[
            ("status", AttrValue::Text("open".into())),
            ("count", AttrValue::Int(3)),
        ]);
        let p = Predicate {
            conj: vec![
                Term {
                    field: "status".into(),
                    op: Op::Eq,
                    value: AttrValue::Text("open".into()),
                },
                Term {
                    field: "count".into(),
                    op: Op::Gt,
                    value: AttrValue::Int(1),
                },
            ],
        };
        assert!(p.matches(&r));
        let p2 = Predicate {
            conj: vec![Term {
                field: "count".into(),
                op: Op::Lt,
                value: AttrValue::Int(1),
            }],
        };
        assert!(!p2.matches(&r));
        // missing field never matches
        let p3 = Predicate {
            conj: vec![Term {
                field: "absent".into(),
                op: Op::Eq,
                value: AttrValue::Bool(true),
            }],
        };
        assert!(!p3.matches(&r));
    }
}

#[cfg(test)]
mod json_tests {
    use super::*;

    #[test]
    fn json_projection_uses_attr_cmp_semantics_not_f64() {
        let p = Predicate {
            conj: vec![Term {
                field: "x".into(),
                op: Op::Eq,
                value: AttrValue::Float(3.0),
            }],
        };
        // Strict typing: JSON 3 is an Int — never f64-coerced equal.
        assert!(!p.matches_json(&serde_json::json!({"x": 3})));
        assert!(p.matches_json(&serde_json::json!({"x": 3.0})));
        // And range terms only compare same-typed numbers.
        let gt = Predicate {
            conj: vec![Term {
                field: "x".into(),
                op: Op::Gt,
                value: AttrValue::Int(2),
            }],
        };
        assert!(gt.matches_json(&serde_json::json!({"x": 3})));
        assert!(!gt.matches_json(&serde_json::json!({"x": 2.5})));
    }
}
