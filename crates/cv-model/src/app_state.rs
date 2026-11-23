//! Per-lane state slices. Additive by design — a lane's slice lives in
//! `modules/<lane>/`, the shell never matches on module identity.

use cv_core::events::shell::DegradedReason;
use cv_core::id::Selection;
use serde::{Deserialize, Serialize};

/// The cross-lane shell state every module sees.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppState {
    /// Per-window selection (wave-0 pin). Mirrors the primary window.
    pub selection: Selection,
    /// Current snapshot epoch — bumped on IR resync.
    pub snapshot_epoch: u64,
    /// Disclosed degraded runtime state, if any. Latest wins.
    pub degraded: Option<DegradedReason>,
    /// Engine identity from the watch `hello` handshake.
    pub engine: Option<String>,
    /// Envelopes committed (idempotent: replays don't count).
    pub committed: u64,
}
