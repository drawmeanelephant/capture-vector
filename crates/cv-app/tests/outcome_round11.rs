//! Round-11 outcome identity/ordering regressions.

mod common;

use cv_app::{WorkspaceEnv, open_workspace};
use cv_core::engine::CancelToken;
use cv_engine::locate::Locator;
use cv_fixtures::lab::StubLab;
use cv_model::adoption::{AdoptionJob, AdoptionRequest, AdoptionTarget};
use cv_model::workbench::Workbench;
use cv_store::imports::{self as store, Finished, RecordedLab, RunRecord};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const BORIS_SHIM: &str = include_str!("../../cv-engine/tests/fake-boris/boris");

fn write(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

#[cfg(unix)]
fn fake_boris() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("boris");
    const HOLD_CASE: &str = r#"    if [ -f "$D/watch-hold" ]; then"#;
    const WORKSPACE_HOLD_CASE: &str =
        r#"    if [ -f "$D/watch-hold" ] && ! printf '%s' "$*" | grep -q -- '--serve'; then"#;
    const VERB_HOLD: &str = r#"if [ -f "$D/verbs/$V/hold" ]; then sleep 300 & wait; fi"#;
    const CONTROLLED_VERB_HOLD: &str = r#"if [ -f "$D/verbs/$V/hold" ]; then while [ -f "$D/verbs/$V/hold" ]; do sleep 0.02; done; fi"#;
    assert!(BORIS_SHIM.contains(HOLD_CASE));
    assert!(BORIS_SHIM.contains(VERB_HOLD));
    let shim = BORIS_SHIM
        .replace(HOLD_CASE, WORKSPACE_HOLD_CASE)
        .replace(VERB_HOLD, CONTROLLED_VERB_HOLD);
    common::install_executable(&bin, &shim);
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
    write(
        &dir.path().join("watch.ndjson"),
        b"{\"event\":\"hello\",\"watch_events_schema\":1,\"compiler\":\"boris/0.9.0\"}\n",
    );
    write(&dir.path().join("watch-hold"), b"");
    (dir, bin)
}

fn env_for(boris: &Path, recent_file: &Path) -> WorkspaceEnv {
    WorkspaceEnv {
        locator: Some(Locator {
            env_override: Some(boris.to_path_buf()),
            bundled_dirs: Vec::new(),
            path_var: None,
            sibling_dirs: Vec::new(),
        }),
        lab_locator: Some(Locator::default()),
        recent_file: Some(recent_file.to_path_buf()),
    }
}

fn workspace() -> (tempfile::TempDir, PathBuf) {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().canonicalize().unwrap();
    write(
        &p.join("content/index.md"),
        b"---\ntitle: Home\n---\nHome\n",
    );
    (t, p)
}

fn seed_unique_run(root: &Path, run_id: &str) {
    let lock = store::claim_run(root, run_id).unwrap();
    let transcript = StubLab::new()
        .run("hugo", &lock.dir().join(store::OUT))
        .unwrap();
    let mut rec = RunRecord::started(
        run_id,
        "hugo",
        RecordedLab {
            tool_id: StubLab::new().tool_id(),
            source: "fixture".into(),
            path: "fixtures/captures/lab".into(),
        },
        1_764_100_000_000,
    );
    rec.argv = transcript.argv.clone();
    rec.finished = Some(Finished {
        exit_code: Some(0),
        exit: store::FinishedExit::Ok,
        outcome: "exited 0".into(),
        duration_ms: 0,
    });
    store::write(&lock, &rec).unwrap();
    // The concurrency being probed is outcome ownership, not migration
    // collision logic. Replace the shared captured tree with one minimal
    // page whose frontmatter id is unique to this run, so two adoptions are
    // both legitimate and disjoint.
    let content = lock.dir().join(store::OUT).join("content");
    std::fs::remove_dir_all(&content).unwrap();
    write(
        &content.join(format!("{run_id}.md")),
        format!("---\nid: {run_id}\ntitle: {run_id}\nstatus: published\n---\n{run_id}\n")
            .as_bytes(),
    );
}

fn settle(wb: &mut Workbench, dur: Duration) {
    let end = Instant::now() + dur;
    while Instant::now() < end {
        wb.drain();
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn join_with_timeout<T: Send + 'static>(
    h: std::thread::JoinHandle<T>,
    dur: Duration,
    what: &str,
) -> T {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(h.join());
    });
    match rx.recv_timeout(dur) {
        Ok(r) => r.unwrap_or_else(|_| panic!("{what} panicked")),
        Err(e) => panic!("{what} did not finish in {dur:?}: {e}"),
    }
}

fn argv_lines(dir: &Path) -> Vec<String> {
    std::fs::read_to_string(dir.join("argv.log"))
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

fn count_argv(dir: &Path, exact: &str) -> usize {
    argv_lines(dir)
        .into_iter()
        .filter(|l| l == exact || l.trim_start().starts_with(&format!("{exact} ")))
        .count()
}

fn wait_for_argv(dir: &Path, exact: &str, want: usize) {
    let deadline = Instant::now() + Duration::from_secs(8);
    while count_argv(dir, exact) < want {
        assert!(
            Instant::now() < deadline,
            "never saw {want} {exact} invocation(s)"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn wait_for_argv_draining(wb: &mut Workbench, dir: &Path, exact: &str, want: usize) {
    let deadline = Instant::now() + Duration::from_secs(8);
    while count_argv(dir, exact) < want {
        wb.drain();
        assert!(
            Instant::now() < deadline,
            "never saw {want} {exact} invocation(s)"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[derive(Debug)]
struct ActivityRow {
    title: String,
    seen: usize,
}

fn activity_rows(wb: &Workbench) -> Vec<ActivityRow> {
    let i = wb.registry.index_of("activity").expect("activity module");
    let m = wb.registry.get(i).expect("activity module");
    (0..m.list_len(None))
        .map(|r| {
            let row = m.row(None, r);
            let reading = m.reading(&row.key);
            let seen_ids: Vec<String> = reading
                .lines
                .iter()
                .filter(|l| l.label.as_deref() == Some("seen"))
                .map(|l| l.text.split(" · ").next().unwrap_or("").to_string())
                .collect();
            ActivityRow {
                title: row.title,
                seen: seen_ids.len(),
            }
        })
        .collect()
}

fn exact_rows(wb: &Workbench, title: &str) -> Vec<ActivityRow> {
    activity_rows(wb)
        .into_iter()
        .filter(|r| r.title == title)
        .collect()
}

fn prefix_rows(wb: &Workbench, prefix: &str) -> Vec<ActivityRow> {
    activity_rows(wb)
        .into_iter()
        .filter(|r| r.title.starts_with(prefix))
        .collect()
}

fn migrate_badges(wb: &Workbench) -> Vec<(String, String)> {
    let i = wb.registry.index_of("migrate").expect("migrate module");
    let m = wb.registry.get(i).expect("migrate module");
    (0..m.list_len(Some("runs")))
        .map(|r| {
            let row = m.row(Some("runs"), r);
            (row.title, row.badge.map(|b| b.text).unwrap_or_default())
        })
        .collect()
}

fn stop_all(root: &Path) {
    if let Some(s) = cv_app::slots::scheduler_for(root)
        && let Some(w) = s.watch_session()
    {
        s.stop_watch_session(&w);
    }
}

fn healthy(wb: &Workbench, ws: &Path) -> (bool, String) {
    let sched = cv_app::slots::scheduler_for(ws).expect("stable scheduler");
    let state = format!(
        "sched_watch={} ui_watch={} degraded={:?} dialog={:?} status={:?}",
        sched.watch_session().is_some(),
        wb.app.watch_live,
        wb.app.degraded,
        wb.dialogs.open().map(|d| d.spec.id.clone()),
        wb.status,
    );
    (
        sched.watch_session().is_some() && wb.app.watch_live && wb.app.degraded.is_none(),
        state,
    )
}

fn adoption_job(ws: &Path, run_id: &str, subfolder: &str) -> AdoptionJob {
    AdoptionJob {
        source_root: ws.to_path_buf(),
        request: AdoptionRequest {
            run_id: run_id.into(),
            target: AdoptionTarget::ExistingWorkspace {
                subfolder: subfolder.into(),
            },
        },
        cancel: CancelToken::new(),
    }
}

fn validate_rows(wb: &Workbench) -> Vec<ActivityRow> {
    prefix_rows(wb, "validate")
}

fn print_rows(label: &str, wb: &Workbench) -> Vec<(usize, String, String)> {
    let i = wb.registry.index_of("activity").expect("activity module");
    let m = wb.registry.get(i).expect("activity module");
    let rows: Vec<(usize, String, String)> = (0..m.list_len(None))
        .map(|r| {
            let row = m.row(None, r);
            (r, row.title, row.detail)
        })
        .collect();
    println!("{label} rows(newest-first)={rows:?}");
    rows
}

/// The queue-before-live commit rule must not rewrite causality when the UI
/// legitimately batches drains. Here the restored watch's initial build is
/// emitted before the held adoption validate can finish. In newest-first
/// Activity, the later validate terminal therefore belongs above that build.
#[cfg(unix)]
#[test]
fn r11_outcome_replay_cannot_jump_a_causally_earlier_live_watch_build() {
    let (boris_dir, boris) = fake_boris();
    write(&boris_dir.path().join("verbs/validate/exit-code.txt"), b"1");
    let (_w, ws) = workspace();
    let recent = tempfile::tempdir().unwrap();
    let env = env_for(&boris, &recent.path().join("recent.json"));
    seed_unique_run(&ws, "r11order");
    let mut wb = cv_app::init();
    join_with_timeout(
        open_workspace(&mut wb, ws.clone(), &env).expect("initial boot starts"),
        Duration::from_secs(8),
        "initial boot",
    );
    settle(&mut wb, Duration::from_millis(250));

    // The first watch emitted only its hello. The post-copy restored watch
    // will emit a successful initial build before the held validate exits.
    write(
        &boris_dir.path().join("watch.ndjson"),
        br#"{"event":"hello","watch_events_schema":1,"compiler":"boris/0.9.0"}
{"event":"build-started","phase":"initial","mode":"html","targets":["default"]}
{"event":"build-succeeded","phase":"initial","mode":"html","targets":["default"],"pages_written":77,"duration_ms":4}
"#,
    );
    write(&boris_dir.path().join("verbs/validate/hold"), b"");
    let adoption =
        cv_app::migrate::adopt::adopt(&mut wb, adoption_job(&ws, "r11order", "imported"), &env)
            .expect("adoption starts");
    wait_for_argv(boris_dir.path(), "validate", 1);
    // Do not drain: a busy UI frame is allowed to batch the already-emitted
    // watch build and the later validate terminal into one drain.
    std::thread::sleep(Duration::from_millis(100));
    std::fs::remove_file(boris_dir.path().join("verbs/validate/hold")).unwrap();
    join_with_timeout(adoption, Duration::from_secs(12), "adoption worker");
    wb.drain();

    let rows = print_rows("R11-COMMIT-BEFORE-LIVE", &wb);
    let iv = rows.iter().position(|(_, t, _)| t.starts_with("validate"));
    let ib = rows
        .iter()
        .position(|(_, t, d)| t.starts_with("Build succeeded") && d.contains("77 pages"));
    println!("R11-COMMIT-BEFORE-LIVE validate_index={iv:?} watch_build_index={ib:?}");
    assert!(iv.is_some(), "validate terminal missing");
    assert!(ib.is_some(), "restored watch build missing");
    assert!(
        iv.unwrap() < ib.unwrap(),
        "validate finished after the restored watch build, but Activity shows it older (newest-first rows): {rows:?}"
    );
    stop_all(&ws);
}

/// Distinct Unix paths whose bytes are not UTF-8 can collapse to the same
/// lossy string. Workspace identity must not use that lossy display string:
/// workspace A's held adoption outcome is foreign in workspace B even when
/// both names render as the same replacement character.
#[cfg(unix)]
#[test]
fn r11_non_utf8_lossy_identity_collision_must_not_pollute_foreign_workspace() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let (boris_dir_a, boris_a) = fake_boris();
    write(
        &boris_dir_a.path().join("verbs/validate/exit-code.txt"),
        b"1",
    );
    let (_boris_dir_b, boris_b) = fake_boris();
    let base = tempfile::tempdir().unwrap();
    let raw_a = base.path().join(OsString::from_vec(b"ws-\xff".to_vec()));
    let raw_b = base.path().join(OsString::from_vec(b"ws-\xfe".to_vec()));
    if let Err(e) = std::fs::create_dir_all(&raw_a).and_then(|_| std::fs::create_dir_all(&raw_b)) {
        println!("R11-LOSSY-SKIP host filesystem rejects non-UTF-8 workspace names: {e}");
        return;
    }
    let (Ok(ws_a), Ok(ws_b)) = (raw_a.canonicalize(), raw_b.canonicalize()) else {
        println!("R11-LOSSY-SKIP host filesystem cannot canonicalize non-UTF-8 workspace names");
        return;
    };
    assert_ne!(ws_a, ws_b);
    assert_eq!(ws_a.to_string_lossy(), ws_b.to_string_lossy());
    write(&ws_a.join("content/index.md"), b"---\ntitle: A\n---\nA\n");
    write(&ws_b.join("content/index.md"), b"---\ntitle: B\n---\nB\n");
    let recent = tempfile::tempdir().unwrap();
    let env_a = env_for(&boris_a, &recent.path().join("recent-a.json"));
    let env_b = env_for(&boris_b, &recent.path().join("recent-b.json"));
    seed_unique_run(&ws_a, "r11lossy");
    let mut wb = cv_app::init();
    join_with_timeout(
        open_workspace(&mut wb, ws_a.clone(), &env_a).expect("workspace A boot starts"),
        Duration::from_secs(8),
        "workspace A boot",
    );
    settle(&mut wb, Duration::from_millis(150));
    write(&boris_dir_a.path().join("verbs/validate/hold"), b"");
    let adoption =
        cv_app::migrate::adopt::adopt(&mut wb, adoption_job(&ws_a, "r11lossy", "imported"), &env_a)
            .expect("adoption starts");
    wait_for_argv(boris_dir_a.path(), "validate", 1);
    join_with_timeout(
        open_workspace(&mut wb, ws_b.clone(), &env_b).expect("workspace B boot starts"),
        Duration::from_secs(8),
        "workspace B boot",
    );
    settle(&mut wb, Duration::from_millis(100));
    std::fs::remove_file(boris_dir_a.path().join("verbs/validate/hold")).unwrap();
    join_with_timeout(adoption, Duration::from_secs(12), "adoption worker");
    settle(&mut wb, Duration::from_millis(500));

    let rows = print_rows("R11-LOSSY-FOREIGN", &wb);
    let v = validate_rows(&wb).len();
    println!(
        "R11-LOSSY-FOREIGN a={:?} b={:?} a_lossy={:?} b_lossy={:?} current={:?} validate_rows={v}",
        ws_a,
        ws_b,
        ws_a.to_string_lossy(),
        ws_b.to_string_lossy(),
        wb.workspace
    );
    assert_eq!(wb.workspace.as_ref(), Some(&ws_b));
    assert_eq!(
        v, 0,
        "workspace A outcome committed in foreign workspace B through a lossy identity collision: {rows:?}"
    );
    stop_all(&ws_a);
    stop_all(&ws_b);
}

/// A canonical path is not a lifetime identity. If the owner is deleted and a
/// different workspace is created at the same spelling before it is reopened,
/// the old operation's held outcome must not become truth in the replacement.
#[cfg(unix)]
#[test]
fn r11_recreated_workspace_at_same_path_must_not_inherit_stale_held_outcome() {
    let (boris_dir_a, boris_a) = fake_boris();
    write(
        &boris_dir_a.path().join("verbs/validate/exit-code.txt"),
        b"1",
    );
    let (_boris_dir_b, boris_b) = fake_boris();
    let base = tempfile::tempdir().unwrap();
    let ws = base.path().join("reused");
    std::fs::create_dir_all(&ws).unwrap();
    let ws = ws.canonicalize().unwrap();
    write(&ws.join("content/index.md"), b"---\ntitle: Old\n---\nOld\n");
    let (_foreign_tmp, foreign) = workspace();
    let recent = tempfile::tempdir().unwrap();
    let env_a = env_for(&boris_a, &recent.path().join("recent-a.json"));
    let env_b = env_for(&boris_b, &recent.path().join("recent-b.json"));
    seed_unique_run(&ws, "r11reuse");
    let mut wb = cv_app::init();
    join_with_timeout(
        open_workspace(&mut wb, ws.clone(), &env_a).expect("original workspace boot starts"),
        Duration::from_secs(8),
        "original workspace boot",
    );
    settle(&mut wb, Duration::from_millis(150));
    write(&boris_dir_a.path().join("verbs/validate/hold"), b"");
    let adoption =
        cv_app::migrate::adopt::adopt(&mut wb, adoption_job(&ws, "r11reuse", "imported"), &env_a)
            .expect("adoption starts");
    wait_for_argv(boris_dir_a.path(), "validate", 1);
    join_with_timeout(
        open_workspace(&mut wb, foreign.clone(), &env_b).expect("foreign boot starts"),
        Duration::from_secs(8),
        "foreign boot",
    );
    settle(&mut wb, Duration::from_millis(100));
    std::fs::remove_file(boris_dir_a.path().join("verbs/validate/hold")).unwrap();
    join_with_timeout(adoption, Duration::from_secs(12), "adoption worker");
    settle(&mut wb, Duration::from_millis(400));
    assert_eq!(
        validate_rows(&wb).len(),
        0,
        "harness precondition: outcome leaked while foreign"
    );

    std::fs::remove_dir_all(&ws).unwrap();
    std::fs::create_dir_all(&ws).unwrap();
    write(
        &ws.join("content/index.md"),
        b"---\ntitle: Replacement\n---\nReplacement\n",
    );
    assert!(
        !ws.join(".cv").exists(),
        "replacement still has the old workspace sidecar"
    );
    join_with_timeout(
        open_workspace(&mut wb, ws.clone(), &env_a).expect("same-path replacement boot starts"),
        Duration::from_secs(8),
        "same-path replacement boot",
    );
    settle(&mut wb, Duration::from_millis(400));
    let rows = print_rows("R11-PATH-REUSE", &wb);
    let v = validate_rows(&wb).len();
    println!("R11-PATH-REUSE path={ws:?} validate_rows_after_recreate={v}");
    assert_eq!(
        v, 0,
        "stale outcome from the deleted workspace was inherited by a different workspace at the same path: {rows:?}"
    );
    stop_all(&ws);
    stop_all(&foreign);
}

/// Positive control for a true symlink spelling: canonicalization collapses
/// it, so returning through the link is returning to the owning workspace.
#[cfg(unix)]
#[test]
fn r11_symlink_spelling_of_owning_workspace_drains_held_outcome() {
    let (boris_dir_a, boris_a) = fake_boris();
    write(
        &boris_dir_a.path().join("verbs/validate/exit-code.txt"),
        b"1",
    );
    let (_boris_dir_b, boris_b) = fake_boris();
    let (_wa, ws_a) = workspace();
    let (_wb, ws_b) = workspace();
    let link_parent = tempfile::tempdir().unwrap();
    let link = link_parent.path().join("workspace-link");
    std::os::unix::fs::symlink(&ws_a, &link).unwrap();
    let recent = tempfile::tempdir().unwrap();
    let env_a = env_for(&boris_a, &recent.path().join("recent-a.json"));
    let env_b = env_for(&boris_b, &recent.path().join("recent-b.json"));
    seed_unique_run(&ws_a, "r11symlink");
    let mut wb = cv_app::init();
    join_with_timeout(
        open_workspace(&mut wb, ws_a.clone(), &env_a).expect("workspace A boot starts"),
        Duration::from_secs(8),
        "workspace A boot",
    );
    settle(&mut wb, Duration::from_millis(150));
    write(&boris_dir_a.path().join("verbs/validate/hold"), b"");
    let adoption = cv_app::migrate::adopt::adopt(
        &mut wb,
        adoption_job(&ws_a, "r11symlink", "imported"),
        &env_a,
    )
    .expect("adoption starts");
    wait_for_argv(boris_dir_a.path(), "validate", 1);
    join_with_timeout(
        open_workspace(&mut wb, ws_b.clone(), &env_b).expect("workspace B boot starts"),
        Duration::from_secs(8),
        "workspace B boot",
    );
    settle(&mut wb, Duration::from_millis(100));
    std::fs::remove_file(boris_dir_a.path().join("verbs/validate/hold")).unwrap();
    join_with_timeout(adoption, Duration::from_secs(12), "adoption worker");
    settle(&mut wb, Duration::from_millis(400));
    assert_eq!(
        validate_rows(&wb).len(),
        0,
        "harness precondition: outcome leaked while foreign"
    );
    join_with_timeout(
        open_workspace(&mut wb, link.clone(), &env_a).expect("symlink reopen starts"),
        Duration::from_secs(8),
        "symlink reopen",
    );
    settle(&mut wb, Duration::from_millis(400));
    let v = validate_rows(&wb).len();
    println!(
        "R11-SYMLINK link={link:?} canonical_current={:?} validate_rows={v}",
        wb.workspace
    );
    assert_eq!(wb.workspace.as_ref(), Some(&ws_a));
    assert_eq!(
        v, 1,
        "symlink spelling of the owner did not drain exactly once"
    );
    stop_all(&ws_a);
    stop_all(&ws_b);
}

/// macOS firmlinks give one physical directory two canonical spellings.
/// reopening the very same workspace through its `/System/Volumes/Data`
/// alias is not a foreign workspace switch; the held outcome must drain there.
#[cfg(target_os = "macos")]
#[test]
fn r11_firmlink_alias_of_the_owning_workspace_must_drain_held_outcome() {
    use std::os::unix::fs::MetadataExt;

    let (boris_dir, boris) = fake_boris();
    write(&boris_dir.path().join("verbs/validate/exit-code.txt"), b"1");
    let (_alias_boris_dir, alias_boris) = fake_boris();
    let (_w, ws) = workspace();
    let alias = PathBuf::from("/System/Volumes/Data").join(ws.strip_prefix("/").unwrap());
    if !alias.is_dir() {
        eprintln!("firmlink alias unavailable: {}", alias.display());
        return;
    }
    let alias_canon = alias.canonicalize().unwrap();
    if alias_canon == ws {
        eprintln!("firmlink alias canonicalizes to the same spelling on this host");
        return;
    }
    let same_inode = {
        let a = std::fs::metadata(&ws).unwrap();
        let b = std::fs::metadata(&alias_canon).unwrap();
        (a.dev(), a.ino()) == (b.dev(), b.ino())
    };
    assert!(same_inode, "test alias is not the same inode");
    let recent = tempfile::tempdir().unwrap();
    let env = env_for(&boris, &recent.path().join("recent.json"));
    let env_alias = env_for(&alias_boris, &recent.path().join("recent-alias.json"));
    seed_unique_run(&ws, "r11firmlink");
    let mut wb = cv_app::init();
    join_with_timeout(
        open_workspace(&mut wb, ws.clone(), &env).expect("canonical boot starts"),
        Duration::from_secs(8),
        "canonical boot",
    );
    settle(&mut wb, Duration::from_millis(150));
    write(&boris_dir.path().join("verbs/validate/hold"), b"");
    let adoption =
        cv_app::migrate::adopt::adopt(&mut wb, adoption_job(&ws, "r11firmlink", "imported"), &env)
            .expect("adoption starts");
    wait_for_argv(boris_dir.path(), "validate", 1);
    let alias_boot =
        open_workspace(&mut wb, alias_canon.clone(), &env_alias).expect("alias boot starts");
    settle(&mut wb, Duration::from_millis(100));
    std::fs::remove_file(boris_dir.path().join("verbs/validate/hold")).unwrap();
    join_with_timeout(adoption, Duration::from_secs(12), "adoption worker");
    // The alias shares the owning workspace's scheduler, so its serialized
    // replacement boot may wait for the adoption gate to leave the mutation lane.
    join_with_timeout(alias_boot, Duration::from_secs(8), "alias boot");
    settle(&mut wb, Duration::from_millis(500));
    let alias_rows = validate_rows(&wb).len();
    let rows_at_alias = print_rows("R11-FIRMLINK-ALIAS", &wb);

    // Positive control: the held copy is not dropped; returning to the exact
    // string identity drains it. The failure is that the same physical
    // workspace under its firmlink spelling saw nothing.
    join_with_timeout(
        open_workspace(&mut wb, ws.clone(), &env).expect("canonical reopen starts"),
        Duration::from_secs(8),
        "canonical reopen",
    );
    settle(&mut wb, Duration::from_millis(500));
    let canonical_rows = validate_rows(&wb).len();
    println!(
        "R11-FIRMLINK ws={ws:?} alias={alias_canon:?} same_inode={same_inode} validate_rows_alias={alias_rows} validate_rows_after_exact_reopen={canonical_rows}"
    );
    assert_eq!(
        alias_rows, 1,
        "same physical workspace open through its firmlink alias did not receive the held outcome; alias rows were {rows_at_alias:?}; exact-spelling reopen later had {canonical_rows}"
    );
    assert!(
        std::sync::Arc::ptr_eq(
            &cv_app::slots::scheduler_for(&ws).expect("canonical scheduler"),
            &cv_app::slots::scheduler_for(&alias_canon).expect("alias scheduler"),
        ),
        "firmlink alias did not share the owning workspace's scheduler identity"
    );
    stop_all(&ws);
    stop_all(&alias_canon);
}

/// Passing validate+build across two retirements, with the UI drained while
/// the gate is in flight: live copies, per-verb queue copies, and the final
/// journal replay must collapse to exactly one truthful ordered outcome each.
#[test]
#[cfg(unix)]
fn r10_passing_gate_interleaved_drains_repeated_retirement_exactly_once_ordered() {
    let reps: usize = std::env::var("R10_REPS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(6);
    for rep in 0..reps {
        let (boris_dir, boris) = fake_boris();
        let (_w, ws) = workspace();
        let recent = tempfile::tempdir().unwrap();
        let env = env_for(&boris, &recent.path().join("recent.json"));
        let run_id = format!("r10pass{rep}");
        seed_unique_run(&ws, &run_id);
        let mut wb = cv_app::init();
        join_with_timeout(
            open_workspace(&mut wb, ws.clone(), &env).expect("initial boot starts"),
            Duration::from_secs(8),
            "initial boot",
        );
        settle(&mut wb, Duration::from_millis(100));
        write(&boris_dir.path().join("verbs/validate/hold"), b"");
        write(&boris_dir.path().join("verbs/build/hold"), b"");
        let adoption =
            cv_app::migrate::adopt::adopt(&mut wb, adoption_job(&ws, &run_id, "imported"), &env)
                .expect("adoption starts");
        wait_for_argv(boris_dir.path(), "validate", 1);

        let boot_a = open_workspace(&mut wb, ws.clone(), &env).expect("replacement A starts");
        std::fs::remove_file(boris_dir.path().join("verbs/validate/hold")).unwrap();
        wait_for_argv_draining(&mut wb, boris_dir.path(), "build", 1);
        let boot_b = open_workspace(&mut wb, ws.clone(), &env).expect("replacement B starts");
        std::fs::remove_file(boris_dir.path().join("verbs/build/hold")).unwrap();

        join_with_timeout(adoption, Duration::from_secs(12), "adoption worker");
        join_with_timeout(boot_a, Duration::from_secs(8), "replacement A");
        join_with_timeout(boot_b, Duration::from_secs(8), "replacement B");
        settle(&mut wb, Duration::from_millis(700));

        let validate = exact_rows(&wb, "validate finished");
        let build = exact_rows(&wb, "build finished");
        let rows = activity_rows(&wb);
        let (ok, state) = healthy(&wb, &ws);
        println!(
            "R10-PASS-INTERLEAVE rep={rep} validate={} build={} rows={rows:?} healthy={ok} {state}",
            validate.len(),
            build.len()
        );
        assert_eq!(
            validate.len(),
            1,
            "rep {rep}: validate terminal cardinality"
        );
        assert_eq!(build.len(), 1, "rep {rep}: build terminal cardinality");
        assert_eq!(
            validate[0].seen, 1,
            "rep {rep}: validate occurrence replayed visibly"
        );
        assert_eq!(
            build[0].seen, 1,
            "rep {rep}: build occurrence replayed visibly"
        );
        let iv = rows
            .iter()
            .position(|r| r.title == "validate finished")
            .unwrap();
        let ib = rows
            .iter()
            .position(|r| r.title == "build finished")
            .unwrap();
        assert!(
            iv > ib,
            "rep {rep}: Activity inverted the gate order (rows are newest-first): {rows:?}"
        );
        assert!(ok, "rep {rep}: final generation unhealthy: {state}");
        stop_all(&ws);
    }
}

/// Two legitimate adoptions spanning one retirement keep per-run migration
/// attribution and exactly two terminal validate/build outcomes in the final
/// generation — neither replay multiplication nor loss of the operation that
/// finished across the reset.
#[test]
#[cfg(unix)]
fn r10_multiple_adoptions_across_one_retirement_exactly_once_per_run() {
    let (boris_dir, boris) = fake_boris();
    let (_w, ws) = workspace();
    let recent = tempfile::tempdir().unwrap();
    let env = env_for(&boris, &recent.path().join("recent.json"));
    seed_unique_run(&ws, "r10multi-a");
    seed_unique_run(&ws, "r10multi-b");
    let mut wb = cv_app::init();
    join_with_timeout(
        open_workspace(&mut wb, ws.clone(), &env).expect("initial boot starts"),
        Duration::from_secs(8),
        "initial boot",
    );
    settle(&mut wb, Duration::from_millis(100));
    write(&boris_dir.path().join("verbs/validate/hold"), b"");
    let adoption_a =
        cv_app::migrate::adopt::adopt(&mut wb, adoption_job(&ws, "r10multi-a", "imported-a"), &env)
            .expect("adoption A starts");
    let adoption_b =
        cv_app::migrate::adopt::adopt(&mut wb, adoption_job(&ws, "r10multi-b", "imported-b"), &env)
            .expect("adoption B starts");
    wait_for_argv(boris_dir.path(), "validate", 1);
    let boot = open_workspace(&mut wb, ws.clone(), &env).expect("replacement boot starts");
    std::fs::remove_file(boris_dir.path().join("verbs/validate/hold")).unwrap();

    join_with_timeout(adoption_a, Duration::from_secs(12), "adoption A");
    join_with_timeout(adoption_b, Duration::from_secs(12), "adoption B");
    join_with_timeout(boot, Duration::from_secs(8), "replacement boot");
    settle(&mut wb, Duration::from_millis(900));

    let validate = exact_rows(&wb, "validate finished");
    let build = exact_rows(&wb, "build finished");
    let rows = activity_rows(&wb);
    let badges = migrate_badges(&wb);
    let (ok, state) = healthy(&wb, &ws);
    println!(
        "R10-MULTI validate={} build={} badges={badges:?} rows={rows:?} healthy={ok} {state}",
        validate.len(),
        build.len()
    );
    assert_eq!(
        validate.len(),
        2,
        "validate terminal cardinality across two adoptions"
    );
    assert_eq!(
        build.len(),
        2,
        "build terminal cardinality across two adoptions"
    );
    assert!(
        validate.iter().chain(&build).all(|r| r.seen == 1),
        "a terminal outcome occurrence was visibly duplicated: {rows:?}"
    );
    for run in ["r10multi-a", "r10multi-b"] {
        assert!(
            badges
                .iter()
                .any(|(title, badge)| title == run && badge == "adopted"),
            "missing adopted badge for {run}: {badges:?}"
        );
    }
    assert!(ok, "final generation unhealthy: {state}");
    stop_all(&ws);
}

/// build is still held, the replacement generation commits a causally later
/// shell-truth event. The final gate journal then replays the old validate
/// into the replacement. Correct causal order keeps validate older than that
/// replacement-generation truth; commit-order replay cannot express that.
#[test]
#[cfg(unix)]
fn r12_pre_reset_validate_replay_precesses_after_causally_later_watch_build() {
    let (boris_dir, boris) = fake_boris();
    let (_w, ws) = workspace();
    let recent = tempfile::tempdir().unwrap();
    let env = env_for(&boris, &recent.path().join("recent.json"));
    seed_unique_run(&ws, "r12precession");
    let mut wb = cv_app::init();
    join_with_timeout(
        open_workspace(&mut wb, ws.clone(), &env).expect("initial boot starts"),
        Duration::from_secs(8),
        "initial boot",
    );
    settle(&mut wb, Duration::from_millis(150));

    write(&boris_dir.path().join("verbs/build/hold"), b"");
    let adoption = cv_app::migrate::adopt::adopt(
        &mut wb,
        adoption_job(&ws, "r12precession", "imported"),
        &env,
    )
    .expect("adoption starts");
    wait_for_argv(boris_dir.path(), "build", 1);
    settle(&mut wb, Duration::from_millis(350));
    let before = exact_rows(&wb, "validate finished").len();
    println!("R12-PRECESSION validate_rows_before_reset={before}");
    assert_eq!(
        before, 1,
        "harness precondition: validate must commit before reset"
    );

    let boot = open_workspace(&mut wb, ws.clone(), &env).expect("replacement boot starts");
    wb.disclose(cv_core::events::shell::DegradedReason::StoreCorrupt);
    let pre_rows = print_rows("R12-PRECESSION-BEFORE-RELEASE", &wb);
    let ideg = pre_rows
        .iter()
        .position(|(_, t, _)| t == "Degraded: StoreCorrupt");
    println!("R12-PRECESSION replacement_truth_committed_before_release={ideg:?}");
    assert!(
        ideg.is_some(),
        "harness precondition: replacement shell truth must commit first"
    );

    std::fs::remove_file(boris_dir.path().join("verbs/build/hold")).unwrap();
    join_with_timeout(adoption, Duration::from_secs(12), "adoption worker");
    join_with_timeout(boot, Duration::from_secs(12), "replacement boot");
    settle(&mut wb, Duration::from_millis(700));
    let rows = print_rows("R12-PRECESSION-FINAL", &wb);
    let iv = rows.iter().position(|(_, t, _)| t == "validate finished");
    let idg = rows
        .iter()
        .position(|(_, t, _)| t == "Degraded: StoreCorrupt");
    let ia = rows.iter().position(|(_, t, _)| t == "build finished");
    println!(
        "R12-PRECESSION validate_index={iv:?} replacement_truth_index={idg:?} adoption_build_index={ia:?} status={:?}",
        wb.status
    );
    assert!(iv.is_some(), "validate terminal missing after replay");
    assert!(idg.is_some(), "replacement-generation shell truth missing");
    assert!(
        iv.unwrap() > idg.unwrap(),
        "validate causally preceded replacement-generation truth but replay made it newer: {rows:?}"
    );
    stop_all(&ws);
}

/// Renaming the owning workspace must not make its held outcome stale. This
/// attack reuses the vacated spelling before the moved owner is reopened:
/// path-only stale detection then falsely claims the still-existing moved
/// instance was replaced and drops its terminal truth.
#[cfg(unix)]
#[test]
fn r12_moved_workspace_outcome_must_survive_old_path_reuse() {
    let (boris_dir, boris) = fake_boris();
    write(&boris_dir.path().join("verbs/validate/exit-code.txt"), b"1");
    let (_foreign_boris_dir, foreign_boris) = fake_boris();
    let (_foreign_tmp, foreign) = workspace();
    let base = tempfile::tempdir().unwrap();
    let original_uncanon = base.path().join("original");
    std::fs::create_dir_all(&original_uncanon).unwrap();
    write(
        &original_uncanon.join("content/index.md"),
        b"---\ntitle: Original\n---\nOriginal\n",
    );
    let original = original_uncanon.canonicalize().unwrap();
    let recent = tempfile::tempdir().unwrap();
    let env_a = env_for(&boris, &recent.path().join("recent-a.json"));
    let env_b = env_for(&foreign_boris, &recent.path().join("recent-b.json"));
    seed_unique_run(&original, "r12move");
    let mut wb = cv_app::init();
    join_with_timeout(
        open_workspace(&mut wb, original.clone(), &env_a).expect("original boot starts"),
        Duration::from_secs(8),
        "original boot",
    );
    settle(&mut wb, Duration::from_millis(150));
    write(&boris_dir.path().join("verbs/validate/hold"), b"");
    let adoption = cv_app::migrate::adopt::adopt(
        &mut wb,
        adoption_job(&original, "r12move", "imported"),
        &env_a,
    )
    .expect("adoption starts");
    wait_for_argv(boris_dir.path(), "validate", 1);
    join_with_timeout(
        open_workspace(&mut wb, foreign.clone(), &env_b).expect("foreign boot starts"),
        Duration::from_secs(8),
        "foreign boot",
    );
    settle(&mut wb, Duration::from_millis(100));
    std::fs::remove_file(boris_dir.path().join("verbs/validate/hold")).unwrap();
    join_with_timeout(adoption, Duration::from_secs(12), "adoption worker");
    settle(&mut wb, Duration::from_millis(350));
    assert_eq!(
        validate_rows(&wb).len(),
        0,
        "harness precondition: owner outcome must remain held while foreign"
    );

    let moved_uncanon = base.path().join("moved");
    std::fs::rename(&original, &moved_uncanon).unwrap();
    let moved = moved_uncanon.canonicalize().unwrap();
    std::fs::create_dir_all(&original).unwrap();
    write(
        &original.join("content/index.md"),
        b"---\ntitle: Replacement\n---\nReplacement\n",
    );
    join_with_timeout(
        open_workspace(&mut wb, original.clone(), &env_a)
            .expect("old-path replacement boot starts"),
        Duration::from_secs(8),
        "old-path replacement boot",
    );
    settle(&mut wb, Duration::from_millis(350));
    let status_at_replacement = wb.status.clone();
    let rows_at_replacement = print_rows("R12-MOVE-REUSED-PATH", &wb);
    println!("R12-MOVE replacement_status={status_at_replacement:?} rows={rows_at_replacement:?}");

    join_with_timeout(
        open_workspace(&mut wb, moved.clone(), &env_a).expect("moved owner boot starts"),
        Duration::from_secs(8),
        "moved owner boot",
    );
    settle(&mut wb, Duration::from_millis(500));
    let rows = print_rows("R12-MOVE-FINAL", &wb);
    let got = validate_rows(&wb).len();
    println!(
        "R12-MOVE original={original:?} moved={moved:?} validate_rows_at_moved_owner={got} final_status={:?}",
        wb.status
    );
    assert_eq!(
        got, 1,
        "held outcome for the still-existing moved workspace was falsely discarded as stale: {rows:?}"
    );
    stop_all(&original);
    stop_all(&moved);
    stop_all(&foreign);
}
