//! cv-app — composition root. Wires engine/store ports into the shell
//! (`cv_model::workbench`) and hands it to `cv_widgets::app::run`. No
//! rune import here; scenes are shared by the binary and golden tests.

use cv_core::engine::Engine;
use cv_core::events::shell::DegradedReason;
use cv_fixtures::stub_engine::StubEngine;
use cv_fixtures::stub_store::StubStore;
use cv_model::module::Registry;
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

pub fn boot(wb: &mut Workbench) {
    attach_engine(wb, &StubEngine::new(CANARY_CORPUS));
    // Rail order puts Research ahead of the wave-0 canary; keep landing on it.
    wb.run_command("shell.goto.demo");
}

fn canary(wb: &mut Workbench) {
    boot(wb);
    wb.select_row(1);
}

fn pages_5k(wb: &mut Workbench) {
    boot(wb);
    wb.set_nav("pages");
    wb.select_row(2);
}

fn palette(wb: &mut Workbench) {
    boot(wb);
    wb.palette.toggle();
    "degrade".chars().for_each(|c| wb.palette.type_char(c));
}

fn light(wb: &mut Workbench) {
    canary(wb);
    wb.set_mode(Mode::Light);
    wb.set_reduced_motion(true);
}

fn degraded(cmd: &str, wb: &mut Workbench) {
    boot(wb);
    wb.run_command(cmd);
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
