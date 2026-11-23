use super::*;
use std::cell::Cell;

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
