//! cv-app — composition root. Wires engine/store ports into the shell
//! (`cv_model::workbench`) and hands it to `cv_widgets::app::run`. No
//! rune import here; scenes are shared by the binary and golden tests.

pub mod editor;
pub mod live;
pub mod snapshot;
pub mod workspace;

use cv_core::engine::{Engine, Verb};
use cv_core::events::Envelope;
use cv_core::events::shell::DegradedReason;
use cv_fixtures::stub_engine::StubEngine;
use cv_fixtures::stub_store::StubStore;
use cv_model::module::{ProjectSnapshot, Registry};
use cv_model::theme::Mode;
use cv_model::workbench::Workbench;
use cv_widgets::app::{Launch, Scene};
use std::path::PathBuf;
use std::thread::JoinHandle;
use workspace::RecentList;

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
        Ok(_) => match engine.watch(cv_widgets::editor::host::tee_builds(wb.sink())) {
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
    editor::open_from_env(wb);
}

/// Where an opened workspace finds its engine and keeps its recent list.
#[derive(Debug, Clone, Default)]
pub struct WorkspaceEnv {
    /// `None`: the production chain (`CV_BORIS_BIN` → bundled → PATH →
    /// sibling) rooted at the workspace.
    pub locator: Option<cv_engine::locate::Locator>,
    pub recent_file: Option<PathBuf>,
}

impl WorkspaceEnv {
    pub fn native() -> Self {
        Self {
            locator: None,
            recent_file: workspace::config_dir().map(|d| d.join(workspace::RECENT_FILE)),
        }
    }
    pub fn recent(&self) -> RecentList {
        RecentList::load(self.recent_file.clone())
    }
}

/// Wall clock for research records (ms since the Unix epoch).
struct SystemClock;

impl cv_core::sys::Clock for SystemClock {
    fn now(&self) -> cv_core::sys::Timestamp {
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as i64);
        cv_core::sys::Timestamp(ms)
    }
}

/// Open `root` as the workspace: Research persists under `root/.cv/`,
/// module state resets, the recent list is updated, and the engine boots
/// on a worker thread (returned so tests can join it). Runs on a
/// boot/command path, never inside a frame.
pub fn open_workspace(
    wb: &mut Workbench,
    root: PathBuf,
    env: &WorkspaceEnv,
) -> Option<JoinHandle<()>> {
    let root = match workspace::normalize(&root) {
        Ok(r) if r.is_dir() => r,
        _ => {
            wb.status = format!("cannot open workspace {}: not a folder", root.display());
            return None;
        }
    };
    let slot = live::EngineSlot::default();
    let research = cv_notes::disk::open(
        &root,
        cv_core::id::AccountId("local".into()),
        Box::new(SystemClock),
    );
    let research_err = match research {
        Ok((r, _)) => {
            let port = r.port.with_writer(Box::new(live::SlotEngine(slot.clone())));
            let input = snapshot::content_input(&root);
            cv_model::modules::research::install_in(Box::new(port), r.notebook, &input);
            None
        }
        Err(e) => Some(e.to_string()),
    };
    wb.reset_project(Registry::with_all());
    wb.workspace = Some(root.clone());
    show(wb, "pages");
    let warning = research_err.map(|e| format!("research notes not persisted: {e}"));
    if let Some(w) = &warning {
        wb.disclose(DegradedReason::StoreCorrupt);
        wb.status = w.clone();
    } else {
        wb.status = format!("{} · locating Boris…", root.display());
    }
    let mut recent = env.recent();
    recent.push(&root);
    let _ = recent.save();
    wb.recent = recent.entries;

    let mut cfg = cv_engine::EngineConfig::new(&root);
    if let Some(l) = &env.locator {
        cfg.locator = l.clone();
    }
    // A bare corpus (no profile) must name its input to boris.
    let input = snapshot::content_input(&root);
    if !root.join("boris.json").is_file() && input != "." {
        cfg.watch_args = vec!["--input".into(), input];
    }
    Some(live::spawn(live::EngineBoot {
        cfg,
        slot,
        inbox: wb.inbox(),
        warning,
    }))
}

/// Native boot: `--project` → `CV_PROJECT` → most recent workspace. With
/// none, the fixture demo stays up and the status line says how to open
/// one. A `--scene` that loaded its own project is left alone.
pub fn native_boot(wb: &mut Workbench) {
    if wb.app.snapshot_epoch == 0 {
        let env = WorkspaceEnv::native();
        let recent = env.recent();
        wb.recent = recent.entries.clone();
        let pick = workspace::resolve(
            workspace::project_arg(std::env::args().skip(1)),
            workspace::project_env(),
            &recent,
        );
        match pick {
            Some(root) => {
                open_workspace(wb, root, &env);
            }
            None => {
                let _ = open_corpus(wb, CANARY_CORPUS);
                wb.status = "No workspace open — showing the fixture demo. \
                             Open workspace… (mod+shift+O) or launch with --project <dir>."
                    .into();
            }
        }
    }
    editor::open_from_env(wb);
}

fn open_native(wb: &mut Workbench, root: PathBuf) {
    open_workspace(wb, root, &WorkspaceEnv::native());
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
        boot: native_boot,
        open: open_native,
        scenes: SCENES,
    }
}
