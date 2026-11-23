//! Native boot: one primary window + companion windows (editor popouts
//! host). Headless runs (`--tree`, `--snap`, `--scene`) open no companions.

use crate::harness;
use crate::shell::view;
use crate::theme::{color, font};
use cv_model::module::ReadingModel;
use cv_model::theme::Theme;
use cv_model::windows::WindowId;
use cv_model::workbench::Workbench;
use rune::{AppSpec, Cx, El, Tx, Waker, WindowSpec, col, row, text};
use std::cell::RefCell;
use std::collections::{BTreeSet, VecDeque};
use std::sync::OnceLock;

pub type Scene = (&'static str, fn(&mut Workbench));

pub struct Launch {
    pub title: &'static str,
    pub init: fn() -> Workbench,
    /// Runs once with the waker installed: start engine watch, etc.
    pub boot: fn(&mut Workbench),
    pub scenes: &'static [Scene],
}

static BOOT: OnceLock<fn(&mut Workbench)> = OnceLock::new();

thread_local! {
    static OPENED: RefCell<BTreeSet<WindowId>> = const { RefCell::new(BTreeSet::new()) };
    static PENDING: RefCell<VecDeque<Companion>> = const { RefCell::new(VecDeque::new()) };
}

/// A companion window's own state: a pinned noun's reading model.
/// L3 replaces the body with its editor; the host contract stays.
pub struct Companion {
    pub title: String,
    pub reading: ReadingModel,
    pub theme: Theme,
}

fn companion_init() -> Companion {
    PENDING
        .with(|p| p.borrow_mut().pop_front())
        .unwrap_or(Companion {
            title: String::new(),
            reading: ReadingModel::default(),
            theme: Theme::dark(),
        })
}

pub fn companion_view(s: &Companion, _cx: &mut Cx<Companion>) -> El {
    let t = &s.theme;
    let mut body = col()
        .key("companion")
        .w_full()
        .h_full()
        .padding(t.spacing.unit * 4.0)
        .gap(t.spacing.unit * 1.5)
        .fill(color(t.colors.bg))
        .child(
            text(s.title.clone())
                .font(font(t.type_.title))
                .color(color(t.colors.text)),
        );
    for l in &s.reading.lines {
        let mut r = row().gap(t.spacing.unit * 2.0);
        if let Some(lb) = &l.label {
            r = r.child(
                text(lb.clone())
                    .font(font(t.type_.label))
                    .color(color(t.colors.text_dim)),
            );
        }
        body = body.child(
            r.child(
                text(l.text.clone())
                    .font(font(t.type_.body))
                    .color(color(t.colors.text)),
            ),
        );
    }
    body
}

fn open_companions(s: &Workbench) {
    for c in &s.windows.companions {
        if OPENED.with(|o| o.borrow_mut().insert(c.id)) {
            let reading = s
                .registry
                .index_of(&c.module)
                .and_then(|i| s.registry.get(i))
                .map(|m| m.reading(&c.key))
                .unwrap_or_default();
            PENDING.with(|p| {
                p.borrow_mut().push_back(Companion {
                    title: c.title.clone(),
                    reading,
                    theme: s.theme.clone(),
                })
            });
            rune::open_window(WindowSpec::new(
                format!("companion-{}", c.id.0),
                AppSpec::new(
                    "Capture Vector — companion",
                    (560.0, 640.0),
                    companion_init,
                    companion_view,
                ),
            ));
        }
    }
}

fn setup(s: &mut Workbench, w: Waker) {
    s.set_waker(w);
    if let Some(boot) = BOOT.get() {
        boot(s);
    }
}

fn poll(s: &mut Workbench, t: f64, tx: &mut Tx) -> bool {
    let changed = harness::poll(s, t, tx);
    open_companions(s);
    changed
}

/// Boot the primary window (or a headless run, per rune's CLI flags).
pub fn run(l: Launch) {
    let _ = BOOT.set(l.boot);
    rune::run(AppSpec {
        scenes: l.scenes,
        setup: Some(setup),
        poll: Some(poll),
        on_key: Some(harness::on_key),
        min_size: Some((960.0, 560.0)),
        ..AppSpec::new(l.title, harness::SIZE, l.init, view)
    });
}
