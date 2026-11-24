//! Binary discovery + version gate. Search order is fixed:
//! `CV_BORIS_BIN` → bundled → `PATH` → sibling checkout. The sibling
//! candidate sits next to the *workspace*, which may come from anywhere,
//! and the gate has to execute a binary (`--version`) to learn its
//! identity — so it is searched only when `CV_BORIS_SIBLING=1` opts in
//! (a developer with a trusted boris checkout beside their corpora). The
//! first candidate that exists is gated; a mismatch is a typed error, never a
//! fallthrough to the next candidate (no BYO engine).

use crate::error::{EngineError, Result};
use crate::proc::{CancelToken, boris_command, run_captured};
use cv_contracts::build_info::BuildInfo;
use cv_core::engine::EngineSource;
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub const ENV_OVERRIDE: &str = "CV_BORIS_BIN";
/// Opt-in for the sibling-checkout candidate (`1` enables it).
pub const ENV_SIBLING: &str = "CV_BORIS_SIBLING";

/// The one engine pin (mirrors `fixtures/MANIFEST.toml` `engine_version`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pin {
    pub version: &'static str,
    pub vcs_revision: &'static str,
}

pub const PINNED: Pin = Pin {
    version: "boris/0.9.0",
    vcs_revision: "66efcfb",
};

/// Ledger record of exactly which engine this workspace runs against.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineIdentity {
    pub source: EngineSource,
    pub path: PathBuf,
    pub version: String,
    pub vcs_revision: String,
    /// `boris --build-info` stdout, verbatim.
    pub build_info_raw: String,
}

#[derive(Debug, Clone, Default)]
pub struct Locator {
    pub env_override: Option<PathBuf>,
    pub bundled_dirs: Vec<PathBuf>,
    pub path_var: Option<OsString>,
    pub sibling_dirs: Vec<PathBuf>,
}

/// Bundled engines live in `<exe dir>/engines/` — the layout
/// `cargo run -p xtask -- release` produces.
pub const BUNDLED_DIR: &str = "engines";

/// Bundled search dirs for an app binary at `exe`, in order:
/// `<dir>/engines`, the macOS `.app` `Contents/Resources/engines`, then
/// `<dir>` itself.
pub fn bundled_dirs(exe: &Path) -> Vec<PathBuf> {
    let Some(dir) = exe.parent() else {
        return vec![];
    };
    vec![
        dir.join(BUNDLED_DIR),
        // macOS .app: Contents/MacOS/<exe> → Contents/Resources/engines
        dir.join("..").join("Resources").join(BUNDLED_DIR),
        dir.to_path_buf(),
    ]
}

pub fn exe_name() -> String {
    format!("boris{}", std::env::consts::EXE_SUFFIX)
}

impl Locator {
    /// The production search space for a workspace rooted at `root`.
    pub fn from_env(root: &Path) -> Self {
        let bundled_dirs = std::env::current_exe()
            .map(|exe| bundled_dirs(&exe))
            .unwrap_or_default();
        let sibling_dirs = sibling_dirs(
            root,
            std::env::var_os(ENV_SIBLING).is_some_and(|v| v == "1"),
        );
        Self {
            env_override: std::env::var_os(ENV_OVERRIDE)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from),
            bundled_dirs,
            path_var: std::env::var_os("PATH"),
            sibling_dirs,
        }
    }

    /// Every path probed, in order, with the source it represents.
    pub fn candidates(&self) -> Vec<(EngineSource, PathBuf)> {
        let name = exe_name();
        let mut out = Vec::new();
        if let Some(p) = &self.env_override {
            out.push((EngineSource::EnvOverride, p.clone()));
        }
        out.extend(
            self.bundled_dirs
                .iter()
                .map(|d| (EngineSource::Bundled, d.join(&name))),
        );
        if let Some(pv) = &self.path_var {
            out.extend(std::env::split_paths(pv).map(|d| (EngineSource::Path, d.join(&name))));
        }
        out.extend(
            self.sibling_dirs
                .iter()
                .map(|d| (EngineSource::SiblingCheckout, d.join(&name))),
        );
        out
    }

    /// First existing candidate. An explicit override that doesn't exist
    /// is an error: silently using some other boris would hide a typo.
    pub fn find(&self) -> Result<(EngineSource, PathBuf)> {
        if let Some(p) = &self.env_override
            && !is_executable(p)
        {
            return Err(EngineError::OverrideMissing(p.clone()));
        }
        let all = self.candidates();
        all.iter()
            .find(|(_, p)| is_executable(p))
            .cloned()
            .ok_or_else(|| EngineError::NotFound {
                searched: all.into_iter().map(|(_, p)| p).collect(),
            })
    }

    pub fn locate(&self, cwd: &Path, pin: Pin) -> Result<EngineIdentity> {
        let (source, path) = self.find()?;
        gate(source, &path, cwd, pin)
    }
}

/// `<root>/../boris/zig-out/bin`, only when opted in: nothing derived
/// from the workspace location is executed by default.
pub fn sibling_dirs(root: &Path, opt_in: bool) -> Vec<PathBuf> {
    if !opt_in {
        return vec![];
    }
    root.parent()
        .map(|p| vec![p.join("boris").join("zig-out").join("bin")])
        .unwrap_or_default()
}

pub fn is_executable(p: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(p) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn revision_matches(found: &str, pinned: &str) -> bool {
    !found.is_empty() && (found.starts_with(pinned) || pinned.starts_with(found))
}

/// `--version` gate, then `--build-info` recorded verbatim.
pub fn gate(source: EngineSource, path: &Path, cwd: &Path, pin: Pin) -> Result<EngineIdentity> {
    let never = CancelToken::new();
    let mut v = boris_command(path, cwd);
    v.arg("--version");
    let out = run_captured(v, None, &never)?;
    let version = String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();

    let mut b = boris_command(path, cwd);
    b.arg("--build-info");
    let bi = run_captured(b, None, &never)?;
    let raw = String::from_utf8_lossy(&bi.stdout).trim().to_string();
    let info: Option<BuildInfo> = serde_json::from_str(&raw).ok().filter(BuildInfo::is_boris);

    let mismatch = |found_vcs: Option<String>| EngineError::VersionMismatch {
        path: path.to_path_buf(),
        source_kind: source.clone(),
        found: version.clone(),
        found_vcs,
        expected: pin.version,
        expected_vcs: pin.vcs_revision,
    };
    if version != pin.version {
        return Err(mismatch(info.map(|i| i.vcs_revision)));
    }
    let Some(info) = info else {
        return Err(EngineError::BuildInfoInvalid {
            path: path.to_path_buf(),
            raw,
        });
    };
    if info.version != pin.version || !revision_matches(&info.vcs_revision, pin.vcs_revision) {
        return Err(mismatch(Some(info.vcs_revision)));
    }
    Ok(EngineIdentity {
        source,
        path: path.to_path_buf(),
        version,
        vcs_revision: info.vcs_revision,
        build_info_raw: raw,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch_exe(dir: &Path) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let p = dir.join(exe_name());
        std::fs::write(&p, b"").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        p
    }

    #[test]
    fn search_order_is_override_bundled_path_sibling() {
        let t = tempfile::tempdir().unwrap();
        let (b, pa, s) = (t.path().join("b"), t.path().join("p"), t.path().join("s"));
        let loc = Locator {
            env_override: None,
            bundled_dirs: vec![b.clone()],
            path_var: Some(std::env::join_paths([&pa]).unwrap()),
            sibling_dirs: vec![s.clone()],
        };
        let kinds: Vec<_> = loc.candidates().into_iter().map(|(k, _)| k).collect();
        assert_eq!(
            kinds,
            [
                EngineSource::Bundled,
                EngineSource::Path,
                EngineSource::SiblingCheckout
            ]
        );
        assert!(matches!(loc.find(), Err(EngineError::NotFound { .. })));
        touch_exe(&s);
        assert_eq!(loc.find().unwrap().0, EngineSource::SiblingCheckout);
        touch_exe(&pa);
        assert_eq!(loc.find().unwrap().0, EngineSource::Path);
        touch_exe(&b);
        assert_eq!(loc.find().unwrap().0, EngineSource::Bundled);
    }

    /// Every level populated: each one shadows everything after it.
    #[test]
    fn each_level_wins_over_the_ones_after_it() {
        let t = tempfile::tempdir().unwrap();
        let exe = t.path().join("app").join("cv-app");
        let [bundled, _, exe_dir] = bundled_dirs(&exe).try_into().unwrap();
        let (env, pa, sib) = (t.path().join("env"), t.path().join("p"), t.path().join("s"));
        let env_bin = touch_exe(&env);
        let bundled_bin = touch_exe(&bundled);
        let exe_dir_bin = touch_exe(&exe_dir);
        let path_bin = touch_exe(&pa);
        let sib_bin = touch_exe(&sib);
        let mut loc = Locator {
            env_override: Some(env_bin.clone()),
            bundled_dirs: bundled_dirs(&exe),
            path_var: Some(std::env::join_paths([&pa]).unwrap()),
            sibling_dirs: vec![sib],
        };
        // CV_BORIS_BIN wins over bundled, PATH and sibling.
        assert_eq!(loc.find().unwrap(), (EngineSource::EnvOverride, env_bin));
        // Unset: bundled `engines/` wins over the exe dir, PATH, sibling.
        loc.env_override = None;
        assert_eq!(
            loc.find().unwrap(),
            (EngineSource::Bundled, bundled_bin.clone())
        );
        std::fs::remove_file(&bundled_bin).unwrap();
        assert_eq!(
            loc.find().unwrap(),
            (EngineSource::Bundled, exe_dir_bin.clone())
        );
        // No bundled engine: PATH wins over the sibling checkout.
        std::fs::remove_file(&exe_dir_bin).unwrap();
        assert_eq!(loc.find().unwrap(), (EngineSource::Path, path_bin.clone()));
        std::fs::remove_file(&path_bin).unwrap();
        assert_eq!(
            loc.find().unwrap(),
            (EngineSource::SiblingCheckout, sib_bin)
        );
    }

    #[test]
    fn release_layout_bundles_into_engines_next_to_the_app() {
        let exe = Path::new("dist").join("cv").join("cv-app");
        let dirs = bundled_dirs(&exe);
        assert_eq!(dirs[0], Path::new("dist").join("cv").join(BUNDLED_DIR));
        assert_eq!(
            dirs[1],
            Path::new("dist")
                .join("cv")
                .join("..")
                .join("Resources")
                .join(BUNDLED_DIR)
        );
        assert_eq!(dirs[2], Path::new("dist").join("cv"));
        assert!(bundled_dirs(Path::new("")).is_empty());
    }

    /// A non-executable file in a bundled slot is not an engine.
    #[cfg(unix)]
    #[test]
    fn non_executable_bundled_file_is_skipped() {
        let t = tempfile::tempdir().unwrap();
        let (b, pa) = (t.path().join("b"), t.path().join("p"));
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(b.join(exe_name()), b"").unwrap();
        let path_bin = touch_exe(&pa);
        let loc = Locator {
            bundled_dirs: vec![b],
            path_var: Some(std::env::join_paths([&pa]).unwrap()),
            ..Locator::default()
        };
        assert_eq!(loc.find().unwrap(), (EngineSource::Path, path_bin));
    }

    #[test]
    fn missing_override_is_an_error_not_a_fallthrough() {
        let t = tempfile::tempdir().unwrap();
        let loc = Locator {
            env_override: Some(t.path().join("nope")),
            path_var: None,
            bundled_dirs: vec![],
            sibling_dirs: vec![],
        };
        touch_exe(&t.path().join("b"));
        assert!(matches!(loc.find(), Err(EngineError::OverrideMissing(_))));
    }

    #[test]
    fn sibling_checkout_is_opt_in() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("corpus");
        let sib = t.path().join("boris").join("zig-out").join("bin");
        touch_exe(&sib);
        assert!(sibling_dirs(&root, false).is_empty());
        assert_eq!(sibling_dirs(&root, true), vec![sib.clone()]);
        // Default search space never offers (or runs) the sibling binary.
        let loc = Locator {
            sibling_dirs: sibling_dirs(&root, false),
            ..Locator::default()
        };
        assert!(matches!(loc.find(), Err(EngineError::NotFound { .. })));
        let loc = Locator {
            sibling_dirs: sibling_dirs(&root, true),
            ..Locator::default()
        };
        assert_eq!(
            loc.find().unwrap(),
            (EngineSource::SiblingCheckout, sib.join(exe_name()))
        );
    }

    #[test]
    fn revision_prefixes() {
        assert!(revision_matches("66efcfb586b4", "66efcfb"));
        assert!(revision_matches("66efcfb", "66efcfb586b4"));
        assert!(!revision_matches("ce40d78", "66efcfb"));
        assert!(!revision_matches("", "66efcfb"));
    }
}
