//! Research lane event payload (L6).

use crate::id::StableId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NotesEvent {
    Captured { note: StableId },
    Promoted { note: StableId, page: StableId },
    LinkAdded { from: StableId, to: StableId },
}
