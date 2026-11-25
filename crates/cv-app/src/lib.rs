//! cv-app — composition root. Wires engine/store ports into the shell
//! (`cv_model::workbench`) and hands it to `cv_widgets::app::run`. No
//! rune import here; scenes are shared by the binary and golden tests.

pub mod audit;
pub mod build_info;
pub mod dialogs;
pub mod doctor;
pub mod editor;
mod file_identity;
pub mod live;
pub mod logging;
pub mod pages;
pub mod publish;
pub mod snapshot;
pub mod workspace;

use cv_core::engine::{Engine, Verb};
use cv_core::events::Envelope;
use cv_core::events::shell::DegradedReason;
use cv_fixtures::stub_engine::StubEngine;
use cv_fixtures::stub_store::StubStore;
use cv_model::module::{ProjectSnapshot, Registry};
use cv_model::modules::doctor::{EngineState, LockHolder};
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
    init_with(Box::new(publish::DiskPort::ephemeral()))
}

fn init_with(publish: Box<dyn cv_model::modules::publish::PublishPort>) -> Workbench {
    cv_model::modules::audit::install(Box::new(audit::DiskPort::ephemeral()));
    cv_model::modules::publish::install(publish);
    doctor::install(None);
    Workbench::new(Registry::with_all()).with_store(Box::new(StubStore::new()))
}

/// The publish ledger is shown read-only; one that cannot be trusted is
/// disclosed like any other corrupt sidecar store.
fn publish_warning(port: &publish::DiskPort) -> Option<String> {
    port.error()
        .map(|e| format!("publish ledger unreadable: {e}"))
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
/// on a worker thread (returned so tests can join it; `None` when the
/// root does not open or the thread cannot start). Runs on a
/// boot/command path, never inside a frame.
pub fn open_workspace(
    wb: &mut Workbench,
    root: PathBuf,
    env: &WorkspaceEnv,
) -> Option<JoinHandle<()>> {
    let rebuild_indexes = wb.take_index_rebuild_request();
    let root = match workspace::normalize(&root) {
        Ok(r) if r.is_dir() => {
            logging::info(
                "workspace_open",
                &[("root", serde_json::json!(r.display().to_string()))],
            );
            r
        }
        _ => {
            logging::warn(
                "workspace_rejected",
                &[
                    ("root", serde_json::json!(root.display().to_string())),
                    ("reason", serde_json::json!("not a folder")),
                ],
            );
            wb.status = format!("cannot open workspace {}: not a folder", root.display());
            return None;
        }
    };
    // Editor buffers belong to the workspace they were opened from; their
    // saves and Problems rails must not follow a switch.
    let editors = wb
        .windows
        .companions
        .iter()
        .filter(|c| c.module == cv_widgets::editor::MODULE)
        .count();
    if editors > 0 {
        logging::warn(
            "workspace_switch_blocked",
            &[("editors", serde_json::json!(editors))],
        );
        wb.status = format!(
            "close {editors} editor window(s) before opening {}",
            root.display()
        );
        return None;
    }
    wb.stop_watch();
    wb.registry = Registry::default();
    let slot = live::EngineSlot::default();
    let input = snapshot::content_input(&root);
    cv_model::modules::pages::install_in(
        Box::new(pages::WorkspacePages::new(&root, slot.clone())),
        &input,
    );
    let research = if rebuild_indexes {
        cv_notes::disk::open_rebuilding_index(
            &root,
            cv_core::id::AccountId("local".into()),
            Box::new(SystemClock),
        )
    } else {
        cv_notes::disk::open(
            &root,
            cv_core::id::AccountId("local".into()),
            Box::new(SystemClock),
        )
    };
    let mut opened = doctor::StoreOpen::default();
    let mut research_opened = false;
    let research_err = match research {
        Ok((r, report)) => {
            research_opened = true;
            opened.research.lock = Some(LockHolder::ThisWindow);
            // Promotion writes under the input; one that resolves outside
            // the workspace (a symlink) gets no writer, so promotion
            // refuses instead of writing elsewhere.
            let inside =
                workspace::normalize(&root.join(&input)).is_ok_and(|p| p.starts_with(&root));
            let port = if inside {
                r.port.with_writer(Box::new(live::SlotEngine(slot.clone())))
            } else {
                r.port
            };
            cv_model::modules::research::install_in(Box::new(port), r.notebook, &input);
            // Undecodable journal lines stay on disk but load as nothing;
            // disclose that instead of opening cleanly.
            let bad = report.notes.undecodable + report.provenance.undecodable;
            (bad > 0).then(|| format!("research journal skipped {bad} undecodable line(s)"))
        }
        Err(e) => {
            if matches!(e, cv_notes::disk::DiskError::Locked(_)) {
                opened.research.lock = Some(LockHolder::OtherProcess);
            }
            opened.research.error = Some(e.to_string());
            Some(format!("research notes not persisted: {e}"))
        }
    };
    let audit_err = match audit::DiskPort::open(&root) {
        Ok(p) => {
            cv_model::modules::audit::install(Box::new(p));
            opened.audit.lock = Some(LockHolder::ThisWindow);
            None
        }
        Err(e) => {
            if matches!(e, cv_store::audit::Error::Locked(_)) {
                opened.audit.lock = Some(LockHolder::OtherProcess);
            }
            cv_model::modules::audit::install(Box::new(audit::DiskPort::ephemeral()));
            opened.audit.error = Some(e.to_string());
            Some(e.to_string())
        }
    };
    let mut rebuild_errors = Vec::new();
    if rebuild_indexes {
        if !research_opened {
            rebuild_errors.push(format!(
                "research: {}",
                opened.research.error.as_deref().unwrap_or("open failed")
            ));
        }
        match cv_ledger::LedgerIndex::rebuild_from_journal(&root) {
            Ok(_) => {}
            Err(e) => {
                rebuild_errors.push(format!("ledger: {e}"));
            }
        }
    }
    let publish = publish::DiskPort::open(&root);
    let publish_err = publish_warning(&publish);
    opened.ledger.error = publish.error().map(str::to_string);
    cv_model::modules::publish::install(Box::new(publish));
    let feed = doctor::install(Some(&root));
    feed.update(|f| f.store = doctor::store(&root, &opened));
    wb.reset_project(workspace_registry());
    wb.workspace = Some(root.clone());
    show(wb, "pages");
    let warning = research_err
        .or_else(|| audit_err.map(|e| format!("finding snoozes not persisted: {e}")))
        .or(publish_err);
    if let Some(w) = &warning {
        wb.disclose(DegradedReason::StoreCorrupt);
        wb.status = w.clone();
    } else {
        wb.status = format!("{} · locating Boris…", root.display());
    }
    let mut recent = env.recent();
    recent.push(&root);
    if let Err(e) = recent.save() {
        logging::warn(
            "recent_save_failed",
            &[("error", serde_json::json!(e.to_string()))],
        );
    }
    wb.recent = recent.entries;

    let notice = rebuild_indexes.then(|| {
        if rebuild_errors.is_empty() {
            "indexes rebuilt from .cv JSONL (research, ledger)".to_string()
        } else {
            format!("index rebuild failed: {}", rebuild_errors.join("; "))
        }
    });
    let mut cfg = cv_engine::EngineConfig::new(&root);
    cfg.content_input = input.clone().into();
    if let Some(l) = &env.locator {
        cfg.locator = l.clone();
    }
    feed.update(|f| {
        f.engine = EngineState::Locating;
        f.candidates = doctor::candidates(&cfg.locator);
    });
    // A bare corpus (no profile) must name its input to boris.
    if !root.join("boris.json").is_file() && input != "." {
        cfg.watch_args = vec!["--input".into(), input];
    }
    live::spawn(live::EngineBoot {
        cfg,
        slot,
        inbox: wb.inbox(),
        warning,
        notice,
        doctor: feed,
    })
}

/// Native boot: `--project` → `CV_PROJECT` → most recent workspace. With
/// none, see [`boot_without_workspace`]. A `--scene` that loaded its own project is left alone.
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
                logging::info("workspace_none", &[]);
                boot_without_workspace(wb, demo_available());
            }
        }
    }
    editor::open_from_env(wb);
}

/// The fixture demo reads the build checkout's `fixtures/`; an installed
/// package has none.
pub fn demo_available() -> bool {
    cv_fixtures::fixtures_root().join("MANIFEST.toml").is_file()
}

/// No workspace picked: show the fixture demo when the checkout is
/// present, otherwise an empty workbench. Either way, say how to open one.
pub fn boot_without_workspace(wb: &mut Workbench, demo: bool) {
    let how = "Open workspace… (mod+shift+O) or launch with --project <dir>.";
    wb.status = if demo {
        let _ = open_corpus(wb, CANARY_CORPUS);
        format!("No workspace open — showing the fixture demo. {how}")
    } else {
        wb.reset_project(workspace_registry());
        format!("No workspace open. {how}")
    };
}

/// `CV_DEBUG=1` keeps the demo module in every workspace.
pub fn debug_enabled() -> bool {
    std::env::var_os("CV_DEBUG").is_some_and(|v| v == "1")
}

/// Modules for anything but the fixture demo. The demo's "Simulate: …"
/// verbs raise real-looking banners for fake failures, so a real
/// workspace only gets them under [`debug_enabled`]. Demo sorts last
/// (`Slot::Extra`), so core `mod+1…9` numbering is the same either way.
pub fn workspace_registry() -> Registry {
    let mut r = Registry::with_all();
    if !debug_enabled() {
        r.remove(cv_model::modules::demo::ID);
    }
    r
}

fn open_native(wb: &mut Workbench, root: PathBuf) {
    open_workspace(wb, root, &WorkspaceEnv::native());
}

/// Load the corpus' page/output snapshot, then replay its watch session.
pub fn open_corpus(wb: &mut Workbench, corpus: &str) -> StubEngine {
    wb.load_snapshot(&snapshot::corpus_snapshot(corpus));
    let engine = StubEngine::new(corpus);
    cv_model::modules::doctor::installed().update(|f| f.engine = doctor::fixture(corpus, &engine));
    attach_engine(wb, &engine);
    engine
}

/// Broken corpora's watch sessions fail before emitting diagnostics; the
/// one-shot build's stderr carries them (as `ProcessEngine` does).
pub fn open_broken(wb: &mut Workbench, corpus: &str) {
    open_broken_inner(wb, corpus, false);
}

/// The same broken capture with the fatal watch retired before the
/// diagnostic one-shot, matching native recovery's event order.
pub fn open_broken_recovered(wb: &mut Workbench, corpus: &str) {
    open_broken_inner(wb, corpus, true);
}

fn open_broken_inner(wb: &mut Workbench, corpus: &str, stop_watch: bool) {
    let engine = open_corpus(wb, corpus);
    // Recover stderr diagnostics only when the watch session failed
    // without any: the one-shot build must not replace located ones.
    if !has_watch_diagnostics(wb) {
        if stop_watch {
            wb.stop_watch();
            wb.drain();
        }
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

/// Signature effects on (issue #57): the canary with the opt-in flag set,
/// a live watch session and finished builds. Draws rune-fx layers only in
/// a build with the `effects` feature.
fn effects(wb: &mut Workbench) {
    wb.set_effects(true);
    canary(wb);
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

/// A folder with only `content/`: Boris's first watch build fails
/// non-recoverably (`ELAYOUT`, no theme) and the banner names that cause.
fn content_only(wb: &mut Workbench) {
    open_broken(wb, "content-only");
    show(wb, "pages");
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

/// Engine absent on a real machine: the boot status names how many
/// places were searched, and the toast stays compact and closable.
fn engine_absent_toast(wb: &mut Workbench) {
    pages_degraded(DegradedReason::EngineAbsent, wb);
    let searched = (0..24)
        .map(|i| PathBuf::from(format!("/opt/tool-{i}/bin/boris")))
        .collect();
    let e = cv_engine::error::EngineError::NotFound { searched };
    wb.status = format!("/workspace/happy: {e}");
}

fn audit(wb: &mut Workbench) {
    boot(wb);
    show(wb, "audit");
}

fn audit_empty(wb: &mut Workbench) {
    wb.load_snapshot(&ProjectSnapshot {
        epoch: 1,
        ..Default::default()
    });
    show(wb, "audit");
}

fn audit_detail(wb: &mut Workbench) {
    audit(wb);
    wb.select_row(0);
}

/// The build finished but its rendered pages are unreadable: their rules
/// disclose "not checked" instead of passing silently.
fn audit_degraded(wb: &mut Workbench) {
    let mut snap = snapshot::corpus_snapshot(CANARY_CORPUS);
    snap.artifact_root = None;
    wb.load_snapshot(&snap);
    attach_engine(wb, &StubEngine::new(CANARY_CORPUS));
    show(wb, "audit");
    wb.set_nav("limited");
    wb.select_row(0);
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
    ("engine-absent-toast", engine_absent_toast),
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
    ("audit-empty", audit_empty),
    ("audit", audit),
    ("audit-detail", audit_detail),
    ("audit-degraded", audit_degraded),
    ("publish", publish::scene::runs),
    ("publish-detail", publish::scene::detail),
    ("publish-unknown", publish::scene::unknown),
    ("publish-empty", publish::scene::empty),
    ("publish-degraded", publish::scene::degraded),
    ("dialog-confirm", dialogs::confirm),
    ("dialog-prompt", dialogs::prompt),
    ("dialog-form", dialogs::form),
    ("dialog-choose", dialogs::choose),
    ("dialog-secret", dialogs::secret),
    ("dialog-progress", dialogs::progress),
    ("effects", effects),
    ("content-only", content_only),
    ("doctor", doctor::scene::healthy),
    ("doctor-engine-absent", doctor::scene::engine_absent),
    ("doctor-version-mismatch", doctor::scene::version_mismatch),
    ("doctor-watch-dead", doctor::scene::watch_dead),
];

pub fn launch() -> Launch {
    Launch {
        title: "Capture Vector",
        init,
        boot: native_boot,
        open: open_native,
        edit: editor::edit_page,
        scenes: SCENES,
    }
}
