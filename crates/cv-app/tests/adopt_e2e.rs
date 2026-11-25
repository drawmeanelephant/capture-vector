//! End-to-end seam proof for the migration lanes: an import run produced
//! by the real `run_lab` execution path — argv validation, canonicalized
//! paths, process spawn, exit classification, run-record writing — is
//! adoptable through the real crash-aware adoption transaction, and the
//! post-adoption validate/build gate runs through a real engine
//! scheduler with validate strictly before build.
//!
//! This is the contract the New import wizard (#172/#211) plugs into:
//! however a run is started, what lands in `.cv/imports/<run-id>/` must
//! be exactly what adoption consumes.

use cv_core::engine::{CancelToken, EventSink, Verb};
use cv_engine::adopt;
use cv_engine::lab as engine_lab;
use cv_engine::locate::Locator;
use cv_engine::{EngineConfig, EngineScheduler, ProcessEngine, SeatPolicy};
use std::path::{Path, PathBuf};

const LAB_SHIM: &str = include_str!("../../cv-engine/tests/fake-lab/boris-migration-lab");
const BORIS_SHIM: &str = include_str!("../../cv-engine/tests/fake-boris/boris");

fn write(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

#[cfg(unix)]
fn executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// A temp dir holding the fake lab shim plus the pinned version record
/// and the captured hugo output tree as its replay.
#[cfg(unix)]
fn fake_lab() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join(engine_lab::exe_name());
    std::fs::write(&bin, LAB_SHIM).unwrap();
    executable(&bin);
    write(
        &dir.path().join("version.txt"),
        format!("{}\n", engine_lab::PINNED.tool_id).as_bytes(),
    );
    let captured = cv_fixtures::lab::lab_capture("hugo").transcript();
    write(&dir.path().join("stdout.txt"), captured.stdout.as_bytes());
    write(&dir.path().join("stderr.txt"), captured.stderr.as_bytes());
    write(
        &dir.path().join("exit-code.txt"),
        captured.exit_code.to_string().as_bytes(),
    );
    for (rel, bytes) in &captured.out {
        write(&dir.path().join("replay").join(rel), bytes);
    }
    (dir, bin)
}

/// A temp dir holding the fake boris shim with the pinned engine
/// identity; validate and build exit 0 and log their argv in order.
#[cfg(unix)]
fn fake_boris() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("boris");
    std::fs::write(&bin, BORIS_SHIM).unwrap();
    executable(&bin);
    let raw =
        std::fs::read_to_string(cv_fixtures::fixtures_root().join("engine-version.txt")).unwrap();
    let build_info = raw
        .lines()
        .find(|l| l.trim_start().starts_with('{'))
        .unwrap()
        .to_string();
    let v: serde_json::Value = serde_json::from_str(&build_info).unwrap();
    write(
        &dir.path().join("version.txt"),
        format!("{}\n", v["version"].as_str().unwrap()).as_bytes(),
    );
    write(&dir.path().join("build-info.txt"), build_info.as_bytes());
    write(&dir.path().join("verbs/validate/exit-code.txt"), b"0");
    write(&dir.path().join("verbs/build/exit-code.txt"), b"0");
    (dir, bin)
}

#[cfg(unix)]
#[test]
fn import_run_produced_by_run_lab_is_adoptable_end_to_end() {
    // The workspace the import runs in and the adoption merges into.
    let ws_tmp = tempfile::tempdir().unwrap();
    let ws = ws_tmp.path().canonicalize().unwrap();
    write(
        &ws.join("content/index.md"),
        b"---\ntitle: Home\n---\nHello\n",
    );

    // 1. A genuine import run through the real execution path. The fake
    //    lab replays the captured hugo output; everything around it —
    //    input validation, canonicalization, spawn, exit classification,
    //    run-record writing — is the production machinery.
    let (_lab_dir, lab_bin) = fake_lab();
    let lab = engine_lab::gate(
        cv_core::engine::EngineSource::EnvOverride,
        &lab_bin,
        &ws,
        engine_lab::PINNED,
    )
    .expect("fake lab passes the pinned gate");
    let input_tmp = tempfile::tempdir().unwrap();
    let input = input_tmp.path().canonicalize().unwrap();
    let run_id = "e2e-hugo";
    let rec = cv_app::migrate::run_lab(
        &ws,
        run_id,
        &lab,
        engine_lab::mode("hugo").unwrap(),
        vec![("--root".into(), input)],
        &CancelToken::new(),
    )
    .expect("run_lab executes");
    assert_eq!(rec.refused, None, "the run is not refused");
    assert_eq!(
        rec.finished.as_ref().map(|f| f.exit.as_str()),
        Some("ok"),
        "the run finishes ok: {rec:?}"
    );

    // 2. THE SEAM: what the import lane wrote is exactly what the
    //    adoption lane consumes — no hand-built records anywhere.
    let src = adopt::source(&ws, run_id).expect("a real run_lab record is adoptable");
    assert!(!src.files.is_empty(), "candidate content present");
    assert!(!src.pages.is_empty(), "report pages decoded");

    // 3. The crash-aware transaction merges into a subfolder of the
    //    existing workspace's content input.
    let input_name = cv_app::snapshot::content_input(&ws);
    let ids: std::collections::BTreeMap<String, String> =
        cv_app::snapshot::workspace_snapshot(&ws, "dist", None, 0)
            .pages
            .into_iter()
            .map(|p| (p.id, p.path))
            .collect();
    let tx = adopt::transact(
        &ws,
        &src,
        &input_name,
        Some("imported"),
        "existing",
        &ids,
        7,
    )
    .expect("merge transacts");
    assert_eq!(tx.written, src.files.len());
    assert_eq!(tx.resumed, 0);
    assert!(
        cv_store::imports::read_pending(&ws, run_id)
            .unwrap()
            .is_none(),
        "a converged adoption clears its journal"
    );

    // 4. Content landed byte-for-byte below the subfolder, including
    //    binary assets.
    for f in &src.files {
        let landed = ws.join(&input_name).join("imported").join(&f.relative);
        assert_eq!(
            std::fs::read(&landed).unwrap(),
            std::fs::read(&f.source).unwrap(),
            "{}",
            f.relative
        );
    }

    // 5. Provenance landed in .cv/imports/<run-id>/, never in
    //    frontmatter; the run record carries the adopted marker.
    let run_dir = ws.join(".cv/imports").join(run_id);
    assert!(run_dir.join("report.json").is_file(), "report copied");
    assert!(
        run_dir.join("provenance").is_dir(),
        "provenance tree copied"
    );
    let runs = cv_store::imports::scan(&ws).unwrap();
    let rec = runs
        .iter()
        .find_map(|r| r.record.as_ref().ok().filter(|r| r.run_id == run_id))
        .expect("run record readable");
    assert!(rec.adopted.is_some(), "the run is marked adopted");
    for f in &src.files {
        let landed = ws.join(&input_name).join("imported").join(&f.relative);
        let body = std::fs::read_to_string(&landed).unwrap_or_default();
        assert!(
            !body.contains("imported_from") && !body.contains("x-cv-"),
            "provenance never enters frontmatter: {}",
            f.relative
        );
    }

    // 6. The validate/build gate runs through a real engine scheduler,
    //    validate strictly before build.
    let (_boris_dir, boris_bin) = fake_boris();
    let mut cfg = EngineConfig::new(&ws);
    let mut locator = Locator::from_env(&ws);
    locator.env_override = Some(boris_bin.clone());
    cfg.locator = locator;
    let engine = ProcessEngine::start(cfg).expect("fake boris passes the pinned gate");
    let sched = EngineScheduler::new(std::sync::Arc::new(engine), SeatPolicy::StopWatch);
    let cancel = CancelToken::new();
    let run =
        |verb: Verb, sink: EventSink| sched.run_reported(&verb, sink, &cancel).map(|r| r.exit);
    let sink = || Box::new(|_: cv_core::events::Envelope| {}) as EventSink;
    let (validate, build) = adopt::validate_then_build(&run, &sink).expect("gate runs");
    assert_eq!(validate, cv_core::engine::ExitClass::Ok);
    assert_eq!(build, Some(cv_core::engine::ExitClass::Ok));
    let argv = std::fs::read_to_string(_boris_dir.path().join("argv.log")).unwrap();
    let validate_at = argv
        .lines()
        .position(|l| l.starts_with("validate"))
        .expect("validate ran");
    let build_at = argv
        .lines()
        .position(|l| l.starts_with("build"))
        .expect("build ran");
    assert!(
        validate_at < build_at,
        "validate strictly before build:\n{argv}"
    );

    // 7. Loop closure: the Pages projection keys provenance by stable
    //    entity id straight off the real disk truth this test produced.
    let snap = cv_app::snapshot::workspace_snapshot(&ws, "dist", None, 1);
    let adopted_pages: Vec<_> = snap
        .pages
        .iter()
        .filter(|p| p.imported_from.is_some())
        .collect();
    assert_eq!(
        adopted_pages.len(),
        src.pages.len(),
        "every adopted page shows Imported from"
    );
    assert!(
        snap.pages
            .iter()
            .any(|p| p.path == "index.md" && p.imported_from.is_none()),
        "the pre-existing starter page is not marked"
    );
}
