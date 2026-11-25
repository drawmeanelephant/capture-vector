//! Live checks against a REAL pinned `boris-migration-lab`. Ignored by
//! default; run with
//! `CV_MIGRATION_LAB_BIN=/path/to/boris-migration-lab cargo test -p cv-engine --test lab_live -- --ignored`.
//! The binary must sit in its checkout (`<checkout>/zig-out/bin/`), which
//! is where `scripts/fetch-migration-lab.sh` builds it: the fixtures come
//! from `<checkout>/fixtures/`. The pin-skew check also needs
//! `CV_BORIS_BIN` pointing at the pinned Boris.

use cv_core::engine::ExitClass;
use cv_engine::lab::{self, LabExit, LabIdentity, LabRun, PINNED};
use cv_engine::locate::{self, Locator};
use cv_engine::proc::CancelToken;
use cv_engine::verbs::run_argv;
use std::path::{Path, PathBuf};

fn pinned_lab() -> LabIdentity {
    let cwd = std::env::temp_dir();
    lab::locate(&lab::locator_from_env(&cwd), &cwd, PINNED).expect("pinned lab")
}

fn lab_fixture(lab: &LabIdentity, rel: &str) -> PathBuf {
    let checkout = lab
        .path
        .ancestors()
        .nth(3)
        .expect("binary under <checkout>/zig-out/bin");
    let p = checkout.join("fixtures").join(rel);
    assert!(p.exists(), "lab fixture missing: {p:?}");
    p
}

fn run(lab: &LabIdentity, mode: &str, flag: &str, fixture: &str, out: PathBuf) -> lab::LabOutput {
    let run = LabRun {
        mode: lab::mode(mode).unwrap(),
        inputs: vec![(flag.into(), lab_fixture(lab, fixture))],
        extras: Vec::new(),
        out,
    };
    lab::run(lab, &run, &CancelToken::new()).unwrap()
}

#[test]
#[ignore = "needs the pinned lab via CV_MIGRATION_LAB_BIN"]
fn live_pinned_lab_converts_one_fixture_into_a_temp_dir() {
    let lab = pinned_lab();
    // Deliberately not canonicalized: on macOS this is under /var → /private/var,
    // which the lab refuses unless the adapter resolves it.
    let tmp = tempfile::tempdir().unwrap();
    let done = run(
        &lab,
        "jekyll",
        "--root",
        "mini-jekyll",
        tmp.path().join("imports").join("run-1"),
    );
    eprintln!("argv: {:?}\nstderr: {}", done.run.argv, done.stderr);
    assert_eq!(done.exit, LabExit::Ok, "{}", done.stderr);
    for f in ["content", "report.json", "REPORT.md"] {
        assert!(done.run.out.join(f).exists(), "{f}");
    }
    assert!(done.stderr.contains("wrote"), "{}", done.stderr);
}

/// Probes recorded against the real pinned lab at
/// ce40d78a0cd20f3e36550c7a85391903672bcb5a: symlinked spellings are refused
/// with a stable error name on stderr, and nothing is written. The exit
/// code varies by mode (2 for mode-specific input validation, 3 once
/// validateRoots runs) — the error NAME is the stable refusal signal,
/// which is why the adapter keys refusal detection on it instead of the
/// exit code. WordPress probes use `mini-wxr` so validateRoots itself fires.
#[test]
#[ignore = "needs the pinned lab via CV_MIGRATION_LAB_BIN"]
fn live_pinned_lab_refuses_symlinked_input_and_output() {
    let lab = pinned_lab();
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let wxr = lab_fixture(&lab, "mini-wxr/export.xml");

    // Symlinked input spelling: InputSymlink refusal, nothing written.
    let link_in = root.join("link-export.xml");
    std::os::unix::fs::symlink(&wxr, &link_in).unwrap();
    let out1 = root.join("out-input");
    let output = std::process::Command::new(&lab.path)
        .arg("--mode=wordpress")
        .arg(format!("--wxr={}", link_in.display()))
        .arg(format!("--out={}", out1.display()))
        .output()
        .unwrap();
    let code = output.status.code().unwrap();
    assert!(code == 2 || code == 3, "exit {code}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("InputSymlink"), "{stderr}");
    assert!(!out1.exists());

    // Symlinked output spelling: OutputSymlink refusal, nothing written —
    // including through the symlink target.
    let real_parent = root.join("real-parent");
    std::fs::create_dir_all(&real_parent).unwrap();
    let link_parent = root.join("link-parent");
    std::os::unix::fs::symlink(&real_parent, &link_parent).unwrap();
    let output = std::process::Command::new(&lab.path)
        .arg("--mode=wordpress")
        .arg(format!("--wxr={}", wxr.display()))
        .arg(format!("--out={}", link_parent.join("out2").display()))
        .output()
        .unwrap();
    let code = output.status.code().unwrap();
    assert!(code == 2 || code == 3, "exit {code}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("OutputSymlink"), "{stderr}");
    assert!(!real_parent.join("out2").exists());
}

#[test]
#[ignore = "needs the pinned lab via CV_MIGRATION_LAB_BIN"]
fn live_help_documents_every_allowlisted_mode_and_flag() {
    let lab = pinned_lab();
    let help = lab::help(&lab, &std::env::temp_dir(), &CancelToken::new()).unwrap();
    let modes_line = help
        .lines()
        .find(|l| l.trim_start().starts_with("--mode=MODE"))
        .expect("--mode line");
    let listed: Vec<&str> = modes_line
        .split_once("MODE")
        .unwrap()
        .1
        .split('|')
        .map(|m| m.split_whitespace().next().unwrap_or_default())
        .collect();
    for m in lab::MODES {
        assert!(
            listed.contains(&m.name),
            "--help does not list mode {}",
            m.name
        );
        for s in m.inputs {
            assert!(
                help.contains(&format!("{}=", s.flag)),
                "--help does not document {} (mode {})",
                s.flag,
                m.name
            );
        }
    }
}

/// Emit modes whose fixture output the pinned Boris compiles clean.
const CLEAN: &[(&str, &str, &str)] = &[
    ("jekyll", "--root", "mini-jekyll"),
    ("hugo", "--root", "mini-hugo"),
    ("substack", "--root", "mini-substack"),
    ("mkdocs", "--root", "mini-mkdocs"),
    ("enex", "--root", "mini-enex"),
    ("ghost", "--root", "mini-ghost"),
    ("mastodon", "--root", "mini-mastodon"),
    ("atproto", "--root", "mini-atproto"),
    ("wordpress", "--wxr", "mini-wxr/export.xml"),
    ("filed", "--filed-root", "mini-filed"),
];

/// Emit modes whose fixture output fails Boris on graph or route checks
/// (`EREFERENCE*`, `EPARENTMISSING`, `EROUTEMISSING`), never on
/// frontmatter. Recorded at the pins in `fixtures/MANIFEST.toml`.
const GRAPH_FAILURES: &[(&str, &str, &str)] = &[
    ("obsidian", "--vault", "mini-obsidian"),
    ("notion", "--export", "mini-notion"),
    ("instagram", "--dump", "mini-instagram"),
    ("starlight", "--root", "mini-starlight"),
];

/// Pin skew: the lab's parser gate pins Boris main `2eb2c915`, Capture
/// Vector pins Boris 0.9.0. Each emit mode's output goes into a
/// `boris init` scaffold and through `validate` and `build`.
#[test]
#[ignore = "needs the pinned lab via CV_MIGRATION_LAB_BIN and boris via CV_BORIS_BIN"]
fn live_emit_output_is_accepted_by_the_pinned_boris() {
    let lab = pinned_lab();
    let tmp = tempfile::tempdir().unwrap();
    let boris = Locator::from_env(tmp.path())
        .locate(tmp.path(), locate::PINNED)
        .expect("pinned boris");
    let never = CancelToken::new();
    let mut failures = Vec::new();
    for (set, clean) in [(CLEAN, true), (GRAPH_FAILURES, false)] {
        for &(mode, flag, fixture) in set {
            let done = run(&lab, mode, flag, fixture, tmp.path().join(mode));
            assert_eq!(done.exit, LabExit::Ok, "{mode}: {}", done.stderr);
            let site = tmp.path().join(format!("site-{mode}"));
            let init = run_argv(
                &boris.path,
                tmp.path(),
                &["init".into(), site.to_string_lossy().into_owned()],
                &never,
            )
            .unwrap();
            assert_eq!(init.outcome.class(), Some(ExitClass::Ok), "{}", init.stderr);
            std::fs::remove_dir_all(site.join("content")).unwrap();
            copy_tree(&done.run.out.join("content"), &site.join("content"));
            for verb in [&["validate"][..], &["build", "--profile", "boris.json"][..]] {
                let args: Vec<String> = verb.iter().map(|s| s.to_string()).collect();
                let v = run_argv(&boris.path, &site, &args, &never).unwrap();
                let codes = diagnostic_codes(&v.stderr);
                eprintln!("{mode} {verb:?}: {:?} {codes:?}", v.outcome);
                let class = v.outcome.class();
                let ok = if clean {
                    class == Some(ExitClass::Ok)
                } else {
                    class == Some(ExitClass::ContentFailure)
                        && !codes.is_empty()
                        && !codes.contains(&"EFRONTMATTER")
                };
                if !ok {
                    failures.push(format!("{mode} {verb:?}: {:?}\n{}", v.outcome, v.stderr));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// `E*` codes from `error: ECODE: …` lines. Route errors carry no column,
/// so `StderrDiagnostic` does not parse them; only the code matters here.
fn diagnostic_codes(stderr: &str) -> Vec<&str> {
    stderr
        .lines()
        .filter_map(|l| l.strip_prefix("error: ")?.split_once(':'))
        .map(|(code, _)| code)
        .filter(|c| c.len() > 1 && c.starts_with('E') && c.bytes().all(|b| b.is_ascii_uppercase()))
        .collect()
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let dst = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_tree(&e.path(), &dst);
        } else {
            std::fs::copy(e.path(), dst).unwrap();
        }
    }
}
