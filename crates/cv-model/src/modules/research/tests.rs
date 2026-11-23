use super::*;
use cv_core::engine::{Fingerprint, PageDraft};
use cv_core::sys::{Hash, Timestamp};

/// Memory port with a writer that accepts every draft (records it).
#[derive(Default)]
struct Writer {
    mem: MemoryPort,
    drafts: Vec<PageDraft>,
}

impl ResearchPort for Writer {
    fn now(&self) -> Timestamp {
        self.mem.now()
    }
    fn commit(&mut self, rec: &cv_notes::Record) -> Result<(), String> {
        self.mem.commit(rec)
    }
    fn commit_provenance(&mut self, p: &cv_notes::Provenance) -> Result<(), String> {
        self.mem.commit_provenance(p)
    }
    fn write_page(&mut self, d: &PageDraft) -> Option<Result<WriteOutcome, String>> {
        self.drafts.push(d.clone());
        Some(Ok(WriteOutcome::Written {
            fingerprint: Fingerprint {
                sha256: Hash([0; 32]),
                mtime: Timestamp(0),
            },
        }))
    }
    fn describe(&self) -> String {
        "test".into()
    }
}

fn module(port: Box<dyn ResearchPort>) -> ResearchModule {
    ResearchModule::new(Research::new(Notebook::default(), port))
}

fn status(v: Vec<Intent>) -> String {
    match v.as_slice() {
        [Intent::Status(s)] => s.clone(),
        o => panic!("{o:?}"),
    }
}

#[test]
fn empty_inbox_has_a_placeholder_row() {
    let m = module(Box::new(MemoryPort::default()));
    assert_eq!(m.list_len(Some("inbox")), 1);
    assert_eq!(m.row(Some("inbox"), 0).key, EMPTY);
    assert!(m.noun(EMPTY).is_none());
    assert_eq!(m.list_len(Some("all")), 0);
    let keys: Vec<String> = m.nav().items.into_iter().map(|i| i.key).collect();
    assert_eq!(keys, ["inbox", "all", "filter"]);
}

#[test]
fn capture_triage_link_promote_flow() {
    let mut m = module(Box::new(Writer::default()));
    status(m.run("research.capture.idea"));
    status(m.run("research.capture.idea"));
    assert_eq!(m.list_len(Some("inbox")), 2);
    let first = m.row(Some("inbox"), 1).key;
    let on = |m: &mut ResearchModule, cmd: &str| {
        m.select(Some(&first));
        status(m.run(cmd))
    };
    assert!(on(&mut m, "research.link.recent").starts_with("linked"));
    assert!(on(&mut m, "research.triage.file").contains("unsorted"));
    assert_eq!(m.list_len(Some("nb:unsorted")), 1);
    let s = on(&mut m, "research.promote");
    assert!(s.starts_with("promoted → idea-1"), "{s}");
    assert_eq!(m.row(Some("all"), 0).badge.as_deref(), Some("promoted"));
    let r = m.reading(&first);
    assert!(
        r.lines
            .iter()
            .any(|l| l.label.as_deref() == Some("promoted"))
    );
    let other = m.row(Some("inbox"), 0).key;
    assert!(
        m.reading(&other)
            .lines
            .iter()
            .any(|l| l.label.as_deref() == Some("backlink"))
    );
}

#[test]
fn promote_without_writer_is_disclosed() {
    let mut m = module(Box::new(MemoryPort::default()));
    status(m.run("research.capture.idea"));
    let key = m.row(Some("inbox"), 0).key;
    m.select(Some(&key));
    let s = status(m.run("research.promote"));
    assert!(s.contains("no content writer attached"), "{s}");
    let f = m.inspector(&key).fields;
    assert!(
        f.iter()
            .any(|(k, v)| k == "promotion" && v.contains("no content writer"))
    );
}

#[test]
fn filter_builder_saves_a_smart_mailbox() {
    let mut m = module(Box::new(MemoryPort::default()));
    status(m.run("research.capture.idea"));
    status(m.run("research.capture.idea"));
    let key = m.row(Some("inbox"), 0).key;
    m.select(Some(&key));
    status(m.run("research.triage.archive"));
    status(m.run("research.filter.add"));
    assert_eq!(m.list_len(Some("filter")), 1);
    assert!(status(m.run("research.filter.save")).starts_with("saved smart mailbox"));
    let nav = m.nav().items;
    let saved = nav.iter().find(|i| i.key.starts_with("view:")).unwrap();
    assert_eq!(saved.count, Some(1));
    assert_eq!(m.list_len(Some(&saved.key)), 1);
}

#[test]
fn commands_act_only_on_the_selection_resolved_for_them() {
    let mut m = module(Box::new(MemoryPort::default()));
    status(m.run("research.capture.idea"));
    let first = m.row(Some("inbox"), 0).key;
    m.select(Some(&first));
    status(m.run("research.capture.idea"));
    // The capture consumed the focus; nothing is retargeted silently.
    assert_eq!(
        status(m.run("research.triage.archive")),
        "research: no note selected"
    );
    assert_eq!(m.list_len(Some("inbox")), 2);
}

#[test]
fn promoted_contains_degrades_to_equality() {
    let mut m = module(Box::new(Writer::default()));
    status(m.run("research.capture.idea"));
    let key = m.row(Some("inbox"), 0).key;
    m.select(Some(&key));
    status(m.run("research.promote"));
    for _ in 0..4 {
        status(m.run("research.filter.field")); // → promoted
    }
    status(m.run("research.filter.op"));
    status(m.run("research.filter.op")); // Contains
    status(m.run("research.filter.add"));
    assert_eq!(m.list_len(Some("filter")), 1);
}
