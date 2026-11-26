//! One recorded lab run, shared by `examples/capture_lab.rs` (which writes
//! `fixtures/captures/lab/`) and `tests/lab_conformance.rs` (which checks
//! a fresh run against those captures). Both go through `lab::run`, so the
//! captured `argv.txt` is exactly what `PreparedRun` builds.

use cv_core::engine::ExitClass;
use cv_engine::exit::ExitOutcome;
use cv_engine::lab::{self, LabIdentity, LabRun};
use cv_engine::locate::{self, EngineIdentity, Locator};
use cv_engine::proc::CancelToken;
use cv_engine::verbs::run_argv;
use cv_fixtures::lab::{
    LabCaptureSpec, LabInput, LabTranscript, copy_tree, site_dir, stage_fixture,
};
use std::error::Error;
use std::path::{Path, PathBuf};

pub type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// Overrides where the lab's input fixtures live. By default they are the
/// `fixtures/` of the checkout the binary was built in
/// (`<checkout>/zig-out/bin/boris-migration-lab`).
pub const ENV_LAB_FIXTURES: &str = "CV_MIGRATION_LAB_FIXTURES";

/// The pinned lab, the pinned Boris, and the lab checkout's fixtures.
pub struct Tools {
    pub lab: LabIdentity,
    pub boris: EngineIdentity,
    pub lab_fixtures: PathBuf,
}

impl Tools {
    pub fn from_env(cwd: &Path) -> Result<Self> {
        let lab = lab::locate(&lab::locator_from_env(cwd), cwd, lab::PINNED)?;
        let boris = Locator::from_env(cwd).locate(cwd, locate::PINNED)?;
        let lab_fixtures = match std::env::var_os(ENV_LAB_FIXTURES) {
            Some(p) => PathBuf::from(p),
            None => lab
                .path
                .ancestors()
                .nth(3)
                .ok_or("lab binary is not under <checkout>/zig-out/bin")?
                .join("fixtures"),
        };
        if !lab_fixtures.is_dir() {
            return Err(format!("lab fixtures not found at {lab_fixtures:?}").into());
        }
        Ok(Self {
            lab,
            boris,
            lab_fixtures,
        })
    }
}

/// Run one capture under `root`, which must be canonical. Captures that
/// share `root` share staged inputs, and a [`LabInput::BorisHtmlOf`]
/// input reads an earlier capture's output there.
pub fn capture(tools: &Tools, spec: &LabCaptureSpec, root: &Path) -> Result<LabTranscript> {
    let mut inputs = Vec::new();
    for &(flag, input) in spec.inputs {
        let path = match input {
            LabInput::Fixture(rel) => stage_fixture(&tools.lab_fixtures, rel, root)?,
            LabInput::BorisHtmlOf(mode) => boris_html(tools, root, mode)?,
        };
        inputs.push((flag.to_string(), path));
    }
    std::fs::create_dir_all(root.join("out"))?;
    let run = LabRun {
        mode: lab::mode(spec.mode).ok_or_else(|| format!("{} is not allowlisted", spec.mode))?,
        inputs,
        extras: Vec::new(),
        out: spec.out(root),
    };
    let done = lab::run(&tools.lab, &run, &CancelToken::new())?;
    let ExitOutcome::Exited { code } = done.outcome else {
        return Err(format!(
            "{}: lab ended {:?}\n{}",
            spec.mode, done.outcome, done.stderr
        )
        .into());
    };
    Ok(LabTranscript::observe(
        root,
        &done.run.argv,
        code,
        &done.stdout,
        &done.stderr,
        &done.run.out,
    )?)
}

/// `boris init`, swap in the capture's `content/`, then
/// `build --profile boris.json`. Built once per root.
fn boris_html(tools: &Tools, root: &Path, mode: &str) -> Result<PathBuf> {
    let site = site_dir(root, mode);
    let dist = site.join("dist");
    if dist.is_dir() {
        return Ok(dist);
    }
    let never = CancelToken::new();
    let boris = |cwd: &Path, args: &[&str]| -> Result<()> {
        let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        let out = run_argv(&tools.boris.path, cwd, &args, &never)?;
        if out.outcome.class() != Some(ExitClass::Ok) {
            return Err(format!("boris {args:?}: {:?}\n{}", out.outcome, out.stderr).into());
        }
        Ok(())
    };
    boris(root, &["init", &site.to_string_lossy()])?;
    std::fs::remove_dir_all(site.join("content"))?;
    let content = root.join("out").join(mode).join("content");
    copy_tree(&content, &site.join("content"))?;
    boris(&site, &["build", "--profile", "boris.json"])?;
    Ok(dist)
}
