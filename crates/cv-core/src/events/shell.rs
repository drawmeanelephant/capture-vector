//! Shell lane event payload (L2) — chrome-level facts, no lane logic.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ShellEvent {
    /// Generic wake — the rune loop drains pending work.
    Wake,
    ModuleActivated {
        module: String,
    },
    /// A degraded runtime state was detected and disclosed.
    Degraded {
        reason: DegradedReason,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DegradedReason {
    EngineAbsent,
    EngineVersionMismatch {
        found: String,
        want: String,
    },
    /// The diagnostic build after a fatal native watch failure failed.
    EngineBuildFailing,
    WatchDead,
    StoreCorrupt,
}
