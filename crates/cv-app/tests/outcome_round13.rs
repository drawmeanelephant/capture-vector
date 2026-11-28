//! Round-13 app-level moved-owner outcome probes.
#![cfg(unix)]

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
    const CONTROLLED_VERB_HOLD: &str = r#"if [ -f "$D/verbs/$V/hold" ]; then while [ -f "$D/verbs/$V/hold" ]; do sleep 0.02; done; touch "$D/verbs/$V/released"; fi"#;
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

fn validate_rows(wb: &Workbench) -> usize {
    let i = wb.registry.index_of("activity").expect("activity module");
    let m = wb.registry.get(i).expect("activity module");
    (0..m.list_len(None))
        .filter(|r| m.row(None, *r).title.starts_with("validate"))
        .count()
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

fn stop_all(root: &Path) {
    if let Some(s) = cv_app::slots::scheduler_for(root)
        && let Some(w) = s.watch_session()
    {
        s.stop_watch_session(&w);
    }
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

/// The owning directory is renamed twice while its adoption validate is
/// actively held, the terminal fact lands while a foreign workspace is open,
/// and the vacated spelling is reused before the surviving instance returns.
/// Device+file-index identity must follow the instance through both moves;
/// the reused spelling must not capture or discard the outcome.
#[cfg(unix)]
#[test]
fn r13_active_adoption_double_move_old_path_reuse_drains_only_at_surviving_owner() {
    let (boris_dir, boris) = fake_boris();
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
    let env = env_for(&boris, &recent.path().join("recent.json"));
    let env_foreign = env_for(&foreign_boris, &recent.path().join("recent-foreign.json"));
    seed_unique_run(&original, "r13activemove");
    let mut wb = cv_app::init();
    join_with_timeout(
        open_workspace(&mut wb, original.clone(), &env).expect("original boot starts"),
        Duration::from_secs(8),
        "original boot",
    );
    settle(&mut wb, Duration::from_millis(150));

    write(&boris_dir.path().join("verbs/build/hold"), b"");
    let adoption = cv_app::migrate::adopt::adopt(
        &mut wb,
        adoption_job(&original, "r13activemove", "imported"),
        &env,
    )
    .expect("adoption starts");
    wait_for_argv(boris_dir.path(), "build", 1);

    let moved_once_uncanon = base.path().join("moved-once");
    std::fs::rename(&original, &moved_once_uncanon).unwrap();
    let moved_once = moved_once_uncanon.canonicalize().unwrap();
    let moved_twice_uncanon = base.path().join("moved-twice");
    std::fs::rename(&moved_once, &moved_twice_uncanon).unwrap();
    let moved_twice = moved_twice_uncanon.canonicalize().unwrap();
    std::fs::create_dir_all(&original).unwrap();
    write(
        &original.join("content/index.md"),
        b"---\ntitle: Replacement\n---\nReplacement\n",
    );

    join_with_timeout(
        open_workspace(&mut wb, foreign.clone(), &env_foreign).expect("foreign boot starts"),
        Duration::from_secs(8),
        "foreign boot",
    );
    settle(&mut wb, Duration::from_millis(120));
    assert!(
        !boris_dir.path().join("verbs/build/released").exists(),
        "harness precondition: build must still be held before the foreign reset"
    );
    std::fs::remove_file(boris_dir.path().join("verbs/build/hold")).unwrap();
    join_with_timeout(adoption, Duration::from_secs(12), "adoption worker");
    assert!(
        boris_dir.path().join("verbs/build/released").exists(),
        "harness precondition: build must have passed the hold"
    );
    settle(&mut wb, Duration::from_millis(400));
    println!(
        "R13-ACTIVE-MOVE status_after_adoption={:?} argv={:?}",
        wb.status,
        argv_lines(boris_dir.path())
    );
    let foreign_rows = print_rows("R13-ACTIVE-MOVE-FOREIGN", &wb);
    let foreign_validate = validate_rows(&wb);
    println!("R13-ACTIVE-MOVE foreign_validate={foreign_validate} rows={foreign_rows:?}");
    assert_eq!(
        foreign_validate, 0,
        "owner outcome committed into the foreign workspace"
    );

    join_with_timeout(
        open_workspace(&mut wb, original.clone(), &env).expect("old-path replacement boot starts"),
        Duration::from_secs(8),
        "old-path replacement boot",
    );
    settle(&mut wb, Duration::from_millis(300));
    let replacement_validate = validate_rows(&wb);
    println!("R13-ACTIVE-MOVE old_path_replacement_validate={replacement_validate}");
    assert_eq!(
        replacement_validate, 0,
        "recreated old path captured the moved owner's outcome"
    );

    join_with_timeout(
        open_workspace(&mut wb, moved_twice.clone(), &env).expect("surviving owner boot starts"),
        Duration::from_secs(8),
        "surviving owner boot",
    );
    settle(&mut wb, Duration::from_millis(500));
    let rows = print_rows("R13-ACTIVE-MOVE-FINAL", &wb);
    let got = validate_rows(&wb);
    println!(
        "R13-ACTIVE-MOVE original={original:?} moved_twice={moved_twice:?} validate_rows_at_surviving_owner={got} final_status={:?}",
        wb.status
    );
    assert_eq!(
        got, 1,
        "held outcome did not follow the active adoption's surviving moved owner: {rows:?}"
    );
    stop_all(&original);
    stop_all(&moved_twice);
    stop_all(&foreign);
}
