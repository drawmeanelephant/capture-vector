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
pub use modes::{InputKind, InputSpec, MODES, ModeSpec, mode};
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use std::time::Instant;
use thiserror::Error;

pub const ENV_OVERRIDE: &str = "CV_MIGRATION_LAB_BIN";
/// Opt-in for the sibling-checkout candidate (`1` enables it).
pub const ENV_SIBLING: &str = "CV_MIGRATION_LAB_SIBLING";
const CHECKOUT: &str = "boris-migration-lab";

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
    #[error("--out {path:?} {reason}")]
    OutPath { path: PathBuf, reason: String },
    #[error("--out {out:?} overlaps {flag} {input:?}")]
    Overlap {
        flag: &'static str,
        input: PathBuf,
        out: PathBuf,
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
    /// The deepest existing ancestor of `out` (canonical).
    pub cwd: PathBuf,
}

impl LabRun {
    /// Check the run against the allowlist and canonicalize every path.
    /// The lab refuses symlinked paths (`OutputSymlink`, exit 2), and on
    /// macOS `/tmp` and `$TMPDIR` are symlinks, so nothing reaches argv
    /// uncanonicalized.
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
            let canonical = std::fs::canonicalize(path).map_err(|source| LabError::InputPath {
                flag: spec.flag,
                path: path.clone(),
                source,
            })?;
            let kind_ok = match spec.kind {
                InputKind::Dir => canonical.is_dir(),
                InputKind::File => canonical.is_file(),
            };
            if !kind_ok {
                return Err(LabError::InputKind {
                    flag: spec.flag,
                    path: canonical,
                    expected: spec.kind,
                });
            }
            given.push((spec, canonical));
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
        for (flag, input) in &inputs {
            if out.starts_with(input) || input.starts_with(&out) {
                return Err(LabError::Overlap {
                    flag,
                    input: input.clone(),
                    out,
                });
            }
        }

        let mut argv = vec![OsString::from(format!("--mode={}", mode.name))];
        argv.extend(inputs.iter().map(|(flag, p)| flag_value(flag, p)));
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

fn flag_value(flag: &str, path: &Path) -> OsString {
    let mut s = OsString::from(flag);
    s.push("=");
    s.push(path.as_os_str());
    s
}

/// Canonicalize the deepest existing ancestor and re-append the rest.
/// Returns `(out, that ancestor)`.
fn canonical_out(out: &Path) -> Result<(PathBuf, PathBuf), LabError> {
    let refuse = |reason: String| LabError::OutPath {
        path: out.to_path_buf(),
        reason,
    };
    if out.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(refuse("must not contain `..`".into()));
    }
    let abs = std::path::absolute(out).map_err(|e| refuse(e.to_string()))?;
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
    let canonical = missing.iter().rev().fold(base.clone(), |p, c| p.join(c));
    Ok((canonical, base))
}

/// The lab's exit codes. It has no content-failure code (`1`): refusals
/// such as `OutputSymlink` and `InputOutputOverlap` exit 2.
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

/// A finished run. `stderr` is the lab's evidence, kept verbatim; with
/// `-q` off it holds the progress lines too.
#[derive(Debug, Clone)]
pub struct LabOutput {
    pub run: PreparedRun,
    pub outcome: ExitOutcome,
    pub exit: LabExit,
    pub stdout: String,
    pub stderr: String,
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
    Ok(LabOutput {
        run: prepared,
        outcome: c.outcome,
        exit,
        stdout: String::from_utf8_lossy(&c.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&c.stderr).into_owned(),
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
    fn symlinked_inputs_and_out_are_canonicalized() {
        let (_t, root) = scratch();
        let real = root.join("real");
        std::fs::create_dir_all(real.join("site")).unwrap();
        let link = root.join("tmp");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let p = jekyll(
            vec![("--root", link.join("site"))],
            link.join("imports").join("run-1"),
        )
        .prepare()
        .unwrap();
        assert_eq!(p.inputs, vec![("--root", real.join("site"))]);
        assert_eq!(p.out, real.join("imports").join("run-1"));
        assert_eq!(p.cwd, real);
        let link_str = link.to_str().unwrap();
        assert!(
            p.argv.iter().all(
                |a| !Path::new(a).starts_with(&link) && !a.to_string_lossy().contains(link_str)
            ),
            "{:?}",
            p.argv
        );
        // A dangling link in the out path cannot be canonicalized.
        std::os::unix::fs::symlink(root.join("gone"), root.join("dangling")).unwrap();
        assert!(matches!(
            jekyll(
                vec![("--root", real.join("site"))],
                root.join("dangling").join("x")
            )
            .prepare(),
            Err(LabError::OutPath { .. })
        ));
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
}
