//! cv-ledger — evidence chain: artifacts → checks → claims → deliveries,
//! first-class nouns. JSONL in `.cv/` is truth; SQLite is the index (L5).
//!
//! - [`Journal`] is the append-only, fsynced JSONL truth at
//!   `<project>/.cv/ledger/ledger.jsonl`. A record is committed once its
//!   trailing newline is durable; a torn tail (crash mid-append) is
//!   dropped on open and reported, never guessed at.
//! - [`LedgerIndex`] is a SQLite projection that can always be deleted
//!   and rebuilt from the journal (`store-corrupt` recovery).
//! - [`runs`] folds the journal into per-run views; the publish loop's
//!   crash semantics are read from here, never from in-memory optimism.

mod error;
pub mod index;
pub mod ingest;
pub mod journal;
pub mod nouns;
pub mod runs;

pub use error::{LedgerError, Result};
pub use index::LedgerIndex;
pub use journal::{Entry, Journal, Record};
pub use runs::{RunPhase, RunView, runs};
