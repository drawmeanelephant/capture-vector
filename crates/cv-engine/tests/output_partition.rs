//! Caller-controlled output partition regressions: owned CV lanes pass,
//! source/profile/metadata overlaps refuse before Boris runs.

mod common;
mod output_partition_support;

use common::FakeBoris;
use cv_core::engine::{Engine, ExitClass, ExportKind, Verb};
use cv_engine::proc::CancelToken;
use cv_engine::secrets::MockSecretSource;
use cv_engine::{EngineConfig, ProcessEngine};
use output_partition_support::{
    copy_tree, custom, partition_engine, partition_error, write_custom_profile,
    write_happy_profile, write_input_profile,
};
use std::path::Path;

#[test]
fn production_publish_preflight_spellings_use_the_owned_publish_lane() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("content")).unwrap();
    cv_store::workspace_instance::ensure(&root).unwrap();
    let engine = partition_engine(&root, "content");
    let site = ".cv/publish/run-1/site";
    let ir = ".cv/publish/run-1/ir";
    for verb in [
        custom("build", &["--html-dir", site]),
        custom("validate", &["--html-dir", site]),
        custom("proof", &["verify", "--html-dir", site]),
        custom("build", &["--out", ir]),
        custom("publish", &["--target", "nostr-nip23", "--html-dir", site]),
        custom(
            "standard-site",
            &["verify", "--profile", "boris.json", "--dist", site],
        ),
        custom(
            "nostr",
            &[
                "publish",
                "--plan",
                ".cv/publish/run-1/nostr-plan.json",
                "--bundle",
                ".cv/publish/run-1/nostr-bundle.json",
            ],
        ),
        custom("build", &["--out", ".cv/ir"]),
        custom("build", &["--html-dir", ".cv/out/1"]),
    ] {
        let err =
            partition_error(&engine, &verb).expect("missing fake Boris must be the only error");
        assert!(
            !err.contains("overlaps"),
            "{verb:?} was partition-refused: {err}"
        );
    }
}

#[test]
fn production_publish_preflight_verbs_run_end_to_end_through_process_engine() {
    let fake = FakeBoris::new();
    std::fs::create_dir_all(fake.root().join("content")).unwrap();
    for verb in ["build", "validate", "check", "proof"] {
        fake.set(&format!("verbs/{verb}/exit-code.txt"), "0");
    }
    let engine = fake.engine();
    let site = ".cv/publish/run-1/site";
    let ir = ".cv/publish/run-1/ir";
    let verbs = [
        custom("build", &["--html-dir", site]),
        custom("validate", &["--html-dir", site]),
        custom("check", &[]),
        custom("proof", &["verify", "--html-dir", site]),
        custom("build", &["--out", ir]),
    ];
    for verb in &verbs {
        let exit = engine.run(verb, Box::new(|_| {})).unwrap();
        assert_eq!(exit, ExitClass::Ok, "{verb:?}");
    }
    let argv = fake.read("argv.log");
    for expected in [
        "build --html-dir .cv/publish/run-1/site",
        "validate --html-dir .cv/publish/run-1/site",
        "check",
        "proof verify --html-dir .cv/publish/run-1/site",
        "build --out .cv/publish/run-1/ir",
    ] {
        assert!(
            argv.lines().any(|l| l == expected),
            "missing {expected}: {argv}"
        );
    }
}

#[test]
fn typed_rag_export_cannot_replace_a_same_named_content_input() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("rag")).unwrap();
    std::fs::write(root.join("rag/index.md"), "user content\n").unwrap();
    cv_store::workspace_instance::ensure(&root).unwrap();
    let engine = partition_engine(&root, "rag");
    let err = engine
        .run(
            &Verb::Export {
                kind: ExportKind::Rag,
            },
            Box::new(|_| {}),
        )
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("overlaps the workspace content input"),
        "{err}"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("rag/index.md")).unwrap(),
        "user content\n"
    );
}

#[test]
fn unicode_and_case_aliases_cannot_bypass_the_content_partition() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("café")).unwrap();
    cv_store::workspace_instance::ensure(&root).unwrap();
    let engine = partition_engine(&root, "café");
    for out in ["cafe\u{301}", "CAFÉ", "./café"] {
        let verb = Verb::Ir {
            out: out.into(),
            input: None,
        };
        let err = engine.run(&verb, Box::new(|_| {})).unwrap_err().to_string();
        assert!(
            err.contains("overlaps the workspace content input"),
            "{out}: {err}"
        );
    }
    std::fs::create_dir_all(root.join("straße")).unwrap();
    let casefold_engine = partition_engine(&root, "straße");
    let verb = Verb::Ir {
        out: "STRASSE".into(),
        input: None,
    };
    let err = casefold_engine
        .run(&verb, Box::new(|_| {}))
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("overlaps the workspace content input"),
        "full case folding: {err}"
    );
}

#[test]
fn default_html_output_cannot_overlap_a_dist_content_input() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("dist")).unwrap();
    std::fs::write(root.join("dist/index.md"), "user content\n").unwrap();
    cv_store::workspace_instance::ensure(&root).unwrap();
    let engine = partition_engine(&root, "dist");
    for verb in [
        Verb::Build,
        custom("build", &[]),
        custom("build", &["--html"]),
    ] {
        let err = engine.run(&verb, Box::new(|_| {})).unwrap_err().to_string();
        assert!(
            err.contains("overlaps the workspace content input"),
            "{verb:?}: {err}"
        );
    }
    assert_eq!(
        std::fs::read_to_string(root.join("dist/index.md")).unwrap(),
        "user content\n"
    );
}

#[test]
fn invocation_input_overrides_resolve_the_partition_input() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("rag")).unwrap();
    std::fs::create_dir_all(root.join("dist")).unwrap();
    cv_store::workspace_instance::ensure(&root).unwrap();
    let engine = partition_engine(&root, "content");
    for verb in [
        custom("build", &["--input", "rag", "--out", "rag"]),
        custom("build", &["--input", "dist"]),
        Verb::Ir {
            out: "rag/graph".into(),
            input: Some("rag".into()),
        },
    ] {
        let err = engine.run(&verb, Box::new(|_| {})).unwrap_err().to_string();
        assert!(
            err.contains("overlaps the workspace content input"),
            "{verb:?}: {err}"
        );
    }
}

#[test]
fn output_influencing_flag_spellings_are_partition_validated() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("content")).unwrap();
    cv_store::workspace_instance::ensure(&root).unwrap();
    let engine = partition_engine(&root, "content");
    for verb in [
        custom("build", &["--llms", "--llms-path", "boris.json"]),
        custom("build", &["--target", "public=.cv"]),
        custom("build", &["--target=public=content"]),
        custom("build", &["--rag-dir", "content"]),
        custom("build", &["--context-dir", ".cv/imports"]),
        custom("build", &["--rss-path", "boris.json"]),
        custom("build", &["--sitemap-path", ".cv/workspace-mutation.lock"]),
        custom("build", &["--report-out", "content/report.json"]),
        custom("build", &["--report", "content/report.json"]),
        custom("build", &["--dist", "content"]),
    ] {
        let err = engine.run(&verb, Box::new(|_| {})).unwrap_err().to_string();
        assert!(err.contains("overlaps"), "{verb:?}: {err}");
    }
    assert!(!root.join("boris.json").exists());
    assert!(!root.join("content/report.json").exists());
}

#[test]
fn custom_init_validates_the_real_positional_target_after_flag_values() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("content")).unwrap();
    cv_store::workspace_instance::ensure(&root).unwrap();
    let engine = partition_engine(&root, "content");
    let outside = tempfile::tempdir().unwrap();
    let outside_target = outside.path().join("outside-target");
    for args in [
        vec!["--type", "docs", ".cv/evil"],
        vec!["--type=docs", ".cv/evil"],
        vec!["--quiet", "--type", "docs", ".boris/evil"],
        vec!["--type", "docs"],
    ] {
        let verb = custom("init", &args);
        let err = engine.run(&verb, Box::new(|_| {})).unwrap_err().to_string();
        assert!(err.contains("init target"), "{args:?}: {err}");
    }
    let absolute = outside_target.to_string_lossy().into_owned();
    let verb = custom("init", &["--type", "docs", &absolute]);
    let err = engine.run(&verb, Box::new(|_| {})).unwrap_err().to_string();
    assert!(err.contains("init target"), "{err}");
    assert!(!root.join(".cv/evil").exists());
    assert!(!root.join(".boris/evil").exists());
    assert!(!outside_target.exists());

    // Flag arity is real: `docs` is the archetype value, not a target. A
    // legitimate target reaches Boris argv validation rather than being
    // partition-refused merely because it followed --type.
    let verb = custom("init", &["--type", "docs", "site"]);
    let err = partition_error(&engine, &verb).expect("missing fake Boris");
    assert!(!err.contains("overlaps"), "{err}");
}

#[test]
#[ignore = "needs a real pinned boris via CV_BORIS_BIN"]
fn real_boris_custom_init_flag_first_targets_refuse_pre_spawn() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("content")).unwrap();
    let engine = ProcessEngine::start(EngineConfig::new(&root)).unwrap();
    let outside = tempfile::tempdir().unwrap();
    let absolute = outside.path().join("outside-target");
    for target in [
        ".cv/evil".to_string(),
        absolute.to_string_lossy().into_owned(),
    ] {
        let verb = custom("init", &["--type", "docs", &target]);
        let err = engine.run(&verb, Box::new(|_| {})).unwrap_err().to_string();
        assert!(err.contains("init target"), "{target}: {err}");
    }
    assert!(!root.join(".cv/evil").exists());
    assert!(!absolute.exists());
}

#[test]
fn sign_nostr_absolute_output_outside_the_workspace_fails_closed() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("content")).unwrap();
    cv_store::workspace_instance::ensure(&root).unwrap();
    let engine = partition_engine(&root, "content");
    let plan = root.join("plan.json");
    std::fs::write(&plan, "{}").unwrap();
    let outside = tempfile::tempdir().unwrap();
    let out = outside.path().join("bundle.json");
    let err = engine
        .sign_nostr(
            &plan,
            Some(&out),
            &MockSecretSource::new("nsec1fake"),
            &CancelToken::new(),
        )
        .unwrap_err()
        .to_string();
    assert!(err.contains("outside the workspace"), "{err}");
    assert!(!out.exists());

    // Absolute output inside an owned lane is the same authority as its
    // workspace-relative spelling and is not refused merely for being absolute.
    let inside = root.join(".cv/publish/run-1/bundle.json");
    let err = engine
        .sign_nostr(
            &plan,
            Some(&inside),
            &MockSecretSource::new("nsec1fake"),
            &CancelToken::new(),
        )
        .unwrap_err()
        .to_string();
    assert!(
        !err.contains("outside the workspace") && !err.contains("overlaps"),
        "{err}"
    );
}

#[test]
fn absolute_configured_and_invocation_inputs_compare_at_their_true_location() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("content")).unwrap();
    cv_store::workspace_instance::ensure(&root).unwrap();
    let absolute_input = root.join("content").to_string_lossy().into_owned();
    let engine = partition_engine(&root, &absolute_input);
    let err = engine
        .run(&Verb::Build, Box::new(|_| {}))
        .unwrap_err()
        .to_string();
    // Default dist is outside the true input and remains legitimate.
    assert!(!err.contains("overlaps"), "{err}");
    let verb = custom(
        "build",
        &["--input", &absolute_input, "--out", "content/graph"],
    );
    let err = engine.run(&verb, Box::new(|_| {})).unwrap_err().to_string();
    assert!(
        err.contains("overlaps the workspace content input"),
        "{err}"
    );

    // An absolute input outside the workspace is legitimate Boris authority:
    // workspace-relative output cannot overlap it merely because the strings
    // do not share a workspace-relative prefix.
    let external = tempfile::tempdir().unwrap();
    let external_input = external.path().join("external-content");
    std::fs::create_dir_all(&external_input).unwrap();
    let external_engine = partition_engine(
        &root,
        &external_input.canonicalize().unwrap().to_string_lossy(),
    );
    let err = external_engine
        .run(&Verb::Build, Box::new(|_| {}))
        .unwrap_err()
        .to_string();
    assert!(!err.contains("overlaps"), "{err}");
}

#[test]
fn typed_watch_startup_is_partition_validated_before_spawning() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("dist")).unwrap();
    cv_store::workspace_instance::ensure(&root).unwrap();
    let engine = partition_engine(&root, "dist");
    let err = match engine.start_watch(Box::new(|_| {})) {
        Ok(_) => panic!("watch overlapping its input must not start"),
        Err(e) => e.to_string(),
    };
    assert!(
        err.contains("overlaps the workspace content input"),
        "{err}"
    );
    assert!(
        !engine.watch_live(),
        "failed validation must not mark watch live"
    );
    assert!(!root.join("dist/index.html").exists());
}

#[cfg(unix)]
#[test]
fn owned_lane_symlinked_into_content_is_still_an_input_overlap() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("content")).unwrap();
    cv_store::workspace_instance::ensure(&root).unwrap();
    std::fs::create_dir_all(root.join(".cv")).unwrap();
    std::os::unix::fs::symlink(root.join("content"), root.join(".cv/out")).unwrap();
    let engine = partition_engine(&root, "content");
    let verb = custom("build", &["--html-dir", ".cv/out/site"]);
    let err = engine.run(&verb, Box::new(|_| {})).unwrap_err().to_string();
    assert!(
        err.contains("overlaps the workspace content input"),
        "{err}"
    );
}

#[cfg(unix)]
#[test]
fn typed_ir_staging_alias_to_its_transient_root_refuses_pre_spawn() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("content")).unwrap();
    cv_store::workspace_instance::ensure(&root).unwrap();
    std::fs::create_dir_all(root.join(".cv/ir.staging")).unwrap();
    std::os::unix::fs::symlink("ir.staging", root.join(".cv/ir.staging.boris-stage")).unwrap();
    let engine = partition_engine(&root, "content");
    let verb = Verb::Ir {
        out: ".cv/ir".into(),
        input: None,
    };
    let err = engine.run(&verb, Box::new(|_| {})).unwrap_err().to_string();
    assert!(
        err.contains("IR output") && err.contains("physically overlap"),
        "staged IR output aliasing its derived stage root must refuse: {err}"
    );
}

#[test]
fn root_input_html_stage_uses_the_same_permitted_output_lane_as_dist() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    cv_store::workspace_instance::ensure(&root).unwrap();
    let engine = partition_engine(&root, ".");
    let verb = custom("build", &["--html-dir", "dist"]);
    let err = partition_error(&engine, &verb).expect("missing fake Boris");
    assert!(
        !err.contains("overlaps the workspace content input"),
        "dist.boris-stage is the derived write lane for permitted dist output: {err}"
    );
}

#[test]
fn full_unicode_casefold_alias_matrix_cannot_bypass_the_content_partition() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("content")).unwrap();
    cv_store::workspace_instance::ensure(&root).unwrap();
    let equivalent = [
        ("café", "cafe\u{301}"),
        ("straße", "STRASSE"),
        ("STRASSE", "straße"),
        ("ẞtraße", "sstrasse"),
        ("σίσυφος", "ΣΊΣΥΦΟΣ"),
        ("ς", "σ"),
        ("σ", "ς"),
        ("ﬀerent", "FFERENT"),
        ("ǅungla", "ǆUNGLA"),
        ("ǆungla", "ǅUNGLA"),
        ("İstanbul", "i\u{307}stanbul"),
    ];
    for (input, out) in equivalent {
        let engine = partition_engine(&root, input);
        let verb = Verb::Ir {
            out: out.into(),
            input: None,
        };
        let err = engine.run(&verb, Box::new(|_| {})).unwrap_err().to_string();
        assert!(
            err.contains("overlaps the workspace content input"),
            "{input:?} vs {out:?}: {err}"
        );
    }

    // Turkish dotted capital I folds to `i` + combining dot, not plain `i`.
    // The comparison must be consistent rather than ASCII-insensitive.
    let engine = partition_engine(&root, "İstanbul");
    let verb = Verb::Ir {
        out: "istanbul".into(),
        input: None,
    };
    let err = partition_error(&engine, &verb).expect("missing fake Boris");
    assert!(!err.contains("overlaps"), "plain i must not equal İ: {err}");
}

#[test]
fn absolute_invocation_input_reaches_boris_byte_exactly() {
    let fake = FakeBoris::new();
    let external = tempfile::tempdir().unwrap();
    let leaf = if cfg!(windows) {
        "absolute-input"
    } else {
        "ümlaut and spaces"
    };
    let input_path = external.path().join(leaf);
    std::fs::create_dir_all(&input_path).unwrap();
    let input = cv_engine::paths::canonical_display(&input_path)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let mut cfg = fake.config();
    cfg.content_input = input.clone().into();
    let engine = ProcessEngine::start(cfg).unwrap();
    let verb = custom("validate", &["--input", &input]);
    let out = engine.run(&verb, Box::new(|_| {})).unwrap();
    assert_eq!(out, ExitClass::Ok);
    let argv = fake.read("argv.log");
    let unquoted = format!("--input {input}");
    let quoted = format!("--input \"{input}\"");
    let logged = if cfg!(windows) {
        argv.contains(&unquoted) || argv.contains(&quoted)
    } else {
        argv.contains(&unquoted)
    };
    assert!(logged, "{argv}");
}

#[cfg(unix)]
#[test]
fn dangling_parent_symlink_cannot_bootstrap_an_output_escape() {
    let base = tempfile::tempdir().unwrap();
    let root = base.path().join("ws");
    let outside = base.path().join("outside");
    std::fs::create_dir_all(root.join("content")).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    let root = root.canonicalize().unwrap();
    cv_store::workspace_instance::ensure(&root).unwrap();
    std::os::unix::fs::symlink("missing/../../outside", root.join("link")).unwrap();
    let engine = partition_engine(&root, "content");
    let verb = custom(
        "build",
        &["--html-dir", "missing", "--report", "link/report.json"],
    );
    let err = engine.run(&verb, Box::new(|_| {})).unwrap_err().to_string();
    assert!(err.contains("outside the workspace"), "{err}");
    assert!(!outside.join("report.json").exists());
    assert!(!root.join("missing").exists());

    // The stronger form names an absolute external content input. The report
    // is outside the workspace and therefore cannot be proven disjoint from
    // that input; it must fail closed just the same.
    let external_input = outside.join("input");
    std::fs::create_dir_all(&external_input).unwrap();
    let external_engine = partition_engine(&root, external_input.to_str().unwrap());
    let err = external_engine
        .run(&verb, Box::new(|_| {}))
        .unwrap_err()
        .to_string();
    assert!(err.contains("outside the workspace"), "{err}");
    assert!(!outside.join("report.json").exists());
}

#[cfg(unix)]
#[test]
fn contained_dangling_parent_symlink_spellings_are_not_false_escapes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("content")).unwrap();
    cv_store::workspace_instance::ensure(&root).unwrap();
    // missing/../inside collapses arithmetically to workspace child `inside`.
    std::os::unix::fs::symlink("missing/../inside", root.join("link")).unwrap();
    let engine = partition_engine(&root, "content");
    let verb = custom(
        "build",
        &["--html-dir", "missing", "--report", "link/report.json"],
    );
    let err = partition_error(&engine, &verb).expect("missing fake Boris");
    assert!(
        !err.contains("outside the workspace") && !err.contains("overlaps"),
        "contained dangling symlink arithmetic must not false-refuse: {err}"
    );
}

#[test]
fn contained_and_escaping_dotdot_output_spellings_are_distinguished() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("content")).unwrap();
    cv_store::workspace_instance::ensure(&root).unwrap();
    let engine = partition_engine(&root, "content");

    let contained = custom("build", &["--html-dir", "sub/../site"]);
    let err = partition_error(&engine, &contained).expect("missing fake Boris");
    assert!(
        !err.contains("overlaps") && !err.contains("outside the workspace"),
        "contained sub/../site must not be a phantom escape: {err}"
    );

    for raw in [
        "../outside",
        "../../outside",
        "sub/../../outside",
        "a/b/../../../outside",
    ] {
        let verb = custom("build", &["--html-dir", raw]);
        let err = engine.run(&verb, Box::new(|_| {})).unwrap_err().to_string();
        assert!(
            err.contains("parent traversal") || err.contains("outside the workspace"),
            "{raw}: {err}"
        );
    }
}

#[test]
fn profile_declared_target_outputs_are_partition_validated() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("content")).unwrap();
    cv_store::workspace_instance::ensure(&root).unwrap();
    let engine = partition_engine(&root, "content");

    write_happy_profile(&root, ".cv/imports");
    let verb = custom("build", &["--profile", "boris.json"]);
    let err = engine.run(&verb, Box::new(|_| {})).unwrap_err().to_string();
    assert!(
        err.contains("Capture Vector metadata") && err.contains("profile target output"),
        "{err}"
    );
    assert!(!root.join(".cv/imports/index.html").exists());

    write_happy_profile(&root, "dist");
    let err = partition_error(&engine, &verb).expect("missing fake Boris");
    assert!(
        !err.contains("overlaps") && !err.contains("profile target output"),
        "happy profile shape must pass partition validation: {err}"
    );

    std::fs::write(root.join("boris.json"), b"not json").unwrap();
    let err = engine.run(&verb, Box::new(|_| {})).unwrap_err().to_string();
    assert!(err.contains("cannot be parsed"), "{err}");
}

#[test]
fn llms_and_rss_path_exports_suppress_default_html_like_their_mode_flags() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("dist")).unwrap();
    cv_store::workspace_instance::ensure(&root).unwrap();
    let engine = partition_engine(&root, "content");
    for args in [
        vec!["--input", "dist", "--llms-path", "x.txt"],
        vec!["--input", "dist", "--rss-path", "x.xml"],
        vec!["--input", "dist", "--llms"],
        vec!["--input", "dist", "--rss"],
    ] {
        let verb = custom("build", &args);
        let err = partition_error(&engine, &verb).expect("missing fake Boris");
        assert!(
            !err.contains("overlaps"),
            "{args:?} is export-only in pinned Boris and must not be phantom-refused: {err}"
        );
    }
    let verb = custom("build", &["--input", "dist"]);
    let err = engine.run(&verb, Box::new(|_| {})).unwrap_err().to_string();
    assert!(
        err.contains("overlaps"),
        "plain HTML build must still refuse: {err}"
    );
}

#[cfg(unix)]
#[test]
#[ignore = "needs a real pinned boris via CV_BORIS_BIN"]
fn real_boris_dangling_parent_symlink_bootstrap_refuses_pre_spawn() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/corpus-src/happy")
        .canonicalize()
        .unwrap();
    let base = tempfile::tempdir().unwrap();
    let root = base.path().join("ws");
    let outside = base.path().join("outside");
    copy_tree(&fixture, &root);
    std::fs::create_dir_all(&outside).unwrap();
    let root = root.canonicalize().unwrap();
    std::os::unix::fs::symlink("missing/../../outside", root.join("link")).unwrap();
    let engine = ProcessEngine::start(EngineConfig::new(&root)).unwrap();
    let verb = custom(
        "build",
        &["--html-dir", "missing", "--report", "link/report.json"],
    );
    let err = engine.run(&verb, Box::new(|_| {})).unwrap_err().to_string();
    assert!(err.contains("outside the workspace"), "{err}");
    assert!(!outside.join("report.json").exists());
    assert!(!root.join("missing").exists());
}

#[test]
#[ignore = "needs a real pinned boris via CV_BORIS_BIN"]
fn real_boris_happy_profile_build_passes_and_malicious_profile_refuses() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/corpus-src/happy")
        .canonicalize()
        .unwrap();
    for output in ["dist", ".cv/imports"] {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("ws");
        copy_tree(&fixture, &root);
        let root = root.canonicalize().unwrap();
        write_happy_profile(&root, output);
        let engine = ProcessEngine::start(EngineConfig::new(&root)).unwrap();
        let verb = custom("build", &["--profile", "boris.json"]);
        let result = engine.run(&verb, Box::new(|_| {}));
        if output == "dist" {
            assert_eq!(result.unwrap(), ExitClass::Ok);
            assert!(root.join("dist/index.html").exists());
        } else {
            let err = result.unwrap_err().to_string();
            assert!(err.contains("profile target output"), "{err}");
            assert!(!root.join(".cv/imports/index.html").exists());
        }
    }
}

#[test]
fn profile_outputs_resolve_against_the_profile_files_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("content")).unwrap();
    cv_store::workspace_instance::ensure(&root).unwrap();
    std::fs::create_dir_all(root.join("profiles")).unwrap();
    write_happy_profile(&root.join("profiles"), ".cv/imports");
    std::fs::rename(
        root.join("profiles/boris.json"),
        root.join("profiles/p.json"),
    )
    .unwrap();
    let engine = partition_engine(&root, "content");
    let verb = custom("build", &["--profile", "profiles/p.json"]);
    let err = partition_error(&engine, &verb).expect("missing fake Boris");
    assert!(
        !err.contains("Capture Vector metadata"),
        "profiles/.cv/imports is ordinary workspace state, not the metadata lane: {err}"
    );

    write_happy_profile(&root.join(".cv"), "imports");
    std::fs::rename(root.join(".cv/boris.json"), root.join(".cv/p.json")).unwrap();
    let verb = custom("build", &["--profile", ".cv/p.json"]);
    let err = engine.run(&verb, Box::new(|_| {})).unwrap_err().to_string();
    assert!(
        err.contains("Capture Vector metadata") && err.contains("profile target output"),
        "{err}"
    );
}

#[cfg(unix)]
#[test]
fn existing_symlink_hidden_behind_bootstrapped_dotdot_is_re_resolved() {
    let base = tempfile::tempdir().unwrap();
    let root = base.path().join("ws");
    let outside = base.path().join("outside");
    std::fs::create_dir_all(root.join("content")).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    let root = root.canonicalize().unwrap();
    cv_store::workspace_instance::ensure(&root).unwrap();
    std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
    let engine = partition_engine(&root, "content");
    let verb = custom(
        "build",
        &[
            "--html-dir",
            "missing",
            "--report",
            "missing/../link/report.json",
        ],
    );
    let err = engine.run(&verb, Box::new(|_| {})).unwrap_err().to_string();
    assert!(err.contains("outside the workspace"), "{err}");
    assert!(!outside.join("report.json").exists());
    assert!(!root.join("missing").exists());
}

#[test]
fn profile_edition_and_target_nested_artifact_outputs_are_inspected() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("content")).unwrap();
    cv_store::workspace_instance::ensure(&root).unwrap();
    let engine = partition_engine(&root, "content");
    let write_shape = |edition_output: &str, artifact_path: &str| {
        std::fs::write(
            root.join("boris.json"),
            format!(
                r#"{{
  "format": "boris-publication-profile",
  "schema_version": 1,
  "input": "content",
  "site": {{ "title": "Profile Probe", "url": "https://example.com/", "description": "probe" }},
  "targets": [
    {{ "name": "public", "output": "dist", "public": true, "theme": "themes/boris",
       "llms": {{ "path": "{artifact_path}" }} }}
  ],
  "editions": {{ "ir": {{ "output": "{edition_output}" }} }}
}}"#
            ),
        )
        .unwrap();
    };
    let verb = custom("build", &["--profile", "boris.json"]);

    write_shape(".cv/imports", "llms.txt");
    let err = engine.run(&verb, Box::new(|_| {})).unwrap_err().to_string();
    assert!(
        err.contains("profile target output") && err.contains("Capture Vector metadata"),
        "edition output must be inspected: {err}"
    );

    write_shape(".boris", "../.cv/imports");
    let err = engine.run(&verb, Box::new(|_| {})).unwrap_err().to_string();
    assert!(
        err.contains("profile artifact output") && err.contains("Capture Vector metadata"),
        "target-relative artifact path must be arithmetically resolved: {err}"
    );

    write_shape(".boris", "llms.txt");
    let err = partition_error(&engine, &verb).expect("missing fake Boris");
    assert!(
        !err.contains("Capture Vector metadata"),
        "the contract shapes assumed by inspection must pass when clean: {err}"
    );
}

#[cfg(unix)]
#[test]
fn profile_input_symlink_alias_is_the_output_overlap_authority() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("source")).unwrap();
    cv_store::workspace_instance::ensure(&root).unwrap();
    std::os::unix::fs::symlink("source", root.join("inlink")).unwrap();
    write_input_profile(&root, Some("inlink"), "source");
    let engine = partition_engine(&root, "content");
    let verb = custom("build", &["--profile", "boris.json"]);
    let err = engine.run(&verb, Box::new(|_| {})).unwrap_err().to_string();
    assert!(
        err.contains("profile target output") && err.contains("workspace content input"),
        "canonical symlink input authority must refuse pre-spawn: {err}"
    );
}

#[test]
fn profile_input_unicode_alias_is_the_output_overlap_authority() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("content")).unwrap();
    cv_store::workspace_instance::ensure(&root).unwrap();
    write_input_profile(&root, Some("café"), "cafe\u{301}");
    let engine = partition_engine(&root, "content");
    let verb = custom("build", &["--profile", "boris.json"]);
    let err = engine.run(&verb, Box::new(|_| {})).unwrap_err().to_string();
    assert!(
        err.contains("profile target output") && err.contains("workspace content input"),
        "unicode-canonical input authority must refuse pre-spawn: {err}"
    );
}

#[test]
fn profile_input_precedence_matches_pinned_boris() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("content")).unwrap();
    std::fs::create_dir_all(root.join("profilein")).unwrap();
    cv_store::workspace_instance::ensure(&root).unwrap();
    let engine = partition_engine(&root, "content");

    write_input_profile(&root, Some("profilein"), "profilein/out");
    let overridden = custom("build", &["--profile", "boris.json", "--input", "content"]);
    let err = partition_error(&engine, &overridden).expect("missing fake Boris");
    assert!(
        !err.contains("workspace content input"),
        "CLI --input overrides the profile input in pinned Boris: {err}"
    );

    write_input_profile(&root, Some("profilein"), "content/out");
    let err = engine
        .run(&overridden, Box::new(|_| {}))
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("workspace content input"),
        "the overriding CLI input is the overlap authority: {err}"
    );
}

#[test]
fn omitted_profile_input_defaults_to_content_beside_the_profile() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("content")).unwrap();
    std::fs::create_dir_all(root.join("profiles")).unwrap();
    cv_store::workspace_instance::ensure(&root).unwrap();
    write_input_profile(&root.join("profiles"), None, "content/out");
    std::fs::rename(
        root.join("profiles/boris.json"),
        root.join("profiles/p.json"),
    )
    .unwrap();
    let engine = partition_engine(&root, "content");
    let verb = custom("build", &["--profile", "profiles/p.json"]);
    let err = engine.run(&verb, Box::new(|_| {})).unwrap_err().to_string();
    assert!(
        err.contains("profile target output") && err.contains("workspace content input"),
        "omitted input must mean profiles/content, not workspace content: {err}"
    );
}

#[cfg(unix)]
#[test]
#[ignore = "needs a real pinned boris via CV_BORIS_BIN"]
fn real_boris_profile_input_symlink_alias_refuses_before_source_bytes_move() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/corpus-src/happy")
        .canonicalize()
        .unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("ws");
    std::fs::create_dir_all(&root).unwrap();
    copy_tree(&fixture.join("content"), &root.join("source"));
    copy_tree(&fixture.join("themes"), &root.join("themes"));
    let root = root.canonicalize().unwrap();
    let sentinel = root.join("source/assets/css/boris.css");
    std::fs::create_dir_all(sentinel.parent().unwrap()).unwrap();
    std::fs::write(&sentinel, "AUTHOR-OWNED-SENTINEL\n").unwrap();
    std::os::unix::fs::symlink("source", root.join("inlink")).unwrap();
    write_input_profile(&root, Some("inlink"), "source");
    let engine = ProcessEngine::start(EngineConfig::new(&root)).unwrap();
    let verb = custom("build", &["--profile", "boris.json"]);
    let err = engine.run(&verb, Box::new(|_| {})).unwrap_err().to_string();
    assert!(
        err.contains("profile target output") && err.contains("workspace content input"),
        "{err}"
    );
    assert_eq!(
        std::fs::read_to_string(&sentinel).unwrap(),
        "AUTHOR-OWNED-SENTINEL\n"
    );
    assert!(!root.join("source/index.html").exists());
}

#[test]
fn profile_mode_relative_cli_input_resolves_beside_the_profile() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("profiles")).unwrap();
    cv_store::workspace_instance::ensure(&root).unwrap();
    write_input_profile(&root.join("profiles"), Some("missing"), "STRASSE");
    std::fs::rename(
        root.join("profiles/boris.json"),
        root.join("profiles/p.json"),
    )
    .unwrap();
    let engine = partition_engine(&root, "content");
    for spelling in [
        vec!["--input", "straße", "--profile", "profiles/p.json"],
        vec!["--profile", "profiles/p.json", "--input", "straße"],
        vec!["--profile", "profiles/p.json", "--input=straße"],
    ] {
        let verb = custom("build", &spelling);
        let err = engine.run(&verb, Box::new(|_| {})).unwrap_err().to_string();
        assert!(
            err.contains("workspace content input"),
            "{spelling:?} must resolve cli input beside the profile: {err}"
        );
    }
}

#[cfg(unix)]
#[test]
fn profile_mode_relative_cli_input_symlink_resolves_beside_the_profile() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("profiles/source")).unwrap();
    cv_store::workspace_instance::ensure(&root).unwrap();
    std::os::unix::fs::symlink("source", root.join("profiles/inlink")).unwrap();
    write_input_profile(&root.join("profiles"), Some("missing"), "source");
    std::fs::rename(
        root.join("profiles/boris.json"),
        root.join("profiles/p.json"),
    )
    .unwrap();
    let engine = partition_engine(&root, "content");
    let verb = custom(
        "build",
        &["--profile", "profiles/p.json", "--input", "inlink"],
    );
    let err = engine.run(&verb, Box::new(|_| {})).unwrap_err().to_string();
    assert!(
        err.contains("workspace content input"),
        "profile-local symlink input authority must refuse pre-spawn: {err}"
    );
}

#[test]
fn profile_theme_static_and_layout_inputs_are_protected() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("content")).unwrap();
    cv_store::workspace_instance::ensure(&root).unwrap();
    let engine = partition_engine(&root, "content");
    let verb = custom("build", &["--profile", "boris.json"]);

    write_input_profile(&root, Some("content"), "themes/boris");
    let err = engine.run(&verb, Box::new(|_| {})).unwrap_err().to_string();
    assert!(
        err.contains("theme footer file"),
        "theme input surface must refuse: {err}"
    );

    write_custom_profile(
        &root,
        r#"{ "name": "public", "output": "assets-src", "public": true,
            "theme": "themes/boris", "static": { "dir": "assets-src" } }"#,
    );
    let err = engine.run(&verb, Box::new(|_| {})).unwrap_err().to_string();
    assert!(
        err.contains("static input tree"),
        "static tree must refuse: {err}"
    );

    write_custom_profile(
        &root,
        r#"{ "name": "public", "output": "layouts", "public": true,
            "layout": "layouts/main.html" }"#,
    );
    let err = engine.run(&verb, Box::new(|_| {})).unwrap_err().to_string();
    assert!(
        err.contains("layout file"),
        "layout file authority must refuse an overlapping output root: {err}"
    );

    write_custom_profile(
        &root,
        r#"{ "name": "public", "output": "layouts/generated", "public": true,
            "layout": "layouts/main.html" }"#,
    );
    let err = engine.run(&verb, Box::new(|_| {})).unwrap_err().to_string();
    assert!(
        err.contains("layout parent tree"),
        "Boris-protected layout parent tree must refuse nested output: {err}"
    );
}

#[test]
fn physically_aliased_or_nested_profile_output_roots_refuse() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("content")).unwrap();
    cv_store::workspace_instance::ensure(&root).unwrap();
    let engine = partition_engine(&root, "content");
    let verb = custom("build", &["--profile", "boris.json"]);
    std::fs::write(
        root.join("boris.json"),
        r#"{
  "format": "boris-publication-profile", "schema_version": 1, "input": "content",
  "targets": [
    { "name": "a", "output": "STRASSE", "theme": "themes/boris" },
    { "name": "b", "output": "straße", "theme": "themes/boris" }
  ]
}"#,
    )
    .unwrap();
    let err = engine.run(&verb, Box::new(|_| {})).unwrap_err().to_string();
    assert!(err.contains("physically overlap"), "unicode aliases: {err}");

    std::fs::write(
        root.join("boris.json"),
        r#"{
  "format": "boris-publication-profile", "schema_version": 1, "input": "content",
  "targets": [
    { "name": "a", "output": "out", "theme": "themes/boris" },
    { "name": "b", "output": "out/sub", "theme": "themes/boris" }
  ]
}"#,
    )
    .unwrap();
    let err = engine.run(&verb, Box::new(|_| {})).unwrap_err().to_string();
    assert!(err.contains("physically overlap"), "nested roots: {err}");

    std::fs::write(
        root.join("boris.json"),
        r#"{
  "format": "boris-publication-profile", "schema_version": 1, "input": "content",
  "targets": [
    { "name": "a", "output": "out", "theme": "themes/boris",
      "sitemap": { "path": "STRASSE.xml" }, "rss": { "path": "straße.xml" } }
  ]
}"#,
    )
    .unwrap();
    let err = engine.run(&verb, Box::new(|_| {})).unwrap_err().to_string();
    assert!(
        err.contains("artifact outputs") && err.contains("physically overlap"),
        "unicode artifact aliases: {err}"
    );
}
