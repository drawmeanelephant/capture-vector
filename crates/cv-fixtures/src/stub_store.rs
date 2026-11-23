//! `StubStore` — in-memory `Store` for lanes. The JSONL-truth /
//! SQLite-index duality doesn't exist here; tests assert behavior, not
//! persistence.

use cv_core::id::StableId;
use cv_core::store::{Namespace, Predicate, Store, Value};
use std::collections::BTreeMap;
use std::sync::Mutex;

#[derive(Default)]
pub struct StubStore {
    inner: Mutex<BTreeMap<(Namespace, StableId), Value>>,
}

impl StubStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Store for StubStore {
    fn put(&self, ns: &Namespace, key: &StableId, doc: &Value) -> cv_core::Result<()> {
        self.inner
            .lock()
            .unwrap()
            .insert((ns.clone(), key.clone()), doc.clone());
        Ok(())
    }

    fn get(&self, ns: &Namespace, key: &StableId) -> cv_core::Result<Option<Value>> {
        Ok(self
            .inner
            .lock()
            .unwrap()
            .get(&(ns.clone(), key.clone()))
            .cloned())
    }

    fn query(&self, ns: &Namespace, pred: &Predicate) -> cv_core::Result<Vec<Value>> {
        let inner = self.inner.lock().unwrap();
        Ok(inner
            .iter()
            .filter(|((n, _), _)| n == ns)
            .filter(|(_, doc)| predicate_matches_json(pred, doc))
            .map(|(_, doc)| doc.clone())
            .collect())
    }
}

/// JSON-document predicate evaluation matching `Predicate::matches`
/// semantics on `NounAttrRow` projections.
fn predicate_matches_json(pred: &Predicate, doc: &Value) -> bool {
    use cv_core::id::AttrValue;
    use cv_core::store::Op;
    pred.conj.iter().all(|t| {
        let got = doc.get(&t.field);
        let want = &t.value;
        let num = |v: &Value| v.as_f64();
        match (got, &t.op) {
            (None, _) => false,
            (Some(g), Op::Eq) => json_eq(g, want),
            (Some(g), Op::Ne) => !json_eq(g, want),
            (Some(g), Op::Contains) => match (g.as_str(), want) {
                (Some(h), AttrValue::Text(n)) => h.contains(n.as_str()),
                _ => false,
            },
            (Some(g), Op::Lt) => matches!((num(g), attr_num(want)), (Some(a), Some(b)) if a < b),
            (Some(g), Op::Le) => matches!((num(g), attr_num(want)), (Some(a), Some(b)) if a <= b),
            (Some(g), Op::Gt) => matches!((num(g), attr_num(want)), (Some(a), Some(b)) if a > b),
            (Some(g), Op::Ge) => matches!((num(g), attr_num(want)), (Some(a), Some(b)) if a >= b),
        }
    })
}

fn attr_num(v: &cv_core::id::AttrValue) -> Option<f64> {
    use cv_core::id::AttrValue::*;
    match v {
        Int(i) => Some(*i as f64),
        Float(f) => Some(*f),
        _ => None,
    }
}

fn json_eq(g: &Value, want: &cv_core::id::AttrValue) -> bool {
    use cv_core::id::AttrValue::*;
    match want {
        Bool(b) => g.as_bool() == Some(*b),
        Int(i) => g.as_i64() == Some(*i),
        Float(f) => g.as_f64() == Some(*f),
        Text(t) => g.as_str() == Some(t.as_str()),
    }
}
