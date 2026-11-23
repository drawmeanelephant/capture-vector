//! The note model: `research.*` nouns with typed captures.

use cv_core::id::StableId;
use cv_core::sys::Timestamp;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Source markup of a note body. Only Oliver renders Textile/Cooklang.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    #[default]
    Markdown,
    Textile,
    Cooklang,
}

impl Format {
    /// The `oliver render --from` spelling.
    pub fn oliver_from(self) -> &'static str {
        match self {
            Format::Markdown => "markdown",
            Format::Textile => "textile",
            Format::Cooklang => "cooklang",
        }
    }
}

/// What was captured. Paths and URIs are payload, never identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Capture {
    Selection {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<String>,
    },
    Url {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
    File {
        path: String,
    },
    Image {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        alt: Option<String>,
    },
    Idea {
        text: String,
    },
}

impl Capture {
    pub fn kind(&self) -> &'static str {
        match self {
            Capture::Selection { .. } => "selection",
            Capture::Url { .. } => "url",
            Capture::File { .. } => "file",
            Capture::Image { .. } => "image",
            Capture::Idea { .. } => "idea",
        }
    }

    pub fn summary(&self) -> String {
        match self {
            Capture::Selection { text, .. } | Capture::Idea { text } => first_line(text),
            Capture::Url { url, title } => title.clone().unwrap_or_else(|| url.clone()),
            Capture::File { path } => path.clone(),
            Capture::Image { path, alt } => alt.clone().unwrap_or_else(|| path.clone()),
        }
    }

    /// The URI a capture came from, when it has one.
    pub fn source_uri(&self) -> Option<String> {
        match self {
            Capture::Selection { source, .. } => source.clone(),
            Capture::Url { url, .. } => Some(url.clone()),
            Capture::File { path } | Capture::Image { path, .. } => Some(path.clone()),
            Capture::Idea { .. } => None,
        }
    }
}

pub(crate) fn first_line(s: &str) -> String {
    s.lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim()
        .to_string()
}

/// Triage state. `Inbox` is where every capture lands.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum Triage {
    #[default]
    Inbox,
    Filed {
        notebook: String,
    },
    Archived,
}

impl Triage {
    pub fn label(&self) -> &'static str {
        match self {
            Triage::Inbox => "inbox",
            Triage::Filed { .. } => "filed",
            Triage::Archived => "archived",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Note {
    pub id: StableId,
    pub title: String,
    pub body_md: String,
    #[serde(default)]
    pub format: Format,
    #[serde(default)]
    pub captures: Vec<Capture>,
    /// Ids of outgoing link-graph edges (see [`crate::graph`]).
    #[serde(default)]
    pub links: Vec<StableId>,
    #[serde(default)]
    pub tags: BTreeSet<String>,
    pub created: Timestamp,
    pub modified: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_uri: Option<String>,
    #[serde(default)]
    pub triage: Triage,
}
