//! `boris init`: which starters this Boris offers, and writing one into
//! a new folder. What a starter contains, and whether a folder may take
//! one, is Boris's to decide; this reads the names `boris init --help`
//! lists and runs `Verb::Init`.

use crate::error::{EngineError, Result};
use crate::locate::{EngineIdentity, Locator, Pin};
use crate::proc::CancelToken;
use crate::verbs::{VerbOutput, argv, run_argv};
use cv_core::engine::Verb;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// The starter archetypes a Boris build lists, in its order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Starters {
    pub archetypes: Vec<String>,
    /// What `--type` defaults to, when the help names it.
    pub default: Option<String>,
}

/// Read the archetype table out of `boris init --help`: the indented
/// block after the line introducing `--type`, one name per line that
/// starts at the block's indent (deeper lines continue a description).
pub fn parse_help(help: &str) -> Starters {
    let mut lines = help.lines();
    let intro = lines
        .by_ref()
        .find(|l| l.contains("--type") && l.trim_end().ends_with(':'))
        .unwrap_or_default();
    let default = intro
        .split_once("default `")
        .and_then(|(_, rest)| rest.split_once('`'))
        .map(|(name, _)| name.to_string());
    let archetypes: Vec<String> = lines
        .skip_while(|l| l.trim().is_empty())
        .take_while(|l| !l.trim().is_empty())
        .filter_map(|l| {
            let rest = l.strip_prefix("  ")?;
            let name = rest.split_whitespace().next()?;
            let starts_here = !rest.starts_with(' ');
            let plain = name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
            (starts_here && plain && !name.starts_with('-')).then(|| name.to_string())
        })
        .collect();
    let default = default.filter(|d| archetypes.contains(d));
    Starters {
        archetypes,
        default,
    }
}

/// A finished `boris init`, plus the canonical folder it was asked to write,
/// in display form (no Windows drive-letter verbatim prefix).
#[derive(Debug, Clone)]
pub struct InitOutcome {
    pub target: PathBuf,
    pub output: VerbOutput,
}

/// A located, version-gated Boris for creating workspaces; unlike
/// `ProcessEngine` it belongs to no workspace yet.
pub struct Initializer {
    identity: EngineIdentity,
}

fn refused(msg: impl Into<String>) -> EngineError {
    EngineError::Refused(msg.into())
}

/// The one path shape `boris init` may receive from the app: a plain,
/// UTF-8 folder name. Paths and flag-looking names never reach argv.
fn plain_name(name: &OsStr) -> Result<String> {
    let s = name
        .to_str()
        .ok_or_else(|| refused("workspace folder names must be valid UTF-8"))?;
    if s.is_empty() || s == "." || s == ".." || s.contains(['/', '\\']) || s.starts_with('-') {
        return Err(refused(
            "workspace folder name must be one plain name, not a path or a flag",
        ));
    }
    Ok(s.to_string())
}

/// Resolve `target` to `(canonical parent, plain name, canonical target)`.
/// Canonicalizing the parent collapses `..` and symlinked locations before
/// the child runs. An existing target is never followed through a symlink,
/// and a canonical existing directory must still be inside that parent.
fn resolve_target(target: &Path) -> Result<(PathBuf, String, PathBuf)> {
    let (Some(parent), Some(name)) = (target.parent(), target.file_name()) else {
        return Err(refused(format!(
            "{} has no parent folder",
            target.display()
        )));
    };
    let name = plain_name(name)?;
    let parent = std::fs::canonicalize(parent)
        .map_err(|e| refused(format!("{} is not a usable folder: {e}", parent.display())))?;
    if !parent.is_dir() {
        return Err(refused(format!("{} is not a folder", parent.display())));
    }
    let target = parent.join(&name);
    if let Ok(meta) = std::fs::symlink_metadata(&target) {
        if meta.file_type().is_symlink() {
            return Err(refused(format!(
                "{} is a symlink; choose a new folder",
                target.display()
            )));
        }
        if !meta.is_dir() {
            return Err(refused(format!(
                "{} already exists and is not a folder",
                target.display()
            )));
        }
        let canonical = std::fs::canonicalize(&target)?;
        if !canonical.starts_with(&parent) {
            return Err(refused(format!(
                "{} resolves outside {}",
                target.display(),
                parent.display()
            )));
        }
    }
    Ok((parent, name, target))
}

impl Initializer {
    pub fn locate(locator: &Locator, cwd: &Path, pin: Pin) -> Result<Self> {
        Ok(Self {
            identity: locator.locate(cwd, pin)?,
        })
    }

    pub fn identity(&self) -> &EngineIdentity {
        &self.identity
    }

    /// Run `boris init --help` and read its archetype table.
    pub fn starters(&self, cwd: &Path, cancel: &CancelToken) -> Result<Starters> {
        let args = ["init".to_string(), "--help".to_string()];
        let out = run_argv(&self.identity.path, cwd, &args, cancel)?;
        if out.outcome.class() != Some(cv_core::engine::ExitClass::Ok) {
            return Err(EngineError::Abnormal(out.outcome));
        }
        let starters = parse_help(&format!("{}\n{}", out.stdout, out.stderr));
        if starters.archetypes.is_empty() {
            return Err(EngineError::Refused(
                "boris init --help lists no archetypes".into(),
            ));
        }
        Ok(starters)
    }

    /// `boris init <name> --type <archetype>` from the target's canonical
    /// parent, so the target sits inside the folder Boris runs in and its
    /// compile check runs. Boris refuses a target that is not empty.
    pub fn create(
        &self,
        target: &Path,
        archetype: &str,
        cancel: &CancelToken,
    ) -> Result<InitOutcome> {
        let (parent, name, target) = resolve_target(target)?;
        let verb = Verb::Init {
            dir: name,
            archetype: archetype.to_string(),
        };
        let output = run_argv(&self.identity.path, &parent, &argv(&verb)?, cancel)?;
        // Validation and the spawn used the fully canonical target. Only the
        // path handed onward is display-normalized, so the symlink/traversal
        // decisions above are unchanged.
        Ok(InitOutcome {
            target: crate::paths::strip_verbatim(target),
            output,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_lists_names_in_order_and_skips_continuations_and_options() {
        let help = "Usage: boris init [DIR] [--type NAME]\n\nWrites a tree selected by\n`--type` (default `b`):\n\n  a      first\n         more about a\n  b-two  second\n  c three\n\nOptions:\n  --type NAME  pick\n  boris check  not an archetype\n";
        let s = parse_help(help);
        assert_eq!(s.archetypes, ["a", "b-two", "c"]);
        assert_eq!(s.default, None, "b is not listed");
    }

    #[test]
    fn help_without_a_type_table_lists_nothing() {
        assert!(parse_help("Usage: boris init\n").archetypes.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn an_existing_target_symlink_is_never_followed() {
        let t = tempfile::tempdir().unwrap();
        let outside = t.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        let link = t.path().join("site");
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        let e = resolve_target(&link).unwrap_err();
        assert!(e.to_string().contains("symlink"), "{e}");
    }

    #[test]
    fn parent_traversal_and_symlinks_resolve_before_init_runs() {
        let t = tempfile::tempdir().unwrap();
        let real = t.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = t.path().join("link");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, &link).unwrap();
        #[cfg(windows)]
        return;
        let (parent, name, target) =
            resolve_target(&link.join("..").join("link").join("site")).unwrap();
        assert_eq!(parent, std::fs::canonicalize(&real).unwrap());
        assert_eq!(name, "site");
        assert_eq!(target, parent.join("site"));
    }
}
