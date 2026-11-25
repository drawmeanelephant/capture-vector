//! Native boot: one primary window + companion windows (editor popouts
//! host). Headless runs (`--tree`, `--snap`, `--scene`) open no companions.

use crate::editor;
use crate::shell::view;
use cv_model::module::ReadingModel;
use cv_model::theme::Theme;
use cv_model::windows::WindowId;
use cv_model::workbench::{ShellRequest, Workbench};
use rune::{AppSpec, Cx, El, FileDialogOptions, Tx, Waker, WindowHandle, WindowSpec};
use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

pub type Scene = (&'static str, fn(&mut Workbench));

pub struct Launch {
    pub title: &'static str,
    pub init: fn() -> Workbench,
    /// Runs once after the first frame (`on_ready`), with the waker
    /// installed: open the workspace, start the engine watch, etc.
    pub boot: fn(&mut Workbench),
    /// Open a workspace root (folder dialog answer, recent entry).
    pub open: fn(&mut Workbench, PathBuf),
    /// Open a page (relative to the content input) in an editor window.
    pub edit: fn(&mut Workbench, &str),
    pub scenes: &'static [Scene],
}

static BOOT: OnceLock<fn(&mut Workbench)> = OnceLock::new();
static OPEN: OnceLock<fn(&mut Workbench, PathBuf)> = OnceLock::new();
static EDIT: OnceLock<fn(&mut Workbench, &str)> = OnceLock::new();
static WAKE: OnceLock<Waker> = OnceLock::new();
static BOOTED: AtomicBool = AtomicBool::new(false);

// All windows share one event-loop thread, so companion bookkeeping is
// thread-local: native handles by id, specs awaiting `init`, and ids whose
// native window closed (title bar) for the primary's `poll` to retire.
thread_local! {
    static OPENED: RefCell<BTreeMap<WindowId, WindowHandle>> = const { RefCell::new(BTreeMap::new()) };
    static PENDING: RefCell<VecDeque<Companion>> = const { RefCell::new(VecDeque::new()) };
    static CLOSED: RefCell<Vec<WindowId>> = const { RefCell::new(Vec::new()) };
}

/// A reading companion's own state: a pinned noun's reading model.
/// Editable pages get an editor companion instead (`editor::host`).
pub struct Companion {
    pub id: WindowId,
    pub reading: ReadingModel,
    pub theme: Theme,
}

fn companion_init() -> Companion {
    PENDING
        .with(|p| p.borrow_mut().pop_front())
        .unwrap_or(Companion {
            id: WindowId(u32::MAX),
            reading: ReadingModel::default(),
            theme: Theme::dark(),
        })
}

pub fn companion_view(s: &Companion, _cx: &mut Cx<Companion>) -> El {
    crate::shell::reading_pane(&s.theme, "companion", s.reading.clone()).w_full()
}

fn companion_closed(s: &mut Companion, _tx: &mut Tx) {
    CLOSED.with(|c| c.borrow_mut().push(s.id));
    if let Some(w) = WAKE.get() {
        w();
    }
}

/// Reconcile native companion windows with `Workbench::windows`: retire
/// records whose window the user closed, close windows whose record went,
/// open windows for new records.
fn sync_companions(s: &mut Workbench) -> bool {
    let mut closed: Vec<WindowId> = CLOSED.with(|c| std::mem::take(&mut *c.borrow_mut()));
    closed.extend(editor::host::take_closed());
    let mut changed = false;
    for id in closed {
        OPENED.with(|o| o.borrow_mut().remove(&id));
        changed |= s.windows.companions.iter().any(|c| c.id == id);
        s.windows.close(id, true);
    }
    // Still companions in the model: the loop below opens them afresh.
    for id in editor::host::take_reopen() {
        OPENED.with(|o| o.borrow_mut().remove(&id));
    }
    OPENED.with(|o| {
        o.borrow_mut().retain(|id, h| {
            let live = s.windows.companions.iter().any(|c| c.id == *id);
            if !live {
                h.close();
            }
            live
        })
    });
    for c in &s.windows.companions {
        if OPENED.with(|o| o.borrow().contains_key(&c.id)) {
            continue;
        }
        if c.module == editor::MODULE {
            if let Some(handle) = editor::host::open(c.id, &c.key, &s.theme) {
                OPENED.with(|o| o.borrow_mut().insert(c.id, handle));
            }
            continue;
        }
        let reading = s
            .registry
            .index_of(&c.module)
            .and_then(|i| s.registry.get(i))
            .map(|m| m.reading(&c.key))
            .unwrap_or_default();
        PENDING.with(|p| {
            p.borrow_mut().push_back(Companion {
                id: c.id,
                reading,
                theme: s.theme.clone(),
            })
        });
        let handle = rune::open_window(
            WindowSpec::new(
                format!("companion-{}", c.id.0),
                AppSpec::new(
                    "Capture Vector — companion",
                    (560.0, 640.0),
                    companion_init,
                    companion_view,
                ),
            )
            .on_close(companion_closed),
        );
        OPENED.with(|o| o.borrow_mut().insert(c.id, handle));
    }
    editor::host::ingest_builds();
    editor::host::sync_dirty(&mut s.windows);
    changed
}

fn setup(s: &mut Workbench, w: Waker) {
    if std::env::var_os("CV_EFFECTS").is_some_and(|v| v == "1") {
        s.set_effects(true);
    }
    let _ = WAKE.set(w.clone());
    editor::host::set_waker(w.clone());
    s.set_waker(w);
    // Headless exports have no `on_ready` unless `--realtime`; they render
    // the booted state, as before.
    let realtime = std::env::args().any(|a| a == "--realtime");
    if rune::is_headless() && !realtime && !BOOTED.swap(true, Ordering::Relaxed) {
        run_boot(s);
    }
}

fn run_boot(s: &mut Workbench) {
    if let Some(boot) = BOOT.get() {
        boot(s);
    }
}

/// Boot lands after the first frame, without motion, so launch shows the
/// window at once and the first second stays still. `on_ready` runs again
/// when the native window is recreated; boot does not.
fn boot(s: &mut Workbench, tx: &mut Tx) {
    tx.instant();
    run_boot(s);
}

fn open_workspace(s: &mut Workbench, root: PathBuf) {
    match OPEN.get() {
        Some(open) => open(s, root),
        None => s.status = "this build cannot open workspaces".into(),
    }
}

/// Carry out shell requests that need the window: the native folder
/// picker answers on a later frame through `open_workspace`.
fn serve_requests(s: &mut Workbench, tx: &mut Tx) -> bool {
    let reqs = std::mem::take(&mut s.requests);
    let changed = !reqs.is_empty();
    for r in reqs {
        match r {
            ShellRequest::PickWorkspace => {
                let options = FileDialogOptions {
                    title: "Open workspace".into(),
                    directory: s.workspace.clone(),
                    ..Default::default()
                };
                tx.open_folder(options, |s: &mut Workbench, answer, _| match answer {
                    Ok(Some(root)) => open_workspace(s, root),
                    Ok(None) => {}
                    Err(e) => {
                        // The hint first: the error text can be long (raw D-Bus).
                        let e: String = e.to_string().chars().take(60).collect();
                        s.status = format!(
                            "No folder picker — launch with --project <dir> or set CV_PROJECT ({e})"
                        )
                    }
                });
            }
            ShellRequest::OpenWorkspace(root) => open_workspace(s, root),
            ShellRequest::EditPage(page) => match EDIT.get() {
                Some(edit) => edit(s, &page),
                None => s.status = "this build cannot edit pages".into(),
            },
        }
    }
    changed
}

/// Window size the shell boots at — also the harness's frame size.
pub const SIZE: (f32, f32) = (1280.0, 800.0);

/// rune `on_key` plumbing: keymap lives in cv-model; reveal the newly
/// selected row. Shared by the real window and the (feature-gated)
/// headless harness — it is product input handling, not QA machinery.
pub(crate) fn input_on_key(s: &mut Workbench, ev: &rune::KeyEvent, tx: &mut Tx) -> bool {
    let Some(k) = crate::keys::map(ev) else {
        return false;
    };
    let before = s.selected_key().map(str::to_string);
    let used = s.key(&k);
    if let Some(key) = s.selected_key()
        && before.as_deref() != Some(key)
    {
        tx.scroll_into_view_in("list-pane", format!("row-{key}"));
    }
    used
}

/// rune `poll` plumbing: drain engine envelopes; true when visible
/// state changed.
pub(crate) fn input_poll(s: &mut Workbench, _t: f64, _tx: &mut Tx) -> bool {
    s.drain()
}

/// Palette commands and chords can queue requests; serve them now so a
/// dialog opens on this keypress's frame.
fn on_key(s: &mut Workbench, ev: &rune::KeyEvent, tx: &mut Tx) -> bool {
    let used = input_on_key(s, ev, tx);
    serve_requests(s, tx) || used
}

/// Quit or primary-window close: stop the engine watch so no `boris
/// watch` child outlives the app (it runs in its own process group).
fn shutdown(s: &mut Workbench, _: &mut Tx) {
    s.stop_watch();
}

fn poll(s: &mut Workbench, t: f64, tx: &mut Tx) -> bool {
    let changed = input_poll(s, t, tx);
    let served = serve_requests(s, tx);
    sync_companions(s) || changed || served
}

/// Boot the primary window (or a headless run, per rune's CLI flags).
pub fn run(l: Launch) {
    let _ = BOOT.set(l.boot);
    let _ = OPEN.set(l.open);
    let _ = EDIT.set(l.edit);
    // `rune::run` without the hooks: quitting must stop the engine watch.
    let app = rune::App::new(AppSpec {
        scenes: l.scenes,
        setup: Some(setup),
        poll: Some(poll),
        on_key: Some(on_key),
        min_size: Some((960.0, 560.0)),
        ..AppSpec::new(l.title, SIZE, l.init, view)
    });
    #[cfg(feature = "effects")]
    let app = app.compositor(crate::fx::gfx::Gfx::default());
    let handle = app.handle();
    app.on_quit(shutdown)
        .on_close(shutdown)
        .on_ready(move |_| {
            if !BOOTED.swap(true, Ordering::Relaxed) {
                handle.update(boot);
            }
        })
        .run();
}

#[cfg(test)]
mod tests {
    use super::*;
    use cv_model::module::ReadingLine;

    #[test]
    fn a_reading_companion_shows_its_whole_reading_model() {
        let s = Companion {
            id: WindowId(1),
            reading: ReadingModel {
                title: "Guide".into(),
                lines: vec![ReadingLine {
                    label: Some("tags".into()),
                    text: "guides".into(),
                    ..Default::default()
                }],
                breadcrumbs: vec!["Home".into(), "Guide".into()],
                markdown: Some("Body text of the guide.".into()),
            },
            theme: Theme::dark(),
        };
        let env = rune::Env {
            system_dark: true,
            scale: 1.0,
        };
        let mut rt = rune::Runtime::new(s, companion_view, (560.0, 640.0), env);
        rt.frame(0.0);
        for key in ["companion", "crumb-0", "line-0", "sketch"] {
            assert!(rt.find(key).is_some(), "no {key}");
        }
        let texts: Vec<String> = rt.elements().into_iter().filter_map(|e| e.text).collect();
        assert!(
            texts.iter().any(|t| t.contains("Body text of the guide.")),
            "{texts:?}"
        );
    }
}
