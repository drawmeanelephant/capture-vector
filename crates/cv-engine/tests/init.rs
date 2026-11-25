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
        std::fs::canonicalize(fake.root()).unwrap().join("site")
    );
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
