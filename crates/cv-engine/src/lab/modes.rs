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
    /// Boris starter suggested when this mode is adopted into a new
    /// workspace. `None` leaves Boris's own default selected.
    pub archetype: Option<&'static str>,
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
        archetype: None,
    }
}

const fn with(name: &'static str, family: Family, inputs: &'static [InputSpec]) -> ModeSpec {
    ModeSpec {
        name,
        family,
        inputs,
        archetype: None,
    }
}

const fn root_as(name: &'static str, family: Family, archetype: &'static str) -> ModeSpec {
    ModeSpec {
        archetype: Some(archetype),
        ..root(name, family)
    }
}

const fn with_as(
    name: &'static str,
    family: Family,
    inputs: &'static [InputSpec],
    archetype: &'static str,
) -> ModeSpec {
    ModeSpec {
        archetype: Some(archetype),
        ..with(name, family, inputs)
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
    with_as("obsidian", Notes, &[dir("--vault")], "garden"),
    with("tinderbox-inventory", Notes, &[file("--tbx")]),
    with("tinderbox", Notes, &[file("--tbx")]),
    with("notion", Notes, &[dir("--export")]),
    with("filed", StaticSite, &[dir("--filed-root")]),
    root("filed-scan", StaticSite),
    root_as("starlight", StaticSite, "docs"),
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
    root_as("ghost", BlogCms, "blog"),
    root("mkdocs-inventory", StaticSite),
    root_as("mkdocs", StaticSite, "docs"),
    root("mastodon-inventory", Social),
    root("mastodon", Social),
    root("atproto", Social),
    root("atproto-inventory", Social),
    root("enex-inventory", Notes),
    root_as("enex", Notes, "garden"),
    root("substack-inventory", BlogCms),
    root_as("substack", BlogCms, "blog"),
];
/// The allowlisted mode named `name` (canonical names only).
pub fn mode(name: &str) -> Option<&'static ModeSpec> {
    MODES.iter().find(|m| m.name == name)
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
