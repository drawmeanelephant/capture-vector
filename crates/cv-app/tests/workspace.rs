//! Workspace open flow: research persistence under `.cv/`, gated engine
//! boot (fake-boris shim from cv-engine's tests, never a real boris),
//! engine-absent / version-mismatch disclosure, live watch diagnostics
//! through the cursor-resync sink, and the open/recent commands.

use cv_app::{WorkspaceEnv, init, open_workspace};
use cv_core::events::shell::DegradedReason;
use cv_engine::locate::Locator;
use cv_model::dialog::PickPurpose;
use cv_model::module::RowModel;
use cv_model::new_workspace;
use cv_model::windows::Companion;
use cv_model::workbench::{KeyInput, KeyPress, ShellRequest, Workbench};
use cv_widgets::editor::{self, host};
use cv_widgets::harness::Harness;
use cv_widgets::shell::degraded_text;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const SHIM_SH: &str = include_str!("../../cv-engine/tests/fake-boris/boris");
const SHIM_BAT: &str = include_str!("../../cv-engine/tests/fake-boris/boris.bat");

#[test]
fn new_workspace_from_an_archetype_creates_and_opens_the_workspace() {
    let parent = tempfile::tempdir().unwrap();
    let recent = tempfile::tempdir().unwrap();
    let fake = Fake::new("happy", None);
    fake.set(
        "verbs/init/stderr.txt",
        cv_fixtures::StubEngine::init_help().as_bytes(),
    );
    copy_tree(
        &cv_fixtures::fixtures_root().join("captures/init/docs/tree"),
        &fake.dir.path().join("verbs/init/tree"),
    );
    let env = fake.env(recent.path());
    let mut wb = init();

    cv_app::new_workspace::probe_archetypes(&mut wb, &env)
        .expect("inventory worker")
        .join()
        .unwrap();
    wb.drain();
    let archetypes = wb.archetypes.as_ref().expect("archetypes located");
    assert_eq!(archetypes.names.len(), 15);
    assert_eq!(archetypes.names[archetypes.default], "docs");

    wb.key(&KeyPress {
        key: KeyInput::Command('n'),
        shift: true,
    });
    assert_eq!(
        wb.dialogs.open().map(|d| d.spec.id.as_str()),
        Some(new_workspace::NEW)
    );
    wb.picked(
        PickPurpose::DialogField {
            dialog: new_workspace::NEW.into(),
            field: 1,
        },
        Some(parent.path().to_path_buf()),
    );
    wb.dialog_field(2, None);
    for c in "created-site".chars() {
        wb.key(&KeyPress::new(KeyInput::Char(c)));
    }
    wb.dialog_act("create");
    assert_eq!(
        wb.dialogs.open().map(|d| d.spec.id.as_str()),
        Some(new_workspace::CREATING),
        "creation discloses progress"
    );
    let draft = match wb.requests.pop() {
        Some(ShellRequest::CreateWorkspace(draft)) => draft,
        other => panic!("unexpected request {other:?}"),
    };
    assert_eq!(draft.archetype, "docs");
    assert_eq!(draft.parent, parent.path());
    assert_eq!(draft.name, "created-site");

    fake.set(
        "verbs/init/stderr.txt",
        cv_fixtures::StubEngine::init_report("docs").as_bytes(),
    );
    fake.set("verbs/init/exit-code.txt", b"0");
    cv_app::new_workspace::create_workspace(&mut wb, draft, &env)
        .expect("create worker")
        .join()
        .unwrap();
    wb.drain();
    let root = match wb.requests.pop() {
        Some(ShellRequest::OpenWorkspace(root)) => root,
        other => panic!("unexpected request {other:?}"),
    };
    assert_eq!(
        root.file_name().and_then(|n| n.to_str()),
        Some("created-site")
    );
    assert!(
        root.join("content/index.md").is_file(),
        "the docs starter materialized"
    );

    open(&mut wb, &root, &env);
    assert_eq!(wb.workspace.as_deref(), Some(root.as_path()));
    assert!(wb.dialogs.open().is_none(), "creation progress closed");
    assert!(
        module_len(&wb, "pages", None) >= 3,
        "the opened starter has pages"
    );
}

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

fn doctor_row(wb: &Workbench, nav: &str, key: &str) -> RowModel {
    let m = wb
        .registry
        .get(wb.registry.index_of("doctor").unwrap())
        .unwrap();
    let i = m
        .index_of_key(Some(nav), key)
        .unwrap_or_else(|| panic!("doctor has no {key} under {nav}"));
    m.row(Some(nav), i)
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
    assert_eq!(
        doctor_row(&wb, "engine", "engine.status").title,
        "Boris not found"
    );
    let lock = doctor_row(&wb, "store", "store.lock.research");
    assert_eq!(lock.detail, "this window", "{lock:?}");
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
    let status = doctor_row(&wb, "engine", "engine.status");
    assert_eq!(
        (status.title.as_str(), status.detail.as_str()),
        ("boris/0.8.2", "CV_BORIS_BIN")
    );
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
    let status = doctor_row(&wb, "engine", "engine.status");
    assert_eq!(status.title, "boris/0.9.0", "{status:?}");
    assert!(module_len(&wb, "doctor", Some("probe")) > 1, "probe facts");
    assert_eq!(
        doctor_row(&wb, "watch", "watch.handshake").detail,
        "boris/0.9.0"
    );
}

#[test]
fn failed_diagnostic_build_stops_retries_and_discloses_the_failure() {
    let ws = workspace("broken-frontmatter");
    let cfg = tempfile::tempdir().unwrap();
    let fake = Fake::new("broken-frontmatter", None);
    fake.set("watch-exit.txt", b"1\n");
    // The captured watch failure is fatal and empty. Its one-shot
    // diagnostic build fails without diagnostics too.
    fake.set("verbs/build/stderr.txt", b"");
    fake.set("verbs/build/exit-code.txt", b"3\n");
    let mut wb = init();
    for expected_attempts in [1, 2] {
        open(&mut wb, ws.path(), &fake.env(cfg.path()));
        wait_for(&mut wb, "failed diagnostic build disclosure", |wb| {
            wb.app.degraded == Some(DegradedReason::EngineBuildFailing)
                && wb.status.contains("reopen workspace")
        });
        let argv = std::fs::read_to_string(fake.dir.path().join("argv.log")).unwrap();
        assert_eq!(
            argv.lines()
                .filter(|line| line.split_whitespace().next() == Some("build"))
                .count(),
            expected_attempts,
            "only one diagnostic build per workspace open"
        );
    }
    let tree = Harness::new(wb).tree().to_string();
    assert!(tree.contains(&degraded_text(&DegradedReason::EngineBuildFailing)));
}

#[test]
fn successful_diagnostic_build_does_not_report_the_watcher_as_live() {
    let ws = workspace("broken-frontmatter");
    let cfg = tempfile::tempdir().unwrap();
    let fake = Fake::new("broken-frontmatter", None);
    fake.set("watch-exit.txt", b"1\n");
    fake.set("verbs/build/stderr.txt", b"");
    fake.set("verbs/build/exit-code.txt", b"0\n");
    let mut wb = init();
    open(&mut wb, ws.path(), &fake.env(cfg.path()));
    wait_for(&mut wb, "successful diagnostic build disclosure", |wb| {
        wb.status.contains("one-shot build succeeded")
    });
    assert_eq!(wb.app.degraded, Some(DegradedReason::WatchDead));
    assert!(wb.status.contains("reopen workspace"));
}

#[test]
fn polling_fallback_rebuilds_on_edits_without_an_automatic_retry() {
    let ws = workspace("broken-frontmatter");
    let cfg = tempfile::tempdir().unwrap();
    let fake = Fake::new("broken-frontmatter", None);
    fake.set("watch-refuse.txt", b"usage: watch unsupported\n");
    fake.set("verbs/build/stderr.txt", b"");
    fake.set("verbs/build/exit-code.txt", b"3\n");
    let mut wb = init();
    open(&mut wb, ws.path(), &fake.env(cfg.path()));
    assert!(wb.status.contains("one-shot rebuilds"), "{}", wb.status);
    wait_for(&mut wb, "initial polling build", |wb| {
        module_len(wb, "activity", Some("builds")) == 1
    });
    std::fs::write(ws.path().join("added.md"), "# Added\n").unwrap();
    wait_for(&mut wb, "polling build after an edit", |wb| {
        module_len(wb, "activity", Some("builds")) == 2
    });
    assert!(wb.status.contains("one-shot rebuilds"), "{}", wb.status);
    wb.stop_watch();
    let argv = std::fs::read_to_string(fake.dir.path().join("argv.log")).unwrap();
    assert_eq!(
        argv.lines()
            .filter(|line| line.split_whitespace().next() == Some("build"))
            .count(),
        2
    );
}

/// Capture an idea through the capture prompt, as a typist would.
fn capture(wb: &mut Workbench, text: &str) {
    wb.run_command("research.capture.idea");
    for c in text.chars() {
        wb.key(&KeyPress::new(KeyInput::Char(c)));
    }
    wb.key(&KeyPress::new(KeyInput::Enter));
    assert!(!wb.dialogs.is_open(), "{}", wb.status);
}

#[test]
fn research_notes_are_exclusive_and_persist_across_reopen() {
    let ws = workspace("happy");
    let cfg = tempfile::tempdir().unwrap();
    let env = absent_env(cfg.path());
    let mut wb = init();
    open(&mut wb, ws.path(), &env);
    wb.run_command("shell.goto.research");
    capture(&mut wb, "Idea 1");
    capture(&mut wb, "Idea 2");
    assert_eq!(module_len(&wb, "research", Some("all")), 2);
    assert!(ws.path().join(".cv/research/notes.jsonl").is_file());

    let mut again = init();
    open(&mut again, ws.path(), &env);
    assert!(
        again.status.contains("research notes not persisted:"),
        "{}",
        again.status
    );
    assert_eq!(module_len(&again, "research", Some("all")), 0);

    drop(wb);
    open(&mut again, ws.path(), &env);
    assert!(
        !again.status.contains("research notes not persisted:"),
        "{}",
        again.status
    );
    assert_eq!(module_len(&again, "research", Some("all")), 2);
    // A promotion without a gated engine refuses rather than writing.
    assert!(!ws.path().join("content/idea-1.md").exists());
}

#[test]
fn publish_ledger_loads_read_only_on_open_and_a_corrupt_one_is_disclosed() {
    let ws = workspace("happy");
    cv_app::publish::scene::seed(ws.path());
    let journal = ws.path().join(".cv/ledger/ledger.jsonl");
    let cfg = tempfile::tempdir().unwrap();
    let env = absent_env(cfg.path());
    let mut wb = init();
    open(&mut wb, ws.path(), &env);
    assert_ne!(wb.app.degraded, Some(DegradedReason::StoreCorrupt));
    assert_eq!(module_len(&wb, "publish", Some("runs")), 4);
    assert_eq!(module_len(&wb, "publish", Some("unresolved")), 1);
    drop(wb);

    cv_app::publish::scene::corrupt(ws.path());
    let corrupt = std::fs::read(&journal).unwrap();
    let mut again = init();
    open(&mut again, ws.path(), &env);
    assert_eq!(again.app.degraded, Some(DegradedReason::StoreCorrupt));
    assert!(
        again.status.contains("publish ledger unreadable"),
        "{}",
        again.status
    );
    // Nothing guessed from a journal that cannot be trusted; it stays as found.
    assert_eq!(module_len(&again, "publish", Some("runs")), 1);
    assert_eq!(module_len(&again, "publish", Some("completed")), 0);
    assert_eq!(std::fs::read(&journal).unwrap(), corrupt);
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
    capture(&mut wb, "Idea 1");
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
fn undecodable_journal_lines_open_as_a_disclosed_degradation() {
    let ws = workspace("happy");
    let cfg = tempfile::tempdir().unwrap();
    let env = absent_env(cfg.path());
    let mut wb = init();
    open(&mut wb, ws.path(), &env);
    wb.run_command("shell.goto.research");
    capture(&mut wb, "Idea 1");
    assert_eq!(module_len(&wb, "research", Some("all")), 1);
    // A complete-but-undecodable committed line (not a torn tail).
    use std::io::Write;
    let mut j = std::fs::OpenOptions::new()
        .append(true)
        .open(ws.path().join(".cv/research/notes.jsonl"))
        .unwrap();
    writeln!(j, "{{not json").unwrap();
    drop(j);
    drop(wb);

    let mut again = init();
    open(&mut again, ws.path(), &env);
    assert_eq!(again.app.degraded, Some(DegradedReason::StoreCorrupt));
    assert!(again.status.contains("undecodable"), "{}", again.status);
    // The good lines still loaded — degradation, not data loss.
    assert_eq!(module_len(&again, "research", Some("all")), 1);
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
    capture(&mut wb, "Idea 1");
    wb.set_nav("all");
    wb.select_key("note-0001");
    wb.run_command("research.promote");
    assert!(!outside.path().join("notes").exists(), "{}", wb.status);
}

#[test]
fn installed_package_without_fixtures_boots_empty() {
    let mut wb = init();
    cv_app::boot_without_workspace(&mut wb, false);
    assert_eq!(wb.app.snapshot_epoch, 0);
    assert!(wb.status.contains("Open workspace"), "{}", wb.status);
    assert!(!wb.status.contains("fixture demo"), "{}", wb.status);

    let mut wb = init();
    cv_app::boot_without_workspace(&mut wb, cv_app::demo_available());
    assert_ne!(wb.app.snapshot_epoch, 0, "checkout fixtures load the demo");
}

fn ids(wb: &Workbench) -> Vec<&'static str> {
    wb.registry.iter().map(|m| m.spec().id).collect()
}

fn demo_commands(wb: &Workbench) -> Vec<String> {
    wb.registry
        .iter()
        .flat_map(|m| m.commands())
        .map(|c| c.id)
        .filter(|id| id.starts_with("demo."))
        .collect()
}

#[test]
fn only_the_fixture_demo_registers_the_demo_module() {
    let (_ws, _cfg, real) = happy_workspace();
    assert!(!ids(&real).contains(&"demo"), "{:?}", ids(&real));
    assert_eq!(demo_commands(&real), Vec::<String>::new());

    let mut demo = init();
    cv_app::boot_without_workspace(&mut demo, true);
    assert!(ids(&demo).contains(&"demo"));
    assert!(!demo_commands(&demo).is_empty());
    let core: Vec<_> = ids(&demo).into_iter().filter(|i| *i != "demo").collect();
    assert_eq!(ids(&real), core, "core rail order (mod+1…9) is unchanged");

    let mut empty = init();
    cv_app::boot_without_workspace(&mut empty, false);
    assert!(!ids(&empty).contains(&"demo"), "{:?}", ids(&empty));
}

/// A real workspace launch: the rail has no Demo. Engine-absent keeps the
/// boot synchronous, so the tree is deterministic; the status line names
/// the temp workspace, so it is cleared.
#[test]
fn real_workspace_launch_golden() {
    let (_ws, _cfg, mut wb) = happy_workspace();
    wb.status.clear();
    let h = Harness::new(wb);
    assert!(h.has("rail-pages") && !h.has("rail-demo"));
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/ui/golden/tree");
    for (name, v) in [
        ("workspace.tree", h.tree()),
        ("workspace.layout", h.layout()),
    ] {
        cv_widgets::harness::check_golden(&dir, name, &v).unwrap();
    }
}

#[test]
fn restart_watch_reopens_the_workspace_and_a_new_watch_comes_up() {
    let ws = workspace("happy");
    let cfg = tempfile::tempdir().unwrap();
    let fake = Fake::new("happy", None);
    fake.set("watch-exit.txt", b"1\n");
    fake.set("verbs/build/stderr.txt", b"");
    fake.set("verbs/build/exit-code.txt", b"0\n");
    let env = fake.env(cfg.path());
    let mut wb = init();
    open(&mut wb, ws.path(), &env);
    wait_for(&mut wb, "watch death", |wb| {
        wb.app.degraded == Some(DegradedReason::WatchDead)
    });
    wb.run_command("shell.watch.restart");
    let root = wb.workspace.clone().unwrap();
    assert_eq!(
        std::mem::take(&mut wb.requests),
        vec![ShellRequest::OpenWorkspace(root.clone())]
    );
    std::fs::remove_file(fake.dir.path().join("watch-exit.txt")).unwrap();
    open(&mut wb, &root, &env);
    wait_for(&mut wb, "new watch handshake", |wb| {
        wb.app.degraded.is_none() && wb.app.engine.as_deref() == Some("boris/0.9.0")
    });
}

/// Select `page` in Pages and press Enter, serving the requests it queues
/// as the native shell does. The companion that opened for it.
fn enter_on_page(wb: &mut Workbench, page: &str) -> Companion {
    wb.select_key(page);
    assert_eq!(wb.selected_key(), Some(page));
    assert!(wb.key(&KeyPress::new(KeyInput::Enter)));
    for r in std::mem::take(&mut wb.requests) {
        match r {
            ShellRequest::EditPage(p) => (cv_app::launch().edit)(wb, &p),
            other => panic!("unexpected request {other:?}"),
        }
    }
    wb.windows
        .companions
        .last()
        .cloned()
        .expect("a companion opened")
}

fn happy_workspace() -> (tempfile::TempDir, tempfile::TempDir, Workbench) {
    let ws = workspace("happy");
    let cfg = tempfile::tempdir().unwrap();
    let mut wb = init();
    open(&mut wb, ws.path(), &absent_env(cfg.path()));
    (ws, cfg, wb)
}

#[test]
fn enter_on_a_pages_row_opens_the_editor_on_that_page() {
    let (ws, _cfg, mut wb) = happy_workspace();
    let c = enter_on_page(&mut wb, "guides/getting-started.md");
    assert_eq!(
        (c.module.as_str(), c.key.as_str()),
        (editor::MODULE, "content/guides/getting-started.md")
    );
    let disk = std::fs::read_to_string(ws.path().join(&c.key)).unwrap();
    let w = host::window(c.id, &c.key, &wb.theme).expect("buffer open for the companion");
    assert_eq!(w.error(), None);
    assert!(w.writer.is_some(), "saves have somewhere to go");
    let ses = host::session();
    assert_eq!(ses.borrow().get(w.buffer).unwrap().buffer.text(), disk);
}

#[test]
fn a_companion_editor_from_pages_shows_every_panel_at_its_initial_size() {
    let (_ws, _cfg, mut wb) = happy_workspace();
    let c = enter_on_page(&mut wb, "index.md");
    let w = host::window(c.id, &c.key, &wb.theme).expect("buffer open for the companion");
    let mut rt = editor::runtime(w);
    rt.frame(1.0 / 30.0);
    let (w, h) = editor::SIZE;
    assert!(w >= 960.0 && h >= 560.0, "{:?}", editor::SIZE);
    for key in [
        "editor-header",
        "editor-form",
        "editor-body",
        "problems",
        "draft-sketch",
    ] {
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no {key}"));
        assert!(x > 0.0 && x < w && y > 0.0 && y < h, "{key} at ({x}, {y})");
    }
}

#[test]
fn typing_and_saving_in_a_companion_editor_from_pages_writes_the_page() {
    let (ws, _cfg, mut wb) = happy_workspace();
    let c = enter_on_page(&mut wb, "guides/getting-started.md");
    let path = ws.path().join(&c.key);
    let before = std::fs::read_to_string(&path).unwrap();
    let mut rt = editor::runtime(host::window(c.id, &c.key, &wb.theme).unwrap());
    for _ in 0..40 {
        rt.frame(1.0 / 30.0);
    }
    let (x, y) = rt.find("editor-body").expect("body editor");
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 30.0);
    rt.state.body.set_caret(0);
    let body_at = before.len() - rt.state.body.text.len();
    rt.paste("Edited from Pages. ");
    rt.frame(1.0 / 30.0);
    assert!(rt.state.is_dirty());
    rt.key(cv_widgets::keys::parse("mod+s").remove(0));
    rt.frame(0.0);
    rt.state.wait_save();
    rt.frame(0.0);
    assert!(!rt.state.is_dirty(), "{:?}", rt.state.save_status());
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        format!(
            "{}Edited from Pages. {}",
            &before[..body_at],
            &before[body_at..]
        )
    );
}

#[test]
fn pages_and_cv_edit_open_one_editor_for_the_same_page() {
    let (ws, _cfg, mut wb) = happy_workspace();
    let booted = cv_app::editor::open_page(&mut wb, ws.path(), "content/index.md").unwrap();
    let c = enter_on_page(&mut wb, "index.md");
    assert_eq!(c.id, booted);
    assert_eq!(wb.windows.companions.len(), 1);
}
