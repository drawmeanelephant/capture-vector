//! Workspace-reopen adoption regression: the composition root must keep
//! one serialized scheduler identity per workspace across switch-away/back.

mod common;

use cv_app::{WorkspaceEnv, open_workspace};
use cv_core::engine::CancelToken;
use cv_engine::locate::Locator;
use cv_fixtures::lab::StubLab;
use cv_model::adoption::{AdoptionJob, AdoptionRequest, AdoptionTarget};
use cv_model::modules::engine::EnginePort;
use cv_store::imports::{self as store, Finished, RecordedLab, RunRecord};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

const BORIS_SHIM: &str = include_str!("../../cv-engine/tests/fake-boris/boris");

fn write(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

#[cfg(unix)]
fn fake_boris() -> (tempfile::TempDir, PathBuf) {
    fake_boris_holding_watch(true)
}

#[cfg(unix)]
fn fake_boris_holding_watch(hold: bool) -> (tempfile::TempDir, PathBuf) {
    fake_boris_holding_watch_with_version_delay(hold, false)
}

#[cfg(unix)]
fn fake_boris_holding_watch_with_version_delay(
    hold: bool,
    version_delay: bool,
) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("boris");
    const VERSION_CASE: &str = r#"  --version) cat "$D/version.txt"; exit 0 ;;"#;
    const DELAYED_VERSION_CASE: &str = r#"  --version) [ -f "$D/version-delay" ] && sleep "$(cat "$D/version-delay")"; cat "$D/version.txt"; exit 0 ;;"#;
    const HOLD_CASE: &str = r#"    if [ -f "$D/watch-hold" ]; then"#;
    const WORKSPACE_HOLD_CASE: &str =
        r#"    if [ -f "$D/watch-hold" ] && ! printf '%s' "$*" | grep -q -- '--serve'; then"#;
    assert!(BORIS_SHIM.contains(VERSION_CASE));
    assert!(BORIS_SHIM.contains(HOLD_CASE));
    let shim = BORIS_SHIM.replace(HOLD_CASE, WORKSPACE_HOLD_CASE);
    let shim = if version_delay {
        shim.replace(VERSION_CASE, DELAYED_VERSION_CASE)
    } else {
        shim
    };
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
    if hold {
        write(&dir.path().join("watch-hold"), b"");
    }
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
        recent_file: Some(recent_file.to_path_buf()),
    }
}

fn seed_long_run(root: &Path, run_id: &str, extra_files: usize) {
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
    // Small but numerous files make the journaled copy phase long enough
    // for a workspace switch-away/switch-back sequence without a large write.
    for i in 0..extra_files {
        write(
            &lock
                .dir()
                .join(store::OUT)
                .join(format!("content/zz/file-{i:04}.md")),
            format!("file {i}\n").as_bytes(),
        );
    }
    drop(lock);
}

fn settle(wb: &mut cv_model::workbench::Workbench, dur: Duration) {
    let end = Instant::now() + dur;
    while Instant::now() < end {
        wb.drain();
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(unix)]
fn fake_boris_with_controlled_verb_holds() -> (tempfile::TempDir, PathBuf) {
    let (dir, bin) = fake_boris_holding_watch(true);
    const HOLD: &str = r#"if [ -f "$D/verbs/$V/hold" ]; then sleep 300 & wait; fi"#;
    const CONTROLLED: &str = r#"if [ -f "$D/verbs/$V/hold" ]; then while [ -f "$D/verbs/$V/hold" ]; do sleep 0.02; done; fi"#;
    let shim = std::fs::read_to_string(&bin).unwrap();
    assert!(shim.contains(HOLD));
    common::install_executable(&bin, &shim.replace(HOLD, CONTROLLED));
    (dir, bin)
}

fn wait_for_path(path: &Path, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !path.exists() {
        assert!(Instant::now() < deadline, "never reached {what}");
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn wait_for_argv(dir: &Path, needle: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if std::fs::read_to_string(dir.join("argv.log"))
            .unwrap_or_default()
            .lines()
            .any(|l| l.starts_with(needle))
        {
            return;
        }
        assert!(Instant::now() < deadline, "never saw argv {needle}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn join_with_timeout<T: Send + 'static>(
    handle: std::thread::JoinHandle<T>,
    timeout: Duration,
    what: &str,
) -> T {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(handle.join());
    });
    rx.recv_timeout(timeout)
        .unwrap_or_else(|_| panic!("{what} did not finish within {timeout:?}"))
        .unwrap_or_else(|_| panic!("{what} panicked"))
}

/// Round-4 natural hang shape, bounded: three reopens can queue behind
/// one another while the profile changes. Every boot must finish, and the
/// final live watch must belong to the last requested configuration.
#[cfg(unix)]
#[test]
fn rapid_reopen_reopen_reopen_converges_without_deadlock() {
    let rounds: usize = std::env::var("R4_REOPEN_REPS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2);
    let (boris_dir, boris) = fake_boris_holding_watch(true);
    let ws_tmp = tempfile::tempdir().unwrap();
    let ws = ws_tmp.path().canonicalize().unwrap();
    write(
        &ws.join("content/index.md"),
        b"---
title: Home
---
Home
",
    );
    write(
        &ws.join("docs/index.md"),
        b"---
title: Docs
---
Docs
",
    );
    let recent_tmp = tempfile::tempdir().unwrap();
    let env = env_for(&boris, &recent_tmp.path().join("recent.json"));
    let mut wb = cv_app::init();
    join_with_timeout(
        open_workspace(&mut wb, ws.clone(), &env).expect("first boot starts"),
        Duration::from_secs(5),
        "first boot",
    );

    for round in 0..rounds {
        let mut boots = Vec::new();
        let mut last_has_profile = false;
        for k in 0..3 {
            last_has_profile = (round + k) % 2 == 0;
            if last_has_profile {
                write(&ws.join("boris.json"), br#"{"input":"docs"}"#);
            } else {
                let _ = std::fs::remove_file(ws.join("boris.json"));
            }
            boots.push(open_workspace(&mut wb, ws.clone(), &env).expect("reopen boot starts"));
        }
        for (k, boot) in boots.into_iter().enumerate() {
            join_with_timeout(
                boot,
                Duration::from_secs(5),
                &format!("round {round} reopen boot {k}"),
            );
        }
        settle(&mut wb, Duration::from_millis(300));
        let want = if last_has_profile {
            "watch --watch-json"
        } else {
            "watch --watch-json --input content"
        };
        let got = std::fs::read_to_string(boris_dir.path().join("argv.log"))
            .unwrap_or_default()
            .lines()
            .rfind(|l| l.starts_with("watch ") && !l.contains("--serve"))
            .unwrap_or_default()
            .to_string();
        let sched = cv_app::slots::scheduler_for(&ws).expect("scheduler remains");
        assert_eq!(
            (
                got.as_str(),
                sched.watch_session().is_some(),
                wb.app.watch_live,
                wb.app.degraded.is_none(),
            ),
            (want, true, true, true),
            "round {round}: rapid reopens must converge to the final profile"
        );
    }
    if let Some(sched) = cv_app::slots::scheduler_for(&ws)
        && let Some(w) = sched.watch_session()
    {
        sched.stop_watch_session(&w);
    }
}

/// Round-6 lock-inversion shape: adoption restore starts after the shell
/// has switched away, so its inbox is dead. A failed watch publication
/// must be dropped only after the scheduler locks are released; the worker
/// and every later scheduler operation must continue to completion.
#[cfg(unix)]
#[test]
fn switching_away_before_adoption_restore_cannot_deadlock_publication() {
    let reps: usize = std::env::var("R6_REPS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);
    for rep in 0..reps {
        let (boris_dir, boris) = fake_boris_with_controlled_verb_holds();
        let (_boris_b_dir, boris_b) = fake_boris_holding_watch(true);
        let wa = tempfile::tempdir().unwrap();
        let ws_a = wa.path().canonicalize().unwrap();
        let wb_tmp = tempfile::tempdir().unwrap();
        let ws_b = wb_tmp.path().canonicalize().unwrap();
        write(
            &ws_a.join("content/index.md"),
            b"---
title: A
---
A
",
        );
        write(
            &ws_b.join("content/index.md"),
            b"---
title: B
---
B
",
        );
        let recent = tempfile::tempdir().unwrap();
        let env_a = env_for(&boris, &recent.path().join("recent-a.json"));
        let env_b = env_for(&boris_b, &recent.path().join("recent-b.json"));
        let run_id = "r6-deadlock-hugo";
        seed_long_run(&ws_a, run_id, 1);

        let mut wb = cv_app::init();
        open_workspace(&mut wb, ws_a.clone(), &env_a)
            .expect("first boot starts")
            .join()
            .unwrap();
        write(&boris_dir.path().join("verbs/validate/hold"), b"");
        let worker = cv_app::migrate::adopt::adopt(
            &mut wb,
            AdoptionJob {
                source_root: ws_a.clone(),
                request: AdoptionRequest {
                    run_id: run_id.into(),
                    target: AdoptionTarget::ExistingWorkspace {
                        subfolder: "imported".into(),
                    },
                },
                cancel: CancelToken::new(),
            },
            &env_a,
        )
        .expect("adoption starts");
        wait_for_path(
            &ws_a.join(".cv/imports").join(run_id).join(store::PENDING),
            "the adoption journal",
        );
        wait_for_argv(boris_dir.path(), "validate ");
        open_workspace(&mut wb, ws_b.clone(), &env_b)
            .expect("switch-away boot starts")
            .join()
            .unwrap();
        std::fs::remove_file(boris_dir.path().join("verbs/validate/hold")).unwrap();

        let (done_tx, done_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = done_tx.send(worker.join().is_ok());
        });
        let worker_completed = done_rx
            .recv_timeout(Duration::from_secs(5))
            .unwrap_or(false);
        assert!(
            worker_completed,
            "rep {rep}: dead-generation publication self-deadlocked the adoption worker"
        );

        let workspace_watches = std::fs::read_to_string(boris_dir.path().join("argv.log"))
            .unwrap_or_default()
            .lines()
            .filter(|l| l.starts_with("watch ") && !l.contains("--serve"))
            .count();
        assert_eq!(
            workspace_watches, 1,
            "rep {rep}: the retired generation should not even start a transient watch"
        );
        let sched = cv_app::slots::scheduler_for(&ws_a).expect("workspace A scheduler remains");
        let (sched_tx, sched_rx) = std::sync::mpsc::channel();
        std::thread::spawn({
            let sched = sched.clone();
            move || {
                let _ = sched.snapshot();
                let _ = sched_tx.send(());
            }
        });
        assert!(
            sched_rx.recv_timeout(Duration::from_secs(1)).is_ok(),
            "rep {rep}: dead-generation publication left the scheduler writer locked"
        );
        assert!(
            !sched.engine().watch_live(),
            "rep {rep}: an unpublished dead-generation watch must not remain live"
        );
        if let Some(w) = sched.watch_session() {
            sched.stop_watch_session(&w);
        }
        if let Some(sched_b) = cv_app::slots::scheduler_for(&ws_b)
            && let Some(w) = sched_b.watch_session()
        {
            sched_b.stop_watch_session(&w);
        }
    }
}

/// Round-5 probe shape: a restore queued behind a successful replacement
/// probe captures the pre-reset delivery and wins the post-replacement
/// start race. That unpublished transient must not consume the
/// replacement's one watch start: the reopened generation owns the session
/// that remains live.
#[cfg(unix)]
#[test]
fn restore_started_during_successful_replacement_uses_live_delivery() {
    let reps: usize = std::env::var("R5_REPS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(6);
    for rep in 0..reps {
        let (boris_dir, boris) = fake_boris_holding_watch_with_version_delay(true, true);
        let ws_tmp = tempfile::tempdir().unwrap();
        let ws = ws_tmp.path().canonicalize().unwrap();
        write(
            &ws.join("content/index.md"),
            b"---
title: Home
---
Home
",
        );
        let recent_tmp = tempfile::tempdir().unwrap();
        let env = env_for(&boris, &recent_tmp.path().join("recent.json"));

        let mut wb = cv_app::init();
        open_workspace(&mut wb, ws.clone(), &env)
            .expect("first boot starts")
            .join()
            .unwrap();
        settle(&mut wb, Duration::from_millis(250));
        let initial_versions = std::fs::read_to_string(boris_dir.path().join("argv.log"))
            .unwrap_or_default()
            .lines()
            .filter(|l| *l == "--version")
            .count();
        write(
            &boris_dir.path().join("version-delay"),
            b"1
",
        );
        let boot = open_workspace(&mut wb, ws.clone(), &env).expect("reopen boot starts");

        // Once the replacement has entered its delayed version probe, the
        // old watch is already stopped and the pre-reset verb route is still
        // installed. Queue restore there; it waits on the replacement writer.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let versions = std::fs::read_to_string(boris_dir.path().join("argv.log"))
                .unwrap_or_default()
                .lines()
                .filter(|l| *l == "--version")
                .count();
            if versions > initial_versions {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "replacement did not reach its delayed version probe"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        let slot = cv_app::slots::slot_for(&ws).expect("stable slot");
        let restore = std::thread::spawn(move || {
            slot.restore_watch(None, "r5 restore".into());
        });
        join_with_timeout(restore, Duration::from_secs(3), "stale restore");
        join_with_timeout(boot, Duration::from_secs(5), "replacement boot");
        settle(&mut wb, Duration::from_millis(1000));

        let sched = cv_app::slots::scheduler_for(&ws).expect("scheduler remains");
        assert_eq!(
            (
                sched.watch_session().is_some(),
                wb.app.watch_live,
                wb.app.degraded.is_none(),
                wb.app.engine.is_some(),
            ),
            (true, true, true, true),
            "rep {rep}: successful replacement must end with exactly one live published watch"
        );
        if let Some(w) = sched.watch_session() {
            sched.stop_watch_session(&w);
        }
    }
}

#[cfg(unix)]
#[test]
fn adoption_cannot_run_inside_a_stale_scheduler_after_workspace_reopen() {
    let (_boris_dir, boris) = fake_boris();
    let ws_tmp = tempfile::tempdir().unwrap();
    let ws = ws_tmp.path().canonicalize().unwrap();
    write(
        &ws.join("content/index.md"),
        b"---\ntitle: Home\n---\nHome\n",
    );
    let other_tmp = tempfile::tempdir().unwrap();
    let other = other_tmp.path().canonicalize().unwrap();
    write(
        &other.join("content/index.md"),
        b"---\ntitle: Other\n---\nOther\n",
    );
    let recent_tmp = tempfile::tempdir().unwrap();
    let env = env_for(&boris, &recent_tmp.path().join("recent.json"));

    let run_id = "reopen-long-hugo";
    seed_long_run(&ws, run_id, 1500);

    let mut wb = cv_app::init();
    open_workspace(&mut wb, ws.clone(), &env)
        .expect("first boot starts")
        .join()
        .unwrap();
    let first = cv_app::slots::scheduler_for(&ws).expect("first scheduler booted");

    let job = AdoptionJob {
        source_root: ws.clone(),
        request: AdoptionRequest {
            run_id: run_id.into(),
            target: AdoptionTarget::ExistingWorkspace {
                subfolder: "imported".into(),
            },
        },
        cancel: CancelToken::new(),
    };
    let worker = cv_app::migrate::adopt::adopt(&mut wb, job, &env).expect("adoption starts");
    let pending = ws.join(".cv/imports").join(run_id).join(store::PENDING);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !pending.exists() {
        assert!(
            Instant::now() < deadline,
            "adoption did not reach its journal"
        );
        std::thread::sleep(Duration::from_millis(2));
    }

    // Exact user shape: switch away, then switch back while the adoption
    // copy is still inside its first scheduler batch.
    open_workspace(&mut wb, other.clone(), &env)
        .expect("switch-away boot starts")
        .join()
        .unwrap();
    open_workspace(&mut wb, ws.clone(), &env)
        .expect("switch-back boot starts")
        .join()
        .unwrap();
    let second = cv_app::slots::scheduler_for(&ws).expect("second scheduler booted");

    let pending_before = pending.exists();
    second.mutation_batch(|| {
        write(
            &ws.join("content/reopened-scheduler-write.md"),
            b"write from the reopened workspace scheduler\n",
        );
    });
    let pending_after = pending.exists();
    let same_scheduler = Arc::as_ptr(&first) == Arc::as_ptr(&second);
    assert!(
        !(pending_before && pending_after && !same_scheduler),
        "SCHEDULER-SPLIT: adoption was still inside the first scheduler batch while a mutation batch completed on the post-reopen scheduler (same_scheduler={same_scheduler}, pending_before={pending_before}, pending_after={pending_after})"
    );

    join_with_timeout(worker, Duration::from_secs(10), "adoption worker");
    if let Some(w) = second.watch_session() {
        second.stop_watch_session(&w);
    }
}

#[cfg(unix)]
#[test]
fn reopen_after_profile_input_change_refreshes_the_watch_args() {
    let (boris_dir, boris) = fake_boris_holding_watch(false);
    let ws_tmp = tempfile::tempdir().unwrap();
    let ws = ws_tmp.path().canonicalize().unwrap();
    write(&ws.join("docs/index.md"), b"---\ntitle: Home\n---\nHome\n");
    write(&ws.join("content/index.md"), b"---\ntitle: Old\n---\nOld\n");
    let recent_tmp = tempfile::tempdir().unwrap();
    let env = env_for(&boris, &recent_tmp.path().join("recent.json"));
    let workspace_watches = || {
        std::fs::read_to_string(boris_dir.path().join("argv.log"))
            .unwrap_or_default()
            .lines()
            .filter(|l| l.starts_with("watch ") && !l.contains("--serve"))
            .map(str::to_string)
            .collect::<Vec<_>>()
    };

    let mut wb = cv_app::init();
    open_workspace(&mut wb, ws.clone(), &env)
        .expect("first boot starts")
        .join()
        .unwrap();
    let first = workspace_watches().last().cloned().unwrap_or_default();
    assert_eq!(first, "watch --watch-json --input content");

    // The documented restart path: the user adds a profile naming its input
    // and reopens the workspace. The fresh watch must reflect that profile —
    // Pages already does, so silently keeping `--input content` splits the
    // UI from the compiler.
    write(&ws.join("boris.json"), b"{\"input\":\"docs\"}\n");
    open_workspace(&mut wb, ws.clone(), &env)
        .expect("reopen boot starts")
        .join()
        .unwrap();
    let reopened = workspace_watches().last().cloned().unwrap_or_default();
    assert_eq!(
        reopened, "watch --watch-json",
        "reopen must not reuse the pre-profile watch argv"
    );

    if let Some(sched) = cv_app::slots::scheduler_for(&ws)
        && let Some(w) = sched.watch_session()
    {
        sched.stop_watch_session(&w);
    }
}

#[cfg(unix)]
#[test]
fn a_failed_reopen_refresh_makes_adoption_and_palette_refuse_before_any_write() {
    let (boris_dir, boris) = fake_boris_holding_watch(false);
    let ws_tmp = tempfile::tempdir().unwrap();
    let ws = ws_tmp.path().canonicalize().unwrap();
    write(
        &ws.join("content/index.md"),
        b"---\ntitle: Home\n---\nHome\n",
    );
    let recent_tmp = tempfile::tempdir().unwrap();
    let env = env_for(&boris, &recent_tmp.path().join("recent.json"));
    seed_long_run(&ws, "failed-refresh", 5);

    let mut wb = cv_app::init();
    open_workspace(&mut wb, ws.clone(), &env)
        .expect("first boot starts")
        .join()
        .unwrap();
    write(&boris_dir.path().join("version.txt"), b"0.8.2\n");
    open_workspace(&mut wb, ws.clone(), &env)
        .expect("failed reopen boot starts")
        .join()
        .unwrap();

    let slot = cv_app::slots::slot_for(&ws).expect("stable slot remains");
    assert!(
        cv_app::live::SlotVerbs(slot).unavailable().is_some(),
        "a failed refresh must make the palette unavailable like fresh open"
    );
    let worker = cv_app::migrate::adopt::adopt(
        &mut wb,
        AdoptionJob {
            source_root: ws.clone(),
            request: AdoptionRequest {
                run_id: "failed-refresh".into(),
                target: AdoptionTarget::ExistingWorkspace {
                    subfolder: "imported".into(),
                },
            },
            cancel: CancelToken::new(),
        },
        &env,
    )
    .expect("adoption worker starts");
    join_with_timeout(worker, Duration::from_secs(10), "adoption worker");
    assert!(
        !ws.join("content/imported").exists(),
        "zombie engine adoption wrote content after the refresh failure"
    );
}
