//! Round-14 adversarial probes for `.cv/workspace-instance` evidence.

use cv_store::workspace_instance::{self, FILE, LOCK};

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
