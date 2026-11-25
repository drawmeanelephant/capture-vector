//! Pages lane event payload: page lifecycle verbs the app performed on
//! source content. Boris judges the result; its diagnostics arrive through
//! the ordinary watch/rebuild path.

use crate::id::StableId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PagesEvent {
    Created {
        page: StableId,
        path: String,
    },
    /// The file moved; `page` is the same Boris id before and after.
    Renamed {
        page: StableId,
        from: String,
        to: String,
    },
    /// `parent: None` made the page a Trunk.
    Reparented {
        page: StableId,
        parent: Option<StableId>,
    },
    Deleted {
        page: StableId,
        path: String,
    },
    /// A verb that wrote nothing, or stopped before its end state
    /// (`message` says which). Failure is activity too: never silent.
    Failed {
        verb: String,
        path: String,
        message: String,
    },
}

impl PagesEvent {
    /// One line for Activity.
    pub fn summary(&self) -> String {
        match self {
            Self::Created { page, path } => format!("Page created: {} ({path})", page.0),
            Self::Renamed { page, from, to } => {
                format!("Page moved: {} ({from} → {to})", page.0)
            }
            Self::Reparented {
                page,
                parent: Some(p),
            } => {
                format!("Page reparented: {} → {}", page.0, p.0)
            }
            Self::Reparented { page, parent: None } => {
                format!("Page reparented: {} → Trunk", page.0)
            }
            Self::Deleted { page, path } => format!("Page deleted: {} ({path})", page.0),
            Self::Failed {
                verb,
                path,
                message,
            } => format!("Page {verb} failed: {path}: {message}"),
        }
    }
}
