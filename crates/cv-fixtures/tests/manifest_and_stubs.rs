//! Manifest integrity + stub conformance scaffold.
//! The `Stub ≡ Process` suite lives here; L1 fills the ProcessEngine side.

use cv_core::engine::{Engine, Verb};
use cv_core::events::engine::EngineEvent;
use cv_core::store::{Namespace, Predicate, Store};
use cv_fixtures::{Corpus, Manifest, StubEngine, StubStore, corpora, corpus, fixtures_root};

#[test]
fn manifest_covers_every_corpus_on_disk() {
    let manifest = Manifest::load(&fixtures_root().join("MANIFEST.toml"));
    for name in corpora() {
        // every corpus has at least one pinned capture file
        assert!(
            manifest
                .pinned_files()
                .any(|(k, _)| k.starts_with(&format!("captures/{name}/"))),
            "corpus '{name}' has no pinned captures in MANIFEST.toml"
        );
    }
}

#[test]
fn every_pinned_file_exists_and_hashes() {
    let manifest = Manifest::load(&fixtures_root().join("MANIFEST.toml"));
    let root = fixtures_root();
    let mut checked = 0;
    for (rel, _) in manifest.pinned_files() {
        let path = root.join(rel);
        let bytes = std::fs::read(&path)
            .unwrap_or_else(|e| panic!("pinned file missing: {rel} at {path:?}: {e}"));
        manifest.verify(rel, &bytes).unwrap();
        checked += 1;
    }
    assert!(
        checked > 100,
        "manifest should pin hundreds of files, got {checked}"
    );
}

#[test]
fn every_corpus_has_a_pinned_ir_capture() {
    let root = fixtures_root().join("ir");
    let manifest = Manifest::load(&root.join("MANIFEST.toml"));
    for name in corpora() {
        let c = cv_fixtures::ir_capture(&name);
        for f in ["stdout.txt", "stderr.txt", "exit-code.txt"] {
            c.file(f);
        }
    }
    for (rel, _) in manifest.pinned_files() {
        manifest
            .verify(rel, &std::fs::read(root.join(rel)).unwrap())
            .unwrap();
    }
}

fn ir(out: &str) -> Verb {
    Verb::Ir {
        out: out.into(),
        input: None,
    }
}

#[test]
fn stub_engine_replays_the_captured_ir_projection() {
    use cv_core::engine::ExitClass;
    use cv_fixtures::stub_engine::IR_CAPTURE_OUT;

    let engine = StubEngine::new("happy");
    let exit = engine.run(&ir(IR_CAPTURE_OUT), Box::new(|_| {})).unwrap();
    assert_eq!(exit, ExitClass::Ok);
    assert!(matches!(
        engine.sent().as_slice(),
        [
            EngineEvent::BuildFinished { pages_written: 0, .. },
            EngineEvent::VerbFinished { verb, exit: ExitClass::Ok },
        ] if verb == "ir"
    ));

    let engine = StubEngine::new("broken-frontmatter");
    let exit = engine.run(&ir(IR_CAPTURE_OUT), Box::new(|_| {})).unwrap();
    assert_eq!(exit, ExitClass::ContentFailure);
    match &engine.sent()[0] {
        EngineEvent::BuildFailed { diagnostics, .. } => {
            assert_eq!(diagnostics[0].code, "EFRONTMATTER");
        }
        e => panic!("expected BuildFailed, got {e:?}"),
    }
    // Only the captured spelling replays.
    assert!(engine.run(&ir(".boris"), Box::new(|_| {})).is_err());
}

#[test]
fn corpus_reads_are_hash_verified() {
    let c: Corpus = corpus("happy");
    // round-trips through manifest verification
    let ndjson = c.file_str("watch/session.ndjson");
    assert!(ndjson.lines().next().unwrap().contains("\"hello\""));
}

#[test]
fn stub_engine_replays_captured_exit_codes() {
    let engine = StubEngine::new("broken-frontmatter");
    let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = std::sync::Arc::clone(&events);
    let exit = engine
        .run(
            &Verb::Build,
            Box::new(move |e| sink.lock().unwrap().push(e)),
        )
        .unwrap();
    assert_eq!(exit, cv_core::engine::ExitClass::ContentFailure);
    // Like ProcessEngine: the one-shot build (with its stderr diagnostics)
    // then `VerbFinished`.
    assert_eq!(events.lock().unwrap().len(), 2);
    match &engine.sent()[0] {
        EngineEvent::BuildFailed { diagnostics, .. } => {
            assert_eq!(diagnostics.len(), 1);
            assert_eq!(diagnostics[0].code, "EFRONTMATTER");
            assert_eq!(diagnostics[0].span.as_ref().unwrap().path, "index.md");
            assert!(diagnostics[0].build_version.is_some());
        }
        e => panic!("expected BuildFailed, got {e:?}"),
    }
}

#[test]
fn stub_engine_maps_watch_diagnostics() {
    let engine = StubEngine::new("dogfood");
    let _h = engine.replay_watch(Box::new(|_| {}));
    let failed: Vec<_> = engine
        .sent()
        .into_iter()
        .filter_map(|e| match e {
            EngineEvent::BuildFailed { diagnostics, .. } => Some(diagnostics),
            _ => None,
        })
        .collect();
    assert!(!failed.is_empty());
    let d = &failed[0][0];
    assert_eq!(d.code, "ELAYOUT");
    assert_eq!(
        d.span.as_ref().unwrap().path,
        "themes/boris/layouts/main.html"
    );
}

#[test]
fn stub_engine_replays_watch_session() {
    let engine = StubEngine::new("happy");
    let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = std::sync::Arc::clone(&events);
    let _h = engine
        .watch(Box::new(move |e| sink.lock().unwrap().push(e)))
        .unwrap();
    assert!(!events.lock().unwrap().is_empty());
    // first meaningful engine event records the hello handshake
    let sent = engine.sent();
    assert!(sent.iter().any(|e| matches!(
        e,
        cv_core::events::engine::EngineEvent::WatchSessionStarted { schema: 1, .. }
    )));
}

#[test]
fn stub_store_roundtrip_and_query() {
    let store = StubStore::new();
    let ns = Namespace("research.notes".into());
    let id = cv_core::id::StableId("n1".into());
    store
        .put(&ns, &id, &serde_json::json!({"status": "open", "count": 3}))
        .unwrap();
    assert!(store.get(&ns, &id).unwrap().is_some());

    let pred = Predicate {
        conj: vec![cv_core::store::Term {
            field: "status".into(),
            op: cv_core::store::Op::Eq,
            value: cv_core::id::AttrValue::Text("open".into()),
        }],
    };
    assert_eq!(store.query(&ns, &pred).unwrap().len(), 1);
}

#[test]
fn mode_flag_corpora_have_green_captures() {
    for name in ["happy-textile", "cook-one"] {
        let c = corpus(name);
        for verb in ["build-modeflag", "validate-modeflag"] {
            assert_eq!(
                c.file_str(&format!("{verb}/exit-code.txt")).trim(),
                "0",
                "{name}/{verb} should be a green capture"
            );
        }
        assert!(
            c.file_str("dist-modeflag/dist/_boris/proof/proof-pack.json")
                .contains("\"boris-publication-proof-pack\""),
            "{name} should carry a mode-flag proof pack"
        );
    }
    // the mode flag is required: without it the build fails mode selection
    assert_eq!(
        corpus("happy-textile")
            .file_str("build/exit-code.txt")
            .trim(),
        "1"
    );
    assert_eq!(
        corpus("cook-one").file_str("build/exit-code.txt").trim(),
        "1"
    );
}

#[test]
fn stub_engine_refuses_verbs_without_a_capture() {
    let engine = StubEngine::new("happy");
    let r = engine.run(
        &Verb::Custom {
            name: "x".into(),
            args: vec![],
        },
        Box::new(|_| {}),
    );
    assert!(
        r.is_err(),
        "a verb with no captured bytes can't replay build evidence"
    );
}

#[test]
#[should_panic(expected = "plain relative path")]
fn fixture_paths_cannot_escape_the_tree() {
    corpus("happy").file("../MANIFEST.toml");
}

#[test]
#[should_panic(expected = "plain relative path")]
fn corpus_names_cannot_escape_the_tree() {
    corpus("../fixtures");
}

fn init(dir: &std::path::Path, archetype: &str) -> cv_core::Result<cv_core::engine::ExitClass> {
    StubEngine::new("happy").run(
        &Verb::Init {
            dir: dir.display().to_string(),
            archetype: archetype.into(),
        },
        Box::new(|_| {}),
    )
}

#[test]
fn stub_init_writes_the_captured_starter_and_refuses_a_folder_that_is_not_empty() {
    use cv_core::engine::ExitClass;
    let t = tempfile::tempdir().unwrap();
    let site = t.path().join("site");
    assert_eq!(init(&site, "docs").unwrap(), ExitClass::Ok);
    for rel in [
        "boris.json",
        "content/index.md",
        "themes/boris/layouts/main.html",
    ] {
        assert!(site.join(rel).is_file(), "{rel} not written");
    }
    let c = corpus(cv_fixtures::INIT);
    assert_eq!(
        std::fs::read(site.join("boris.json")).unwrap(),
        c.file("docs/tree/boris.json")
    );
    assert!(
        c.file_str("docs/stderr.txt")
            .contains("verified: starter compiled")
    );

    assert_eq!(init(&site, "docs").unwrap(), ExitClass::UsageError);
    assert!(StubEngine::init_report("refuse").contains("not empty"));
    assert!(
        init(&t.path().join("other"), "garden").is_err(),
        "garden was never captured"
    );
}

#[test]
fn init_captures_are_not_a_corpus() {
    assert!(!corpora().contains(&cv_fixtures::INIT.to_string()));
    assert!(StubEngine::init_help().contains("--type NAME"));
}
