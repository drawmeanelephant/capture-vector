// Shared by `examples/research-fixture.rs` (which emits the corpora that
// `scripts/capture-research-fixtures.sh` runs real Boris/Oliver over) and
// the tests that assert against those captures.
#![allow(dead_code)]

use cv_core::id::StableId;
use cv_notes::{Capture, MemoryPort, Note, Notebook, PromoteSpec, Research, Target};

pub const PAGE_ID: &str = "field-notes-graph-first-research";

pub const MD_BODY: &str = "# Graph-first research\n\nCaptured while reading [[guides/getting-started]]: the parent key shapes\nthe nav forest, so a promoted note needs a real parent.\n\n- notes stay in `.cv/`\n- pages stay in `content/`\n";
pub const TEXTILE_BODY: &str =
    "h2. Reading list\n\np. Notes keep a \"home page\":./index link.\n\n* one\n* two\n";
pub const COOK_BODY: &str = "Mix @water{2%cups} and @salt{1%pinch}.\n";

/// The research session every fixture note comes from.
pub fn session() -> (Research<MemoryPort>, StableId) {
    let mut r = Research::new(Notebook::default(), MemoryPort::default());
    let id = r
        .capture(
            Capture::Url {
                url: "https://example.org/graphs".into(),
                title: Some("Graphs".into()),
            },
            Some("Field notes: graph-first research"),
        )
        .unwrap();
    r.edit(&id, None, Some(MD_BODY)).unwrap();
    r.tag(&id, "research").unwrap();
    r.tag(&id, "graphs").unwrap();
    let other = r
        .capture(
            Capture::Idea {
                text: "Backlinks resolve at read time".into(),
            },
            None,
        )
        .unwrap();
    r.link(&other, Target::Note(id.clone())).unwrap();
    (r, id)
}

pub fn field_note() -> Note {
    let (r, id) = session();
    r.notebook.note(&id).unwrap().clone()
}

pub fn promoted_spec(n: &Note) -> PromoteSpec {
    PromoteSpec::for_note(n, "content").parent("index")
}

/// The same page after a move + reparent: new path, new parent, same id.
pub fn reparented_spec(n: &Note) -> PromoteSpec {
    let mut s = PromoteSpec::for_note(n, "content").parent("guides/getting-started");
    s.path = format!("content/guides/{PAGE_ID}.md");
    s
}

/// A draft Boris must reject (`EPARENTMISSING`) — Rust never judges it.
pub fn orphan_spec(n: &Note) -> PromoteSpec {
    PromoteSpec::for_note(n, "content").parent("no-such-page")
}
