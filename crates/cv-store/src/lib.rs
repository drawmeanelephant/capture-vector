//! cv-store — JSONL evidence trail in `.cv/`. JSONL is the source of
//! truth; rebuildable SQLite projections live in OS app-data (keyed by
//! the canonical project root, never under `.cv/` — see
//! [`paths::project_index_dir`]). [`docs`] is the on-disk
//! `cv_core::store::Store`; `cv-ledger` (ledger) and `cv-notes`
//! (research FTS5) own their own indexes.
//!
//! [`sidecar`] owns the per-namespace `.cv/<ns>/schema.json` stamps and
//! their forward migrations; every namespace writer calls
//! [`sidecar::ensure`] before opening its files.

pub mod audit;
pub mod docs;
pub(crate) mod identity;
pub mod imports;
pub mod lock;
pub(crate) mod open;
pub mod paths;
pub mod sidecar;
pub mod workspace_instance;
pub mod workspace_mutation;
