//! Research (L6) scenes: empty inbox, promote flow, promote without a
//! content writer (degraded).

use cv_core::engine::{Fingerprint, PageDraft, WriteOutcome};
use cv_core::sys::{Hash, Timestamp};
use cv_model::modules::research;
use cv_model::workbench::Workbench;
use cv_notes::{Capture, MemoryPort, Notebook, Provenance, Record, Research, ResearchPort, Target};

/// Scene-only writer: accepts drafts with a fixed fingerprint so the
/// golden is deterministic. Real promotion goes through the engine's
/// staged write (`cv_notes::disk::DiskPort::with_writer`).
struct SceneWriter(MemoryPort);

impl ResearchPort for SceneWriter {
    fn now(&self) -> Timestamp {
        self.0.now()
    }
    fn commit(&mut self, rec: &Record) -> Result<(), String> {
        self.0.commit(rec)
    }
    fn commit_provenance(&mut self, p: &Provenance) -> Result<(), String> {
        self.0.commit_provenance(p)
    }
    fn write_page(&mut self, _: &PageDraft) -> Option<Result<WriteOutcome, String>> {
        Some(Ok(WriteOutcome::Written {
            fingerprint: Fingerprint {
                sha256: Hash([0; 32]),
                mtime: Timestamp(0),
            },
        }))
    }
    fn describe(&self) -> String {
        "scene writer".into()
    }
}

fn seeded() -> (MemoryPort, Notebook) {
    let mut r = Research::new(Notebook::default(), MemoryPort::default());
    let note = r
        .capture(
            Capture::Url {
                url: "https://example.org/graphs".into(),
                title: Some("Graphs".into()),
            },
            Some("Field notes: graph-first research"),
        )
        .expect("seed");
    r.edit(
        &note,
        None,
        Some("Parents shape the nav forest.\n\nNotes stay in .cv/."),
    )
    .expect("seed");
    r.tag(&note, "research").expect("seed");
    let idea = r
        .capture(
            Capture::Idea {
                text: "Backlinks resolve at read time".into(),
            },
            None,
        )
        .expect("seed");
    r.link(&idea, Target::Note(note.clone())).expect("seed");
    r.capture(
        Capture::Selection {
            text: "Provenance never goes in frontmatter".into(),
            source: Some("AGENTS.md".into()),
        },
        None,
    )
    .expect("seed");
    r.file(&idea, "graphs").expect("seed");
    (r.port, r.notebook)
}

fn open(wb: &mut Workbench, port: Box<dyn ResearchPort>, nb: Notebook) {
    research::install(port, nb);
    *wb = crate::init();
    crate::boot(wb);
    wb.run_command("shell.goto.research");
}

pub fn empty(wb: &mut Workbench) {
    open(wb, Box::new(MemoryPort::default()), Notebook::default());
}

fn promote_with(wb: &mut Workbench, port: impl FnOnce(MemoryPort) -> Box<dyn ResearchPort>) {
    let (mem, nb) = seeded();
    open(wb, port(mem), nb);
    wb.set_nav("all");
    wb.select_key("note-0001");
    wb.run_command("research.promote");
}

/// The seeded notebook with the first note selected, nothing run yet.
pub fn notes(wb: &mut Workbench) {
    let (mem, nb) = seeded();
    open(wb, Box::new(SceneWriter(mem)), nb);
    wb.set_nav("all");
    wb.select_key("note-0001");
}

pub fn promote(wb: &mut Workbench) {
    promote_with(wb, |m| Box::new(SceneWriter(m)));
}

pub fn degraded(wb: &mut Workbench) {
    promote_with(wb, |m| Box::new(m));
}
