//! cv-engine — the ONLY `std::process::Command` owner (L1 lane).
//!
//! Wraps the pinned Boris binary behind `cv_core::engine::Engine`:
//! locate + version gate, per-OS capability probe, `EngineScheduler`,
//! watch NDJSON with a CV-owned cursor and bounded ring, fingerprinted
//! staged writes, exact exit mapping, process-tree cancellation, and a
//! stdin-only secrets channel. The pinned `boris-migration-lab` runs
//! through the same process layer ([`lab`]). No Boris or lab semantics
//! are reimplemented.

pub mod adopt;
mod bounded;
pub mod cursor;
pub mod debounce;
pub mod emit;
pub mod engine;
pub mod error;
pub mod exit;
pub mod fallback;
pub mod file_identity;
pub mod init;
pub mod ir;
pub mod lab;
pub mod locate;
pub mod paths;
pub mod probe;
pub mod proc;
pub mod ring;
pub mod scheduler;
pub mod secrets;
pub mod snapshot;
pub mod verbs;
pub mod watch;
pub mod write;

#[cfg(test)]
mod test_support;

pub use engine::{EngineConfig, ProcessEngine};
pub use error::EngineError;
pub use scheduler::{EngineScheduler, RecoveryBuild, SeatPolicy};
