//! `StubStore` — in-memory `Store` oracle for lane tests and conformance.
//! The JSONL-truth / SQLite-index duality doesn't exist here; the stub has
//! no persistence and production workspace journals use `cv_store::docs`.

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
            .unwrap_or_else(|e| e.into_inner())
            .insert((ns.clone(), key.clone()), doc.clone());
        Ok(())
    }

    fn get(&self, ns: &Namespace, key: &StableId) -> cv_core::Result<Option<Value>> {
        Ok(self
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(ns.clone(), key.clone()))
            .cloned())
    }

    fn query(&self, ns: &Namespace, pred: &Predicate) -> cv_core::Result<Vec<Value>> {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        Ok(inner
            .iter()
            .filter(|((n, _), _)| n == ns)
            .filter(|(_, doc)| predicate_matches_json(pred, doc))
            .map(|(_, doc)| doc.clone())
            .collect())
    }
}

/// JSON-document predicate evaluation — delegates to the shared
/// `Predicate::matches_json` so the stub oracle, `Predicate::matches`,
/// and any indexed implementation can never diverge.
fn predicate_matches_json(pred: &Predicate, doc: &Value) -> bool {
    pred.matches_json(doc)
}
