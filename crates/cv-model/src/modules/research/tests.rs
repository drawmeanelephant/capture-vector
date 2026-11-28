use super::*;
use crate::dialog::{Dialogs, OpenDialog};
use crate::workbench::{KeyInput, KeyPress};
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
        [Intent::Outcome(s)] => s.clone(),
        o => panic!("{o:?}"),
    }
}

fn dialog(v: Vec<Intent>) -> DialogSpec {
    match v.as_slice() {
        [Intent::Dialog(d)] => (**d).clone(),
        o => panic!("{o:?}"),
    }
}

fn type_in(d: &mut OpenDialog, text: &str) {
    for c in text.chars() {
        d.key(&KeyPress::new(KeyInput::Char(c)));
    }
}

/// Answer `spec` with `action` after `edit`, as the shell would.
fn reply(
    m: &mut ResearchModule,
    spec: DialogSpec,
    edit: impl FnOnce(&mut OpenDialog),
    action: &str,
) -> Vec<Intent> {
    let mut ds = Dialogs::default();
    ds.push(spec);
    edit(ds.open_mut().unwrap());
    let f = ds.finish(Some(action)).unwrap();
    m.answer(&f.id, &f.answer)
}

fn capture(m: &mut ResearchModule, text: &str) -> String {
    let spec = dialog(m.run("research.capture.idea"));
    status(reply(m, spec, |d| type_in(d, text), "capture"))
}

fn archive(m: &mut ResearchModule, key: &str) -> String {
    m.select(Some(key));
    let spec = dialog(m.run("research.triage.archive"));
    status(reply(m, spec, |_| {}, "archive"))
}

/// The filter form answered with field `field`, operator `op` and `value`.
fn add_term(
    m: &mut ResearchModule,
    field: usize,
    op: usize,
    value: &str,
    save: bool,
) -> Vec<Intent> {
    let spec = dialog(m.run("research.filter.build"));
    reply(
        m,
        spec,
        |d| {
            d.press_field(0, Some(field));
            d.press_field(1, Some(op));
            d.press_field(2, None);
            type_in(d, value);
            if save {
                d.press_field(3, Some(0));
            }
        },
        "add",
    )
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
    capture(&mut m, "Idea 1");
    capture(&mut m, "Idea 2");
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
    assert_eq!(
        m.row(Some("all"), 0).badge.map(|c| c.text).as_deref(),
        Some("promoted")
    );
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
    capture(&mut m, "Idea 1");
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
    capture(&mut m, "Idea 1");
    capture(&mut m, "Idea 2");
    let key = m.row(Some("inbox"), 0).key;
    archive(&mut m, &key);
    // triage = archived
    status(add_term(&mut m, 0, 0, "archived", false));
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
    capture(&mut m, "Idea 1");
    let first = m.row(Some("inbox"), 0).key;
    m.select(Some(&first));
    capture(&mut m, "Idea 2");
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
    capture(&mut m, "Idea 1");
    let key = m.row(Some("inbox"), 0).key;
    m.select(Some(&key));
    status(m.run("research.promote"));
    // promoted contains true
    status(add_term(&mut m, 4, 2, "true", false));
    assert_eq!(m.list_len(Some("filter")), 1);
}

#[test]
fn capture_asks_for_the_idea_and_stores_what_was_typed() {
    let mut m = module(Box::new(MemoryPort::default()));
    let spec = dialog(m.run("research.capture.idea"));
    assert!(matches!(spec.kind, DialogKind::Prompt(_)), "{spec:?}");
    assert!(capture(&mut m, "Backlinks resolve at read time").starts_with("captured"));
    assert_eq!(
        m.row(Some("inbox"), 0).title,
        "Backlinks resolve at read time"
    );
}

#[test]
fn an_empty_idea_is_asked_for_again_with_the_reason() {
    let mut m = module(Box::new(MemoryPort::default()));
    let spec = dialog(m.run("research.capture.idea"));
    let again = dialog(reply(&mut m, spec, |d| type_in(d, "   "), "capture"));
    assert_eq!(again.id, "research.capture.idea");
    assert!(again.error.is_some());
    assert_eq!(m.list_len(Some("all")), 0);
}

#[test]
fn archive_asks_first_and_cancelling_keeps_the_note_in_the_inbox() {
    let mut m = module(Box::new(MemoryPort::default()));
    capture(&mut m, "Idea 1");
    let key = m.row(Some("inbox"), 0).key;
    m.select(Some(&key));
    let spec = dialog(m.run("research.triage.archive"));
    assert_eq!(spec.context.as_deref(), Some(key.as_str()));
    assert!(spec.body.as_deref().unwrap_or_default().contains("Idea 1"));
    assert!(reply(&mut m, spec, |_| {}, crate::dialog::CANCEL).is_empty());
    assert_eq!(m.list_len(Some("inbox")), 1);
    assert_eq!(archive(&mut m, &key), format!("archived {key}"));
    assert_eq!(m.list_len(Some("all")), 1);
    assert_eq!(m.row(Some("inbox"), 0).key, EMPTY);
}

#[test]
fn the_filter_form_rejects_a_non_boolean_for_promoted_and_can_save_at_once() {
    let mut m = module(Box::new(MemoryPort::default()));
    capture(&mut m, "Idea 1");
    let again = dialog(add_term(&mut m, 4, 0, "maybe", false));
    assert!(again.error.unwrap().contains("true or false"));
    assert_eq!(m.list_len(Some("filter")), 1, "no term was added");
    let s = status(add_term(&mut m, 4, 0, "false", true));
    assert!(s.starts_with("saved smart mailbox"), "{s}");
    assert!(m.nav().items.iter().any(|i| i.key.starts_with("view:")));
}
