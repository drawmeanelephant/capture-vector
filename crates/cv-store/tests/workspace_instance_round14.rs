//! Round-14 adversarial probes for `.cv/workspace-instance` evidence.

use cv_store::workspace_instance::{self, FILE, LOCK};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn valid_token(tag: &str) -> String {
    format!("cvwi-{tag:0<24}")
}

#[cfg(unix)]
#[test]
fn r14_preexisting_symlinks_are_refused_without_touching_targets() {
    use std::os::unix::fs::symlink;
    for case in ["instance", "lock", "tmp", "cv-dir"] {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join(format!("{case}-target"));
        std::fs::write(&target, valid_token("outside")).unwrap();
        let cv = root.path().join(".cv");
        if case == "cv-dir" {
            symlink(outside.path(), &cv).unwrap();
        } else {
            std::fs::create_dir_all(&cv).unwrap();
            let rel = match case {
                "instance" => FILE,
                "lock" => LOCK,
                "tmp" => ".cv/workspace-instance.tmp",
                _ => unreachable!(),
            };
            symlink(&target, root.path().join(rel)).unwrap();
        }
        let err = workspace_instance::ensure(root.path()).unwrap_err();
        println!(
            "R14-SYMLINK case={case} error_kind={:?} error={err}",
            err.kind()
        );
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            valid_token("outside"),
            "{case}: symlink target was modified"
        );
    }
}

#[test]
fn r14_malformed_instance_bytes_are_preserved_and_never_reminted() {
    let long = format!("cvwi-{}", "x".repeat(128));
    for raw in ["", "\n", "foreign", "cvwi-!!!", &long] {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(".cv")).unwrap();
        std::fs::write(root.path().join(FILE), raw).unwrap();
        let err = workspace_instance::ensure(root.path()).unwrap_err();
        println!(
            "R14-MALFORMED raw={raw:?} error_kind={:?} error={err}",
            err.kind()
        );
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(
            std::fs::read_to_string(root.path().join(FILE)).unwrap(),
            raw,
            "malformed evidence must be preserved for diagnosis"
        );
    }
}

#[test]
fn r14_workspace_instance_child() {
    let Ok(root) = std::env::var("CV_R14_ROOT") else {
        return;
    };
    let out = std::env::var("CV_R14_OUT").expect("child output path");
    let id = workspace_instance::ensure(Path::new(&root)).expect("child ensure");
    std::fs::write(out, id).unwrap();
}

fn wait_child(mut child: std::process::Child, timeout: Duration) -> bool {
    let start = Instant::now();
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            return status.success();
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return false;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn r14_concurrent_first_open_cross_process_mint_has_exactly_one_winner() {
    let exe = std::env::current_exe().unwrap();
    for round in 0..12usize {
        let root = tempfile::tempdir().unwrap();
        let mut children = Vec::new();
        for child_index in 0..8usize {
            let out: PathBuf = root.path().join(format!("child-{child_index}.token"));
            children.push(
                Command::new(&exe)
                    .arg("r14_workspace_instance_child")
                    .arg("--exact")
                    .arg("--test-threads=1")
                    .env("CV_R14_ROOT", root.path())
                    .env("CV_R14_OUT", &out)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .expect("spawn mint child"),
            );
        }
        let mut ok = true;
        for child in children {
            ok &= wait_child(child, Duration::from_secs(8));
        }
        assert!(ok, "round {round}: a mint child failed or deadlocked");
        let mut tokens = Vec::new();
        for child_index in 0..8usize {
            tokens.push(
                std::fs::read_to_string(root.path().join(format!("child-{child_index}.token")))
                    .expect("child token"),
            );
        }
        let distinct: std::collections::BTreeSet<_> = tokens.iter().collect();
        let persisted = std::fs::read_to_string(root.path().join(FILE)).unwrap();
        println!(
            "R14-MINT-RACE round={round} distinct={} persisted={} first={}",
            distinct.len(),
            persisted.trim(),
            tokens[0]
        );
        assert_eq!(distinct.len(), 1, "round {round}: split mint tokens");
        assert_eq!(
            persisted.trim(),
            tokens[0],
            "round {round}: winner not persisted"
        );
    }
}
