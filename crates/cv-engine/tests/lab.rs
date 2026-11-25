//! `cv_engine::lab` against a stub lab: locate, version gate, the argv it
//! actually receives, exit mapping, and cancellation. The stub is a POSIX
//! script, so these run on Unix only.
#![cfg(unix)]

use cv_core::engine::EngineSource;
use cv_engine::error::EngineError;
use cv_engine::exit::ExitOutcome;
use cv_engine::lab::{self, LabError, LabExit, LabIdentity, LabRun, PINNED};
use cv_engine::locate::Locator;
use cv_engine::proc::CancelToken;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const STUB: &str = include_str!("fake-lab/boris-migration-lab");

struct FakeLab {
    dir: tempfile::TempDir,
    bin: PathBuf,
}

impl FakeLab {
    fn new(version: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join(lab::exe_name());
        std::fs::write(&bin, STUB).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let f = Self { dir, bin };
        f.set("version.txt", &format!("{version}\n"));
        f
    }

    fn set(&self, rel: &str, s: &str) {
        std::fs::write(self.dir.path().join(rel), s).unwrap();
    }

    fn read(&self, rel: &str) -> Option<String> {
        std::fs::read_to_string(self.dir.path().join(rel)).ok()
    }

    fn identity(&self) -> LabIdentity {
        lab::gate(
            EngineSource::EnvOverride,
            &self.bin,
            self.dir.path(),
            PINNED,
        )
        .unwrap()
    }
}

/// A workspace whose paths are reached through a symlink, the way macOS
/// reaches `/tmp` and `$TMPDIR`.
struct Linked {
    _t: tempfile::TempDir,
    real: PathBuf,
    link: PathBuf,
}

fn linked() -> Linked {
    let t = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(t.path()).unwrap();
    let real = root.join("private-tmp");
    std::fs::create_dir_all(real.join("site")).unwrap();
    let link = root.join("tmp");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    Linked { _t: t, real, link }
}

fn jekyll(root: &Path, out: PathBuf) -> LabRun {
    LabRun {
        mode: lab::mode("jekyll").unwrap(),
        inputs: vec![("--root".into(), root.to_path_buf())],
        out,
    }
}

#[test]
fn located_on_path_and_gated_on_the_pinned_tool_id() {
    let fake = FakeLab::new(PINNED.tool_id);
    let loc = Locator {
        path_var: Some(std::env::join_paths([fake.dir.path()]).unwrap()),
        ..Locator::default()
    };
    let id = lab::locate(&loc, fake.dir.path(), PINNED).unwrap();
    assert_eq!(id.source, EngineSource::Path);
    assert_eq!(id.path, fake.bin);
    assert_eq!(id.tool_id, "boris-migration-lab/0.8.1");
    assert_eq!(fake.read("argv.log").unwrap(), "--version\n");
}

#[test]
fn another_tool_id_is_a_typed_mismatch() {
    for found in ["boris-migration-lab/0.8.0", "boris/0.9.0", ""] {
        let fake = FakeLab::new(found);
        match lab::gate(EngineSource::Bundled, &fake.bin, fake.dir.path(), PINNED) {
            Err(LabError::VersionMismatch {
                found: f,
                expected,
                source_kind: EngineSource::Bundled,
                ..
            }) => {
                assert_eq!(f, found);
                assert_eq!(expected, PINNED.tool_id);
            }
            other => panic!("{found}: {other:?}"),
        }
    }
}

#[test]
fn a_failing_version_probe_does_not_pass_the_gate() {
    let fake = FakeLab::new(PINNED.tool_id);
    // Valid stdout, but the probe exits 3.
    std::fs::write(
        &fake.bin,
        format!("#!/bin/sh\necho {}\nexit 3\n", PINNED.tool_id),
    )
    .unwrap();
    assert!(matches!(
        lab::gate(EngineSource::Path, &fake.bin, fake.dir.path(), PINNED),
        Err(LabError::Engine(EngineError::Abnormal(
            ExitOutcome::Exited { code: 3 }
        )))
    ));
}

#[test]
fn the_lab_receives_canonical_paths_and_runs_beside_its_output() {
    let fake = FakeLab::new(PINNED.tool_id);
    let ws = linked();
    let out = ws.link.join(".cv").join("imports").join("run-1");
    let done = lab::run(
        &fake.identity(),
        &jekyll(&ws.link.join("site"), out),
        &CancelToken::new(),
    )
    .unwrap();
    let want_out = ws.real.join(".cv").join("imports").join("run-1");
    assert_eq!(
        fake.read("argv.log").unwrap(),
        format!(
            "--mode=jekyll\n--root={}\n--out={}\n",
            ws.real.join("site").display(),
            want_out.display()
        )
    );
    assert_eq!(
        fake.read("cwd.log").unwrap().trim(),
        ws.real.to_str().unwrap()
    );
    assert_eq!(done.run.out, want_out);
    assert_eq!(done.exit, LabExit::Ok);
}

#[test]
fn publication_rerun_into_existing_output_runs_from_output_parent() {
    let fake = FakeLab::new(PINNED.tool_id);
    std::fs::create_dir_all(fake.dir.path().join("replay")).unwrap();
    fake.set(
        "replay/.boris-migration-lab-output",
        "format=boris-migration-lab-output\nowner=boris-jekyll-migration-lab\nschema_version=1\n",
    );
    let id = fake.identity();
    let ws = linked();
    let out = ws.real.join("publication");

    for _ in 0..2 {
        let done = lab::run(
            &id,
            &jekyll(&ws.real.join("site"), out.clone()),
            &CancelToken::new(),
        )
        .unwrap();
        assert_eq!(done.exit, LabExit::Ok);
    }

    assert_eq!(
        fake.read("cwd.log").unwrap().trim(),
        ws.real.to_str().unwrap()
    );
}

#[test]
fn exit_codes_map_and_stderr_is_kept_verbatim() {
    let fake = FakeLab::new(PINNED.tool_id);
    let id = fake.identity();
    let ws = linked();
    let stderr = "error: migration-lab (jekyll) failed: OutputSymlink\n";
    fake.set("stderr.txt", stderr);
    for (code, exit) in [
        ("0", LabExit::Ok),
        ("2", LabExit::UsageError),
        ("3", LabExit::IoFailure),
        ("1", LabExit::IoFailure),
    ] {
        fake.set("exit-code.txt", code);
        let done = lab::run(
            &id,
            &jekyll(&ws.real.join("site"), ws.real.join("out")),
            &CancelToken::new(),
        )
        .unwrap();
        assert_eq!(done.exit, exit, "exit {code}");
        assert_eq!(done.stderr, stderr);
    }
}

#[test]
fn a_refused_run_never_spawns_the_lab() {
    let fake = FakeLab::new(PINNED.tool_id);
    let id = fake.identity();
    std::fs::remove_file(fake.dir.path().join("argv.log")).unwrap();
    let ws = linked();
    let err = lab::run(
        &id,
        &jekyll(&ws.link.join("site"), ws.link.join("site").join("out")),
        &CancelToken::new(),
    )
    .unwrap_err();
    assert!(matches!(err, LabError::Overlap { .. }), "{err:?}");
    assert!(fake.read("argv.log").is_none());
}

#[test]
fn cancellation_kills_the_lab_process_tree() {
    let fake = FakeLab::new(PINNED.tool_id);
    let id = fake.identity();
    fake.set("hold", "");
    let ws = linked();
    let cancel = CancelToken::new();
    let c2 = cancel.clone();
    let t = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        c2.cancel();
    });
    let start = Instant::now();
    let r = lab::run(
        &id,
        &jekyll(&ws.real.join("site"), ws.real.join("out")),
        &cancel,
    );
    t.join().unwrap();
    assert!(
        matches!(r, Err(LabError::Engine(EngineError::Cancelled))),
        "{r:?}"
    );
    // The pipes close only once the backgrounded grandchild is dead too.
    assert!(start.elapsed() < Duration::from_secs(10));
}

#[test]
fn the_lab_child_env_is_the_engine_allowlist() {
    // SAFETY: name is unique to this test; no other test reads it.
    unsafe { std::env::set_var("CV_TEST_LAB_UNLISTED", "shouldnotleak") };
    let fake = FakeLab::new(PINNED.tool_id);
    fake.identity();
    let env = fake.read("env.log").unwrap();
    assert!(!env.contains("shouldnotleak"));
    assert!(env.lines().any(|l| l.starts_with("PATH=")));
}
