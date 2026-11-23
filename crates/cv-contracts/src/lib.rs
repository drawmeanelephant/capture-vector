//! cv-contracts — serde mirrors of Boris JSON contracts, version-keyed.
//!
//! Sources of truth: `docs/contracts/` upstream, never our imagination.
//! `serde(default)` tolerates additive fields; unknown variants decode to
//! a typed `Unknown` — drift never crashes a lane. Fixture-decode tests
//! are the entire suite; hand-authored Boris JSON is banned.

pub mod build_info;
pub mod proof;
pub mod watch;

/// Every mirror carries its `format` discriminator + `schema_version`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormatKey {
    pub format: &'static str,
    pub schema_version: u32,
}
