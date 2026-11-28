use super::*;
use crate::module::Registry;
use crate::workbench::Workbench;
use cv_core::engine::Engine;
use cv_fixtures::stub_engine::StubEngine;
use std::rc::Rc;

/// Records every started run; the test decides when Boris answers.
#[derive(Clone, Default)]
struct Held(Rc<RefCell<Vec<VerbRun>>>);

impl EnginePort for Held {
    fn unavailable(&self) -> Option<String> {
        None
    }
    fn start(&mut self, run: VerbRun) -> Result<(), String> {
        self.0.borrow_mut().push(run);
        Ok(())
    }
}

/// The happy corpus's pages, plus one Boris has never heard of.
fn snapshot() -> ProjectSnapshot {
    let page = |path: &str, id: &str, parent: Option<&str>| crate::module::PageRecord {
        path: path.into(),
        id: id.into(),
        title: id.into(),
        parent: parent.map(str::to_string),
        ..Default::default()
    };
    ProjectSnapshot {
        epoch: 1,
        pages: vec![
            page("index.md", "index", None),
            page(
                "guides/getting-started.md",
                "guides/getting-started",
                Some("index"),
            ),
            page("guides/publishing.md", "guides/publishing", Some("index")),
            page("missing.md", "missing", None),
        ],
        ..Default::default()
    }
}

fn wb(_corpus: &str) -> (Workbench, Held) {
    let held = Held::default();
    install(Box::new(held.clone()));
    let mut wb = Workbench::new(Registry::with_all());
    wb.load_snapshot(&snapshot());
    (wb, held)
}

fn select_page(wb: &mut Workbench, path: &str) {
    let pages = wb.registry.index_of(crate::modules::pages::ID).unwrap();
    wb.activate(pages);
    wb.set_nav("all");
    wb.select_key(path);
    assert_eq!(wb.selected_key(), Some(path));
}

/// Answer the oldest held run from the corpus's captured verb results.
fn answer(wb: &mut Workbench, held: &Held, corpus: &str) -> VerbRun {
    let run = held.0.borrow_mut().remove(0);
    let _ = StubEngine::new(corpus).run_reported(&run.verb, wb.sink(), &run.cancel);
    wb.drain();
    run
}

fn activity_titles(wb: &Workbench) -> Vec<String> {
    let m = wb
        .registry
        .get(wb.registry.index_of(crate::modules::activity::ID).unwrap())
        .unwrap();
    (0..m.list_len(None))
        .map(|i| m.row(None, i).title)
        .collect()
}

#[test]
fn without_an_engine_every_verb_is_listed_disabled_with_the_reason() {
    install(Box::new(Unattached));
    let mut wb = Workbench::new(Registry::with_all());
    let cmds: Vec<_> = wb
        .commands()
        .into_iter()
        .filter(|c| c.id.starts_with("engine."))
        .collect();
    assert_eq!(cmds.len(), verbs().len());
    assert!(
        cmds.iter()
            .all(|c| c.disabled.as_deref() == Some(NO_ENGINE))
    );
    wb.run_command("engine.build");
    assert!(!wb.dialogs.is_open());
    assert!(wb.status.contains(NO_ENGINE), "{}", wb.status);
}

#[test]
fn build_holds_a_progress_dialog_until_its_run_lands_in_activity() {
    let (mut wb, held) = wb("happy");
    wb.run_command("engine.build");
    assert_eq!(held.0.borrow()[0].verb, Verb::Build);
    assert_eq!(wb.dialogs.open().map(|d| d.spec.id.as_str()), Some(RUNNING));
    assert_eq!(wb.status, "Building the site…");

    answer(&mut wb, &held, "happy");
    assert!(!wb.dialogs.is_open());
    assert_eq!(wb.status, "Site build finished");
    assert!(
        !wb.status.contains("exit 0"),
        "machinery stays out of the outcome"
    );
    assert!(activity_titles(&wb).contains(&"build finished".to_string()));

    let activity = wb
        .registry
        .get(wb.registry.index_of(crate::modules::activity::ID).unwrap())
        .unwrap();
    let row = (0..activity.list_len(None))
        .map(|i| activity.row(None, i))
        .find(|r| r.title == "build finished")
        .expect("the verb run is an Activity entry");
    let reading = activity.reading(&row.key);
    let line = |label: &str| {
        reading
            .lines
            .iter()
            .find(|l| l.label.as_deref() == Some(label))
            .unwrap_or_else(|| panic!("no {label} line in {reading:?}"))
            .text
            .as_str()
    };
    assert_eq!(line("command"), "boris build");
    assert!(line("exit").starts_with("exit 0"), "{}", line("exit"));
    assert!(line("duration").ends_with(" ms"), "{}", line("duration"));
}

#[test]
fn a_second_long_verb_waits_for_the_first() {
    let (mut wb, held) = wb("happy");
    wb.run_command("engine.build");
    wb.run_command("engine.export.rag");
    assert_eq!(held.0.borrow().len(), 1);
    assert!(wb.status.contains("still running"), "{}", wb.status);
}

#[test]
fn cancelling_progress_cancels_the_run_token() {
    let (mut wb, held) = wb("happy");
    wb.run_command("engine.export.llms");
    wb.dialog_cancel();
    let run = held.0.borrow()[0].clone();
    assert!(run.cancel.is_cancelled());
    assert_eq!(wb.status, "Cancelling llms.txt export…");
}

#[test]
fn short_verbs_report_on_the_status_line_without_a_dialog() {
    let (mut wb, held) = wb("happy");
    wb.run_command("engine.validate");
    assert!(!wb.dialogs.is_open());
    answer(&mut wb, &held, "happy");
    assert_eq!(wb.status, "Validation passed");
}

#[test]
fn a_usage_error_points_at_activity_errors() {
    let (mut wb, held) = wb("happy");
    wb.run_command("engine.export.rss");
    answer(&mut wb, &held, "happy");
    assert!(!wb.dialogs.is_open());
    assert_eq!(wb.status, "RSS feed export failed · see Activity");
}

#[test]
fn check_findings_land_in_audit_as_engine_documented_advisories() {
    let (mut wb, held) = wb("happy");
    wb.run_command("engine.check");
    answer(&mut wb, &held, "happy");
    let audit = wb
        .registry
        .get(wb.registry.index_of(crate::modules::audit::ID).unwrap())
        .unwrap();
    let rows: Vec<_> = (0..audit.list_len(Some("open")))
        .map(|i| audit.row(Some("open"), i))
        .collect();
    let titles: Vec<_> = rows.iter().map(|r| r.title.as_str()).collect();
    assert!(titles.contains(&"unreferenced_page · index"), "{titles:?}");
    let key = &rows
        .iter()
        .find(|r| r.title == "unreferenced_page · index")
        .unwrap()
        .key;
    let reading = audit.reading(key);
    let authority = reading
        .lines
        .iter()
        .find(|l| l.label.as_deref() == Some("authority"))
        .unwrap();
    assert_eq!(authority.text, "engine documented");
    assert!(audit.list_len(Some("blocking")) == 0);
}

#[test]
fn impact_needs_a_selected_page() {
    let (mut wb, held) = wb("happy");
    wb.run_command("engine.impact");
    assert!(held.0.borrow().is_empty());
    assert!(
        wb.status.starts_with("Select a page in Pages"),
        "{}",
        wb.status
    );
}

#[test]
fn impact_of_the_selected_page_opens_its_transitive_set() {
    let (mut wb, held) = wb("happy");
    select_page(&mut wb, "guides/getting-started.md");
    wb.run_command("engine.impact");
    assert_eq!(
        held.0.borrow()[0].verb,
        Verb::Impact {
            id: "guides/getting-started".into()
        }
    );
    answer(&mut wb, &held, "happy");
    assert_eq!(wb.registry.get(wb.active).unwrap().spec().id, ID);
    let key = wb.selected_key().unwrap().to_string();
    let reading = wb.registry.get(wb.active).unwrap().reading(&key);
    assert_eq!(reading.title, "Impact of guides/getting-started");
    let impacted: Vec<_> = reading
        .lines
        .iter()
        .filter(|l| l.label.as_deref() == Some("page") && l.text != "guides/getting-started")
        .map(|l| l.text.as_str())
        .collect();
    assert_eq!(impacted, ["guides/publishing", "index"]);
}

#[test]
fn an_unknown_impact_target_is_an_error_run_not_a_reading() {
    let (mut wb, held) = wb("happy");
    select_page(&mut wb, "missing.md");
    wb.run_command("engine.impact");
    answer(&mut wb, &held, "happy");
    assert_eq!(wb.status, "Impact review failed · see Activity");
    let engine = wb.registry.index_of(ID).unwrap();
    assert_eq!(wb.registry.get(engine).unwrap().list_len(None), 0);
}
