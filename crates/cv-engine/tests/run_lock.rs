//! Cross-process proof for the cv-store per-run lock. The spawn lives in
//! cv-engine because no other crate may create processes.

use cv_store::imports::{self as store, RunProbe};
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

const HOLD_ENV: &str = "CV_TEST_HOLD_IMPORT_RUN_LOCK";
#[cfg(unix)]
const OWNER_ENV: &str = "CV_TEST_OWN_IMPORT_RUN_LOCK";
#[cfg(unix)]
const GRANDCHILD_PID: &str = "grandchild.pid";

#[test]
fn hold_run_lock_when_asked() {
    let Some(root) = std::env::var_os(HOLD_ENV) else {
        return;
    };
    let root = PathBuf::from(root);
    let _lock = store::claim_run(&root, "held-by-child").unwrap();
    std::thread::sleep(Duration::from_secs(30));
}

#[test]
fn a_live_child_holds_a_run_lock_until_it_dies() {
    if std::env::var_os(HOLD_ENV).is_some() {
        return;
    }
    let t = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(t.path()).unwrap();
    let exe = std::env::current_exe().unwrap();
    let mut child = Command::new(exe)
        .arg("hold_run_lock_when_asked")
        .arg("--exact")
        .arg("--nocapture")
        .env(HOLD_ENV, &root)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();

    let mut held = false;
    for _ in 0..150 {
        if store::probe_run(&root, "held-by-child").ok() == Some(RunProbe::Held) {
            held = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(held, "the live child process holds the run lock");
    let lock_path = root
        .join(store::DIR)
        .join("held-by-child")
        .join(store::RUN_LOCK);
    assert!(lock_path.is_file());

    child.kill().unwrap();
    child.wait().unwrap();
    let mut free = false;
    for _ in 0..150 {
        if store::probe_run(&root, "held-by-child").ok() == Some(RunProbe::Free) {
            free = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(free, "the kernel releases the run lock when the child dies");
    assert!(lock_path.is_file(), "the lock file itself is never removed");
}

#[cfg(unix)]
#[test]
fn own_run_lock_and_spawn_grandchild_when_asked() {
    let Some(root) = std::env::var_os(OWNER_ENV) else {
        return;
    };
    let root = PathBuf::from(root);
    let _lock = store::claim_run(&root, "owned-by-dying-parent").unwrap();
    // The grandchild emulates an orphaned lab process. If lock fds leaked
    // through spawn inheritance, this process would keep the parent's run
    // locked after the parent is SIGKILLed.
    let mut grandchild = Command::new("sh")
        .arg("-c")
        .arg("sleep 30")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    std::fs::write(root.join(GRANDCHILD_PID), grandchild.id().to_string()).unwrap();
    std::thread::sleep(Duration::from_secs(30));
    // If the owner is allowed to exit normally, reap its lab stand-in. In
    // the regression below the owner is SIGKILLed first, deliberately
    // orphaning the grandchild.
    let _ = grandchild.wait();
}

#[cfg(unix)]
#[test]
fn a_sigkilled_lock_owner_does_not_leave_the_lock_with_an_orphaned_grandchild() {
    if std::env::var_os(OWNER_ENV).is_some() || std::env::var_os(HOLD_ENV).is_some() {
        return;
    }
    let t = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(t.path()).unwrap();
    let exe = std::env::current_exe().unwrap();
    let mut owner = Command::new(exe)
        .arg("own_run_lock_and_spawn_grandchild_when_asked")
        .arg("--exact")
        .arg("--nocapture")
        .env(OWNER_ENV, &root)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();

    let mut held = false;
    let mut grandchild_pid = None;
    for _ in 0..200 {
        let lock_held =
            store::probe_run(&root, "owned-by-dying-parent").ok() == Some(RunProbe::Held);
        if lock_held && grandchild_pid.is_none() {
            grandchild_pid = std::fs::read_to_string(root.join(GRANDCHILD_PID))
                .ok()
                .and_then(|s| s.trim().parse::<u32>().ok());
        }
        if lock_held && grandchild_pid.is_some() {
            held = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(held, "owner and grandchild both started");

    owner.kill().unwrap();
    owner.wait().unwrap();
    let mut free = false;
    for _ in 0..150 {
        if store::probe_run(&root, "owned-by-dying-parent").ok() == Some(RunProbe::Free) {
            free = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    // Always clean up the orphaned sleeper, even when the inheritance
    // assertion below fails. The kill executable is spawned only inside
    // cv-engine, preserving the process boundary.
    if let Some(pid) = grandchild_pid {
        let _ = Command::new("kill").arg(pid.to_string()).status();
    }
    assert!(
        free,
        "an orphaned grandchild must not inherit or retain the owner's run lock"
    );
}
