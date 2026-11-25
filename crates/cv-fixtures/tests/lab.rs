//! Lab captures: manifest pinning, the capture plan, and `StubLab`.

use cv_fixtures::lab::{
    CAPTURE_ROOT, LAB_CAPTURES, LAB_DIR, StubLabError, lab_capture, lab_captures,
};
use cv_fixtures::{Manifest, StubLab, corpora, fixtures_root};
use std::collections::BTreeMap;
use std::path::Path;

fn manifest() -> Manifest {
    Manifest::load(&fixtures_root().join("MANIFEST.toml"))
}

fn files_under(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        if e.path().is_dir() {
            files_under(&e.path(), out);
        } else {
            out.push(e.path());
        }
    }
}

#[test]
fn every_lab_file_on_disk_is_pinned_and_hashes() {
    let m = manifest();
    let root = fixtures_root();
    let mut on_disk = Vec::new();
    files_under(&root.join("captures").join(LAB_DIR), &mut on_disk);
    assert!(on_disk.len() > 100, "{}", on_disk.len());
    for path in &on_disk {
        let rel = path
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        m.verify(&rel, &std::fs::read(path).unwrap())
            .unwrap_or_else(|e| panic!("{e}"));
    }
    let pinned = m
        .pinned_files()
        .filter(|(k, _)| k.starts_with("captures/lab/"))
        .count();
    assert_eq!(
        pinned,
        on_disk.len(),
        "pinned lab files with no file on disk"
    );
}

#[test]
fn lab_captures_record_the_pinned_lab() {
    let m = manifest();
    let pin = m.migration_lab.as_ref().expect("[migration_lab]");
    let cap = m.lab_captures.as_ref().expect("[lab_captures]");
    assert_eq!(cap.tool_id, pin.tool_id);
    assert_eq!(cap.commit, pin.commit);
    assert_eq!(cap.capture_root, CAPTURE_ROOT);
    assert_eq!(cap.capture_command, "scripts/capture-lab-fixtures.sh");
    let boris: serde_json::Value = serde_json::from_str(&cap.boris_build_info).unwrap();
    assert_eq!(boris["version"], "boris/0.9.0");
    assert_eq!(StubLab::new().tool_id(), pin.tool_id);
}

#[test]
fn the_capture_plan_and_the_captures_on_disk_agree() {
    let mut planned: Vec<_> = LAB_CAPTURES.iter().map(|s| s.mode.to_string()).collect();
    planned.sort();
    assert_eq!(lab_captures(), planned);
    assert!(
        !corpora().iter().any(|c| c == LAB_DIR),
        "lab is not a corpus"
    );
    for s in LAB_CAPTURES {
        let t = lab_capture(s.mode).transcript();
        assert_eq!(t.argv.first(), Some(&format!("--mode={}", s.mode)));
        assert_eq!(
            t.argv.last(),
            Some(&format!("--out={CAPTURE_ROOT}/out/{}", s.mode))
        );
        assert_eq!(t.argv.len(), s.inputs.len() + 2, "{}", s.mode);
        assert_eq!(t.exit_code, 0, "{}: {}", s.mode, t.stderr);
        assert!(!t.out.is_empty(), "{}", s.mode);
    }
}

/// The scratch root is the only host path the lab sees; it must not leak.
#[test]
fn captures_carry_no_host_paths() {
    for mode in lab_captures() {
        let t = lab_capture(&mode).transcript();
        let texts = t
            .out
            .iter()
            .filter_map(|(rel, b)| Some((rel.as_str(), std::str::from_utf8(b).ok()?)))
            .chain([
                ("stdout.txt", t.stdout.as_str()),
                ("stderr.txt", t.stderr.as_str()),
            ])
            .chain(t.argv.iter().map(|a| ("argv.txt", a.as_str())));
        for (rel, s) in texts {
            for host in ["/Users/", "/home/", "/var/folders/", "/private/", "/tmp/"] {
                assert!(!s.contains(host), "{mode}/{rel} contains {host:?}");
            }
        }
    }
}

#[test]
fn stub_lab_writes_the_captured_tree_into_out() {
    let tmp = std::env::temp_dir().join(format!("cv-stub-lab-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let tmp = std::fs::canonicalize(tmp).unwrap();
    let t = StubLab::new().run("jekyll", &tmp).unwrap();
    for (rel, bytes) in &t.out {
        assert_eq!(&std::fs::read(tmp.join(rel)).unwrap(), bytes, "{rel}");
    }
    assert!(t.stderr.starts_with("jekyll: wrote "), "{}", t.stderr);
    assert!(matches!(
        t.document("report.json"),
        Some(Ok(cv_contracts::lab::Document::Report(_)))
    ));
    std::fs::remove_dir_all(&tmp).unwrap();
}

fn test_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("cv-stub-lab-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::canonicalize(dir).unwrap()
}

fn read_output_tree(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(dir: &Path, prefix: &str, out: &mut BTreeMap<String, Vec<u8>>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let rel = format!("{prefix}{}", entry.file_name().to_string_lossy());
            if entry.path().is_dir() {
                walk(&entry.path(), &format!("{rel}/"), out);
            } else {
                out.insert(rel, std::fs::read(entry.path()).unwrap());
            }
        }
    }
    let mut files = BTreeMap::new();
    walk(dir, "", &mut files);
    files
}

fn publication_sibling(out: &Path, suffix: &str) -> std::path::PathBuf {
    let mut path = out.as_os_str().to_os_string();
    path.push(suffix);
    std::path::PathBuf::from(path)
}

#[test]
fn publication_backed_stub_replaces_an_owned_output_and_drops_stale_files() {
    let captured = lab_capture("jekyll").transcript();
    let marker = captured.out[".boris-migration-lab-output"].clone();
    let root = test_dir("owned-replacement");
    let out = root.join("out");
    std::fs::create_dir_all(out.join("stale-dir")).unwrap();
    std::fs::write(out.join(".boris-migration-lab-output"), marker).unwrap();
    std::fs::write(out.join("stale-dir/stale.txt"), b"stale").unwrap();
    std::fs::write(out.join("stale.txt"), b"stale").unwrap();

    StubLab::new().run("jekyll", &out).unwrap();

    assert!(!out.join("stale.txt").exists());
    assert!(!out.join("stale-dir").exists());
    assert_eq!(read_output_tree(&out), captured.out);
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn publication_backed_stub_refuses_an_unowned_output_before_mutation() {
    let root = test_dir("unowned-refusal");
    let out = root.join("out");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("stale.txt"), b"stale").unwrap();

    let err = match StubLab::new().run("jekyll", &out) {
        Ok(_) => panic!("unowned output replayed"),
        Err(err) => err,
    };

    assert!(matches!(err, StubLabError::OutputNotOwned(_)), "{err}");
    assert_eq!(std::fs::read(out.join("stale.txt")).unwrap(), b"stale");
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn publication_backed_stub_refuses_another_mode_owned_output() {
    let root = test_dir("wrong-owner");
    let out = root.join("out");
    std::fs::create_dir_all(&out).unwrap();
    let hugo_marker = &lab_capture("hugo").transcript().out[".boris-migration-lab-output"];
    std::fs::write(out.join(".boris-migration-lab-output"), hugo_marker).unwrap();

    let err = match StubLab::new().run("jekyll", &out) {
        Ok(_) => panic!("another mode's output replayed"),
        Err(err) => err,
    };

    assert!(matches!(err, StubLabError::OutputNotOwned(_)), "{err}");
    assert_eq!(
        std::fs::read(out.join(".boris-migration-lab-output")).unwrap(),
        *hugo_marker
    );
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn publication_backed_stub_refuses_an_unowned_stage_before_mutation() {
    let root = test_dir("unowned-stage");
    let out = root.join("out");
    StubLab::new().run("jekyll", &out).unwrap();
    let before = read_output_tree(&out);
    let stage = publication_sibling(&out, ".boris-migration-lab-stage");
    std::fs::create_dir_all(&stage).unwrap();
    std::fs::write(stage.join("stale.txt"), b"stale").unwrap();

    let err = match StubLab::new().run("jekyll", &out) {
        Ok(_) => panic!("unowned stage replayed"),
        Err(err) => err,
    };

    assert!(matches!(err, StubLabError::StageNotOwned(_)), "{err}");
    assert_eq!(read_output_tree(&out), before);
    assert_eq!(std::fs::read(stage.join("stale.txt")).unwrap(), b"stale");
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn publication_backed_stub_cleans_an_owned_stage_before_replay() {
    let captured = lab_capture("jekyll").transcript();
    let marker = captured.out[".boris-migration-lab-output"].clone();
    let root = test_dir("owned-stage");
    let out = root.join("out");
    StubLab::new().run("jekyll", &out).unwrap();
    let stage = publication_sibling(&out, ".boris-migration-lab-stage");
    std::fs::create_dir_all(&stage).unwrap();
    std::fs::write(stage.join(".boris-migration-lab-output"), marker).unwrap();
    std::fs::write(stage.join("stale.txt"), b"stale").unwrap();

    StubLab::new().run("jekyll", &out).unwrap();

    assert!(!stage.exists());
    assert_eq!(read_output_tree(&out), captured.out);
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn publication_backed_stub_refuses_an_unowned_backup_before_mutation() {
    let root = test_dir("unowned-backup");
    let out = root.join("out");
    StubLab::new().run("jekyll", &out).unwrap();
    let before = read_output_tree(&out);
    let backup = publication_sibling(&out, ".boris-migration-lab-prev");
    std::fs::create_dir_all(&backup).unwrap();
    std::fs::write(backup.join("stale.txt"), b"stale").unwrap();

    let err = match StubLab::new().run("jekyll", &out) {
        Ok(_) => panic!("unowned backup replayed"),
        Err(err) => err,
    };

    assert!(matches!(err, StubLabError::BackupNotOwned(_)), "{err}");
    assert_eq!(read_output_tree(&out), before);
    assert_eq!(std::fs::read(backup.join("stale.txt")).unwrap(), b"stale");
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn publication_backed_stub_cleans_an_owned_backup_before_replay() {
    let captured = lab_capture("jekyll").transcript();
    let marker = captured.out[".boris-migration-lab-output"].clone();
    let root = test_dir("owned-backup");
    let out = root.join("out");
    StubLab::new().run("jekyll", &out).unwrap();
    let backup = publication_sibling(&out, ".boris-migration-lab-prev");
    std::fs::create_dir_all(&backup).unwrap();
    std::fs::write(backup.join(".boris-migration-lab-output"), marker).unwrap();
    std::fs::write(backup.join("stale.txt"), b"stale").unwrap();

    StubLab::new().run("jekyll", &out).unwrap();

    assert!(!backup.exists());
    assert_eq!(read_output_tree(&out), captured.out);
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn publication_backed_stub_refuses_a_file_output_before_mutation() {
    let root = test_dir("file-output");
    let out = root.join("out");
    std::fs::write(&out, b"not a directory").unwrap();

    let err = match StubLab::new().run("jekyll", &out) {
        Ok(_) => panic!("file output replayed"),
        Err(err) => err,
    };

    assert!(matches!(err, StubLabError::OutputNotDirectory(_)), "{err}");
    assert_eq!(std::fs::read(&out).unwrap(), b"not a directory");
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn non_publication_stub_writes_without_deleting_unowned_files() {
    let captured = lab_capture("obsidian").transcript();
    assert!(!captured.out.contains_key(".boris-migration-lab-output"));
    let root = test_dir("direct-write");
    let out = root.join("out");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("keep.txt"), b"keep").unwrap();

    StubLab::new().run("obsidian", &out).unwrap();

    assert_eq!(std::fs::read(out.join("keep.txt")).unwrap(), b"keep");
    for (rel, bytes) in &captured.out {
        assert_eq!(std::fs::read(out.join(rel)).unwrap(), *bytes, "{rel}");
    }
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn stub_lab_refuses_a_mode_it_never_captured() {
    let tmp = std::env::temp_dir();
    assert!(matches!(
        StubLab::new().run("mkdocs", &tmp),
        Err(StubLabError::NoCapture(m)) if m == "mkdocs"
    ));
}
