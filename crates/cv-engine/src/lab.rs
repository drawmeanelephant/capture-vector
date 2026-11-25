//! `boris-migration-lab` adapter: locate and `--version` gate, typed runs
//! built from a per-mode flag allowlist ([`modes`]), canonical paths, and
//! the lab's own `0/2/3` exit codes. What a mode *does* is the lab's
//! business; this module only spells argv and keeps the evidence.

pub mod modes;

use crate::error::EngineError;
use crate::exit::ExitOutcome;
use crate::locate::{Locator, env_opt_in, sibling_checkout_dirs};
use crate::proc::{CancelToken, boris_command, run_captured};
use cv_core::engine::EngineSource;
pub use modes::{Family, InputKind, InputSpec, MODES, ModeSpec, input_extensions, mode};
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use std::time::Instant;
use thiserror::Error;

pub const ENV_OVERRIDE: &str = "CV_MIGRATION_LAB_BIN";
/// Opt-in for the sibling-checkout candidate (`1` enables it).
pub const ENV_SIBLING: &str = "CV_MIGRATION_LAB_SIBLING";
const CHECKOUT: &str = "boris-migration-lab";
const STAGE_SUFFIX: &str = ".boris-migration-lab-stage";
const BACKUP_SUFFIX: &str = ".boris-migration-lab-prev";

/// The one lab pin (mirrors `fixtures/MANIFEST.toml` `[migration_lab]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LabPin {
    /// What `--version` prints.
    pub tool_id: &'static str,
    /// Full commit `scripts/fetch-migration-lab.sh` builds. The binary
    /// cannot report it, so the gate checks `tool_id` only.
    pub commit: &'static str,
}

pub const PINNED: LabPin = LabPin {
    tool_id: "boris-migration-lab/0.8.1",
    commit: "ce40d78a0cd20f3e36550c7a85391903672bcb5a",
};

pub fn exe_name() -> String {
    format!("{CHECKOUT}{}", std::env::consts::EXE_SUFFIX)
}

/// The Boris search order with the lab's names:
/// `CV_MIGRATION_LAB_BIN` → bundled → `PATH` → sibling checkout
/// (`CV_MIGRATION_LAB_SIBLING=1` only).
pub fn locator_from_env(root: &Path) -> Locator {
    Locator::from_env_vars(ENV_OVERRIDE, sibling_dirs(root, env_opt_in(ENV_SIBLING)))
}

/// `<root>/../boris-migration-lab/zig-out/bin`, only when opted in.
pub fn sibling_dirs(root: &Path, opt_in: bool) -> Vec<PathBuf> {
    sibling_checkout_dirs(root, CHECKOUT, opt_in)
}

/// Which lab binary a run uses, recorded once at gate time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LabIdentity {
    pub source: EngineSource,
    pub path: PathBuf,
    /// `--version` stdout, first line.
    pub tool_id: String,
}

/// Typed lab failures. `NotFound`, `OverrideMissing` and
/// `VersionMismatch` mean "no usable lab": callers disclose them and keep
/// running, as they do for an absent Boris.
#[derive(Debug, Error)]
pub enum LabError {
    #[error("boris-migration-lab not found (searched: {searched:?})")]
    NotFound { searched: Vec<PathBuf> },
    #[error("CV_MIGRATION_LAB_BIN points at {0:?}, which is not an executable file")]
    OverrideMissing(PathBuf),
    #[error(
        "boris-migration-lab version mismatch at {path:?} ({source_kind:?}): found {found:?}, pinned {expected}"
    )]
    VersionMismatch {
        path: PathBuf,
        source_kind: EngineSource,
        found: String,
        expected: &'static str,
    },
    #[error("mode {mode} does not take {flag}")]
    FlagNotAllowed { mode: &'static str, flag: String },
    #[error("mode {mode} needs {flag}")]
    MissingInput {
        mode: &'static str,
        flag: &'static str,
    },
    #[error("{flag} given more than once")]
    DuplicateInput { flag: &'static str },
    #[error("{flag} {path:?}: {source}")]
    InputPath {
        flag: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{flag} {path:?} is not a {}", expected.noun())]
    InputKind {
        flag: &'static str,
        path: PathBuf,
        expected: InputKind,
    },
    #[error("{flag} {input:?} contains symlink component {component:?}")]
    InputSymlink {
        flag: &'static str,
        input: PathBuf,
        component: PathBuf,
    },
    #[error("--out {path:?} {reason}")]
    OutPath { path: PathBuf, reason: String },
    #[error("--out {out:?} overlaps {flag} {input:?}")]
    Overlap {
        flag: &'static str,
        input: PathBuf,
        out: PathBuf,
    },
    #[error("{flag} must be an absolute path, got {}", path.display())]
    InputRelative { flag: &'static str, path: PathBuf },
    #[error("{flag} must be {}", kind.noun())]
    ExtraValue {
        flag: &'static str,
        kind: modes::ExtraKind,
    },
    #[error("run id {0:?} must be one plain path segment")]
    RunId(String),
    #[error(transparent)]
    Engine(#[from] EngineError),
}

/// First existing candidate, with the lab's own absence errors.
pub fn find(loc: &Locator) -> Result<(EngineSource, PathBuf), LabError> {
    loc.find_named(&exe_name()).map_err(|e| match e {
        EngineError::OverrideMissing(p) => LabError::OverrideMissing(p),
        EngineError::NotFound { searched } => LabError::NotFound { searched },
        other => LabError::Engine(other),
    })
}

pub fn locate(loc: &Locator, cwd: &Path, pin: LabPin) -> Result<LabIdentity, LabError> {
    let (source, path) = find(loc)?;
    gate(source, &path, cwd, pin)
}

/// `--version` must exit 0 and print exactly `pin.tool_id`. Like the
/// Boris gate this is anti-accident, not a security boundary.
pub fn gate(
    source: EngineSource,
    path: &Path,
    cwd: &Path,
    pin: LabPin,
) -> Result<LabIdentity, LabError> {
    let mut cmd = boris_command(path, cwd);
    cmd.arg("--version");
    let out = run_captured(cmd, None, &CancelToken::new())?;
    if !matches!(out.outcome, ExitOutcome::Exited { code: 0 }) {
        return Err(EngineError::Abnormal(out.outcome).into());
    }
    let found = String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();
    if found != pin.tool_id {
        return Err(LabError::VersionMismatch {
            path: path.to_path_buf(),
            source_kind: source,
            found,
            expected: pin.tool_id,
        });
    }
    Ok(LabIdentity {
        source,
        path: path.to_path_buf(),
        tool_id: found,
    })
}

/// `--help` text, verbatim. The lab prints it on stderr.
pub fn help(lab: &LabIdentity, cwd: &Path, cancel: &CancelToken) -> Result<String, LabError> {
    let mut cmd = boris_command(&lab.path, cwd);
    cmd.arg("--help");
    let out = run_captured(cmd, None, cancel)?;
    if !matches!(out.outcome, ExitOutcome::Exited { code: 0 }) {
        return Err(EngineError::Abnormal(out.outcome).into());
    }
    Ok(String::from_utf8_lossy(&out.stderr).into_owned())
}

/// `<workspace>/.cv/imports/<run_id>`, the default home of a run's output.
pub fn imports_dir(workspace: &Path, run_id: &str) -> Result<PathBuf, LabError> {
    let mut parts = Path::new(run_id).components();
    match (parts.next(), parts.next()) {
        (Some(Component::Normal(_)), None) if !run_id.contains(['/', '\\']) => {
            Ok(workspace.join(".cv").join("imports").join(run_id))
        }
        _ => Err(LabError::RunId(run_id.to_string())),
    }
}

/// One lab invocation. Nothing here is free-form: `inputs` flags must be
/// in `mode.inputs`, and `--mode` / `--out` are spelled by the adapter.
#[derive(Debug, Clone)]
pub struct LabRun {
    pub mode: &'static ModeSpec,
    pub inputs: Vec<(String, PathBuf)>,
    /// Scalar pass-through options (`--locale`, `--max-pages`,
    /// `--relation-map`), validated for type only. Empty means the lab's
    /// defaults.
    pub extras: Vec<(String, String)>,
    /// Need not exist yet. Must not overlap any input.
    pub out: PathBuf,
}

/// A validated run: canonical paths and the exact argv after the binary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedRun {
    pub mode: &'static str,
    /// In `mode.inputs` order.
    pub inputs: Vec<(&'static str, PathBuf)>,
    pub out: PathBuf,
    pub argv: Vec<OsString>,
    /// The deepest existing ancestor of an absent `out` (canonical); the
    /// parent of an existing `out`, which the lab may rename or replace.
    pub cwd: PathBuf,
}

impl LabRun {
    /// Check the run against the allowlist and validate every spelling
    /// no-follow. Symlinked inputs and outputs are refused in the original
    /// spelling — never canonicalized away — because the pinned lab rejects
    /// them on purpose (`InputSymlink`/`OutputSymlink`); only root-level
    /// `/tmp`, `/var`, `/etc` system aliases are normalized, one component
    /// exactly, as the explicit platform policy.
    pub fn prepare(&self) -> Result<PreparedRun, LabError> {
        let mode = self.mode;
        let mut given: Vec<(&'static InputSpec, PathBuf)> = Vec::new();
        for (flag, path) in &self.inputs {
            let spec = mode.inputs.iter().find(|s| s.flag == flag).ok_or_else(|| {
                LabError::FlagNotAllowed {
                    mode: mode.name,
                    flag: flag.clone(),
                }
            })?;
            if given.iter().any(|(s, _)| s.flag == spec.flag) {
                return Err(LabError::DuplicateInput { flag: spec.flag });
            }
            let input = validate_input_spelling(spec, path)?;
            given.push((spec, input));
        }
        let mut inputs = Vec::new();
        for spec in mode.inputs {
            match given.iter().find(|(s, _)| s.flag == spec.flag) {
                Some((_, p)) => inputs.push((spec.flag, p.clone())),
                None if spec.required => {
                    return Err(LabError::MissingInput {
                        mode: mode.name,
                        flag: spec.flag,
                    });
                }
                None => {}
            }
        }

        let (out, cwd) = canonical_out(&self.out)?;
        let output_roots = [
            out.clone(),
            publication_sibling(&out, STAGE_SUFFIX),
            publication_sibling(&out, BACKUP_SUFFIX),
        ];
        for (flag, input) in &inputs {
            // The spelling is already proven symlink-free, so canonicalizing
            // the comparison copy cannot flatten a user-selected symlink; it
            // only normalizes `.`/`..` and case for the overlap math. argv
            // still carries the validated spelling itself.
            let comparison =
                std::fs::canonicalize(input).map_err(|source| LabError::InputPath {
                    flag,
                    path: input.clone(),
                    source,
                })?;
            if output_roots
                .iter()
                .any(|protected| paths_overlap(protected, &comparison))
            {
                return Err(LabError::Overlap {
                    flag,
                    input: input.clone(),
                    out,
                });
            }
        }

        let mut extras: Vec<(&'static str, &str)> = Vec::new();
        for (flag, value) in &self.extras {
            let spec = mode.extras.iter().find(|e| e.flag == flag).ok_or_else(|| {
                LabError::FlagNotAllowed {
                    mode: mode.name,
                    flag: flag.clone(),
                }
            })?;
            if extras.iter().any(|(f, _)| f == &spec.flag) {
                return Err(LabError::DuplicateInput { flag: spec.flag });
            }
            let valid = match spec.kind {
                modes::ExtraKind::Text => !value.is_empty(),
                modes::ExtraKind::Count => value.parse::<usize>().is_ok_and(|n| n > 0),
            };
            if !valid {
                return Err(LabError::ExtraValue {
                    flag: spec.flag,
                    kind: spec.kind,
                });
            }
            extras.push((spec.flag, value));
        }

        let mut argv = vec![OsString::from(format!("--mode={}", mode.name))];
        argv.extend(inputs.iter().map(|(flag, p)| flag_value(flag, p)));
        for (flag, value) in &extras {
            let mut s = OsString::from(flag);
            s.push("=");
            s.push(value);
            argv.push(s);
        }
        argv.push(flag_value("--out", &out));
        Ok(PreparedRun {
            mode: mode.name,
            inputs,
            out,
            argv,
            cwd,
        })
    }
}

/// Validate one input's original spelling and return it unflattened.
///
/// The pinned lab refuses symlinked input roots (`InputSymlink`) because a
/// migration source is often irreplaceable and a symlink can redirect reads
/// outside the tree the user chose. Canonicalizing before the lab runs would
/// erase that evidence, so the spelling is validated instead: a narrow
/// system-alias normalization (below), then a no-follow check of every
/// existing component, then the kind check. The validated spelling itself —
/// never its canonical form — is what argv and the run record carry.
fn validate_input_spelling(spec: &InputSpec, path: &Path) -> Result<PathBuf, LabError> {
    // A relative spelling must never resolve against the app process cwd:
    // that could import a different tree than the one the user picked, and
    // `~/site` is not shell-expanded here either. Absolute or refusal.
    if !path.is_absolute() {
        return Err(LabError::InputRelative {
            flag: spec.flag,
            path: path.to_path_buf(),
        });
    }
    let abs = std::path::absolute(path).map_err(|source| LabError::InputPath {
        flag: spec.flag,
        path: path.to_path_buf(),
        source,
    })?;
    let spelling = normalize_system_aliases(&abs);
    if let Some(component) = symlink_component(&spelling) {
        return Err(LabError::InputSymlink {
            flag: spec.flag,
            input: path.to_path_buf(),
            component,
        });
    }
    let meta = std::fs::symlink_metadata(&spelling).map_err(|source| LabError::InputPath {
        flag: spec.flag,
        path: path.to_path_buf(),
        source,
    })?;
    let kind_ok = match spec.kind {
        InputKind::Dir => meta.is_dir(),
        InputKind::File => meta.is_file(),
    };
    if !kind_ok {
        return Err(LabError::InputKind {
            flag: spec.flag,
            path: spelling,
            expected: spec.kind,
        });
    }
    Ok(spelling)
}

/// Narrow macOS-only platform policy: rewrite a leading `/tmp`, `/var` or
/// `/etc` component when it really is a symlink (macOS aliases them into
/// `/private`). Exactly that component is replaced by its link target;
/// every other component of the spelling is preserved verbatim, so a
/// user-planted symlink can never be flattened by this normalization.
/// On every other platform this is a strict no-op: a symlinked `/tmp`
/// there is user evidence the pinned lab must see and refuse, never
/// something the adapter flattens.
#[cfg(target_os = "macos")]
fn normalize_system_aliases(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    let mut components = path.components();
    let mut rewritten = Vec::new();
    for component in components.by_ref() {
        out.push(component.as_os_str());
        rewritten.push(component);
        // Only root-level `/tmp`, `/var`, `/etc` qualify, and only when
        // they really are symlinks. One component, one rewrite.
        if rewritten.len() == 2
            && matches!(rewritten[0], Component::RootDir)
            && let Component::Normal(name) = component
            && matches!(name.to_str(), Some("tmp") | Some("var") | Some("etc"))
            && let Ok(meta) = std::fs::symlink_metadata(&out)
            && meta.file_type().is_symlink()
            && let Ok(target) = std::fs::read_link(&out)
        {
            out = if target.is_absolute() {
                target
            } else {
                Path::new("/").join(target)
            };
        }
    }
    out
}

#[cfg(not(target_os = "macos"))]
fn normalize_system_aliases(path: &Path) -> PathBuf {
    path.to_path_buf()
}

/// The first symlink component in a spelling, checked with no-follow
/// `lstat` semantics on each progressive prefix. Non-existent tails pass:
/// the lab re-checks everything at run time, so a race cannot bypass the
/// refusal — this is the adapter's early plain-language copy of the same
/// fact, not the authoritative check.
fn symlink_component(path: &Path) -> Option<PathBuf> {
    let mut progressive = PathBuf::new();
    for component in path.components() {
        progressive.push(component.as_os_str());
        if matches!(
            component,
            Component::RootDir | Component::CurDir | Component::ParentDir
        ) {
            continue;
        }
        match std::fs::symlink_metadata(&progressive) {
            Ok(meta) if meta.file_type().is_symlink() => return Some(progressive),
            Ok(_) => {}
            Err(_) => break,
        }
    }
    None
}

fn publication_sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut sibling = path.as_os_str().to_os_string();
    sibling.push(suffix);
    PathBuf::from(sibling)
}

fn paths_overlap(a: &Path, b: &Path) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

fn flag_value(flag: &str, path: &Path) -> OsString {
    let mut s = OsString::from(flag);
    s.push("=");
    s.push(path.as_os_str());
    s
}

/// Canonicalize the deepest existing ancestor and re-append the rest.
/// Returns `(out, cwd)`: that ancestor for an absent output, or the
/// existing output's parent so the child never starts inside the tree the
/// lab may rename or replace.
fn canonical_out(out: &Path) -> Result<(PathBuf, PathBuf), LabError> {
    let refuse = |reason: String| LabError::OutPath {
        path: out.to_path_buf(),
        reason,
    };
    if out.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(refuse("must not contain `..`".into()));
    }
    let abs = std::path::absolute(out).map_err(|e| refuse(e.to_string()))?;
    // The output spelling is adapter-chosen, so the narrow macOS system
    // alias policy applies here exactly as for inputs: `/var/folders`
    // tempdirs are the ordinary macOS spelling, not user evidence.
    let abs = normalize_system_aliases(&abs);
    // ...but its ancestors exist in the user-shared filesystem: a
    // pre-planted symlinked `.cv` or run directory must be refused, never
    // silently followed into its target. Check the spelling before any
    // canonicalization, exactly as for inputs.
    if let Some(component) = symlink_component(&abs) {
        return Err(refuse(format!(
            "contains symlink component {component:?}; the run was not started"
        )));
    }
    let mut existing = abs.as_path();
    let mut missing = Vec::new();
    while std::fs::symlink_metadata(existing).is_err() {
        let (Some(name), Some(parent)) = (existing.file_name(), existing.parent()) else {
            return Err(refuse("has no existing ancestor".into()));
        };
        missing.push(name);
        existing = parent;
    }
    let base = std::fs::canonicalize(existing).map_err(|e| refuse(e.to_string()))?;
    if !base.is_dir() {
        return Err(refuse(format!("{base:?} is not a directory")));
    }
    let cwd = if missing.is_empty() {
        base.parent()
            .ok_or_else(|| refuse("is a filesystem root".into()))?
            .to_path_buf()
    } else {
        base.clone()
    };
    let canonical = missing.iter().rev().fold(base, |p, c| p.join(c));
    Ok((canonical, cwd))
}

/// The lab's exit codes at the pin: `0` success, `2` usage, `3` I/O error.
/// Safety refusals such as `InputSymlink`, `OutputSymlink` and
/// `InputOutputOverlap` are reported on stderr as `migration-lab (<mode>)
/// failed: <ErrorName>` — at exit 2 or 3 depending on whether the mode's
/// own validation or validateRoots fires first (probed against the real
/// pinned lab). The error name, never the exit code, is the refusal
/// signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LabExit {
    Ok,
    UsageError,
    IoFailure,
}

impl LabExit {
    /// Any ending that is not a lab code (1, above 3, a signal, a Windows
    /// crash code) is a system failure. `None` only for our own kill.
    pub fn from_outcome(outcome: ExitOutcome) -> Option<Self> {
        match outcome {
            ExitOutcome::Exited { code: 0 } => Some(Self::Ok),
            ExitOutcome::Exited { code: 2 } => Some(Self::UsageError),
            ExitOutcome::Exited { .. } | ExitOutcome::Signaled { .. } => Some(Self::IoFailure),
            ExitOutcome::Killed => None,
        }
    }
}

/// A finished run. `stderr_bytes` is the lab's raw evidence up to the
/// engine's 8 MiB capture bound; capture overflow fails the run rather than
/// truncating silently. `stderr` is only a lossy UTF-8 view for diagnostics
/// and refusal detection. With `-q` off it holds the progress lines too.
#[derive(Debug, Clone)]
pub struct LabOutput {
    pub run: PreparedRun,
    pub outcome: ExitOutcome,
    pub exit: LabExit,
    pub stdout: String,
    pub stderr: String,
    pub stderr_bytes: Vec<u8>,
    pub duration_ms: u64,
}

/// Validate, spawn, and wait. Cancellation kills the lab's process tree
/// and returns `Engine(Cancelled)`.
pub fn run(lab: &LabIdentity, run: &LabRun, cancel: &CancelToken) -> Result<LabOutput, LabError> {
    let prepared = run.prepare()?;
    let mut cmd = boris_command(&lab.path, &prepared.cwd);
    cmd.args(&prepared.argv);
    let started = Instant::now();
    let c = run_captured(cmd, None, cancel)?;
    let exit = LabExit::from_outcome(c.outcome).ok_or(EngineError::Cancelled)?;
    let stderr = String::from_utf8_lossy(&c.stderr).into_owned();
    Ok(LabOutput {
        run: prepared,
        outcome: c.outcome,
        exit,
        stdout: String::from_utf8_lossy(&c.stdout).into_owned(),
        stderr,
        stderr_bytes: c.stderr,
        duration_ms: started.elapsed().as_millis() as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch_exe(dir: &Path, name: &str) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let p = dir.join(name);
        std::fs::write(&p, b"").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        p
    }

    fn jekyll(inputs: Vec<(&str, PathBuf)>, out: PathBuf) -> LabRun {
        LabRun {
            mode: mode("jekyll").unwrap(),
            inputs: inputs.into_iter().map(|(f, p)| (f.into(), p)).collect(),
            extras: Vec::new(),
            out,
        }
    }

    /// A canonical scratch root, so expectations compare against what
    /// `prepare` produces even where the temp dir is a symlink.
    fn scratch() -> (tempfile::TempDir, PathBuf) {
        let t = tempfile::tempdir().unwrap();
        let c = std::fs::canonicalize(t.path()).unwrap();
        (t, c)
    }

    #[test]
    fn locator_looks_for_the_lab_binary_in_boris_order() {
        let t = tempfile::tempdir().unwrap();
        let (b, pa, s) = (t.path().join("b"), t.path().join("p"), t.path().join("s"));
        let loc = Locator {
            env_override: None,
            bundled_dirs: vec![b.clone()],
            path_var: Some(std::env::join_paths([&pa]).unwrap()),
            sibling_dirs: vec![s.clone()],
        };
        // A Boris binary in a higher slot is not a lab.
        touch_exe(&b, &crate::locate::exe_name());
        let sib = touch_exe(&s, &exe_name());
        assert_eq!(find(&loc).unwrap(), (EngineSource::SiblingCheckout, sib));
        let on_path = touch_exe(&pa, &exe_name());
        assert_eq!(find(&loc).unwrap(), (EngineSource::Path, on_path));
        let bundled = touch_exe(&b, &exe_name());
        assert_eq!(
            find(&loc).unwrap(),
            (EngineSource::Bundled, bundled.clone())
        );
        let env = touch_exe(&t.path().join("env"), "any-name");
        let loc = Locator {
            env_override: Some(env.clone()),
            ..loc
        };
        assert_eq!(find(&loc).unwrap(), (EngineSource::EnvOverride, env));
    }

    #[test]
    fn absence_is_a_typed_error() {
        let t = tempfile::tempdir().unwrap();
        let loc = Locator {
            path_var: Some(std::env::join_paths([t.path()]).unwrap()),
            ..Locator::default()
        };
        match find(&loc) {
            Err(LabError::NotFound { searched }) => {
                assert_eq!(searched, vec![t.path().join(exe_name())]);
            }
            other => panic!("{other:?}"),
        }
        touch_exe(t.path(), &exe_name());
        let missing = Locator {
            env_override: Some(t.path().join("nope")),
            ..loc
        };
        assert!(matches!(
            find(&missing),
            Err(LabError::OverrideMissing(p)) if p == t.path().join("nope")
        ));
    }

    #[test]
    fn sibling_checkout_is_opt_in() {
        let root = Path::new("work").join("site");
        assert!(sibling_dirs(&root, false).is_empty());
        assert_eq!(
            sibling_dirs(&root, true),
            vec![
                Path::new("work")
                    .join("boris-migration-lab")
                    .join("zig-out")
                    .join("bin")
            ]
        );
    }

    #[test]
    fn argv_is_mode_then_inputs_in_table_order_then_out() {
        let (_t, root) = scratch();
        let (wxr, media) = (root.join("export.xml"), root.join("media"));
        std::fs::write(&wxr, b"<rss/>").unwrap();
        std::fs::create_dir(&media).unwrap();
        let run = LabRun {
            mode: mode("wordpress").unwrap(),
            inputs: vec![
                ("--media".into(), media.clone()),
                ("--wxr".into(), wxr.clone()),
            ],
            extras: Vec::new(),
            out: root.join("out"),
        };
        let p = run.prepare().unwrap();
        let want: Vec<OsString> = vec![
            "--mode=wordpress".into(),
            flag_value("--wxr", &wxr),
            flag_value("--media", &media),
            flag_value("--out", &root.join("out")),
        ];
        assert_eq!(p.argv, want);
        assert_eq!(p.cwd, root);
        // The optional input may be left out.
        let bare = LabRun {
            inputs: vec![("--wxr".into(), wxr)],
            ..run
        };
        assert_eq!(bare.prepare().unwrap().argv.len(), 3);
    }

    #[test]
    fn flags_outside_the_mode_allowlist_are_refused() {
        let (_t, root) = scratch();
        let out = root.join("out");
        for flag in [
            "--wxr", "--out", "--mode", "--boris", "-q", "--root=x", "root",
        ] {
            let run = jekyll(vec![(flag, root.clone())], out.clone());
            assert!(
                matches!(
                    run.prepare(),
                    Err(LabError::FlagNotAllowed { mode: "jekyll", .. })
                ),
                "{flag}"
            );
        }
    }

    #[test]
    fn required_inputs_are_present_once_and_of_the_right_kind() {
        let (_t, root) = scratch();
        let src = root.join("src");
        std::fs::create_dir(&src).unwrap();
        let out = root.join("out");
        assert!(matches!(
            jekyll(vec![], out.clone()).prepare(),
            Err(LabError::MissingInput {
                mode: "jekyll",
                flag: "--root"
            })
        ));
        assert!(matches!(
            jekyll(
                vec![("--root", src.clone()), ("--root", src.clone())],
                out.clone()
            )
            .prepare(),
            Err(LabError::DuplicateInput { flag: "--root" })
        ));
        let file = root.join("f.txt");
        std::fs::write(&file, b"x").unwrap();
        assert!(matches!(
            jekyll(vec![("--root", file)], out.clone()).prepare(),
            Err(LabError::InputKind {
                expected: InputKind::Dir,
                ..
            })
        ));
        assert!(matches!(
            jekyll(vec![("--root", root.join("absent"))], out).prepare(),
            Err(LabError::InputPath { flag: "--root", .. })
        ));
    }

    #[test]
    fn out_may_not_overlap_an_input() {
        let (_t, root) = scratch();
        let src = root.join("src");
        std::fs::create_dir(&src).unwrap();
        for out in [src.join("out"), src.clone(), root.clone()] {
            assert!(
                matches!(
                    jekyll(vec![("--root", src.clone())], out.clone()).prepare(),
                    Err(LabError::Overlap { flag: "--root", .. })
                ),
                "{out:?}"
            );
        }
        // A sibling whose name merely starts with the input's is fine.
        assert!(
            jekyll(vec![("--root", src)], root.join("src-out"))
                .prepare()
                .is_ok()
        );
    }

    #[test]
    fn publication_stage_and_backup_siblings_may_not_overlap_inputs() {
        let (_t, root) = scratch();
        let out = root.join("out");
        let media = root.join("media");
        std::fs::create_dir(&media).unwrap();

        for suffix in [".boris-migration-lab-stage", ".boris-migration-lab-prev"] {
            let sibling = root.join(format!("out{suffix}"));
            let descendant = sibling.join("site");
            std::fs::create_dir_all(&descendant).unwrap();
            for input in [sibling.clone(), descendant] {
                assert!(
                    matches!(
                        jekyll(vec![("--root", input.clone())], out.clone()).prepare(),
                        Err(LabError::Overlap { flag: "--root", .. })
                    ),
                    "{input:?}"
                );
            }

            let wxr = sibling.join("nested").join("export.xml");
            std::fs::create_dir_all(wxr.parent().unwrap()).unwrap();
            std::fs::write(&wxr, b"<rss/>").unwrap();
            let run = LabRun {
                mode: mode("wordpress").unwrap(),
                inputs: vec![("--wxr".into(), wxr), ("--media".into(), media.clone())],
                extras: Vec::new(),
                out: out.clone(),
            };
            assert!(
                matches!(run.prepare(), Err(LabError::Overlap { flag: "--wxr", .. })),
                "{suffix}"
            );
        }
    }

    #[test]
    fn an_existing_output_is_never_the_child_cwd() {
        let (_t, root) = scratch();
        let src = root.join("src");
        std::fs::create_dir(&src).unwrap();
        let out = root.join("out");
        std::fs::create_dir(&out).unwrap();

        let existing = jekyll(vec![("--root", src.clone())], out.clone())
            .prepare()
            .unwrap();
        assert_eq!(existing.out, out);
        assert_eq!(existing.cwd, root);

        let absent = jekyll(vec![("--root", src)], existing.out.join("missing"))
            .prepare()
            .unwrap();
        assert_eq!(absent.cwd, existing.out);
    }

    #[test]
    fn out_must_be_a_plain_directory_path() {
        let (_t, root) = scratch();
        let src = root.join("src");
        std::fs::create_dir(&src).unwrap();
        let mut dotdot_raw = root.as_os_str().to_os_string();
        for part in ["a", "..", "out"] {
            dotdot_raw.push(std::path::MAIN_SEPARATOR_STR);
            dotdot_raw.push(part);
        }
        let dotdot = PathBuf::from(dotdot_raw);
        assert!(matches!(
            jekyll(vec![("--root", src.clone())], dotdot).prepare(),
            Err(LabError::OutPath { .. })
        ));
        let file = root.join("file");
        std::fs::write(&file, b"x").unwrap();
        for out in [file.clone(), file.join("below")] {
            assert!(
                matches!(
                    jekyll(vec![("--root", src.clone())], out.clone()).prepare(),
                    Err(LabError::OutPath { .. })
                ),
                "{out:?}"
            );
        }
    }

    /// The macOS `/tmp` → `/private/tmp` case, reproduced with a link we
    /// own so it runs on every Unix.
    #[cfg(unix)]
    #[test]
    fn symlinked_inputs_and_outputs_are_refused_never_flattened() {
        let (_t, root) = scratch();
        let real = root.join("real");
        std::fs::create_dir_all(real.join("site")).unwrap();
        let link = root.join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        // A symlinked input component is a pre-spawn refusal naming the
        // component; the lab never runs and nothing is flattened.
        let err = jekyll(
            vec![("--root", link.join("site"))],
            root.join("imports").join("run-1"),
        )
        .prepare()
        .unwrap_err();
        let LabError::InputSymlink { component, .. } = err else {
            panic!("expected InputSymlink, got {err:?}");
        };
        assert_eq!(component, link);

        // The symlinked final component of a file input is equally refused.
        let export = real.join("export.xml");
        std::fs::write(&export, "<rss/>").unwrap();
        let file_link = root.join("link.xml");
        std::os::unix::fs::symlink(&export, &file_link).unwrap();
        let run = LabRun {
            mode: mode("wordpress").unwrap(),
            inputs: vec![("--wxr".into(), file_link.clone())],
            extras: Vec::new(),
            out: root.join("imports").join("run-2"),
        };
        assert!(
            matches!(run.prepare(), Err(LabError::InputSymlink { .. })),
            "a symlinked file input must be refused, not followed"
        );

        // A symlink in the output spelling is refused before any
        // canonicalization can follow it into the target.
        let err = jekyll(
            vec![("--root", real.join("site"))],
            link.join("imports").join("run-1"),
        )
        .prepare()
        .unwrap_err();
        assert!(
            matches!(err, LabError::OutPath { .. }),
            "symlinked output must be an OutPath refusal, got {err:?}"
        );

        // A dangling link in the out path is still an output refusal.
        std::os::unix::fs::symlink(root.join("gone"), root.join("dangling")).unwrap();
        assert!(matches!(
            jekyll(
                vec![("--root", real.join("site"))],
                root.join("dangling").join("x")
            )
            .prepare(),
            Err(LabError::OutPath { .. })
        ));

        // Honest paths keep their own spelling in argv and the record: the
        // adapter validates but does not rewrite them.
        let p = jekyll(
            vec![("--root", real.join("site"))],
            real.join("imports").join("run-1"),
        )
        .prepare()
        .unwrap();
        assert_eq!(p.inputs, vec![("--root", real.join("site"))]);
        assert!(p.argv.iter().any(|a| {
            a.to_string_lossy()
                .contains(real.join("site").to_str().unwrap())
        }));
    }

    #[cfg(any(unix, target_os = "macos"))]
    #[test]
    fn root_level_system_aliases_are_the_only_normalized_symlinks() {
        // The narrow platform policy: exactly the root-level system alias
        // component is rewritten, everything else stays verbatim.
        for alias in ["/tmp", "/var", "/etc"] {
            let aliased = std::path::Path::new(alias);
            let normalized = super::normalize_system_aliases(&aliased.join("x"));
            if std::fs::symlink_metadata(aliased)
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false)
            {
                assert_ne!(
                    normalized,
                    aliased.join("x"),
                    "{alias} is a symlink here and must be normalized"
                );
                assert!(normalized.ends_with("x"));
                // The rest of the spelling survives untouched.
                let deep = super::normalize_system_aliases(&aliased.join("a/b/c"));
                assert!(deep.ends_with("a/b/c"), "{deep:?}");
            } else {
                assert_eq!(normalized, aliased.join("x"));
            }
        }
        // A symlink deeper than the root-level alias is never rewritten.
        let (_t, root) = scratch();
        let real = root.join("real");
        std::fs::create_dir_all(&real).unwrap();
        let link = root.join("link");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, &link).unwrap();
        #[cfg(unix)]
        assert_eq!(
            super::normalize_system_aliases(&link.join("x")),
            link.join("x")
        );
    }

    #[test]
    fn exit_codes_map_to_lab_classes() {
        for (code, exit) in [
            (0, LabExit::Ok),
            (2, LabExit::UsageError),
            (3, LabExit::IoFailure),
            // Not a lab code: never read as a content verdict.
            (1, LabExit::IoFailure),
            (137, LabExit::IoFailure),
        ] {
            assert_eq!(
                LabExit::from_outcome(ExitOutcome::Exited { code }),
                Some(exit),
                "{code}"
            );
        }
        assert_eq!(
            LabExit::from_outcome(ExitOutcome::Signaled { signal: 9 }),
            Some(LabExit::IoFailure)
        );
        assert_eq!(LabExit::from_outcome(ExitOutcome::Killed), None);
    }

    #[test]
    fn imports_dir_takes_one_plain_segment() {
        let ws = Path::new("ws");
        assert_eq!(
            imports_dir(ws, "20261125-jekyll").unwrap(),
            ws.join(".cv").join("imports").join("20261125-jekyll")
        );
        for bad in ["", ".", "..", "a/b", "a\\b", "/abs"] {
            assert!(
                matches!(imports_dir(ws, bad), Err(LabError::RunId(_))),
                "{bad}"
            );
        }
    }

    #[test]
    fn extras_are_type_validated_and_passed_through_verbatim() {
        let t = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(t.path()).unwrap();
        let input = root.join("site");
        std::fs::create_dir_all(&input).unwrap();
        let mut run = LabRun {
            mode: mode("starlight").unwrap(),
            inputs: vec![("--root".into(), input)],
            extras: vec![
                ("--locale".into(), "en".into()),
                ("--max-pages".into(), "12".into()),
            ],
            out: root.join("out"),
        };
        let prepared = run.prepare().unwrap();
        let argv: Vec<String> = prepared
            .argv
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(argv.contains(&"--locale=en".to_string()), "{argv:?}");
        assert!(argv.contains(&"--max-pages=12".to_string()), "{argv:?}");

        // Non-numeric count: refused before the spawn.
        run.extras = vec![("--max-pages".into(), "abc".into())];
        assert!(matches!(
            run.prepare(),
            Err(LabError::ExtraValue {
                flag: "--max-pages",
                ..
            })
        ));

        // Flags outside the mode's allowlist are refused: process-changing
        // options like --boris/--gate can never pass through.
        run.extras = vec![("--boris".into(), "/tmp/x".into())];
        assert!(matches!(
            run.prepare(),
            Err(LabError::FlagNotAllowed { .. })
        ));
        run.extras = vec![
            ("--locale".into(), "en".into()),
            ("--locale".into(), "de".into()),
        ];
        assert!(matches!(
            run.prepare(),
            Err(LabError::DuplicateInput { .. })
        ));
    }

    #[test]
    fn relative_input_spellings_are_refused_before_any_cwd_resolution() {
        let t = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(t.path()).unwrap();
        let mut run = LabRun {
            mode: mode("jekyll").unwrap(),
            inputs: vec![("--root".into(), PathBuf::from("site"))],
            extras: Vec::new(),
            out: root.join("out"),
        };
        // A bare relative spelling must not resolve against process cwd.
        assert!(matches!(
            run.prepare(),
            Err(LabError::InputRelative { flag: "--root", .. })
        ));
        // `~/site` is not shell-expanded either: it is not absolute here.
        run.inputs = vec![("--root".into(), PathBuf::from("~/site"))];
        assert!(matches!(
            run.prepare(),
            Err(LabError::InputRelative { flag: "--root", .. })
        ));
    }
}
