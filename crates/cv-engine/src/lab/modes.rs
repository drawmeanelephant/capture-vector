//! Per-mode flag allowlist for `boris-migration-lab`, transcribed from its
//! `--help` at the pinned commit (`ce40d78`, `boris-migration-lab/0.8.1`).
//! This table is the only copy of the lab's mode list in Capture Vector.
//! Replace it with `--list-modes --json` once
//! drawmeanelephant/boris-migration-lab#81 lands.
//!
//! Only path-valued flags are listed, and only canonical mode names (no
//! aliases). Left out on purpose: the developer-only `astro-import-plan` /
//! `astro-import-apply` (non-path ids, `--destination` instead of `--out`),
//! `filed-scan` (absent from `--help`), and options that are not input
//! paths or that make the lab run another binary (`--gate`,
//! `--relation-kinds`, `--relation-map`, `--locale`, `--max-pages`,
//! `--boris`).

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

#[derive(Debug, PartialEq, Eq)]
pub struct ModeSpec {
    /// The `--mode=` value.
    pub name: &'static str,
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

const fn root(name: &'static str) -> ModeSpec {
    ModeSpec { name, inputs: ROOT }
}

pub const MODES: &[ModeSpec] = &[
    root("astro"),
    ModeSpec {
        name: "wordpress",
        inputs: &[file("--wxr"), optional_dir("--media")],
    },
    root("wordpress-theme"),
    ModeSpec {
        name: "instagram",
        inputs: &[dir("--dump")],
    },
    ModeSpec {
        name: "obsidian",
        inputs: &[dir("--vault")],
    },
    ModeSpec {
        name: "tinderbox-inventory",
        inputs: &[file("--tbx")],
    },
    ModeSpec {
        name: "tinderbox",
        inputs: &[file("--tbx")],
    },
    ModeSpec {
        name: "notion",
        inputs: &[dir("--export")],
    },
    ModeSpec {
        name: "filed",
        inputs: &[dir("--filed-root")],
    },
    root("starlight"),
    root("asset-filename"),
    root("theme-archaeology"),
    ModeSpec {
        name: "theme-materialize",
        inputs: &[dir("--root"), file("--ledger")],
    },
    root("link-audit"),
    ModeSpec {
        name: "frontmatter-review",
        inputs: &[dir("--content")],
    },
    root("hugo-inventory"),
    root("hugo"),
    root("jekyll-inventory"),
    root("jekyll"),
    root("ghost-inventory"),
    root("ghost"),
    root("mkdocs-inventory"),
    root("mkdocs"),
    root("mastodon-inventory"),
    root("mastodon"),
    root("atproto"),
    root("atproto-inventory"),
    root("enex-inventory"),
    root("enex"),
    root("substack-inventory"),
    root("substack"),
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
        for name in [
            "astro-import-plan",
            "astro-import-apply",
            "filed-scan",
            "jekyll-emit",
        ] {
            assert!(mode(name).is_none(), "{name}");
        }
        assert_eq!(mode("jekyll").unwrap().inputs, ROOT);
    }
}
