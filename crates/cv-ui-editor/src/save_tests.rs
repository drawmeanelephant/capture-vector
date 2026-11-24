use super::*;
use std::cell::Cell;

/// A planted link below the root: a Unix symlink or a Windows directory
/// symlink/junction — all carry the reparse attribute `is_link` checks.
#[cfg(unix)]
fn link_dir(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}
#[cfg(windows)]
fn link_dir(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(target, link)
}
#[cfg(unix)]
fn link_file(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}
#[cfg(windows)]
fn link_file(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_file(target, link)
}

fn sharing() -> std::io::Error {
    std::io::Error::other("sharing violation")
}

#[test]
fn retry_is_five_fixed_fifty_ms_then_honest_failure() {
    let calls = Cell::new(0);
    let mut waits = Vec::new();
    let r: std::io::Result<()> = retry_pinned(
        || {
            calls.set(calls.get() + 1);
            Err(sharing())
        },
        |_| true,
        |d| waits.push(d),
    );
    let err = r.unwrap_err();
    assert_eq!(calls.get(), 1 + RENAME_RETRIES as usize);
    assert_eq!(waits, vec![Duration::from_millis(50); 5]);
    assert!(err.to_string().contains("gave up after 5 retries"), "{err}");
}

#[test]
fn retry_recovers_when_the_handle_releases() {
    let calls = Cell::new(0);
    let r = retry_pinned(
        || {
            calls.set(calls.get() + 1);
            if calls.get() < 3 {
                Err(sharing())
            } else {
                Ok(7)
            }
        },
        |_| true,
        |_| {},
    );
    assert_eq!(r.unwrap(), 7);
    assert_eq!(calls.get(), 3);
}

#[test]
fn non_transient_errors_do_not_retry() {
    let calls = Cell::new(0);
    let r: std::io::Result<()> = retry_pinned(
        || {
            calls.set(calls.get() + 1);
            Err(sharing())
        },
        |_| false,
        |_| panic!("no wait"),
    );
    assert!(r.is_err());
    assert_eq!(calls.get(), 1);
}

#[test]
fn staged_save_writes_then_refuses_a_stale_base() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("page.md");
    std::fs::write(&path, "one").unwrap();
    let base = fingerprint_file(&path).unwrap();
    let writer = FsWriter::new(dir.path());
    let mut buf = Buffer::open("page.md", "one", base);
    buf.insert(3, " two").unwrap();
    let ticket = buf.begin_save();
    buf.insert(7, "!").unwrap();
    let st = buf.finish_save(&ticket, save_blocking(&writer, &ticket));
    assert_eq!(
        st,
        SaveStatus::Saved {
            revision: ticket.revision
        }
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "one two");
    assert!(buf.is_dirty(), "edit made during the save stays dirty");
    let leftovers: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty());

    std::fs::write(&path, "changed elsewhere").unwrap();
    let ticket = buf.begin_save();
    let st = buf.finish_save(&ticket, save_blocking(&writer, &ticket));
    assert!(matches!(st, SaveStatus::Conflict { .. }), "{st:?}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "changed elsewhere");
    assert!(buf.is_dirty());
}

#[test]
fn new_files_need_no_base_and_paths_stay_inside_root() {
    let dir = tempfile::tempdir().unwrap();
    let writer = FsWriter::new(dir.path());
    let buf = Buffer::open("notes/new.md", "# hi", None);
    let t = buf.begin_save();
    assert!(matches!(
        writer.write(&t.draft),
        Ok(WriteOutcome::Written { .. })
    ));
    for bad in ["../x.md", "/etc/x.md", ""] {
        let d = PageDraft {
            path: bad.into(),
            body: String::new(),
            base: None,
        };
        assert!(writer.write(&d).is_err(), "{bad}");
    }
}

#[test]
fn save_refuses_linked_parent_before_creating_directories() {
    let dir = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    link_dir(other.path(), &dir.path().join("linked")).unwrap();
    let writer = FsWriter::new(dir.path());
    let buf = Buffer::open("linked/new/page.md", "# Draft", None);
    assert!(writer.write(&buf.begin_save().draft).is_err());
    assert_eq!(std::fs::read_dir(other.path()).unwrap().count(), 0);
}

#[test]
fn a_symlink_inside_the_root_cannot_redirect_a_write_outside() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    link_dir(outside.path(), &dir.path().join("link")).unwrap();
    let writer = FsWriter::new(dir.path());
    for bad in ["link/x.md", "link/nested/x.md"] {
        let d = PageDraft {
            path: bad.into(),
            body: "planted".into(),
            base: None,
        };
        assert!(writer.write(&d).is_err(), "{bad}");
    }
    assert!(std::fs::read_dir(outside.path()).unwrap().next().is_none());
}

/// Every entry under `dir` with file contents, for "unchanged" checks.
fn tree(dir: &Path) -> Vec<(PathBuf, Option<Vec<u8>>)> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let meta = std::fs::symlink_metadata(&path).unwrap();
        if meta.is_dir() {
            out.push((path.clone(), None));
            out.extend(tree(&path));
        } else {
            out.push((path.clone(), Some(std::fs::read(&path).unwrap())));
        }
    }
    out.sort();
    out
}

fn draft(path: &str, base: Option<Fingerprint>) -> PageDraft {
    PageDraft {
        path: path.into(),
        body: "planted".into(),
        base,
    }
}

#[test]
fn save_through_an_escaping_directory_link_leaves_the_outside_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("authorized_keys"), "theirs").unwrap();
    std::fs::create_dir(outside.path().join("sub")).unwrap();
    link_dir(outside.path(), &dir.path().join("link")).unwrap();
    let before = tree(outside.path());
    let base = fingerprint_file(&outside.path().join("authorized_keys")).unwrap();
    let writer = FsWriter::new(dir.path());

    for (rel, base) in [
        ("link/authorized_keys", None),
        ("link/authorized_keys", base),
        ("link/new.md", None),
        ("link/sub/new.md", None),
        ("link/sub/fresh/deeper/new.md", None),
    ] {
        assert!(writer.write(&draft(rel, base)).is_err(), "{rel}");
    }
    assert_eq!(tree(outside.path()), before);
}

#[test]
fn save_refuses_a_directory_link_that_stays_inside_the_root() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("real")).unwrap();
    link_dir(&dir.path().join("real"), &dir.path().join("alias")).unwrap();
    let writer = FsWriter::new(dir.path());

    assert!(writer.write(&draft("alias/page.md", None)).is_err());
    assert_eq!(
        std::fs::read_dir(dir.path().join("real")).unwrap().count(),
        0
    );
}

#[test]
fn save_refuses_a_linked_final_target_and_keeps_the_link() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let victim = outside.path().join("victim.md");
    std::fs::write(&victim, "theirs").unwrap();
    let link = dir.path().join("page.md");
    link_file(&victim, &link).unwrap();
    let writer = FsWriter::new(dir.path());

    assert!(writer.write(&draft("page.md", None)).is_err());
    assert_eq!(std::fs::read_to_string(&victim).unwrap(), "theirs");
    assert!(
        std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[test]
fn save_works_when_the_root_is_reached_through_a_link() {
    let real = tempfile::tempdir().unwrap();
    let holder = tempfile::tempdir().unwrap();
    let root = holder.path().join("workspace");
    link_dir(real.path(), &root).unwrap();
    let writer = FsWriter::new(&root);

    assert!(matches!(
        writer.write(&draft("content/new/page.md", None)),
        Ok(WriteOutcome::Written { .. })
    ));
    assert_eq!(
        std::fs::read_to_string(real.path().join("content/new/page.md")).unwrap(),
        "planted"
    );
}

#[test]
fn save_creates_nested_directories_inside_the_root() {
    let dir = tempfile::tempdir().unwrap();
    let writer = FsWriter::new(dir.path());
    assert!(matches!(
        writer.write(&draft("content/a/b/page.md", None)),
        Ok(WriteOutcome::Written { .. })
    ));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("content/a/b/page.md")).unwrap(),
        "planted"
    );
}

#[test]
fn engine_errors_surface_as_failed_not_dropped() {
    let w = EngineWriter(RefusingEngine);
    let mut buf = Buffer::open("page.md", "x", None);
    buf.insert(1, "y").unwrap();
    let t = buf.begin_save();
    let st = buf.finish_save(&t, save_blocking(&w, &t));
    assert!(matches!(st, SaveStatus::Failed { .. }), "{st:?}");
    assert!(buf.is_dirty());
}

struct RefusingEngine;
impl Engine for RefusingEngine {
    fn locate(&self) -> cv_core::Result<cv_core::engine::EngineInfo> {
        Err(cv_core::Error::Engine("none".into()))
    }
    fn capabilities(&self) -> cv_core::engine::EngineCaps {
        cv_core::engine::EngineCaps::default()
    }
    fn snapshot(&self) -> cv_core::Result<cv_core::rules::ArtifactSet> {
        Err(cv_core::Error::Engine("none".into()))
    }
    fn watch(
        &self,
        _: cv_core::engine::EventSink,
    ) -> cv_core::Result<Box<dyn cv_core::engine::WatchHandle>> {
        Err(cv_core::Error::Engine("none".into()))
    }
    fn run(
        &self,
        _: &cv_core::engine::Verb,
        _: cv_core::engine::EventSink,
    ) -> cv_core::Result<cv_core::engine::ExitClass> {
        Err(cv_core::Error::Engine("none".into()))
    }
    fn write_content(
        &self,
        _: &PageDraft,
        _: Option<Fingerprint>,
    ) -> cv_core::Result<WriteOutcome> {
        Err(cv_core::Error::Engine("writes unsupported".into()))
    }
}
