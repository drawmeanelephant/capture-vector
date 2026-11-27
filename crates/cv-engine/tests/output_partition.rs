//! Caller-controlled output partition regressions: owned CV lanes pass,
//! source/profile/metadata overlaps refuse before Boris runs.

mod common;

use common::FakeBoris;
use cv_core::engine::{Engine, EngineCaps, EngineSource, ExitClass, ExportKind, Verb};
use cv_engine::locate::{EngineIdentity, PINNED};
use cv_engine::probe::{CapabilityReport, WatchMode};
use cv_engine::proc::CancelToken;
use cv_engine::secrets::MockSecretSource;
use cv_engine::{EngineConfig, ProcessEngine};
use std::path::Path;

fn partition_engine(root: &Path, input: &str) -> ProcessEngine {
    let mut cfg = EngineConfig::new(root);
    cfg.content_input = input.into();
    ProcessEngine::with_report(
        cfg,
        EngineIdentity {
            source: EngineSource::Path,
            path: root.join("boris"),
            version: PINNED.version.into(),
            vcs_revision: PINNED.vcs_revision.into(),
            build_info_raw: String::new(),
        },
        CapabilityReport {
            os: std::env::consts::OS.into(),
            caps: EngineCaps {
                writes: true,
                ..Default::default()
            },
            watch_mode: WatchMode::Native,
            evidence: vec![],
        },
    )
}

fn custom(name: &str, args: &[&str]) -> Verb {
    Verb::Custom {
        name: name.into(),
        args: args.iter().map(|s| s.to_string()).collect(),
    }
}

fn partition_error(engine: &ProcessEngine, verb: &Verb) -> Option<String> {
    engine
        .run(verb, Box::new(|_| {}))
        .err()
        .map(|e| e.to_string())
}

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
    let input = external
        .path()
        .join("ümlaut and spaces")
        .canonicalize()
        .unwrap_or_else(|_| external.path().join("ümlaut and spaces"))
        .to_string_lossy()
        .into_owned();
    std::fs::create_dir_all(&input).unwrap();
    let mut cfg = fake.config();
    cfg.content_input = input.clone().into();
    let engine = ProcessEngine::start(cfg).unwrap();
    let verb = custom("validate", &["--input", &input]);
    let out = engine.run(&verb, Box::new(|_| {})).unwrap();
    assert_eq!(out, ExitClass::Ok);
    let argv = fake.read("argv.log");
    assert!(argv.contains(&format!("--input {input}")), "{argv}");
}
