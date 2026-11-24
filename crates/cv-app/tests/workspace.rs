//! Workspace open flow: research persistence under `.cv/`, gated engine
//! boot (fake-boris shim from cv-engine's tests, never a real boris),
//! engine-absent / version-mismatch disclosure, live watch diagnostics
//! through the cursor-resync sink, and the open/recent commands.

use cv_app::{WorkspaceEnv, init, open_workspace};
use cv_core::events::shell::DegradedReason;
use cv_engine::locate::Locator;
use cv_model::workbench::{ShellRequest, Workbench};
use cv_widgets::harness::Harness;
use cv_widgets::shell::degraded_text;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const SHIM_SH: &str = include_str!("../../cv-engine/tests/fake-boris/boris");
const SHIM_BAT: &str = include_str!("../../cv-engine/tests/fake-boris/boris.bat");

#[test]
fn reopening_workspace_releases_and_reloads_the_findings_journal() {
    let project = workspace("happy");
    let recent = tempfile::tempdir().unwrap();
    let fake = Fake::new("happy", None);
    let env = fake.env(recent.path());
    let mut wb = init();
    for n in 0..2 {
        open_workspace(&mut wb, project.path().into(), &env)
            .unwrap()
            .join()
            .unwrap();
        wb.drain();
        assert!(
            !wb.status.contains("finding snoozes not persisted"),
            "{}",
            wb.status
        );
        cv_app::open_corpus(&mut wb, cv_app::CANARY_CORPUS);
        wb.drain();
        let audit = wb.registry.index_of("audit").unwrap();
        let module = wb.registry.get(audit).unwrap();
        if n == 0 {
            wb.active = audit;
            wb.select_row(0);
            wb.run_command("audit.snooze");
            assert!(wb.status.ends_with("snoozed"), "{}", wb.status);
        } else {
            assert_eq!(module.list_len(Some("snoozed")), 1);
            let row = module.row(Some("snoozed"), 0);
            assert!(
                module
                    .inspector(&row.key)
                    .fields
                    .contains(&("sidecar".into(), ".cv/audit/lifecycle.jsonl".into()))
            );
        }
    }
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let dst = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_tree(&e.path(), &dst);
        } else {
            std::fs::copy(e.path(), dst).unwrap();
        }
    }
}

/// A workspace holding a copy of a captured corpus' sources.
fn workspace(corpus: &str) -> tempfile::TempDir {
    let t = tempfile::Builder::new().prefix("cv-ws-").tempdir().unwrap();
    copy_tree(
        &cv_fixtures::fixtures_root().join("corpus-src").join(corpus),
        t.path(),
    );
    t
}

/// The fake-boris shim in a temp bin dir, replaying `corpus`' watch.
struct Fake {
    dir: tempfile::TempDir,
    bin: PathBuf,
}

impl Fake {
    fn new(corpus: &str, version: Option<&str>) -> Self {
        let dir = tempfile::Builder::new()
            .prefix("cv-bin-")
            .tempdir()
            .unwrap();
        let bin = if cfg!(windows) {
            let p = dir.path().join("boris.bat");
            std::fs::write(&p, SHIM_BAT).unwrap();
            p
        } else {
            let p = dir.path().join("boris");
            std::fs::write(&p, SHIM_SH).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            p
        };
        let raw = std::fs::read_to_string(cv_fixtures::fixtures_root().join("engine-version.txt"))
            .unwrap();
        let build_info = raw
            .lines()
            .find(|l| l.trim_start().starts_with('{'))
            .unwrap()
            .to_string();
        let v: serde_json::Value = serde_json::from_str(&build_info).unwrap();
        let f = Self { dir, bin };
        f.set(
            "version.txt",
            version.unwrap_or(v["version"].as_str().unwrap()).as_bytes(),
        );
        f.set("build-info.txt", build_info.as_bytes());
        f.set("verbs/validate/exit-code.txt", b"0");
        let c = cv_fixtures::corpus(corpus);
        f.set("watch.ndjson", &c.file("watch/session.ndjson"));
        for name in ["stdout.txt", "stderr.txt", "exit-code.txt"] {
            f.set(
                &format!("verbs/build/{name}"),
                &c.file(&format!("build/{name}")),
            );
        }
        f
    }

    fn set(&self, rel: &str, bytes: &[u8]) {
        let p = self.dir.path().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }

    fn env(&self, recent: &Path) -> WorkspaceEnv {
        WorkspaceEnv {
            locator: Some(Locator {
                env_override: Some(self.bin.clone()),
                ..Locator::default()
            }),
            recent_file: Some(recent.join("recent.json")),
        }
    }
}

/// No candidate anywhere: the production chain finds nothing.
fn absent_env(recent: &Path) -> WorkspaceEnv {
    WorkspaceEnv {
        locator: Some(Locator::default()),
        recent_file: Some(recent.join("recent.json")),
    }
}

fn open(wb: &mut Workbench, root: &Path, env: &WorkspaceEnv) {
    open_workspace(wb, root.to_path_buf(), env)
        .expect("workspace opens")
        .join()
        .unwrap();
    wb.drain();
}

fn module_len(wb: &Workbench, id: &str, nav: Option<&str>) -> usize {
    let i = wb.registry.index_of(id).unwrap();
    wb.registry.get(i).unwrap().list_len(nav)
}

fn wait_for(wb: &mut Workbench, what: &str, mut f: impl FnMut(&Workbench) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        wb.drain();
        if f(wb) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}; status {:?}, degraded {:?}",
            wb.status,
            wb.app.degraded
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn engine_absent_is_disclosed_and_the_workspace_stays_browsable() {
    let ws = workspace("happy");
    let cfg = tempfile::tempdir().unwrap();
    let mut wb = init();
    open(&mut wb, ws.path(), &absent_env(cfg.path()));
    assert_eq!(wb.app.degraded, Some(DegradedReason::EngineAbsent));
    assert!(wb.workspace.is_some());
    assert!(module_len(&wb, "pages", None) > 0, "pages read from disk");
    assert!(wb.status.contains("boris not found"), "{}", wb.status);
    // Same banner the L2 engine-absent golden pins.
    let tree = Harness::new(wb).tree().to_string();
    assert!(tree.contains(&degraded_text(&DegradedReason::EngineAbsent)));
}

#[test]
fn version_mismatch_is_disclosed_not_booted() {
    let ws = workspace("happy");
    let cfg = tempfile::tempdir().unwrap();
    let fake = Fake::new("happy", Some("boris/0.8.2"));
    let mut wb = init();
    open(&mut wb, ws.path(), &fake.env(cfg.path()));
    assert_eq!(
        wb.app.degraded,
        Some(DegradedReason::EngineVersionMismatch {
            found: "0.8.2".into(),
            want: "0.9.0".into()
        })
    );
    assert_eq!(wb.app.engine, None, "no watch session on a failed gate");
}

#[test]
fn gated_engine_drives_activity_from_the_live_watch() {
    let ws = workspace("broken-parent");
    let cfg = tempfile::tempdir().unwrap();
    let fake = Fake::new("broken-parent", None);
    let mut wb = init();
    open(&mut wb, ws.path(), &fake.env(cfg.path()));
    wait_for(&mut wb, "watch handshake", |wb| {
        wb.app.engine.as_deref() == Some("boris/0.9.0")
    });
    wait_for(&mut wb, "live diagnostics", |wb| {
        module_len(wb, "activity", Some("diagnostics")) > 0
    });
    assert!(
        !matches!(
            wb.app.degraded,
            Some(DegradedReason::EngineAbsent | DegradedReason::EngineVersionMismatch { .. })
        ),
        "{:?}",
        wb.app.degraded
    );
    assert!(module_len(&wb, "pages", None) > 0);
    assert!(wb.status.contains("boris/0.9.0"), "{}", wb.status);
}

#[test]
fn research_notes_persist_across_reopen() {
    let ws = workspace("happy");
    let cfg = tempfile::tempdir().unwrap();
    let env = absent_env(cfg.path());
    let mut wb = init();
    open(&mut wb, ws.path(), &env);
    wb.run_command("shell.goto.research");
    wb.run_command("research.capture.idea");
    wb.run_command("research.capture.idea");
    assert_eq!(module_len(&wb, "research", Some("all")), 2);
    assert!(ws.path().join(".cv/research/notes.jsonl").is_file());

    let mut again = init();
    open(&mut again, ws.path(), &env);
    assert_eq!(module_len(&again, "research", Some("all")), 2);
    // A promotion without a gated engine refuses rather than writing.
    assert!(!ws.path().join("content/idea-1.md").exists());
}

#[test]
fn open_and_recent_commands_queue_requests() {
    let a = workspace("happy");
    let b = workspace("happy");
    let cfg = tempfile::tempdir().unwrap();
    let env = absent_env(cfg.path());
    let mut wb = init();
    open(&mut wb, a.path(), &env);
    open(&mut wb, b.path(), &env);
    assert_eq!(wb.recent.len(), 2);
    assert_eq!(env.recent().entries, wb.recent, "recent list persisted");
    assert!(wb.recent[0].ends_with(b.path().file_name().unwrap()));

    wb.run_command("shell.workspace.open");
    wb.run_command("shell.workspace.recent.1");
    assert_eq!(
        wb.requests,
        vec![
            ShellRequest::PickWorkspace,
            ShellRequest::OpenWorkspace(wb.recent[1].clone())
        ]
    );
    assert!(
        wb.commands()
            .iter()
            .any(|c| c.id == "shell.workspace.recent.1")
    );
    assert!(open_workspace(&mut wb, a.path().join("missing"), &env).is_none());
    assert!(wb.status.contains("not a folder"));
}

/// Live smoke against a REAL pinned boris; ignored in CI. Run with
/// `CV_BORIS_BIN=/path/to/boris cargo test -p cv-app --test workspace -- --ignored`.
#[test]
#[ignore = "needs a real pinned boris via CV_BORIS_BIN"]
fn live_boris_workspace_boot() {
    // A profile root: bare content roots can't build in place (Boris
    // rejects an output dir nested in the content root).
    let ws = workspace("dogfood");
    let cfg = tempfile::tempdir().unwrap();
    let env = WorkspaceEnv {
        locator: None,
        recent_file: Some(cfg.path().join("recent.json")),
    };
    let mut wb = init();
    open(&mut wb, ws.path(), &env);
    wait_for(&mut wb, "watch handshake", |wb| {
        wb.app.engine.as_deref() == Some("boris/0.9.0")
    });
    wait_for(&mut wb, "live diagnostics", |wb| {
        module_len(wb, "activity", Some("diagnostics")) > 0
    });
    wb.stop_watch();
}

#[test]
fn promotion_writes_under_the_profile_input() {
    let ws = workspace("happy");
    std::fs::rename(ws.path().join("content"), ws.path().join("docs")).unwrap();
    let profile = std::fs::read_to_string(ws.path().join("boris.json")).unwrap();
    std::fs::write(
        ws.path().join("boris.json"),
        profile.replace("\"input\": \"content\"", "\"input\": \"docs\""),
    )
    .unwrap();
    let cfg = tempfile::tempdir().unwrap();
    let fake = Fake::new("happy", None);
    let mut wb = init();
    open(&mut wb, ws.path(), &fake.env(cfg.path()));
    wait_for(&mut wb, "watch handshake", |wb| wb.app.engine.is_some());
    assert!(module_len(&wb, "pages", None) > 0, "pages read from docs/");
    wb.run_command("shell.goto.research");
    wb.run_command("research.capture.idea");
    wb.set_nav("all");
    wb.select_key("note-0001");
    wb.run_command("research.promote");
    assert!(
        ws.path().join("docs/notes/idea-1.md").is_file(),
        "{}",
        wb.status
    );
    assert!(!ws.path().join("content").exists());
}

#[test]
fn research_storage_failure_survives_the_engine_outcome() {
    let ws = workspace("happy");
    std::fs::create_dir_all(ws.path().join(".cv")).unwrap();
    std::fs::write(ws.path().join(".cv/research"), b"not a dir").unwrap();
    let cfg = tempfile::tempdir().unwrap();
    let mut wb = init();
    open(&mut wb, ws.path(), &absent_env(cfg.path()));
    assert_eq!(wb.app.degraded, Some(DegradedReason::StoreCorrupt));
    assert!(
        wb.status.contains("research notes not persisted"),
        "{}",
        wb.status
    );
    assert!(wb.status.contains("boris not found"), "{}", wb.status);
}

#[test]
fn open_editors_block_a_workspace_switch() {
    let a = workspace("happy");
    let b = workspace("happy");
    let cfg = tempfile::tempdir().unwrap();
    let env = absent_env(cfg.path());
    let mut wb = init();
    open(&mut wb, a.path(), &env);
    cv_app::editor::open_page(&mut wb, a.path(), "content/index.md").unwrap();
    assert!(open_workspace(&mut wb, b.path().to_path_buf(), &env).is_none());
    assert!(wb.status.contains("editor window"), "{}", wb.status);
    assert!(
        wb.workspace
            .as_deref()
            .unwrap()
            .ends_with(a.path().file_name().unwrap())
    );
}

#[cfg(unix)]
#[test]
fn symlinked_input_outside_the_workspace_gets_no_promotion_writer() {
    let ws = workspace("happy");
    let outside = tempfile::Builder::new()
        .prefix("cv-out-")
        .tempdir()
        .unwrap();
    let content = ws.path().join("content");
    copy_tree(&content, outside.path());
    std::fs::remove_dir_all(&content).unwrap();
    std::os::unix::fs::symlink(outside.path(), &content).unwrap();
    let cfg = tempfile::tempdir().unwrap();
    let fake = Fake::new("happy", None);
    let mut wb = init();
    open(&mut wb, ws.path(), &fake.env(cfg.path()));
    wait_for(&mut wb, "watch handshake", |wb| wb.app.engine.is_some());
    wb.run_command("shell.goto.research");
    wb.run_command("research.capture.idea");
    wb.set_nav("all");
    wb.select_key("note-0001");
    wb.run_command("research.promote");
    assert!(!outside.path().join("notes").exists(), "{}", wb.status);
}
