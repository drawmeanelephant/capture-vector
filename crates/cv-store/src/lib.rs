//! cv-store — JSONL evidence trail in `.cv/`. JSONL is the source of
//! truth; the rebuildable SQLite index lives in `cv-ledger`. Store lands
//! with the lanes that need it; the `Store` port lives in
//! `cv-core::store`.
//!
//! [`sidecar`] owns the per-namespace `.cv/<ns>/schema.json` stamps and
//! their forward migrations; every namespace writer calls
//! [`sidecar::ensure`] before opening its files.

pub mod audit;
pub mod lock;
pub mod paths;
pub mod sidecar;
