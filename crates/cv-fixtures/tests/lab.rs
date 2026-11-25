//! Lab captures: manifest pinning, the capture plan, and `StubLab`.

use cv_fixtures::lab::{
    CAPTURE_ROOT, LAB_CAPTURES, LAB_DIR, StubLabError, lab_capture, lab_captures,
};
use cv_fixtures::{Manifest, StubLab, corpora, fixtures_root};
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

#[test]
fn stub_lab_refuses_a_mode_it_never_captured() {
    let tmp = std::env::temp_dir();
    assert!(matches!(
        StubLab::new().run("mkdocs", &tmp),
        Err(StubLabError::NoCapture(m)) if m == "mkdocs"
    ));
}
