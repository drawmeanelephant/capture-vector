//! No-follow byte access to lab output evidence inside `.cv/imports`.
//!
//! Run output is evidence written by an external tool inside a synced or
//! cloned workspace. Readers must not let a symlink redirect them outside
//! the run directory: every relevant `.cv` ancestor is checked, path
//! components below `out/` are confined, and the final open uses the
//! platform no-follow flags shared with run-lock handling.

use super::*;
use crate::paths::{confined_file, is_link, portable_relative};
use std::io::Read;

fn invalid_input(message: impl Into<String>) -> Error {
    Error::Io(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        message.into(),
    ))
}

fn refuse_evidence_link(path: &Path) -> Result<(), Error> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if is_link(&meta) => Err(Error::Symlink(path.to_path_buf())),
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Read `rel` below `<root>/.cv/imports/<run>/out/` without following a
/// symlink at any checked component. `rel` is portable `/`-separated
/// evidence syntax. Missing files and permission failures remain ordinary
/// I/O errors; symlinked or non-file evidence is refused explicitly.
pub fn read_out_file(root: &Path, run_id: &str, rel: &str) -> Result<Vec<u8>, Error> {
    let dir = run_dir(root, run_id)?;
    let out = dir.join(OUT);
    for path in [root.join(".cv"), root.join(DIR), dir.clone(), out.clone()] {
        refuse_evidence_link(&path)?;
    }
    let out_meta = std::fs::symlink_metadata(&out)?;
    if !out_meta.is_dir() {
        return Err(Error::NotDirectory(out));
    }
    let rel_path = portable_relative(rel)
        .map_err(|_| invalid_input(format!("evidence path {rel:?} is not portable")))?;
    let path = confined_file(&out, rel_path)?;
    refuse_evidence_link(&path)?;
    let mut file = crate::open::open_existing_no_follow(&path, false)?;
    let opened_meta = file.metadata()?;
    if !opened_meta.file_type().is_file() {
        return Err(invalid_input(format!(
            "evidence {} is not an ordinary file",
            path.display()
        )));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let after = std::fs::symlink_metadata(&path)?;
    if is_link(&after) || !after.is_file() {
        return Err(invalid_input(format!(
            "evidence {} changed shape while it was being read",
            path.display()
        )));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link_file(target: &Path, link: &Path) {
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, link).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(target, link).unwrap();
    }

    fn link_dir(target: &Path, link: &Path) {
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, link).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(target, link).unwrap();
    }

    fn staged_run(root: &Path) -> RunLock {
        let lock = claim_run(root, "evidence").unwrap();
        std::fs::create_dir_all(lock.dir().join(OUT)).unwrap();
        lock
    }

    #[test]
    fn out_evidence_reads_an_ordinary_confined_file() {
        let root = tempfile::tempdir().unwrap();
        let lock = staged_run(root.path());
        std::fs::write(lock.dir().join(OUT).join("report.json"), b"report").unwrap();
        assert_eq!(
            read_out_file(root.path(), "evidence", "report.json").unwrap(),
            b"report"
        );
    }

    #[test]
    fn out_evidence_refuses_a_final_symlink_without_following_it() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let victim = outside.path().join("victim.json");
        std::fs::write(&victim, b"outside").unwrap();
        let lock = staged_run(root.path());
        let link = lock.dir().join(OUT).join("report.json");
        link_file(&victim, &link);

        assert!(matches!(
            read_out_file(root.path(), "evidence", "report.json"),
            Err(Error::Symlink(_)) | Err(Error::Io(_))
        ));
        assert_eq!(std::fs::read(&victim).unwrap(), b"outside");
    }

    #[test]
    fn out_evidence_refuses_a_nonordinary_file_and_a_symlinked_out_dir() {
        let root = tempfile::tempdir().unwrap();
        let lock = staged_run(root.path());
        let evidence = lock.dir().join(OUT).join("report.json");
        std::fs::create_dir(&evidence).unwrap();
        assert!(read_out_file(root.path(), "evidence", "report.json").is_err());
        std::fs::remove_dir(&evidence).unwrap();
        drop(lock);

        let real = tempfile::tempdir().unwrap();
        let run = root.path().join(DIR).join("evidence");
        std::fs::remove_dir_all(run.join(OUT)).unwrap();
        link_dir(real.path(), &run.join(OUT));
        assert!(matches!(
            read_out_file(root.path(), "evidence", "report.json"),
            Err(Error::Symlink(_))
        ));
    }
}
