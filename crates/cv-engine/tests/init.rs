//! `boris init` through cv-engine: the archetype list comes from the
//! captured `init --help`, and creation runs `Verb::Init` from the
//! target's parent.

mod common;

use common::FakeBoris;
use cv_core::engine::ExitClass;
use cv_engine::init::{Initializer, parse_help};
use cv_engine::locate::{Locator, PINNED};
use cv_engine::proc::CancelToken;
use cv_fixtures::StubEngine;

#[test]
fn the_captured_help_lists_every_archetype_with_docs_as_the_default() {
    let s = parse_help(&StubEngine::init_help());
    assert_eq!(s.default.as_deref(), Some("docs"));
    assert_eq!(s.archetypes.first().map(String::as_str), Some("docs"));
    for a in [
        "garden",
        "cookbook",
        "link-in-bio",
        "resume",
        "photo-gallery",
    ] {
        assert!(
            s.archetypes.iter().any(|n| n == a),
            "{a}: {:?}",
            s.archetypes
        );
    }
    assert_eq!(s.archetypes.len(), 15, "{:?}", s.archetypes);
}

fn initializer(fake: &FakeBoris) -> Initializer {
    let locator = Locator {
        env_override: Some(fake.bin.clone()),
        ..Locator::default()
    };
    Initializer::locate(&locator, fake.root(), PINNED).expect("fake boris passes the gate")
}

#[test]
fn starters_come_from_the_located_boris() {
    let fake = FakeBoris::new();
    fake.set("verbs/init/stderr.txt", &StubEngine::init_help());
    let s = initializer(&fake)
        .starters(fake.root(), &CancelToken::new())
        .unwrap();
    assert_eq!(s.archetypes.len(), 15);
    assert!(fake.read("argv.log").contains("init --help"));
}

#[test]
fn create_runs_init_from_the_parent_with_the_folder_name() {
    let fake = FakeBoris::new();
    fake.set("verbs/init/stderr.txt", &StubEngine::init_report("docs"));
    let target = fake.root().join("site");
    let out = initializer(&fake)
        .create(&target, "docs", &CancelToken::new())
        .unwrap();
    assert_eq!(out.output.outcome.class(), Some(ExitClass::Ok));
    assert!(out.output.stderr.contains("verified: starter compiled"));
    assert_eq!(
        out.target,
        cv_engine::paths::canonical_display(fake.root())
            .unwrap()
            .join("site")
    );
    #[cfg(windows)]
    assert!(!out.target.to_string_lossy().starts_with(r"\\?\"));
    assert!(
        fake.read("argv.log")
            .lines()
            .any(|l| l == "init site --type docs"),
        "{}",
        fake.read("argv.log")
    );
    let missing = fake.root().join("nowhere").join("site");
    assert!(
        initializer(&fake)
            .create(&missing, "docs", &CancelToken::new())
            .is_err()
    );
}

fn copy_tree(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).unwrap();
    for e in std::fs::read_dir(src).unwrap() {
        let e = e.unwrap();
        let to = dst.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_tree(&e.path(), &to);
        } else {
            std::fs::copy(e.path(), to).unwrap();
        }
    }
}

#[test]
fn a_refusal_replays_before_any_mutation_of_an_occupied_target() {
    let fake = FakeBoris::new();
    copy_tree(
        &cv_fixtures::fixtures_root().join("captures/init/docs/tree"),
        &fake.path("verbs/init/tree"),
    );
    fake.set("verbs/init/stderr.txt", &StubEngine::init_report("refuse"));
    fake.set("verbs/init/exit-code.txt", "2");
    let target = fake.root().join("site");
    std::fs::create_dir(&target).unwrap();
    std::fs::write(target.join("sentinel.txt"), "keep me").unwrap();

    let out = initializer(&fake)
        .create(&target, "docs", &CancelToken::new())
        .unwrap();
    assert_eq!(out.output.outcome.class(), Some(ExitClass::UsageError));
    assert!(out.output.stderr.contains("not empty"));
    assert_eq!(
        std::fs::read_to_string(target.join("sentinel.txt")).unwrap(),
        "keep me"
    );
    assert!(
        !target.join("boris.json").exists(),
        "the shim copied a starter into a target Boris refused: {:?}",
        std::fs::read_dir(&target)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect::<Vec<_>>()
    );
}

#[cfg(unix)]
#[test]
fn create_refuses_a_symlinked_target_without_spawning_boris() {
    let fake = FakeBoris::new();
    fake.set("verbs/init/stderr.txt", &StubEngine::init_report("docs"));
    let outside = fake.root().join("outside");
    std::fs::create_dir(&outside).unwrap();
    let link = fake.root().join("site-link");
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    let initializer = initializer(&fake);
    let before = fake.read("argv.log");
    let e = initializer
        .create(&link, "docs", &CancelToken::new())
        .unwrap_err();
    assert!(e.to_string().contains("symlink"), "{e}");
    assert_eq!(fake.read("argv.log"), before, "locating Boris is unchanged");
    assert!(
        !fake.read("argv.log").lines().any(|l| l.starts_with("init")),
        "no init spawn after refusal"
    );
}
