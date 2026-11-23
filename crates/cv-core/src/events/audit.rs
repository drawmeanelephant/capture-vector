//! Audit lane event payload (L4).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuditEvent {
    /// cv-rules re-evaluated the sealed artifact set after a build.
    FindingsRefreshed {
        ruleset_hash: crate::sys::Hash,
        opened: u32,
        fixed: u32,
        carried: u32,
    },
    /// Lifecycle transition on a finding, keyed by `DedupKey`.
    FindingLifecycle {
        key: crate::rules::DedupKey,
        to: crate::rules::FindingLifecycle,
    },
}
