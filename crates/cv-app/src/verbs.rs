//! Palette Boris verbs in the fixture demo: the same engine module, its
//! runs answered from the captured `fixtures/captures/verbs/` results by
//! `StubEngine` instead of a scheduler. Scenes for their Activity rows.

use cv_core::engine::Engine;
use cv_fixtures::stub_engine::{StubEngine, verb_capture_key};
use cv_model::modules::engine::{EnginePort, VerbRun};
use cv_model::workbench::{Inbox, Workbench};

pub struct FixtureVerbs {
    engine: StubEngine,
    inbox: Inbox,
}

impl FixtureVerbs {
    pub fn new(engine: StubEngine, inbox: Inbox) -> Self {
        Self { engine, inbox }
    }

    fn captured(&self, key: &str) -> bool {
        !cv_fixtures::corpus(cv_fixtures::VERBS)
            .pinned_under(&format!("{}/{key}", self.engine.corpus))
            .is_empty()
    }
}

impl EnginePort for FixtureVerbs {
    fn unavailable(&self) -> Option<String> {
        (!self.captured("")).then(|| {
            format!(
                "the fixture corpus {} has no captured verb runs",
                self.engine.corpus
            )
        })
    }

    fn start(&mut self, run: VerbRun) -> Result<(), String> {
        let key = verb_capture_key(&run.verb).filter(|k| self.captured(&format!("{k}/")));
        if key.is_none() {
            return Err(format!(
                "the fixture corpus {} has no capture of boris {}",
                self.engine.corpus,
                run.verb.name()
            ));
        }
        self.engine
            .run_reported(&run.verb, self.inbox.sink(), &run.cancel)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

pub mod scene {
    use super::*;

    fn run(wb: &mut Workbench, corpus: &str, command: &str) {
        crate::open_corpus(wb, corpus);
        wb.run_command(command);
        wb.drain();
    }

    /// Activity with the newest run of `verb` selected.
    fn activity_run(wb: &mut Workbench, title: &str) {
        crate::show(wb, "activity");
        let key = (0..wb.list_len())
            .map(|i| wb.row(i))
            .find(|r| r.title == title)
            .map(|r| r.key);
        if let Some(key) = key {
            wb.select_key(&key);
        }
    }

    /// `boris build` from the palette, exit 0.
    pub fn build(wb: &mut Workbench) {
        run(wb, "happy", "engine.build");
        activity_run(wb, "build finished");
    }

    /// Exit 1: the Textile corpus built without `--textile` (ETEXTILE).
    pub fn content_failure(wb: &mut Workbench) {
        run(wb, "happy-textile", "engine.build");
        activity_run(wb, "build failed (exit 1)");
    }

    /// Exit 2: an RSS export without the site URL Boris requires.
    pub fn usage_error(wb: &mut Workbench) {
        run(wb, "happy", "engine.export.rss");
        activity_run(wb, "export:rss failed (exit 2)");
    }

    /// Impact of the selected page, opened as a reading view.
    pub fn impact(wb: &mut Workbench) {
        crate::open_corpus(wb, "happy");
        crate::show(wb, "pages");
        wb.select_key("guides/getting-started.md");
        wb.run_command("engine.impact");
        wb.drain();
    }

    /// `boris check` findings in Audit under their own pack.
    pub fn check(wb: &mut Workbench) {
        run(wb, "happy", "engine.check");
        crate::show(wb, "audit");
        wb.set_nav("pack:boris-check");
        wb.select_row(0);
    }
}
