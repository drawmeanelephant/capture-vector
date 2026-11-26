//! `StubLab ≡ ProcessLab`: for every captured mode, a run through
//! `cv_engine::lab::run` and a `StubLab` replay give the same transcript:
//! argv, exit code, stdout, stderr, and every byte under `--out`.
//!
//! - Over the fake lab replaying each capture (Unix, always on). This pins
//!   the adapter side: the capture plan still yields the captured argv
//!   through `PreparedRun`, and output is read back unchanged.
//! - Over the pinned lab (`#[ignore]`): a fresh capture equals the
//!   checked-in one byte for byte. Run with
//!   `CV_MIGRATION_LAB_BIN=… CV_BORIS_BIN=… cargo test -p cv-engine --test lab_conformance -- --ignored`.

mod common;

#[path = "common/lab_capture.rs"]
mod lab_capture;

use cv_fixtures::StubLab;
use cv_fixtures::lab::{LAB_CAPTURES, LabTranscript};

fn assert_same(mode: &str, stub: &LabTranscript, process: &LabTranscript) {
    assert_eq!(stub.argv, process.argv, "{mode}: argv");
    assert_eq!(stub.exit_code, process.exit_code, "{mode}: exit code");
    assert_eq!(stub.stdout, process.stdout, "{mode}: stdout");
    assert_eq!(stub.stderr, process.stderr, "{mode}: stderr");
    assert_eq!(
        stub.out.keys().collect::<Vec<_>>(),
        process.out.keys().collect::<Vec<_>>(),
        "{mode}: files under --out"
    );
    for (rel, bytes) in &stub.out {
        assert!(
            &process.out[rel] == bytes,
            "{mode}: {rel} differs\n--- stub\n{}\n--- process\n{}",
            String::from_utf8_lossy(bytes),
            String::from_utf8_lossy(&process.out[rel])
        );
    }
}

fn canonical_tempdir() -> (tempfile::TempDir, std::path::PathBuf) {
    let t = tempfile::tempdir().unwrap();
    let c = std::fs::canonicalize(t.path()).unwrap();
    (t, c)
}

#[cfg(unix)]
#[test]
fn stub_lab_equals_process_lab_over_a_replaying_fake() {
    use cv_core::engine::EngineSource;
    use cv_engine::exit::ExitOutcome;
    use cv_engine::lab::{self, InputKind, LabExit, LabRun, PINNED};
    use cv_engine::proc::CancelToken;

    for spec in LAB_CAPTURES {
        let captured = cv_fixtures::lab::lab_capture(spec.mode).transcript();
        let (_fake_dir, fake) = canonical_tempdir();
        let bin = fake.join(lab::exe_name());
        common::install_executable(&bin, include_str!("fake-lab/boris-migration-lab"));
        std::fs::write(fake.join("version.txt"), format!("{}\n", PINNED.tool_id)).unwrap();
        std::fs::write(fake.join("stdout.txt"), &captured.stdout).unwrap();
        std::fs::write(fake.join("stderr.txt"), &captured.stderr).unwrap();
        std::fs::write(fake.join("exit-code.txt"), captured.exit_code.to_string()).unwrap();
        for (rel, bytes) in &captured.out {
            let p = fake.join("replay").join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, bytes).unwrap();
        }
        let identity = lab::gate(EngineSource::EnvOverride, &bin, &fake, PINNED).unwrap();

        // The fake reads no input, but `prepare` checks each one exists
        // with the kind the mode declares.
        let (_root_dir, root) = canonical_tempdir();
        let mode = lab::mode(spec.mode).unwrap();
        let mut inputs = Vec::new();
        for &(flag, input) in spec.inputs {
            let path = input.staged(&root);
            let kind = mode.inputs.iter().find(|s| s.flag == flag).unwrap().kind;
            match kind {
                InputKind::Dir => std::fs::create_dir_all(&path).unwrap(),
                InputKind::File => {
                    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                    std::fs::write(&path, b"").unwrap();
                }
            }
            inputs.push((flag.to_string(), path));
        }
        std::fs::create_dir_all(root.join("out")).unwrap();
        let run = LabRun {
            mode,
            inputs,
            out: spec.out(&root),
        };
        let done = lab::run(&identity, &run, &CancelToken::new()).unwrap();
        assert_eq!(done.exit, LabExit::Ok, "{}", spec.mode);
        assert_eq!(done.run.cwd, root.join("out"), "{}", spec.mode);
        let ExitOutcome::Exited { code } = done.outcome else {
            panic!("{:?}", done.outcome)
        };
        let process = LabTranscript::observe(
            &root,
            &done.run.argv,
            code,
            &done.stdout,
            &done.stderr,
            &done.run.out,
        )
        .unwrap();

        let (_stub_dir, stub_out) = canonical_tempdir();
        let stub = StubLab::new().run(spec.mode, &stub_out).unwrap();
        assert_same(spec.mode, &stub, &process);
    }
}

#[test]
#[ignore = "needs the pinned lab via CV_MIGRATION_LAB_BIN and boris via CV_BORIS_BIN"]
fn live_stub_lab_equals_the_pinned_process_lab() {
    let (_scratch, root) = canonical_tempdir();
    let tools = lab_capture::Tools::from_env(&root).expect("pinned lab and boris");
    let (_stub_dir, stub_out) = canonical_tempdir();
    for spec in LAB_CAPTURES {
        let process = lab_capture::capture(&tools, spec, &root)
            .unwrap_or_else(|e| panic!("{}: {e}", spec.mode));
        let stub = StubLab::new()
            .run(spec.mode, &stub_out.join(spec.mode))
            .unwrap();
        assert_same(spec.mode, &stub, &process);
    }
}
