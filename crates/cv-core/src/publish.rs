//! Publish vocabulary — sealed, reconcilable. A `PreflightGrant` binds
//! the exact artifact set it checked; publish delivers exactly the sealed
//! plan and hash mismatch refuses.

use crate::sys::Hash;
use serde::{Deserialize, Serialize};

/// Sealed grant — binds `{ artifact_set_hash, frozen_graph_hash,
/// plan_hash, target_config_hash, ruleset_hash, boris_version }`.
/// Approval is persisted **before** dispatch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PreflightGrant {
    pub artifact_set_hash: Hash,
    pub graph_hash: Hash,
    pub plan_hash: Hash,
    pub target_cfg_hash: Hash,
    pub ruleset_hash: Hash,
    pub boris_version: String,
}

impl PreflightGrant {
    /// Publish must refuse when the build about to ship doesn't match
    /// the sealed hashes.
    pub fn seals(&self, artifact_set: &Hash, plan: &Hash) -> bool {
        self.artifact_set_hash == *artifact_set && self.plan_hash == *plan
    }
}

/// Run lifecycle. `Unknown` = a crash after network contact but before
/// the report landed — requires reconciliation, never auto-retry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunState {
    Dispatching,
    Submitted,
    Unknown,
    Complete(RunStatus),
}

/// Boris's own outcome words.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunStatus {
    Complete,
    Partial,
    Failed,
    Incomplete,
}

/// A publish target's declared deployment contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetSpec {
    pub id: String,
    /// Labeled exactly as Boris labels them.
    pub verified: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grant_refuses_hash_mismatch() {
        let g = PreflightGrant {
            artifact_set_hash: Hash::from_hex(&"aa".repeat(32)).unwrap(),
            graph_hash: Hash::from_hex(&"bb".repeat(32)).unwrap(),
            plan_hash: Hash::from_hex(&"cc".repeat(32)).unwrap(),
            target_cfg_hash: Hash::from_hex(&"dd".repeat(32)).unwrap(),
            ruleset_hash: Hash::from_hex(&"ee".repeat(32)).unwrap(),
            boris_version: "boris/0.9.0".into(),
        };
        assert!(g.seals(
            &Hash::from_hex(&"aa".repeat(32)).unwrap(),
            &Hash::from_hex(&"cc".repeat(32)).unwrap()
        ));
        assert!(!g.seals(
            &Hash::from_hex(&"ff".repeat(32)).unwrap(),
            &Hash::from_hex(&"cc".repeat(32)).unwrap()
        ));
    }
}
