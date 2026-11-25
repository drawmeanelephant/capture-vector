//! Headless scripted New import runs. The picker is answered through the
//! same `ShellRequest`/`Workbench::picked` seam the native shell uses, and
//! the lab itself is the checked-in capture replay (`StubLab`).

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
    run_stub_lab(&job.workspace, &job.run_id, &job.mode, job.inputs.clone()).unwrap();
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
    assert_eq!(rec.finished.unwrap().exit, "ok");
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
        assert_eq!(rec.finished.unwrap().exit, "ok");
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
    use std::os::unix::fs::PermissionsExt;

    const FAKE: &str = include_str!("../../cv-engine/tests/fake-lab/boris-migration-lab");

    let (_t, root) = workspace();
    let hugo = root.join("hugo-source");
    std::fs::create_dir_all(hugo.join("content")).unwrap();
    let lab_dir = tempfile::tempdir().unwrap();
    let bin = lab_dir.path().join(engine_lab::exe_name());
    std::fs::write(&bin, FAKE).unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(
        lab_dir.path().join("version.txt"),
        format!("{}\n", PINNED.tool_id),
    )
    .unwrap();
    let identity =
        engine_lab::gate(EngineSource::EnvOverride, &bin, lab_dir.path(), PINNED).unwrap();
    std::fs::write(lab_dir.path().join("hold"), "hold\n").unwrap();

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
    assert_eq!(finished.exit, "cancelled");
    assert!(finished.outcome.contains("cancelled"));
    assert_eq!(
        h.state().selected_key(),
        Some(format!("run:{}", inventory.run_id).as_str())
    );
    assert!(h.state().status.contains("cancelled"));
}
