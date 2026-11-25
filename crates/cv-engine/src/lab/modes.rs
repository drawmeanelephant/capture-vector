//! Per-mode flag allowlist for `boris-migration-lab`, transcribed from its
//! `--help` at the pinned commit (`ce40d78`, `boris-migration-lab/0.8.1`).
//! This table is the only copy of the lab's mode list in Capture Vector.
//! Replace it with `--list-modes --json` once
//! drawmeanelephant/boris-migration-lab#81 lands.
//!
//! Only path-valued flags are listed, and only canonical mode names (no
//! aliases). Left out on purpose: the developer-only `astro-import-plan` /
//! `astro-import-apply` (non-path ids, `--destination` instead of `--out`)
//! and options that are not input paths or that make the lab run another
//! binary (`--gate`, `--relation-kinds`, `--relation-map`, `--locale`,
//! `--max-pages`, `--boris`).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    Dir,
    File,
}

impl InputKind {
    pub fn noun(self) -> &'static str {
        match self {
            Self::Dir => "directory",
            Self::File => "file",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputSpec {
    /// Spelled as the lab spells it, e.g. `--root`; passed as `--root=<path>`.
    pub flag: &'static str,
    pub kind: InputKind,
    pub required: bool,
}

/// Where a mode's source comes from, for grouping runs in the UI. Until
/// the lab reports this itself (`--list-modes`), it is transcribed here
/// from the mode table in the lab's README at the pinned commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    StaticSite,
    BlogCms,
    Notes,
    Social,
    CrossCutting,
}

impl Family {
    pub const ALL: [Family; 5] = [
        Family::StaticSite,
        Family::BlogCms,
        Family::Notes,
        Family::Social,
        Family::CrossCutting,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Self::StaticSite => "static-site",
            Self::BlogCms => "blog-cms",
            Self::Notes => "notes",
            Self::Social => "social",
            Self::CrossCutting => "cross-cutting",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::StaticSite => "Static-site generators",
            Self::BlogCms => "Blog/CMS exports",
            Self::Notes => "Notes/outliners",
            Self::Social => "Social/data downloads",
            Self::CrossCutting => "Cross-cutting",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct ModeSpec {
    /// The `--mode=` value.
    pub name: &'static str,
    pub family: Family,
    pub inputs: &'static [InputSpec],
}

const fn dir(flag: &'static str) -> InputSpec {
    InputSpec {
        flag,
        kind: InputKind::Dir,
        required: true,
    }
}

const fn file(flag: &'static str) -> InputSpec {
    InputSpec {
        flag,
        kind: InputKind::File,
        required: true,
    }
}

const fn optional_dir(flag: &'static str) -> InputSpec {
    InputSpec {
        flag,
        kind: InputKind::Dir,
        required: false,
    }
}

const ROOT: &[InputSpec] = &[dir("--root")];

const fn root(name: &'static str, family: Family) -> ModeSpec {
    ModeSpec {
        name,
        family,
        inputs: ROOT,
    }
}

const fn with(name: &'static str, family: Family, inputs: &'static [InputSpec]) -> ModeSpec {
    ModeSpec {
        name,
        family,
        inputs,
    }
}

use Family::{BlogCms, CrossCutting, Notes, Social, StaticSite};

pub const MODES: &[ModeSpec] = &[
    root("astro", StaticSite),
    with(
        "wordpress",
        BlogCms,
        &[file("--wxr"), optional_dir("--media")],
    ),
    root("wordpress-theme", CrossCutting),
    with("instagram", Social, &[dir("--dump")]),
    with("obsidian", Notes, &[dir("--vault")]),
    with("tinderbox-inventory", Notes, &[file("--tbx")]),
    with("tinderbox", Notes, &[file("--tbx")]),
    with("notion", Notes, &[dir("--export")]),
    with("filed", StaticSite, &[dir("--filed-root")]),
    root("filed-scan", StaticSite),
    root("starlight", StaticSite),
    root("asset-filename", CrossCutting),
    root("theme-archaeology", CrossCutting),
    with(
        "theme-materialize",
        CrossCutting,
        &[dir("--root"), file("--ledger")],
    ),
    root("link-audit", CrossCutting),
    with("frontmatter-review", CrossCutting, &[dir("--content")]),
    root("hugo-inventory", StaticSite),
    root("hugo", StaticSite),
    root("jekyll-inventory", StaticSite),
    root("jekyll", StaticSite),
    root("ghost-inventory", BlogCms),
    root("ghost", BlogCms),
    root("mkdocs-inventory", StaticSite),
    root("mkdocs", StaticSite),
    root("mastodon-inventory", Social),
    root("mastodon", Social),
    root("atproto", Social),
    root("atproto-inventory", Social),
    root("enex-inventory", Notes),
    root("enex", Notes),
    root("substack-inventory", BlogCms),
    root("substack", BlogCms),
];
/// The allowlisted mode named `name` (canonical names only).
pub fn mode(name: &str) -> Option<&'static ModeSpec> {
    MODES.iter().find(|m| m.name == name)
}

// ---- presentation and intake metadata ----
//
// These strings describe the pinned lab's published mode contract; they do
// not change how cv-engine prepares or dispatches a run. When the upstream
// lab grows `--list-modes`, this table can be replaced by that machine
// contract at the engine boundary.

const BASE_SAFETY: &[&str] = &[
    "Inputs are read-only: the lab never rewrites the original export or content tree.",
    "The lab uses no network access, no scraping, and no zip extraction.",
];

impl ModeSpec {
    /// One-line input/output summary shown by the import wizard.
    pub fn description(&self) -> &'static str {
        match self.name {
            "astro" => {
                "Examines an Astro project or export tree and writes deterministic archaeology reports."
            }
            "astro-import-plan" => {
                "Builds a read-only proposed-action plan for an Astro plain-Markdown content root."
            }
            "astro-import-apply" => {
                "Applies a reviewed Astro migration plan to a new Boris source tree."
            }
            "wordpress" => {
                "Converts a WordPress WXR/XML export, with optional local media, into Boris Markdown and review reports."
            }
            "instagram" => {
                "Converts an unpacked Instagram data download into Boris Markdown, theme assets, and reports."
            }
            "obsidian" => {
                "Converts a local Obsidian vault into Boris Markdown with attachment and review reports."
            }
            "tinderbox-inventory" => {
                "Reads a Tinderbox .tbx document and writes inventory reports without converting it."
            }
            "tinderbox" => {
                "Converts a Tinderbox .tbx document into candidate Boris Markdown and conversion reports."
            }
            "notion" => {
                "Converts an unpacked official Notion Markdown-and-CSV export into Boris Markdown and review reports."
            }
            "filed" => {
                "Converts a Filed.fyi Astro source root into a bounded Boris changelog and release tree."
            }
            "starlight" => {
                "Converts a Starlight or Astro docs root into a Boris candidate tree with route and asset manifests."
            }
            "asset-filename" => {
                "Sanitizes asset filenames in a content tree and rewrites Markdown references."
            }
            "theme-archaeology" => {
                "Reads an Astro or Starlight-shaped theme and writes a deterministic adaptation ledger."
            }
            "theme-materialize" => {
                "Materializes a reviewed theme adaptation ledger into a safe Boris theme draft."
            }
            "wordpress-theme" => {
                "Reads a classic WordPress theme tree and writes a static Boris prototype and review manifest."
            }
            "link-audit" => {
                "Audits a generated static HTML tree for missing local routes and fragments."
            }
            "hugo-inventory" => {
                "Reads a Hugo project root and writes inventory reports without converting it."
            }
            "hugo" => {
                "Converts a Hugo project root into Boris Markdown, sibling assets, provenance, redirects, and review reports."
            }
            "jekyll-inventory" => {
                "Reads a Jekyll site source tree and writes inventory reports without converting it."
            }
            "jekyll" => {
                "Converts a Jekyll site source tree into Boris Markdown with provenance and review reports."
            }
            "ghost-inventory" => {
                "Reads an unpacked Ghost 5 export and writes inventory reports without converting it."
            }
            "ghost" => {
                "Converts an unpacked Ghost 5 export into candidate Boris Markdown, assets, and provenance reports."
            }
            "mkdocs-inventory" => {
                "Reads an MkDocs project root and writes inventory reports without converting it."
            }
            "mkdocs" => {
                "Converts an MkDocs project root into Boris Markdown with includes, assets, and provenance reports."
            }
            "mastodon-inventory" => {
                "Reads an unpacked Mastodon archive and writes inventory reports without converting it."
            }
            "mastodon" => {
                "Converts an unpacked Mastodon archive into Boris Markdown, media assets, and provenance reports."
            }
            "atproto" => {
                "Converts an AT Protocol repository export into Boris content with embed assets and provenance."
            }
            "atproto-inventory" => {
                "Reads an AT Protocol repository export and writes inventory reports without converting it."
            }
            "enex-inventory" => {
                "Reads a directory of Evernote .enex exports and writes inventory reports without converting them."
            }
            "enex" => {
                "Converts a directory of Evernote .enex exports into candidate Boris Markdown, assets, and reports."
            }
            "substack-inventory" => {
                "Reads an unpacked Substack export and writes inventory reports without converting it."
            }
            "substack" => {
                "Converts an unpacked Substack export into candidate Boris Markdown, assets, and reports."
            }
            _ => {
                "Runs the pinned migration-lab mode and writes reports under the chosen output directory."
            }
        }
    }

    /// The read-only inventory companion for an emitting mode, when the
    /// pinned lab provides one.
    pub fn inventory(&self) -> Option<&'static str> {
        match self.name {
            "tinderbox" => Some("tinderbox-inventory"),
            "hugo" => Some("hugo-inventory"),
            "jekyll" => Some("jekyll-inventory"),
            "ghost" => Some("ghost-inventory"),
            "mkdocs" => Some("mkdocs-inventory"),
            "mastodon" => Some("mastodon-inventory"),
            "atproto" => Some("atproto-inventory"),
            "enex" => Some("enex-inventory"),
            "substack" => Some("substack-inventory"),
            _ => None,
        }
    }

    /// Safety facts the wizard must make visible before a run. Mode-specific
    /// PII entries name the files the pinned lab deliberately does not read.
    pub fn safety_notes(&self) -> Vec<&'static str> {
        let mut notes = BASE_SAFETY.to_vec();
        match self.name {
            "substack" | "substack-inventory" => notes.push(
                "Skips subscriber PII files: subscribers.csv and email_list*.csv are never opened or read.",
            ),
            "enex" | "enex-inventory" => notes.push(
                "Does not copy raw author, email, or GPS values into inventories or candidate content.",
            ),
            _ => {}
        }
        notes
    }
}

/// Filename extensions offered by the picker for a file-valued input flag.
/// Empty means any regular file. Directory inputs do not use this table.
pub fn input_extensions(flag: &str) -> &'static [&'static str] {
    match flag {
        "--wxr" => &["xml"],
        "--tbx" => &["tbx"],
        _ => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_names_are_unique_and_inputs_are_path_flags() {
        for (i, m) in MODES.iter().enumerate() {
            assert!(
                MODES[..i].iter().all(|o| o.name != m.name),
                "duplicate mode {}",
                m.name
            );
            assert!(!m.inputs.is_empty(), "{} takes no input", m.name);
            assert!(m.inputs.iter().any(|s| s.required), "{}", m.name);
            for (j, s) in m.inputs.iter().enumerate() {
                assert!(s.flag.starts_with("--") && !s.flag.contains('='), "{s:?}");
                // The adapter owns these two; a table row must never set them.
                assert!(!matches!(s.flag, "--mode" | "--out"), "{s:?}");
                assert!(m.inputs[..j].iter().all(|o| o.flag != s.flag), "{s:?}");
            }
        }
    }

    #[test]
    fn developer_only_and_unlisted_modes_are_not_runnable() {
        for name in ["astro-import-plan", "astro-import-apply", "jekyll-emit"] {
            assert!(mode(name).is_none(), "{name}");
        }
        assert_eq!(mode("jekyll").unwrap().inputs, ROOT);
    }

    #[test]
    fn filed_scan_is_a_canonical_read_only_scan_mode() {
        let m = mode("filed-scan").unwrap();
        assert_eq!(m.inputs, ROOT);
    }
}
