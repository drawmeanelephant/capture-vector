//! Cross-process proof for the cv-store per-run lock. The spawn lives in
//! cv-engine because no other crate may create processes.

use cv_store::imports::{self as store, RunProbe};
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

const HOLD_ENV: &str = "CV_TEST_HOLD_IMPORT_RUN_LOCK";

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
