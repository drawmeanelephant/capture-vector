//! cv-engine — the ONLY `std::process::Command` owner (L1 lane).
//!
//! Wraps the pinned Boris binary behind `cv_core::engine::Engine`:
//! locate + version gate, per-OS capability probe, `EngineScheduler`,
//! watch NDJSON with a CV-owned cursor and bounded ring, fingerprinted
//! staged writes, exact exit mapping, process-tree cancellation, and a
//! stdin-only secrets channel. No Boris semantics are reimplemented.

mod bounded;
pub mod cursor;
pub mod emit;
pub mod engine;
pub mod error;
pub mod exit;
pub mod fallback;
pub mod locate;
pub mod probe;
pub mod proc;
pub mod ring;
pub mod scheduler;
pub mod secrets;
pub mod snapshot;
pub mod verbs;
pub mod watch;
pub mod write;

pub use engine::{EngineConfig, ProcessEngine};
pub use error::EngineError;
pub use scheduler::{EngineScheduler, SeatPolicy};
