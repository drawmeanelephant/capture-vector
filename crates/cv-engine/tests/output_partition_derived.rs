//! Derived-output and non-profile input authority partition regressions.

mod output_partition_support;

use cv_core::engine::{Engine, ExitClass};
use cv_engine::{EngineConfig, ProcessEngine};
use output_partition_support::{
    copy_tree, custom, partition_engine, partition_error, write_custom_profile, write_input_profile,
};
use std::path::Path;

#[cfg(unix)]
#[test]
#[ignore = "needs a real pinned boris via CV_BORIS_BIN"]
fn real_boris_theme_and_static_symlink_aliases_refuse_before_source_bytes_move() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/corpus-src/happy")
        .canonicalize()
        .unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("ws");
    std::fs::create_dir_all(&root).unwrap();
    copy_tree(&fixture.join("content"), &root.join("content"));
    copy_tree(&fixture.join("themes/boris"), &root.join("theme-real"));
    std::fs::create_dir_all(root.join("static-real")).unwrap();
    let root = root.canonicalize().unwrap();
    let theme_sentinel = root.join("theme-real/layouts/main.html");
    let static_sentinel = root.join("static-real/sentinel.txt");
    let theme_bytes = std::fs::read(&theme_sentinel).unwrap();
    std::fs::write(&static_sentinel, "STATIC-SENTINEL\n").unwrap();
    std::os::unix::fs::symlink("theme-real", root.join("theme-link")).unwrap();
    std::os::unix::fs::symlink("static-real", root.join("static-link")).unwrap();
    std::fs::write(
        root.join("boris.json"),
        r#"{
  "format": "boris-publication-profile", "schema_version": 1, "input": "content",
  "targets": [
    { "name": "a", "output": "theme-real", "theme": "theme-link" },
    { "name": "b", "output": "static-real", "theme": "theme-link", "static": { "dir": "static-link" } }
  ]
}"#,
    )
    .unwrap();
    let engine = ProcessEngine::start(EngineConfig::new(&root)).unwrap();
    let verb = custom("build", &["--profile", "boris.json"]);
    let err = engine.run(&verb, Box::new(|_| {})).unwrap_err().to_string();
    assert!(
        err.contains("theme footer file") || err.contains("static input tree"),
        "{err}"
    );
    assert_eq!(std::fs::read(&theme_sentinel).unwrap(), theme_bytes);
    assert_eq!(
        std::fs::read_to_string(&static_sentinel).unwrap(),
        "STATIC-SENTINEL\n"
    );
}

#[test]
fn derived_stage_roots_are_output_authority() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("content")).unwrap();
    cv_store::workspace_instance::ensure(&root).unwrap();
    write_input_profile(&root, Some("content"), "outstraße");
    let engine = partition_engine(&root, "content");

    let verb = custom(
        "build",
        &[
            "--input",
            "OUTSTRASSE.boris-stage",
            "--profile",
            "boris.json",
        ],
    );
    let err = partition_error(&engine, &verb).expect("partition refusal");
    assert!(
        err.contains("derived stage output") && err.contains("workspace content input"),
        "stage aliased with the effective input must refuse pre-spawn: {err}"
    );

    std::fs::write(
        root.join("boris.json"),
        r#"{
  "format": "boris-publication-profile", "schema_version": 1, "input": "content",
  "targets": [
    { "name": "a", "output": "OUTSTRASSE.boris-stage", "theme": "themes/boris" },
    { "name": "z", "output": "outstraße", "theme": "themes/boris" }
  ]
}"#,
    )
    .unwrap();
    let verb = custom("build", &["--profile", "boris.json"]);
    let err = engine.run(&verb, Box::new(|_| {})).unwrap_err().to_string();
    assert!(
        err.contains("physically overlap"),
        "one target's output aliasing another target's stage must refuse: {err}"
    );
}

#[test]
fn non_profile_derived_stage_roots_are_output_authority() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("content")).unwrap();
    cv_store::workspace_instance::ensure(&root).unwrap();
    let engine = partition_engine(&root, "content");

    let verb = custom(
        "build",
        &[
            "--input",
            "OUTSTRASSE.boris-stage",
            "--html-dir",
            "outstraße",
        ],
    );
    let err = engine.run(&verb, Box::new(|_| {})).unwrap_err().to_string();
    assert!(
        err.contains("derived stage output") && err.contains("workspace content input"),
        "cli stage aliased with effective input must refuse pre-spawn: {err}"
    );

    let verb = custom(
        "build",
        &[
            "--target",
            "a=OUTSTRASSE.boris-stage",
            "--target",
            "z=outstraße",
        ],
    );
    let err = engine.run(&verb, Box::new(|_| {})).unwrap_err().to_string();
    assert!(
        err.contains("HTML output roots") && err.contains("physically overlap"),
        "cli target output aliasing another target stage must refuse: {err}"
    );
}

#[test]
fn non_profile_cli_theme_layout_rule_and_static_inputs_are_protected() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("content")).unwrap();
    cv_store::workspace_instance::ensure(&root).unwrap();
    let engine = partition_engine(&root, "content");
    for (args, expected) in [
        (
            vec![
                "--theme",
                "straße-theme",
                "--html-dir",
                "STRASSE-THEME/layouts",
            ],
            "layout",
        ),
        (
            vec![
                "--html-layout",
                "cli-theme/layouts/main.html",
                "--html-dir",
                "cli-theme/layouts",
            ],
            "layout",
        ),
        (
            vec![
                "--target-layout",
                "default=cli-theme/layouts/main.html",
                "--html-dir",
                "cli-theme/layouts",
            ],
            "layout",
        ),
        (
            vec![
                "--theme",
                "rule-theme",
                "--layout-rule",
                "default",
                "id:index",
                "rule-theme/layouts/alt.html",
                "--html-dir",
                "rule-theme/layouts",
            ],
            "layout",
        ),
        (
            vec!["--static-dir", "static-in", "--html-dir", "static-in"],
            "static input tree",
        ),
    ] {
        let verb = custom("build", &args);
        let err = engine.run(&verb, Box::new(|_| {})).unwrap_err().to_string();
        assert!(
            err.contains(expected),
            "{args:?} must refuse through {expected}: {err}"
        );
    }
}

#[test]
fn derived_theme_siblings_are_not_over_refused_but_read_surfaces_are_protected() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("content")).unwrap();
    cv_store::workspace_instance::ensure(&root).unwrap();
    let engine = partition_engine(&root, "content");

    let verb = custom(
        "build",
        &[
            "--html-layout",
            "straße-derived/layouts/main.html",
            "--html-dir",
            "straße-derived/out",
        ],
    );
    let err = engine.run(&verb, Box::new(|_| {})).unwrap_err().to_string();
    assert!(
        !err.contains("overlaps input authority"),
        "sibling output inside a derived theme root must not be over-refused: {err}"
    );

    write_custom_profile(
        &root,
        r#"{ "name": "public", "output": "derived/out", "public": true,
            "layout": "derived/layouts/main.html" }"#,
    );
    let verb = custom("build", &["--profile", "boris.json"]);
    let err = engine.run(&verb, Box::new(|_| {})).unwrap_err().to_string();
    assert!(
        !err.contains("overlaps input authority"),
        "profile sibling output inside a derived theme root must not be over-refused: {err}"
    );

    write_custom_profile(
        &root,
        r#"{ "name": "public", "output": "themes/boris/assets", "public": true,
            "theme": "themes/boris" }"#,
    );
    let err = engine.run(&verb, Box::new(|_| {})).unwrap_err().to_string();
    assert!(
        err.contains("theme assets tree"),
        "theme assets remain protected: {err}"
    );

    write_custom_profile(
        &root,
        r#"{ "name": "public", "output": "themes/boris/footer.html", "public": true,
            "theme": "themes/boris" }"#,
    );
    let err = engine.run(&verb, Box::new(|_| {})).unwrap_err().to_string();
    assert!(
        err.contains("theme footer file"),
        "theme footer remains protected: {err}"
    );
}

#[cfg(unix)]
#[test]
#[ignore = "needs a real pinned boris via CV_BORIS_BIN"]
fn real_boris_stage_input_alias_refuses_before_source_bytes_move() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/corpus-src/happy")
        .canonicalize()
        .unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("ws");
    copy_tree(&fixture, &root);
    let root = root.canonicalize().unwrap();
    write_input_profile(&root, Some("content"), "outstraße");
    let sentinel = root.join("OUTSTRASSE.boris-stage/sentinel.txt");
    std::fs::create_dir_all(sentinel.parent().unwrap()).unwrap();
    std::fs::write(&sentinel, "STAGE-INPUT-SENTINEL\n").unwrap();
    let engine = ProcessEngine::start(EngineConfig::new(&root)).unwrap();
    let verb = custom(
        "build",
        &[
            "--input",
            "OUTSTRASSE.boris-stage",
            "--profile",
            "boris.json",
        ],
    );
    let err = engine.run(&verb, Box::new(|_| {})).unwrap_err().to_string();
    assert!(
        err.contains("derived stage output") && err.contains("workspace content input"),
        "{err}"
    );
    assert_eq!(
        std::fs::read_to_string(&sentinel).unwrap(),
        "STAGE-INPUT-SENTINEL\n"
    );
}

#[cfg(unix)]
#[test]
#[ignore = "needs a real pinned boris via CV_BORIS_BIN"]
fn real_boris_cli_theme_alias_refuses_before_layout_bytes_move() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/corpus-src/happy")
        .canonicalize()
        .unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("ws");
    copy_tree(&fixture, &root);
    copy_tree(&fixture.join("themes/boris"), &root.join("straße-theme"));
    let root = root.canonicalize().unwrap();
    let layout = root.join("straße-theme/layouts/main.html");
    let layout_bytes = std::fs::read(&layout).unwrap();
    let engine = ProcessEngine::start(EngineConfig::new(&root)).unwrap();
    let verb = custom(
        "build",
        &[
            "--theme",
            "straße-theme",
            "--html-dir",
            "STRASSE-THEME/layouts",
        ],
    );
    let err = engine.run(&verb, Box::new(|_| {})).unwrap_err().to_string();
    assert!(err.contains("layout"), "{err}");
    assert_eq!(std::fs::read(&layout).unwrap(), layout_bytes);
}

#[test]
#[ignore = "needs a real pinned boris via CV_BORIS_BIN"]
fn real_boris_derived_theme_sibling_output_builds_and_preserves_boundary() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/corpus-src/happy")
        .canonicalize()
        .unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("ws");
    copy_tree(&fixture, &root);
    copy_tree(&fixture.join("themes/boris"), &root.join("derived-theme"));
    let root = root.canonicalize().unwrap();
    std::fs::write(root.join("derived-theme/footer.html"), "FOOTER-MARKER").unwrap();
    std::fs::write(root.join("derived-theme/arb sibling.txt"), "ARBITRARY").unwrap();
    let engine = ProcessEngine::start(EngineConfig::new(&root)).unwrap();
    let verb = custom(
        "build",
        &[
            "--html-layout",
            "derived-theme/layouts/main.html",
            "--html-dir",
            "derived-theme/out",
        ],
    );
    assert_eq!(engine.run(&verb, Box::new(|_| {})).unwrap(), ExitClass::Ok);
    let html = std::fs::read_to_string(root.join("derived-theme/out/index.html")).unwrap();
    assert!(html.contains("FOOTER-MARKER"));
    assert!(root.join("derived-theme/out/assets/css/boris.css").exists());
    assert!(!root.join("derived-theme/out/arb sibling.txt").exists());
    assert_eq!(
        std::fs::read_to_string(root.join("derived-theme/arb sibling.txt")).unwrap(),
        "ARBITRARY"
    );
}
