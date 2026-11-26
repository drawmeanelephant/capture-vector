//! Caller-controlled output partition regressions: owned CV lanes pass,
//! source/profile/metadata overlaps refuse before Boris runs.

mod common;

use common::FakeBoris;
use cv_core::engine::{Engine, EngineCaps, EngineSource, ExitClass, ExportKind, Verb};
use cv_engine::locate::{EngineIdentity, PINNED};
use cv_engine::probe::{CapabilityReport, WatchMode};
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
    cv_store::workspace_instance::ensure(&root).unwrap();
    let engine = partition_engine(&root, "content");
    for verb in [
        custom("build", &["--input", "rag", "--out", "rag"]),
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
