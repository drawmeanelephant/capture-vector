//! JSONL truth vocabulary. The notebook is a pure fold over these; the
//! SQLite index is rebuilt from them.

use crate::graph::Link;
use crate::model::Note;
use crate::notebook::SmartMailbox;
use cv_core::engine::Fingerprint;
use cv_core::id::StableId;
use cv_core::sys::Timestamp;
use serde::{Deserialize, Serialize};

/// One line of `.cv/research/notes.jsonl`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Record {
    Captured {
        note: Note,
    },
    Edited {
        note: StableId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body_md: Option<String>,
        at: Timestamp,
    },
    Tagged {
        note: StableId,
        tag: String,
        at: Timestamp,
    },
    Filed {
        note: StableId,
        notebook: String,
        at: Timestamp,
    },
    Archived {
        note: StableId,
        at: Timestamp,
    },
    Linked {
        link: Link,
        at: Timestamp,
    },
    Unlinked {
        link: StableId,
        at: Timestamp,
    },
    ViewSaved {
        view: SmartMailbox,
    },
}

/// One line of the `.cv/research/provenance.jsonl` sidecar map:
/// note stable id → page stable id. `path` is where the page was written
/// at promotion time — informational only; resolution goes by `page`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    pub note: StableId,
    pub page: StableId,
    pub path: String,
    pub fingerprint: Fingerprint,
    pub at: Timestamp,
}
