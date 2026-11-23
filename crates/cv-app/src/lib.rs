//! cv-app — composition root. Wires engine/store ports into the shell
//! (`cv_model::workbench`) and hands it to `cv_widgets::app::run`. No
//! rune import here; scenes are shared by the binary and golden tests.

pub mod snapshot;

use cv_core::engine::{Engine, Verb};
use cv_core::events::Envelope;
use cv_core::events::shell::DegradedReason;
use cv_fixtures::stub_engine::StubEngine;
use cv_fixtures::stub_store::StubStore;
use cv_model::module::{ProjectSnapshot, Registry};
use cv_model::theme::Mode;
use cv_model::workbench::Workbench;
use cv_widgets::app::{Launch, Scene};

mod research;

/// Corpus the canary replays (`fixtures/captures/<corpus>/watch/session.ndjson`).
pub const CANARY_CORPUS: &str = "happy";

pub fn init() -> Workbench {
    Workbench::new(Registry::with_all()).with_store(Box::new(StubStore::new()))
}

/// Locate the engine, start its watch session into the shell's sink.
/// Engine-absent is disclosed, never fatal.
pub fn attach_engine(wb: &mut Workbench, engine: &dyn Engine) {
    match engine.locate() {
        Ok(_) => match engine.watch(wb.sink()) {
            Ok(h) => wb.attach_watch(h),
            Err(_) => wb.disclose(DegradedReason::WatchDead),
        },
        Err(_) => wb.disclose(DegradedReason::EngineAbsent),
    }
    wb.drain();
}

/// Every captured broken corpus; each gets a Pages golden.
pub const BROKEN: &[&str] = &[
    "broken-duplicate-id",
    "broken-frontmatter",
    "broken-parent",
    "broken-textile",
    "broken-wikilink",
];

/// Native boot runs after a `--scene` has set up its own project; it
/// only opens the canary corpus when nothing is loaded yet.
pub fn boot(wb: &mut Workbench) {
    if wb.app.snapshot_epoch == 0 {
        let _ = open_corpus(wb, CANARY_CORPUS);
    }
}

/// Load the corpus' page/output snapshot, then replay its watch session.
pub fn open_corpus(wb: &mut Workbench, corpus: &str) -> StubEngine {
    wb.load_snapshot(&snapshot::corpus_snapshot(corpus));
    let engine = StubEngine::new(corpus);
    attach_engine(wb, &engine);
    engine
}

/// Broken corpora's watch sessions fail before emitting diagnostics; the
/// one-shot build's stderr carries them (as `ProcessEngine` does).
pub fn open_broken(wb: &mut Workbench, corpus: &str) {
    let engine = open_corpus(wb, corpus);
    // Recover stderr diagnostics only when the watch session failed
    // without any: the one-shot build must not replace located ones.
    if !has_watch_diagnostics(wb) {
        let _ = engine.run(&Verb::Build, wb.sink());
        wb.drain();
    }
}

fn has_watch_diagnostics(wb: &Workbench) -> bool {
    wb.registry
        .index_of("activity")
        .and_then(|i| wb.registry.get(i))
        .is_some_and(|m| m.list_len(Some("diagnostics")) > 0)
}

fn show(wb: &mut Workbench, module: &str) {
    let i = wb.registry.index_of(module).expect("module registered");
    wb.activate(i);
}

/// A live session mid-watch: every captured envelope but the last, so
/// the session is open and the rebuild has not landed yet.
pub fn mid_watch(wb: &mut Workbench, corpus: &str) {
    wb.load_snapshot(&snapshot::corpus_snapshot(corpus));
    let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Envelope>::new()));
    let c = captured.clone();
    let _h = StubEngine::new(corpus).replay_watch(Box::new(move |e| c.lock().unwrap().push(e)));
    let envs = std::mem::take(&mut *captured.lock().unwrap());
    let mut sink = wb.sink();
    for e in envs.into_iter().rev().skip(1).rev() {
        sink(e);
    }
    wb.drain();
}

fn demo(wb: &mut Workbench) {
    boot(wb);
    show(wb, "demo");
}

fn canary(wb: &mut Workbench) {
    demo(wb);
    wb.select_row(1);
}

fn pages_5k(wb: &mut Workbench) {
    demo(wb);
    wb.set_nav("pages");
    wb.select_row(2);
}

fn palette(wb: &mut Workbench) {
    demo(wb);
    wb.palette.toggle();
    "degrade".chars().for_each(|c| wb.palette.type_char(c));
}

fn light(wb: &mut Workbench) {
    canary(wb);
    wb.set_mode(Mode::Light);
    wb.set_reduced_motion(true);
}

fn degraded(cmd: &str, wb: &mut Workbench) {
    demo(wb);
    wb.run_command(cmd);
}

fn pages(wb: &mut Workbench) {
    boot(wb);
    show(wb, "pages");
    wb.select_key("guides/publishing.md");
}

fn outputs(wb: &mut Workbench) {
    boot(wb);
    show(wb, "outputs");
    wb.select_key("index.html");
}

fn activity(wb: &mut Workbench) {
    boot(wb);
    show(wb, "activity");
    wb.select_row(0);
}

fn broken(corpus: &str, wb: &mut Workbench) {
    open_broken(wb, corpus);
    show(wb, "pages");
    wb.set_nav("issues");
    wb.select_row(0);
}

fn activity_broken(wb: &mut Workbench) {
    open_broken(wb, "broken-parent");
    show(wb, "activity");
    wb.set_nav("file:orphan.md");
    wb.select_row(0);
}

fn empty(wb: &mut Workbench) {
    wb.load_snapshot(&ProjectSnapshot {
        epoch: 1,
        ..Default::default()
    });
    show(wb, "pages");
}

fn mid(wb: &mut Workbench) {
    mid_watch(wb, "happy");
    show(wb, "activity");
    wb.select_row(0);
}

fn pages_degraded(reason: DegradedReason, wb: &mut Workbench) {
    mid_watch(wb, "happy");
    wb.disclose(reason);
    show(wb, "pages");
    wb.select_key("index.md");
}

/// Named scenes: `cv-app --scene <name> --tree t.json --snap s.png`.
pub const SCENES: &[Scene] = &[
    ("canary", canary),
    ("pages-5k", pages_5k),
    ("palette", palette),
    ("light", light),
    ("engine-absent", |wb| {
        degraded("demo.degrade.engine-absent", wb)
    }),
    ("version-mismatch", |wb| {
        degraded("demo.degrade.version-mismatch", wb)
    }),
    ("watch-dead", |wb| degraded("demo.degrade.watch-dead", wb)),
    ("store-corrupt", |wb| {
        degraded("demo.degrade.store-corrupt", wb)
    }),
    ("pages", pages),
    ("outputs", outputs),
    ("activity", activity),
    ("broken-duplicate-id", |wb| {
        broken("broken-duplicate-id", wb)
    }),
    ("broken-frontmatter", |wb| broken("broken-frontmatter", wb)),
    ("broken-parent", |wb| broken("broken-parent", wb)),
    ("broken-textile", |wb| broken("broken-textile", wb)),
    ("broken-wikilink", |wb| broken("broken-wikilink", wb)),
    ("dogfood", |wb| broken("dogfood", wb)),
    ("activity-broken", activity_broken),
    ("empty", empty),
    ("mid-watch", mid),
    ("pages-engine-absent", |wb| {
        pages_degraded(DegradedReason::EngineAbsent, wb)
    }),
    ("pages-version-mismatch", |wb| {
        pages_degraded(
            DegradedReason::EngineVersionMismatch {
                found: "0.8.2".into(),
                want: "0.9.0".into(),
            },
            wb,
        )
    }),
    ("pages-watch-dead", |wb| {
        pages_degraded(DegradedReason::WatchDead, wb)
    }),
    ("pages-store-corrupt", |wb| {
        pages_degraded(DegradedReason::StoreCorrupt, wb)
    }),
    ("research-empty", research::empty),
    ("research-promote", research::promote),
    ("research-degraded", research::degraded),
];

pub fn launch() -> Launch {
    Launch {
        title: "Capture Vector",
        init,
        boot,
        scenes: SCENES,
    }
}
