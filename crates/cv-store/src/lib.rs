//! cv-store — JSONL evidence trail in `.cv/`. JSONL is the source of
//! truth. This crate has no SQLite dependency: the rebuildable indexes
//! live in `cv-ledger` (ledger) and `cv-notes` (research FTS5), and
//! [`paths::project_index_dir`] picks their OS app-data location. The
//! `Store` port lives in `cv-core::store`; no on-disk implementation of
//! it exists yet (the app wires `cv_fixtures::stub_store::StubStore`).
//!
//! [`sidecar`] owns the per-namespace `.cv/<ns>/schema.json` stamps and
//! their forward migrations; every namespace writer calls
//! [`sidecar::ensure`] before opening its files.

pub mod audit;
pub(crate) mod identity;
pub mod imports;
pub mod lock;
pub(crate) mod open;
pub mod paths;
pub mod sidecar;
pub mod workspace_instance;
pub mod workspace_mutation;
