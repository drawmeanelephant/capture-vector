use super::*;

#[cfg(unix)]
fn write_adoption_and_pending(root: &Path, run: &str) {
    let lock = claim_run(root, run).unwrap();
    let adoption = AdoptionRecord::new(run, "hugo", 5, "workspace", "existing");
    write_adoption(&lock, &adoption).unwrap();
    let pending = PendingAdoption::new(run, "existing", "content");
    write_pending(&lock, &pending).unwrap();
}

#[cfg(unix)]
fn link_dir_or_file(target: &Path, link: &Path) {
    std::os::unix::fs::symlink(target, link).unwrap();
}

#[cfg(unix)]
#[test]
fn direct_adoption_readers_refuse_a_linked_run_directory() {
    let d = tempfile::tempdir().unwrap();
    write_adoption_and_pending(d.path(), "run");
    let base = d.path().join(DIR);
    let run = base.join("run");
    let moved = d.path().join("moved-run");
    std::fs::rename(&run, &moved).unwrap();
    link_dir_or_file(&moved, &run);

    for err in [
        read_adoption_record(d.path(), "run").unwrap_err(),
        read_pending(d.path(), "run").unwrap_err(),
    ] {
        assert!(err.contains("refusing symlinked"), "{err}");
    }
    assert!(scan(d.path()).unwrap().is_empty());
    assert!(scan_adoptions(d.path()).unwrap().is_empty());
}

#[cfg(unix)]
#[test]
fn direct_adoption_readers_refuse_linked_namespace_ancestors() {
    for linked_imports in [true, false] {
        let d = tempfile::tempdir().unwrap();
        write_adoption_and_pending(d.path(), "run");
        if linked_imports {
            let imports = d.path().join(DIR);
            let moved = d.path().join(".cv/imports-real");
            std::fs::rename(&imports, &moved).unwrap();
            link_dir_or_file(&moved, &imports);
        } else {
            let cv = d.path().join(".cv");
            let moved = d.path().join(".cv-real");
            std::fs::rename(&cv, &moved).unwrap();
            link_dir_or_file(&moved, &cv);
        }

        for err in [
            read_adoption_record(d.path(), "run").unwrap_err(),
            read_pending(d.path(), "run").unwrap_err(),
        ] {
            assert!(err.contains("refusing symlinked"), "{err}");
        }
    }
}

#[cfg(unix)]
#[test]
fn linked_adoption_and_pending_evidence_errors_instead_of_disappearing() {
    let d = tempfile::tempdir().unwrap();
    write_adoption_and_pending(d.path(), "run");
    let dir = run_dir(d.path(), "run").unwrap();
    let outside = tempfile::tempdir().unwrap();

    for name in [ADOPTION, PENDING] {
        let evidence = dir.join(name);
        let bytes = std::fs::read(&evidence).unwrap();
        let target = outside.path().join(name);
        std::fs::write(&target, bytes).unwrap();
        std::fs::remove_file(&evidence).unwrap();
        link_dir_or_file(&target, &evidence);
    }

    assert!(read_adoption_record(d.path(), "run").is_err());
    assert!(read_pending(d.path(), "run").is_err());
    assert!(matches!(scan_adoptions(d.path()), Err(Error::Symlink(_))));

    std::fs::remove_file(outside.path().join(ADOPTION)).unwrap();
    assert!(matches!(scan_adoptions(d.path()), Err(Error::Symlink(_))));
}
