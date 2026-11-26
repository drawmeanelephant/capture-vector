//! Headless scripted New import runs. The picker is answered through the
//! same `ShellRequest`/`Workbench::picked` seam the native shell uses, and
//! the lab itself is the checked-in capture replay (`StubLab`).

mod common;

use cv_app::migrate::{DiskPort, run_stub_lab};
use cv_model::command::ImportJob;
use cv_model::dialog::PickPurpose;
use cv_model::module::Registry;
use cv_model::modules::migrate::{self as migrate_module, LabState};
use cv_model::new_workspace::ShellNotice;
use cv_model::workbench::{ShellRequest, Workbench};
use cv_store::imports as store;
use cv_widgets::harness::Harness;
use std::path::{Path, PathBuf};

fn workspace() -> (tempfile::TempDir, PathBuf) {
    let t = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(t.path()).unwrap();
    (t, root)
}

fn harness(root: &Path) -> Harness {
    migrate_module::install(Box::new(DiskPort::open(
        root,
        LabState::Fixture {
            tool_id: cv_fixtures::lab::StubLab::new().tool_id(),
        },
    )));
    let mut wb = Workbench::new(Registry::with_all());
    wb.workspace = Some(root.to_path_buf());
    let i = wb.registry.index_of(migrate_module::ID).unwrap();
    wb.activate(i);
    Harness::new(wb)
}

fn choose(h: &mut Harness, text: &str) {
    h.script(&[&format!("key:text:{text}"), "key:enter"]);
}

fn take_request(h: &mut Harness) -> ShellRequest {
    assert!(!h.state().requests.is_empty(), "no shell request queued");
    h.state_mut().requests.remove(0)
}

fn answer_picker(h: &mut Harness, _browse_key: &str, path: PathBuf) -> ShellRequest {
    // Drive the same browse operation the Browse button invokes. The native
    // picker itself is outside the headless surface; the request kind is the
    // file/folder contract under test.
    h.state_mut().dialog_browse(0);
    h.settle();
    let request = take_request(h);
    let purpose = match &request {
        ShellRequest::PickFile { purpose, .. } | ShellRequest::PickFolder { purpose } => {
            purpose.clone()
        }
        other => panic!("expected a picker request, got {other:?}"),
    };
    assert!(matches!(purpose, PickPurpose::DialogField { .. }));
    h.state_mut().picked(purpose, Some(path));
    h.settle();
    request
}

fn take_import(h: &mut Harness) -> ImportJob {
    match take_request(h) {
        ShellRequest::RunImport(job) => job,
        other => panic!("expected an import job, got {other:?}"),
    }
}

fn finish_stub(h: &mut Harness, job: &ImportJob) {
    run_stub_lab(
        &job.workspace,
        &job.run_id,
        &job.mode,
        job.inputs.clone(),
        job.extras.clone(),
    )
    .unwrap();
    h.state().notices().send(ShellNotice::ImportFinished {
        job: job.clone(),
        result: Ok(()),
    });
    h.settle();
}

fn run_record(root: &Path, id: &str) -> store::RunRecord {
    let text = std::fs::read_to_string(root.join(".cv").join("imports").join(id).join("run.json"))
        .unwrap();
    serde_json::from_str(&text).unwrap()
}

#[test]
fn wordpress_wxr_file_picker_emits_into_runs_without_copying_the_input() {
    let (_t, root) = workspace();
    let export_dir = root.join("elsewhere");
    std::fs::create_dir_all(&export_dir).unwrap();
    let wxr = export_dir.join("export.xml");
    std::fs::write(&wxr, "<rss version='2.0'><channel /></rss>").unwrap();
    let mut h = harness(&root);

    h.script(&["cmd:migrate.new"]);
    assert!(h.has("dialog-dialog"));
    choose(&mut h, "blog");
    choose(&mut h, "wordpress");

    let request = answer_picker(&mut h, "dialog-field-wxr-browse", wxr.clone());
    match request {
        ShellRequest::PickFile { filters, .. } => {
            assert_eq!(filters.len(), 1);
            assert_eq!(filters[0].extensions, vec!["xml"]);
        }
        other => panic!("wordpress --wxr must pick a file, got {other:?}"),
    }
    h.script(&["key:enter"]);

    assert!(h.has("dialog-progress"));
    let job = take_import(&mut h);
    assert_eq!(job.mode, "wordpress");
    assert_eq!(job.inputs, vec![("--wxr".into(), wxr.clone())]);
    finish_stub(&mut h, &job);

    assert!(!h.has("dialog-dialog"), "completion closes progress");
    assert_eq!(
        h.state().selected_key(),
        Some(format!("run:{}", job.run_id).as_str())
    );
    let rec = run_record(&root, &job.run_id);
    assert_eq!(rec.mode, "wordpress");
    assert_eq!(rec.finished.unwrap().exit, store::FinishedExit::Ok);
    assert_eq!(rec.inputs.len(), 1);
    assert_eq!(rec.inputs[0].path, wxr.display().to_string());
    assert_eq!(rec.inputs[0].path_sha256.len(), 64);
    assert!(
        !root
            .join(".cv/imports")
            .join(&job.run_id)
            .join("out")
            .join("export.xml")
            .exists(),
        "the WXR input itself is never copied into the staged output"
    );
    assert!(h.state().status.contains("Import finished"));
}

#[test]
fn hugo_folder_picker_inventories_first_then_emits_into_runs() {
    let (_t, root) = workspace();
    let hugo = root.join("mini-hugo-source");
    std::fs::create_dir_all(hugo.join("content")).unwrap();
    std::fs::write(hugo.join("hugo.toml"), "baseURL = 'https://example.test'\n").unwrap();
    let mut h = harness(&root);

    h.script(&["cmd:migrate.new"]);
    choose(&mut h, "static");
    choose(&mut h, "hugo");

    let request = answer_picker(&mut h, "dialog-field-root-browse", hugo.clone());
    assert!(
        matches!(request, ShellRequest::PickFolder { .. }),
        "hugo --root must pick a folder"
    );
    h.script(&["key:enter"]);

    assert!(h.has("dialog-progress"));
    let inventory = take_import(&mut h);
    assert_eq!(inventory.mode, "hugo-inventory");
    assert_eq!(inventory.inputs, vec![("--root".into(), hugo.clone())]);
    finish_stub(&mut h, &inventory);

    assert!(h.has("dialog-dialog"), "inventory review gate opens");
    assert!(h.state().status.contains("Inventory finished"));
    h.script(&["key:enter"]);

    assert!(h.has("dialog-progress"));
    let emit = take_import(&mut h);
    assert_eq!(emit.mode, "hugo");
    assert_eq!(emit.inputs, vec![("--root".into(), hugo.clone())]);
    assert_ne!(emit.run_id, inventory.run_id);
    finish_stub(&mut h, &emit);

    assert_eq!(
        h.state().selected_key(),
        Some(format!("run:{}", emit.run_id).as_str())
    );
    for (id, mode) in [
        (&inventory.run_id, "hugo-inventory"),
        (&emit.run_id, "hugo"),
    ] {
        let rec = run_record(&root, id);
        assert_eq!(rec.mode, mode);
        assert_eq!(rec.finished.unwrap().exit, store::FinishedExit::Ok);
        assert_eq!(rec.inputs[0].path, hugo.display().to_string());
    }
    assert!(
        !root
            .join(".cv/imports")
            .join(&emit.run_id)
            .join("out")
            .join("hugo.toml")
            .exists(),
        "the source tree is not copied wholesale into the run"
    );
}

#[cfg(unix)]
#[test]
fn cancelling_progress_kills_the_lab_and_records_a_cancelled_run() {
    use cv_core::engine::EngineSource;
    use cv_engine::lab::{self as engine_lab, PINNED};
    const FAKE: &str = include_str!("../../cv-engine/tests/fake-lab/boris-migration-lab");

    let (_t, root) = workspace();
    let hugo = root.join("hugo-source");
    std::fs::create_dir_all(hugo.join("content")).unwrap();
    let lab_dir = tempfile::tempdir().unwrap();
    let bin = lab_dir.path().join(engine_lab::exe_name());
    common::install_executable(&bin, FAKE);
    std::fs::write(
        lab_dir.path().join("version.txt"),
        format!("{}\n", PINNED.tool_id),
    )
    .unwrap();
    let identity =
        engine_lab::gate(EngineSource::EnvOverride, &bin, lab_dir.path(), PINNED).unwrap();
    std::fs::write(lab_dir.path().join("hold"), "hold\n").unwrap();
    let _ = std::fs::remove_file(lab_dir.path().join("argv.log"));

    let mut h = harness(&root);
    h.script(&["cmd:migrate.new"]);
    choose(&mut h, "static");
    choose(&mut h, "hugo");
    h.state_mut().dialog_browse(0);
    let request = take_request(&mut h);
    let ShellRequest::PickFolder { purpose } = request else {
        panic!("hugo --root must pick a folder");
    };
    h.state_mut().picked(purpose, Some(hugo));
    h.settle();
    h.script(&["key:enter"]);

    let inventory = take_import(&mut h);
    assert!(h.has("dialog-progress"));
    let worker =
        cv_app::migrate::spawn_import_with(h.state_mut(), inventory.clone(), Some(identity))
            .expect("import worker spawned");
    for _ in 0..100 {
        if lab_dir.path().join("argv.log").exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(lab_dir.path().join("argv.log").exists(), "fake lab started");

    h.script(&["key:esc"]);
    assert!(inventory.cancel.is_cancelled());
    worker.join().unwrap();
    h.settle();

    let rec = run_record(&root, &inventory.run_id);
    let finished = rec.finished.expect("cancelled run has a finish record");
    assert_eq!(finished.exit, store::FinishedExit::Cancelled);
    assert!(finished.outcome.contains("cancelled"));
    assert_eq!(
        h.state().selected_key(),
        Some(format!("run:{}", inventory.run_id).as_str())
    );
    assert!(h.state().status.contains("cancelled"));
}

#[cfg(unix)]
#[test]
fn switching_workspaces_during_import_keeps_the_run_lock_and_record_intact() {
    use cv_core::engine::EngineSource;
    use cv_engine::lab::{self as engine_lab, PINNED};
    const FAKE: &str = include_str!("../../cv-engine/tests/fake-lab/boris-migration-lab");

    let (_t, root) = workspace();
    let (_t2, other_root) = workspace();
    let hugo = root.join("hugo-source");
    std::fs::create_dir_all(hugo.join("content")).unwrap();
    let lab_dir = tempfile::tempdir().unwrap();
    let bin = lab_dir.path().join(engine_lab::exe_name());
    common::install_executable(&bin, FAKE);
    std::fs::write(
        lab_dir.path().join("version.txt"),
        format!("{}\n", PINNED.tool_id),
    )
    .unwrap();
    let identity =
        engine_lab::gate(EngineSource::EnvOverride, &bin, lab_dir.path(), PINNED).unwrap();
    std::fs::write(lab_dir.path().join("hold"), "hold\n").unwrap();
    let _ = std::fs::remove_file(lab_dir.path().join("argv.log"));

    let mut h = harness(&root);
    h.script(&["cmd:migrate.new"]);
    choose(&mut h, "static");
    choose(&mut h, "hugo");
    h.state_mut().dialog_browse(0);
    let request = take_request(&mut h);
    let ShellRequest::PickFolder { purpose } = request else {
        panic!("hugo --root must pick a folder");
    };
    h.state_mut().picked(purpose, Some(hugo));
    h.settle();
    h.script(&["key:enter"]);
    let inventory = take_import(&mut h);
    let worker =
        cv_app::migrate::spawn_import_with(h.state_mut(), inventory.clone(), Some(identity))
            .expect("import worker spawned");
    for _ in 0..100 {
        if lab_dir.path().join("argv.log").exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(lab_dir.path().join("argv.log").exists(), "fake lab started");
    let mut held = false;
    for _ in 0..150 {
        if store::probe_run(&root, &inventory.run_id).ok() == Some(store::RunProbe::Held) {
            held = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(held, "the live import owns exactly its narrow per-run lock");
    let run_dir = root.join(store::DIR).join(&inventory.run_id);
    let before_record = std::fs::read(run_dir.join(store::RECORD)).unwrap();

    // Opening another workspace used to drop a process-wide workspace lock.
    // The import must survive that switch because its RunLock is an
    // independent capability held by the worker.
    let env = cv_app::WorkspaceEnv {
        locator: Some(cv_engine::locate::Locator::default()),
        recent_file: Some(other_root.join("recent-workspaces.json")),
    };
    if let Some(boot) = cv_app::open_workspace(h.state_mut(), other_root.clone(), &env) {
        boot.join().unwrap();
    }
    assert_eq!(
        store::probe_run(&root, &inventory.run_id).unwrap(),
        store::RunProbe::Held,
        "workspace switching must not release the import run lock"
    );
    assert_eq!(
        std::fs::read(run_dir.join(store::RECORD)).unwrap(),
        before_record,
        "workspace switching must not rewrite the active run record"
    );

    // Reopening the original workspace is the stale-view case that used to
    // sweep active records as interrupted. It must observe only: the worker
    // still owns the run lock and the record bytes stay exactly intact.
    let env_a = cv_app::WorkspaceEnv {
        locator: Some(cv_engine::locate::Locator::default()),
        recent_file: Some(other_root.join("recent-a.json")),
    };
    if let Some(boot) = cv_app::open_workspace(h.state_mut(), root.clone(), &env_a) {
        boot.join().unwrap();
    }
    assert_eq!(
        store::probe_run(&root, &inventory.run_id).unwrap(),
        store::RunProbe::Held,
        "reopening the workspace must not release the active import lock"
    );
    assert_eq!(
        std::fs::read(run_dir.join(store::RECORD)).unwrap(),
        before_record,
        "reopening the workspace must not rewrite the active run record"
    );

    inventory.cancel.cancel();
    worker.join().unwrap();
    let rec = run_record(&root, &inventory.run_id);
    assert_eq!(
        rec.finished.expect("finished record").exit,
        store::FinishedExit::Cancelled
    );
    assert_eq!(
        store::probe_run(&root, &inventory.run_id).unwrap(),
        store::RunProbe::Free,
        "the worker releases only its per-run lock at the end"
    );
}

/// Re-seed the migrate module after a real workspace switch so the wizard
/// can drive with the fixture lab state. The switch itself already happened
/// through the production `open_workspace` path — module state recreated,
/// dialogs cleared — this only replaces the post-switch lab gate the test
/// environment cannot satisfy, mirroring the initial harness install.
fn reseed_fixture_migrate(h: &mut Harness, root: &Path) {
    h.state_mut().registry.remove(migrate_module::ID);
    migrate_module::install(Box::new(DiskPort::open(
        root,
        LabState::Fixture {
            tool_id: cv_fixtures::lab::StubLab::new().tool_id(),
        },
    )));
    migrate_module::register(&mut h.state_mut().registry);
}

fn drive_hugo_inventory(h: &mut Harness, root: &Path) -> ImportJob {
    h.script(&["cmd:migrate.new"]);
    choose(h, "static");
    choose(h, "hugo");
    h.state_mut().dialog_browse(0);
    let request = take_request(h);
    let ShellRequest::PickFolder { purpose } = request else {
        panic!("hugo --root must pick a folder");
    };
    h.state_mut()
        .picked(purpose, Some(root.join("hugo-source")));
    h.settle();
    h.script(&["key:enter"]);
    take_import(h)
}

#[test]
fn foreign_workspace_completion_does_not_close_the_active_cancellation_dialog() {
    let (_t, root_a) = workspace();
    let (_t2, root_b) = workspace();
    for root in [&root_a, &root_b] {
        std::fs::create_dir_all(root.join("hugo-source").join("content")).unwrap();
    }
    let env = |other: &Path| cv_app::WorkspaceEnv {
        locator: Some(cv_engine::locate::Locator::default()),
        recent_file: Some(other.join("recent-workspaces.json")),
    };

    // Start an import in workspace A, then switch to B while it continues —
    // an explicitly supported sequence. Only the job object is needed: the
    // notice routing under test does not depend on a real worker.
    let mut h = harness(&root_a);
    let job_a = drive_hugo_inventory(&mut h, &root_a);
    assert_eq!(job_a.workspace, root_a);
    if let Some(boot) = cv_app::open_workspace(h.state_mut(), root_b.clone(), &env(&root_b)) {
        boot.join().unwrap();
    }
    reseed_fixture_migrate(&mut h, &root_b);

    // Start an import in B: its wizard is Running and its progress dialog
    // is the user's cancellation control.
    let _job_b = drive_hugo_inventory(&mut h, &root_b);
    assert!(
        h.has("dialog-progress"),
        "B progress dialog is the active cancellation control"
    );

    // A finishes: a foreign completion must not close B's dialog, reset B's
    // wizard, or otherwise touch the current operation.
    h.state().notices().send(ShellNotice::ImportFinished {
        job: job_a,
        result: Ok(()),
    });
    h.settle();
    assert!(
        h.has("dialog-progress"),
        "a foreign workspace completion must leave the cancellation control open"
    );
}

#[test]
fn stale_completion_after_reopen_does_not_complete_the_wrong_wizard() {
    let (_t, root) = workspace();
    let (_t2, other) = workspace();
    std::fs::create_dir_all(root.join("hugo-source").join("content")).unwrap();
    let env = |dir: &Path| cv_app::WorkspaceEnv {
        locator: Some(cv_engine::locate::Locator::default()),
        recent_file: Some(dir.join("recent-workspaces.json")),
    };

    // Start import A in W, then switch away and reopen W: the module and
    // wizard state are recreated, while shell notices survive the switch.
    let mut h = harness(&root);
    let job_a = drive_hugo_inventory(&mut h, &root);
    if let Some(boot) = cv_app::open_workspace(h.state_mut(), other.clone(), &env(&other)) {
        boot.join().unwrap();
    }
    if let Some(boot) = cv_app::open_workspace(h.state_mut(), root.clone(), &env(&root)) {
        boot.join().unwrap();
    }
    reseed_fixture_migrate(&mut h, &root);

    // Start import B in the reopened workspace. Nothing was ever written to
    // disk, so B stages the same candidate run id A had — the run id cannot
    // discriminate the two operations; only the cancellation token can.
    let _job_b = drive_hugo_inventory(&mut h, &root);
    assert!(
        h.has("dialog-progress"),
        "B progress dialog is the active cancellation control"
    );

    // A's completion arrives late. Same workspace, but a different
    // operation: it must not complete, reset, or close anything of B's.
    h.state().notices().send(ShellNotice::ImportFinished {
        job: job_a,
        result: Ok(()),
    });
    h.settle();
    assert!(
        h.has("dialog-progress"),
        "a stale completion must leave the running wizard's cancellation control open"
    );
    assert!(
        !h.state().status.contains("ended before"),
        "B's wizard must not be reset by A's completion: {}",
        h.state().status
    );
}

/// The production race from adversarial review: two operations stage the
/// same unclaimed candidate id across a workspace reopen; one claims it,
/// the other collides and retargets. The colliding operation's start notice
/// carries the collided requested id — which string-matches the running
/// wizard's candidate — but its cancellation token belongs to the other
/// operation. The wizard must not rebind: token identity, never the run id,
/// discriminates operations that staged the same candidate.
#[test]
fn collision_retarget_for_a_different_operation_does_not_rebind_the_wizard() {
    let (_t, root) = workspace();
    let (_t2, other) = workspace();
    std::fs::create_dir_all(root.join("hugo-source").join("content")).unwrap();
    let env = |dir: &Path| cv_app::WorkspaceEnv {
        locator: Some(cv_engine::locate::Locator::default()),
        recent_file: Some(dir.join("recent-workspaces.json")),
    };

    // Import A is accepted in W, its worker still waiting in the external
    // gate before claiming. The workspace switches away and back, recreating
    // module state while shell notices survive.
    let mut h = harness(&root);
    let job_a = drive_hugo_inventory(&mut h, &root);
    if let Some(boot) = cv_app::open_workspace(h.state_mut(), other.clone(), &env(&other)) {
        boot.join().unwrap();
    }
    if let Some(boot) = cv_app::open_workspace(h.state_mut(), root.clone(), &env(&root)) {
        boot.join().unwrap();
    }
    reseed_fixture_migrate(&mut h, &root);

    // Import B is accepted in the reopened workspace. Nothing was ever
    // written to disk, so B stages the same candidate run id A had.
    let job_b = drive_hugo_inventory(&mut h, &root);
    assert_eq!(
        job_b.run_id, job_a.run_id,
        "both operations stage the same unclaimed candidate"
    );
    assert_ne!(job_b.cancel, job_a.cancel, "distinct operations");

    // B's worker claims the candidate first; A's gate then releases, A
    // collides, claims the next id, and publishes its ordered retargeting
    // notice. B's finished record is on disk for the candidate id, exactly
    // as B's worker leaves it when its lab ends.
    let lock = store::claim_run(&root, &job_b.run_id).expect("B claims the candidate");
    let mut record = store::RunRecord::started(
        &job_b.run_id,
        "hugo-inventory",
        store::RecordedLab {
            tool_id: "stub".into(),
            source: "fixture".into(),
            path: "stub".into(),
        },
        1,
    );
    record.finished = Some(store::Finished {
        exit_code: Some(1),
        exit: store::FinishedExit::Io,
        outcome: "exited 1".into(),
        duration_ms: 3,
    });
    store::write(&lock, &record).expect("B's worker records the outcome");
    drop(lock);

    let mut retargeted_a = job_a.clone();
    retargeted_a.run_id = format!("{}-collided", job_a.run_id);
    h.state().notices().send(ShellNotice::ImportStarted {
        requested: job_a.run_id.clone(),
        job: retargeted_a,
    });
    h.settle();

    // B completes. Its token correlates, so completion proceeds — and must
    // read B's own run record at the candidate id. Had A's notice rebound
    // the wizard to A's collided id, the lookup would miss every record and
    // report a run that "ended before its run record could be written".
    h.state().notices().send(ShellNotice::ImportFinished {
        job: job_b.clone(),
        result: Ok(()),
    });
    h.settle();
    assert!(
        !h.state().status.contains("ended before"),
        "B's completion must read B's own run record, not A's collided id: {}",
        h.state().status
    );
    assert!(
        h.state().status.contains("did not complete"),
        "the recorded outcome is what the wizard reports: {}",
        h.state().status
    );
}
