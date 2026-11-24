//! Note → page draft. Only the closed Boris frontmatter keys are written
//! (`id`, `title`, `parent`, `tags`); provenance goes to the `.cv/`
//! sidecar. Nothing here validates — `boris validate` judges the draft.

use crate::model::{Format, Note};
use cv_core::engine::{Fingerprint, PageDraft};
use cv_core::id::StableId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromoteSpec {
    /// The page's Boris id, written as `id:` so the page keeps its
    /// identity when the file is renamed or reparented.
    pub page: StableId,
    /// Workspace-relative target, under the profile's content input.
    pub path: String,
    pub parent: Option<StableId>,
    /// `None` = create; an existing file is then a stale base.
    pub base: Option<Fingerprint>,
}

impl PromoteSpec {
    /// Default plan: `<content>/notes/<slug>.md` with `id: <slug>`.
    pub fn for_note(note: &Note, content_dir: &str) -> Self {
        let slug = slug(&note.title);
        let slug = if slug.is_empty() {
            note.id.0.clone()
        } else {
            slug
        };
        Self {
            path: format!("{}/notes/{slug}.md", content_dir.trim_end_matches('/')),
            page: StableId(slug),
            parent: None,
            base: None,
        }
    }
    pub fn parent(mut self, parent: &str) -> Self {
        self.parent = Some(StableId(parent.into()));
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PromoteError {
    #[error("only markdown notes promote to pages (note is {0:?})")]
    Format(Format),
    #[error("{0} contains a line break")]
    Multiline(&'static str),
    #[error("{0} is empty")]
    Empty(&'static str),
    #[error("{0} cannot be represented in frontmatter (raw \" or a line separator)")]
    Unquotable(&'static str),
    #[error("target {0} is not a content path")]
    Path(String),
}

/// Emit `s` as a Boris `dquoted` scalar: `"…"`. Boris strips the
/// surrounding quotes with no escape sequences, so a raw `"` can never
/// appear inside — callers reject it via `scalar` first.
fn quoted(s: &str) -> String {
    debug_assert!(!s.contains('"'));
    format!("\"{}\"", s.trim())
}

/// `promote(note) -> PageDraft` with the default plan under `content/`.
pub fn promote(note: &Note) -> Result<PageDraft, PromoteError> {
    promote_with(note, &PromoteSpec::for_note(note, "content"))
}

pub fn promote_with(note: &Note, spec: &PromoteSpec) -> Result<PageDraft, PromoteError> {
    if note.format != Format::Markdown {
        return Err(PromoteError::Format(note.format));
    }
    // Scalar validation. Beyond \n/\r, YAML also breaks lines on
    // U+0085/U+2028/U+2029, and a raw `"` is illegal inside Boris's
    // `dquoted` (which supports no escapes). Quoting every emitted
    // scalar keeps `|`, `>`, `,`, `yes`/`123` coercion, and other
    // YAML-plain-scalar traps inert — no semantic validation here,
    // Boris stays the judge.
    let scalar = |what: &'static str, s: &str| {
        if s.trim().is_empty() {
            Err(PromoteError::Empty(what))
        } else if s.contains(['\n', '\r']) {
            Err(PromoteError::Multiline(what))
        } else if s.contains(['"', '\u{85}', '\u{2028}', '\u{2029}']) {
            Err(PromoteError::Unquotable(what))
        } else {
            Ok(())
        }
    };
    scalar("title", &note.title)?;
    scalar("page id", &spec.page.0)?;
    let p = &spec.path;
    if p.starts_with(".cv/") || p.starts_with('/') || p.split('/').any(|c| c == "..") {
        return Err(PromoteError::Path(p.clone()));
    }
    let mut fm = format!(
        "---\nid: {}\ntitle: {}\n",
        quoted(&spec.page.0),
        quoted(&note.title)
    );
    if let Some(parent) = &spec.parent {
        scalar("parent", &parent.0)?;
        fm.push_str(&format!("parent: {}\n", quoted(&parent.0)));
    }
    if !note.tags.is_empty() {
        for t in &note.tags {
            scalar("tag", t)?;
        }
        let tags: Vec<String> = note.tags.iter().map(|t| quoted(t)).collect();
        fm.push_str(&format!("tags: [{}]\n", tags.join(", ")));
    }
    fm.push_str("---\n\n");
    let body = note.body_md.trim_end();
    let mut out = fm;
    out.push_str(body);
    out.push('\n');
    Ok(PageDraft {
        path: spec.path.clone(),
        body: out,
        base: spec.base,
    })
}

/// Lowercase ASCII alphanumerics joined by single dashes.
pub fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_string()
}
