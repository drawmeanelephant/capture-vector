//! Inbox → triage → promote → rebuild → backlink on StubEngine, then a
//! restart that recovers everything from `.cv/research/` JSONL truth.

#[path = "support/fixture_notes.rs"]
mod fixture_notes;
#[path = "support/research.rs"]
mod research;

use cv_core::engine::{Engine, ExitClass, Verb, WriteOutcome};
use cv_core::id::{AccountId, AttrValue, StableId};
use cv_core::store::{Op, Predicate, Term};
use cv_core::sys::FixedClock;
use cv_fixtures::StubEngine;
use cv_notes::disk::{self, DiskPort};
use cv_notes::{Capture, PageIndex, Research, Target, Triage};
use fixture_notes::*;
use std::path::Path;

fn open(root: &Path) -> (Research<DiskPort>, disk::OpenReport) {
    let clock = Box::new(FixedClock(cv_core::sys::Timestamp(1_700_000_000_000)));
    let (r, rep) = disk::open(root, AccountId("local".into()), clock).unwrap();
    let engine = research::StagedStub {
        stub: StubEngine::new("happy"),
        root: root.to_path_buf(),
    };
    (
        Research::new(r.notebook, r.port.with_writer(Box::new(engine))),
        rep,
    )
}

fn snapshot(r: &Research<DiskPort>) -> String {
    let nb = &r.notebook;
    format!(
        "{:?}|{:?}|{:?}|{:?}",
        nb.all(),
        nb.links().collect::<Vec<_>>(),
        nb.views().collect::<Vec<_>>(),
        nb.provenance_map().collect::<Vec<_>>()
    )
}

#[test]
fn inbox_promote_rebuild_backlink_restart() {
    let ws = tempfile::tempdir().unwrap();
    research::copy_corpus("promoted", ws.path());
    let page = ws.path().join(format!("content/notes/{PAGE_ID}.md"));
    std::fs::remove_file(&page).unwrap();

    let (mut r, rep) = open(ws.path());
    assert_eq!(rep.notes.lines, 0);
    // Replay the fixture session's verbs against disk truth.
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
    let idea = r
        .capture(
            Capture::Idea {
                text: "Backlinks resolve at read time".into(),
            },
            None,
        )
        .unwrap();
    let link = r.link(&idea, Target::Note(id.clone())).unwrap();
    assert_eq!(r.notebook.inbox().len(), 2);

    // Triage.
    r.file(&id, "graphs").unwrap();
    assert_eq!(r.notebook.inbox().len(), 1);
    assert_eq!(r.notebook.notebooks()["graphs"], 1);

    // Promote through the staged writer — exactly the bytes Boris validated.
    let note = r.notebook.note(&id).unwrap().clone();
    let p = r.promote(&id, &promoted_spec(&note)).unwrap();
    assert!(matches!(p.outcome, WriteOutcome::Written { .. }));
    assert_eq!(
        std::fs::read_to_string(&page).unwrap(),
        research::file_str(&format!("corpus-src/promoted/content/notes/{PAGE_ID}.md"))
    );
    assert!(
        r.promote(&id, &promoted_spec(&note)).is_err(),
        "promote is once"
    );

    // Rebuild (replayed) and resolve page → notes at read time.
    let exit = r.port_engine_build();
    assert_eq!(exit, ExitClass::Ok);
    let ir = PageIndex::from_graph_json(&research::file("captures/promoted/build-ir/graph.json"))
        .unwrap();
    let page_id = StableId(PAGE_ID.into());
    assert!(ir.get(&page_id).is_some());
    assert_eq!(r.notebook.page_backlinks(&page_id), vec![id.clone()]);
    assert_eq!(r.notebook.backlinks(&Target::Note(id.clone()))[0].id, link);

    // Smart mailbox + FTS5.
    let view = r
        .save_view(
            "Promoted research",
            Predicate {
                conj: vec![
                    Term {
                        field: "promoted".into(),
                        op: Op::Eq,
                        value: AttrValue::Bool(true),
                    },
                    Term {
                        field: "tags".into(),
                        op: Op::Contains,
                        value: AttrValue::Text(",research,".into()),
                    },
                ],
            },
        )
        .unwrap();
    let pred = r.notebook.view(&view).unwrap().predicate.clone();
    assert_eq!(r.notebook.select(&pred).len(), 1);
    assert_eq!(r.search("nav forest"), vec![id.clone()]);
    assert_eq!(
        r.search("graphs research"),
        vec![id.clone()],
        "tags are indexed"
    );
    assert_eq!(r.search("\"unbalanced"), Vec::<StableId>::new());
    let before = snapshot(&r);
    drop(r);

    // Restart: JSONL truth replays to identical state; index is reused.
    let (r, rep) = open(ws.path());
    assert_eq!((rep.rejected, rep.index_rebuilt), (0, false));
    assert_eq!(snapshot(&r), before);
    assert_eq!(r.search("nav forest"), vec![id.clone()]);
    drop(r);

    // Lose the index and tear the journal tail: both recover.
    let dir = ws.path().join(disk::DIR);
    std::fs::remove_file(dir.join(disk::INDEX)).unwrap();
    let mut j = std::fs::OpenOptions::new()
        .append(true)
        .open(dir.join(disk::NOTES))
        .unwrap();
    std::io::Write::write_all(&mut j, b"{\"op\":\"tagg").unwrap();
    drop(j);
    let (r, rep) = open(ws.path());
    assert!(rep.index_rebuilt);
    assert_eq!(rep.notes.torn_bytes, 11);
    assert_eq!(snapshot(&r), before);
    assert_eq!(r.search("nav forest"), vec![id.clone()]);
    assert_eq!(r.notebook.page_backlinks(&page_id), vec![id]);
    assert!(
        !ws.path().join("content/.cv").exists(),
        "notes never live in content/"
    );
}

#[test]
fn sidecar_survives_reparent() {
    let ws = tempfile::tempdir().unwrap();
    research::copy_corpus("promoted", ws.path());
    std::fs::remove_file(ws.path().join(format!("content/notes/{PAGE_ID}.md"))).unwrap();
    let (mut r, _) = open(ws.path());
    let id = r
        .capture(
            Capture::Idea { text: "x".into() },
            Some("Field notes: graph-first research"),
        )
        .unwrap();
    r.edit(&id, None, Some(MD_BODY)).unwrap();
    r.tag(&id, "research").unwrap();
    r.tag(&id, "graphs").unwrap();
    let note = r.notebook.note(&id).unwrap().clone();
    r.promote(&id, &promoted_spec(&note)).unwrap();
    drop(r);

    // The author moves + reparents the page (the captured `reparented` corpus).
    let from = ws.path().join(format!("content/notes/{PAGE_ID}.md"));
    let to = ws.path().join(format!("content/guides/{PAGE_ID}.md"));
    std::fs::remove_file(&from).unwrap();
    std::fs::write(
        &to,
        research::file(&format!(
            "corpus-src/reparented/content/guides/{PAGE_ID}.md"
        )),
    )
    .unwrap();

    let (r, _) = open(ws.path());
    let prov = r.notebook.provenance(&id).unwrap();
    assert_eq!(
        prov.path,
        format!("content/notes/{PAGE_ID}.md"),
        "historical"
    );
    let ir = PageIndex::from_graph_json(&research::file("captures/reparented/build-ir/graph.json"))
        .unwrap();
    let now = ir
        .get(&prov.page)
        .expect("sidecar page id resolves after reparent");
    assert_eq!(now.source_path, format!("guides/{PAGE_ID}.md"));
    assert_eq!(r.notebook.page_backlinks(&prov.page), vec![id.clone()]);
    assert!(matches!(
        r.notebook.note(&id).unwrap().triage,
        Triage::Inbox
    ));
}

trait Build {
    fn port_engine_build(&self) -> ExitClass;
}

impl Build for Research<DiskPort> {
    fn port_engine_build(&self) -> ExitClass {
        StubEngine::new("happy")
            .run(&Verb::Build, Box::new(|_| {}))
            .unwrap()
    }
}
