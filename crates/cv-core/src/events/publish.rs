//! Publish lane event payload (L5).

use crate::publish::{PreflightGrant, RunState};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PublishEvent {
    PreflightIssued {
        grant: Box<PreflightGrant>,
    },
    RunStateChanged {
        run: crate::id::StableId,
        to: RunState,
    },
}
