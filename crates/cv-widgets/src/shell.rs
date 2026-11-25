//! The shell view: degraded banner, module rail, nav / virtualized list /
//! reading / inspector providers, status bar, command palette. Renders
//! `*Model` data only — no lane logic, no IO.

use crate::theme::{self, Size, Styled, color, font};
use crate::{controls, fx, icons};
use cv_core::events::shell::DegradedReason;
use cv_model::module::{Chip, RowModel, Slot, Tone};
use cv_model::theme::{Mode, Rgba, Theme, TypeStyle};
use cv_model::workbench::Workbench;
use rune::{
    Color, Cx, Edge, El, GrowthSpec, Icon, ScrollSpec, Shadow, Span, Transition, col, icon, row,
    spacer, text,
};
use rune_kit::{RollingText, Toast, ToastRegion};
use std::borrow::Cow;

pub(crate) fn tone(t: &Theme, tn: Tone) -> Color {
    color(tone_token(t, tn))
}

pub(crate) fn label(t: &Theme, s: impl Into<String>) -> El {
    text(s)
        .font(font(t.type_.label))
        .color(color(t.colors.text_dim))
        .single_line()
}

pub fn degraded_text(r: &DegradedReason) -> String {
    match r {
        DegradedReason::EngineAbsent => {
            "Engine absent — Boris not found; browsing cached state only.".into()
        }
        DegradedReason::EngineVersionMismatch { found, want } => {
            format!("Engine version mismatch — found boris {found}, want {want}.")
        }
        DegradedReason::EngineBuildFailing => {
            "Engine build failed after one retry. Fix the build, then reopen the workspace.".into()
        }
        DegradedReason::WatchDead => "Watch session died — showing last snapshot.".into(),
        DegradedReason::StoreCorrupt => {
            "Store corrupt — journal writes failing; changes are not persisted.".into()
        }
        DegradedReason::BuildFailed => {
            "Boris build failed — workspace remains browsable; see Activity for build details."
                .into()
        }
        DegradedReason::InitialBuildFailed { code, message } => {
            format!("Initial build failed: {code} — {message}")
        }
    }
}

/// What the author can do about a degraded state the banner cannot fix.
pub fn degraded_hint(r: &DegradedReason) -> Option<&'static str> {
    match r {
        DegradedReason::InitialBuildFailed { code, .. } if code == "ELAYOUT" => Some(
            "A bare content tree has no theme or boris.json profile; `boris init` scaffolds a \
             starter site. Fix the folder, then reopen the workspace.",
        ),
        DegradedReason::InitialBuildFailed { .. } => Some(
            "Restarting the watch fails the same way. Fix the folder, then reopen the workspace.",
        ),
        _ => None,
    }
}

/// How serious each degraded reason reads: a stale or mismatched engine
/// is a warning; no engine, a failing build or a corrupt store is an error.
pub fn degraded_tone(r: &DegradedReason) -> Tone {
    match r {
        DegradedReason::EngineVersionMismatch { .. } | DegradedReason::WatchDead => Tone::Warn,
        DegradedReason::EngineAbsent
        | DegradedReason::EngineBuildFailing
        | DegradedReason::StoreCorrupt
        | DegradedReason::BuildFailed
        | DegradedReason::InitialBuildFailed { .. } => Tone::Error,
    }
}

pub(crate) fn tone_token(t: &Theme, tn: Tone) -> Rgba {
    let c = &t.colors;
    match tn {
        Tone::Normal => c.text,
        Tone::Dim => c.text_dim,
        Tone::Ok => c.ok,
        Tone::Warn => c.warn,
        Tone::Error => c.error,
    }
}

/// A small tinted capsule: `label` in the tone colour on a wash of it.
pub(crate) fn chip(t: &Theme, key: &str, label: impl Into<String>, tn: Tone) -> El {
    let fg = tone_token(t, tn);
    row()
        .key(key.to_string())
        .px(t.spacing.unit * 1.5)
        .items_center()
        .fill(theme::tint(fg, 0x2e))
        .rounded(t, Size::Pill)
        .child(
            text(label)
                .font(font(t.type_.label))
                .color(color(fg))
                .single_line(),
        )
}

/// A pane's header: it sticks to the top of the pane's scroll viewport
/// with a top-lit sheen, and shows a hairline while content runs under it.
pub(crate) fn pane_header(t: &Theme, key: &str, fill: Rgba) -> El {
    let sheen = t.gradients.pane_sheen;
    col()
        .key(key.to_string())
        .w_full()
        .fill(color(fill))
        .gradient(color(sheen.start), color(sheen.end))
        .sticky_top(0.0)
        .stuck_shadow(Shadow {
            y: 0.5,
            color: color(t.colors.border),
            ..Default::default()
        })
}

/// A pane's vertical scroll viewport, filling what the pane leaves.
pub(crate) fn pane_scroll(key: &str) -> El {
    col()
        .key(key.to_string())
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
}

/// A centred empty state: glyph, title, one-line hint.
pub(crate) fn empty_state(
    t: &Theme,
    key: &str,
    glyph: &'static Icon,
    title: &str,
    hint: String,
) -> El {
    col()
        .key(key.to_string())
        .w_full()
        .grow(1.0)
        .items_center()
        .justify_center()
        .gap(t.spacing.unit * 2.0)
        .px(t.spacing.unit * 4.0)
        .child(icon(glyph, 28.0).color(color(t.colors.text_dim)))
        .child(
            text(title.to_string())
                .key(format!("{key}-title"))
                .font(font(t.type_.title))
                .color(color(t.colors.text))
                .single_line(),
        )
        .child(label(t, hint).key(format!("{key}-hint")).truncate())
}

/// A faint accent glow behind an empty pane, painted on the pane itself.
pub(crate) fn empty_glow(t: &Theme, pane: El, width: f32) -> El {
    pane.radial_gradient(
        [0.5, 0.45],
        [width * 0.6, 260.0],
        [
            (0.0, theme::tint(t.colors.accent, 0x10)),
            (1.0, theme::tint(t.colors.accent, 0)),
        ],
    )
}

fn banner(s: &Workbench, lights: f32) -> Option<El> {
    let t = &s.theme;
    let r = s.app.degraded.as_ref()?;
    let tn = degraded_tone(r);
    let fg = tone_token(t, tn);
    let wash = match tn {
        Tone::Error => color(t.gradients.banner.start),
        _ => theme::tint(fg, 0x38),
    };
    Some(
        row()
            .key("banner")
            .role("alert")
            .w_full()
            .pl((lights + t.spacing.unit * 2.0).max(t.spacing.unit * 3.0))
            .pr(t.spacing.unit * 3.0)
            .py(t.spacing.unit * 1.5)
            .gap(t.spacing.unit * 2.0)
            .items_center()
            .fill(color(t.colors.banner))
            .linear_gradient(
                std::f32::consts::FRAC_PI_2,
                [(0.0, wash), (1.0, color(t.gradients.banner.end))],
            )
            .border_sides([0.0, 0.0, 1.0, 0.0], color(fg))
            .transition(Transition::move_edge(Edge::Top))
            .motion(theme::spring(t.motion.panel))
            .child(icon(&icons::TRIANGLE_ALERT, 14.0).color(color(fg)))
            .child(chip(t, "banner-chip", "DEGRADED", tn))
            .children(banner_message(t, r))
            .children((*r == DegradedReason::WatchDead).then(|| {
                controls::button(
                    t,
                    "banner-restart",
                    "Restart watch",
                    controls::Variant::Neutral,
                    |s: &mut Workbench| s.run_command("shell.watch.restart"),
                )
            }))
            .children(
                matches!(r, DegradedReason::InitialBuildFailed { .. })
                    .then(|| {
                        [
                            controls::button(
                                t,
                                "banner-reveal",
                                "Show diagnostic",
                                controls::Variant::Neutral,
                                |s: &mut Workbench| s.run_command("shell.degraded.reveal"),
                            ),
                            controls::button(
                                t,
                                "banner-reopen",
                                "Reopen workspace",
                                controls::Variant::Ghost,
                                |s: &mut Workbench| s.run_command("shell.watch.restart"),
                            ),
                        ]
                    })
                    .into_iter()
                    .flatten(),
            )
            .child(controls::button(
                t,
                "banner-dismiss",
                "Dismiss",
                controls::Variant::Ghost,
                |s: &mut Workbench| s.run_command("shell.degraded.dismiss"),
            )),
    )
}
/// The banner's message, then whatever fills the row before its buttons.
fn banner_message(t: &Theme, r: &DegradedReason) -> Vec<El> {
    let message = text(degraded_text(r))
        .font(font(t.type_.body))
        .color(color(t.colors.text));
    let Some(hint) = degraded_hint(r) else {
        return vec![message, spacer()];
    };
    vec![
        col()
            .key("banner-message")
            .grow(1.0)
            .min_w(0.0)
            .gap(t.spacing.unit * 0.5)
            .child(message.wrap())
            .child(
                text(hint)
                    .key("banner-hint")
                    .font(font(t.type_.label))
                    .color(color(t.colors.text_dim))
                    .wrap(),
            ),
    ]
}

/// AppKit's titlebar band: the traffic lights sit in its top 28 pt when the
/// content fills the window (`Chrome::full_size_content`).
const TITLEBAR_BAND: f32 = 28.0;

fn rail(s: &Workbench, lights: bool) -> El {
    let t = &s.theme;
    let top = if lights && s.app.degraded.is_none() {
        TITLEBAR_BAND
    } else {
        t.spacing.unit * 2.0
    };
    let mut rail = col()
        .key("rail")
        .w(t.spacing.rail_w)
        .h_full()
        .fill(color(t.colors.rail))
        .pt(top)
        .pb(t.spacing.unit * 2.0)
        .px(t.spacing.unit)
        .gap(t.spacing.unit)
        .motion(theme::spring(t.motion.snap));
    let aurora = fx::rail_aurora(t);
    // Items at rest let the aurora show through instead of repainting the rail.
    let rest = if aurora.is_some() {
        Color::TRANSPARENT
    } else {
        color(t.colors.rail)
    };
    if let Some(a) = aurora {
        rail = rail.child(a);
    }
    let mut entries: Vec<(String, String, Option<usize>)> = Slot::CORE
        .iter()
        .map(|slot| {
            let idx = s.registry.iter().position(|m| m.spec().slot == *slot);
            (
                format!("{slot:?}").to_lowercase(),
                slot.title().to_string(),
                idx,
            )
        })
        .collect();
    for (i, m) in s.registry.iter().enumerate() {
        if m.spec().slot == Slot::Extra {
            entries.push((m.spec().id.to_string(), m.spec().title.to_string(), Some(i)));
        }
    }
    for (key, title, idx) in entries {
        rail = rail.child(rail_item(s, &key, title, idx, rest));
    }
    rail
}

fn rail_item(s: &Workbench, key: &str, title: String, idx: Option<usize>, rest: Color) -> El {
    let t = &s.theme;
    let active = idx == Some(s.active);
    let mut item = col()
        .key(format!("rail-{key}"))
        .role("tab")
        .focus_selected(active)
        .w_full()
        .px(t.spacing.unit * 2.0)
        .py(t.spacing.unit * 1.5)
        .gap(t.spacing.unit)
        .rounded(t, Size::Md)
        .fill(rest);
    if active {
        let g = t.gradients.rail_active;
        item = item
            .linear_gradient(
                std::f32::consts::FRAC_PI_2,
                [(0.0, color(g.start)), (1.0, color(g.end))],
            )
            .border_sides([0.0, 0.0, 0.0, 3.0], color(t.colors.accent));
    }
    let glyph = if active {
        t.colors.accent
    } else {
        t.colors.text
    };
    // Icon above the label: "Research" beside a 16 pt icon overflows the 88 pt rail.
    let mut head = row().w_full().items_center().child(
        icon(rail_icon(key), 16.0)
            .key(format!("rail-{key}-icon"))
            .color(color(glyph)),
    );
    if let Some(n) = idx
        .and_then(|i| s.registry.get(i))
        .and_then(|m| m.badge())
        .filter(|n| *n > 0)
    {
        head = head.child(spacer()).child(
            row()
                .px(t.spacing.unit * 1.5)
                .rounded(t, Size::Pill)
                .fill(color(t.colors.accent))
                .child(
                    text(n.to_string())
                        .key(format!("rail-{key}-badge"))
                        .font(font(t.type_.label))
                        .color(color(t.colors.accent_text))
                        .single_line(),
                ),
        );
    }
    let weight = if active { 700.0 } else { t.type_.title.weight };
    item = item.child(head).child(
        text(title)
            .font(font(TypeStyle {
                weight,
                ..t.type_.title
            }))
            .color(color(t.colors.text))
            .single_line(),
    );
    item = match idx {
        Some(i) => item
            .states(t)
            .on_press(move |s: &mut Workbench| s.activate(i)),
        None => item
            .label("not registered")
            .help("Not registered in this build")
            .dim(0.45),
    };
    if key == "activity"
        && let Some(glow) = fx::build_glow(t, s.app.builds_finished)
    {
        item = item.child(glow);
    }
    item
}
fn rail_icon(key: &str) -> &'static Icon {
    match key {
        "pages" => &icons::FILES,
        "research" => &icons::NOTEBOOK_PEN,
        "audit" => &icons::SHIELD_CHECK,
        "publish" => &icons::SEND,
        "outputs" => &icons::PACKAGE,
        "activity" => &icons::ACTIVITY,
        cv_model::modules::demo::ID => &icons::PLAY,
        _ => &icons::CIRCLE,
    }
}

fn nav(s: &Workbench) -> El {
    let t = &s.theme;
    let nav = col()
        .key("nav")
        .w(t.spacing.nav_w)
        .h_full()
        .min_h(0.0)
        .fill(color(t.colors.bg))
        .motion(theme::spring(t.motion.snap));
    let Some(m) = s.registry.get(s.active) else {
        return nav.child(label(t, "no modules registered").padding(t.spacing.unit * 2.0));
    };
    let head = pane_header(t, "nav-head", t.colors.bg)
        .px(t.spacing.unit * 2.0)
        .pt(t.spacing.unit * 2.0)
        .pb(t.spacing.unit)
        .child(
            text(m.spec().title)
                .font(font(t.type_.title))
                .color(color(t.colors.text)),
        );
    let mut items = col()
        .w_full()
        .px(t.spacing.unit * 2.0)
        .pb(t.spacing.unit * 2.0)
        .gap(t.spacing.unit);
    for item in m.nav().items {
        let sel = s.active_nav() == Some(item.key.as_str());
        let key = item.key.clone();
        // Up/down always drive the list, so the nav never holds keyboard
        // focus: its selection wears the inactive colour, as Outlook's folder
        // pane does while the message list has focus.
        let mut r = row()
            .key(format!("nav-{}", item.key))
            .role("treeitem")
            .focus_selected(sel)
            .w_full()
            .px(t.spacing.unit * 2.0)
            .py(t.spacing.unit)
            .gap(t.spacing.unit)
            .items_center()
            .rounded(t, Size::Sm)
            .fill(color(if sel {
                t.states.selection_inactive
            } else {
                t.colors.bg
            }))
            .states(t)
            .on_press(move |s: &mut Workbench| s.set_nav(&key))
            .child(nav_label(t, item.label, sel))
            .child(spacer());
        if let Some(c) = item.count {
            r = r.child(
                row()
                    .px(t.spacing.unit * 1.5)
                    .rounded(t, Size::Pill)
                    .fill(color(t.states.selection_inactive))
                    .child(label(t, c.to_string())),
            );
        }
        items = items.child(r);
    }
    nav.child(pane_scroll("nav-scroll").child(head).child(items))
}

/// A facet label such as "Pack · seo-core" draws its group prefix dim, so
/// groups read at a glance without header rows.
fn nav_label(t: &Theme, s: String, sel: bool) -> El {
    const SEP: &str = " · ";
    let prefix = s.find(SEP).map(|i| i + SEP.len());
    let weight = if sel { 600.0 } else { t.type_.body.weight };
    let mut el = text(s)
        .font(font(TypeStyle {
            weight,
            ..t.type_.body
        }))
        .color(color(t.colors.text))
        .single_line()
        .truncate();
    if let Some(end) = prefix {
        el = el.spans(vec![Span {
            color: Some(color(t.colors.text_dim)),
            ..Span::new(0, end)
        }]);
    }
    el
}

fn list_row(s: &Workbench, i: usize) -> El {
    let t = &s.theme;
    let RowModel {
        key,
        title,
        detail,
        badge,
        tone: tn,
        rail: rl,
        path,
    } = s.row(i);
    let sel = s.selected_key() == Some(key.as_str());
    let k = key.clone();
    let mut r = row()
        .key(format!("row-{key}"))
        .role("listitem")
        .focus_selected(sel)
        .w_full()
        .h(t.spacing.row_h)
        .px(t.spacing.unit * 2.0)
        .gap(t.spacing.unit * 2.0)
        .items_center()
        .fill(color(t.colors.bg))
        .border_sides([0.0, 0.0, 0.5, 0.0], color(t.colors.border))
        .states(t)
        .on_press(move |s: &mut Workbench| s.select_key(&k));
    if sel {
        let c = t.colors.selection;
        r = r.linear_gradient(
            std::f32::consts::FRAC_PI_2,
            [
                (0.0, color(c)),
                (1.0, theme::tint(c, scale_alpha(c.3, 0.6))),
            ],
        );
    } else if rl == Some(Tone::Error) {
        r = r.radial_gradient(
            [0.0, 0.5],
            [t.spacing.list_w * 0.6, t.spacing.row_h * 1.5],
            [
                (0.0, theme::tint(t.colors.error, 0x14)),
                (1.0, theme::tint(t.colors.error, 0)),
            ],
        );
    }
    if let Some(rt) = rl {
        let c = tone_token(t, rt);
        r = r.child(
            col()
                .key("rail")
                .w(3.0)
                .h(t.spacing.row_h * 0.6)
                .radius(1.5)
                .fill(color(c))
                .gradient(color(c), theme::tint(c, 0x66)),
        );
    }
    let head = text(title.clone())
        .font(font(t.type_.body))
        .color(tone(t, tn))
        .single_line();
    let head = if path {
        head.truncate_head()
    } else {
        head.truncate_middle()
    };
    r = r
        .child(head.help_when_truncated(title).grow(1.0))
        // The title tells rows apart; the detail gives way first.
        .child(
            label(t, detail.clone())
                .truncate_fade()
                .help_when_truncated(detail)
                .max_w(t.spacing.list_w * 0.25),
        );
    if let Some(Chip { text: b, tone: bt }) = badge {
        let fg = tone_token(t, bt);
        r = r.child(
            text(b)
                .font(font(t.type_.label))
                .color(color(fg))
                .single_line()
                .px(6.0)
                .fill(theme::tint(fg, 0x26))
                .rounded(t, Size::Pill),
        );
    }
    r
}

fn scale_alpha(a: u8, k: f32) -> u8 {
    (f32::from(a) * k).round() as u8
}

fn list(s: &Workbench) -> El {
    let t = &s.theme;
    let rows = col()
        .key("list")
        .w_full()
        .growth(GrowthSpec::default())
        .virtual_rows::<Workbench>(
            s.list_len(),
            t.spacing.row_h,
            |s: &Workbench, i| Cow::Owned(format!("row-{}", s.row(i).key)),
            list_row,
        )
        .virtual_rows_version((
            s.active,
            s.active_nav().map(str::to_string),
            s.app.committed,
        ));
    let empty = s.list_len() == 0;
    let mut pane = col()
        .key("list-pane")
        .w(t.spacing.list_w)
        .h_full()
        .min_h(0.0)
        .fill(color(t.colors.bg))
        .border_sides([0.0, 0.5, 0.0, 0.5], color(t.colors.border));
    if empty {
        let hint = match (&s.workspace, crate::keys::keycaps("mod+shift+o")) {
            (None, Some(caps)) => format!("Open a workspace with {}", caps.concat()),
            _ => "Items appear here as Boris reports them".into(),
        };
        let mut state = empty_state(t, "list-empty", &icons::INBOX, "Nothing here yet", hint);
        if s.workspace.is_none() {
            state = state.child(workspace_actions(s));
        }
        pane = empty_glow(t, pane, t.spacing.list_w).child(state);
    }
    pane.child(
        pane_scroll("list-viewport")
            .grow(if empty { 0.0 } else { 1.0 })
            .child(rows),
    )
}

/// No workspace open: open one, or create one from a Boris starter.
fn workspace_actions(s: &Workbench) -> El {
    let t = &s.theme;
    let why = s.archetypes.as_ref().err();
    let mut actions = col()
        .key("list-empty-actions")
        .items_center()
        .gap(t.spacing.unit * 1.5)
        .pt(t.spacing.unit)
        .child(controls::command_button(
            t,
            "list-empty-open",
            "Open workspace…",
            controls::Variant::Neutral,
            "mod+shift+o",
            true,
            |s: &mut Workbench, _| s.run_command("shell.workspace.open"),
        ))
        .child(controls::command_button(
            t,
            "list-empty-new",
            "New workspace…",
            controls::Variant::Neutral,
            "mod+shift+n",
            why.is_none(),
            |s: &mut Workbench, _| s.run_command(cv_model::new_workspace::NEW),
        ));
    if let Some(why) = why {
        actions = actions.child(
            label(t, format!("New workspace needs Boris: {why}"))
                .key("list-empty-new-why")
                .truncate(),
        );
    }
    actions
}

/// Engine health for the status chip.
fn engine_tone(s: &Workbench) -> Tone {
    let engine_reason = s
        .app
        .degraded
        .as_ref()
        .filter(|r| !matches!(r, DegradedReason::StoreCorrupt));
    match (engine_reason, &s.app.engine) {
        (Some(r), _) => degraded_tone(r),
        (None, Some(_)) => Tone::Ok,
        (None, None) => Tone::Dim,
    }
}

fn engine_chip(s: &Workbench) -> El {
    let t = &s.theme;
    let engine = s.app.engine.clone().unwrap_or_else(|| "no engine".into());
    let tn = engine_tone(s);
    let side = 6.0;
    let chip = row()
        .key("status-0")
        .role("button")
        .help("Open Doctor: engine, watch and .cv/ health")
        .h(t.spacing.row_h - t.spacing.unit)
        .px(t.spacing.unit * 2.0)
        .gap(t.spacing.unit * 1.5)
        .items_center()
        .fill(color(t.colors.pane_alt))
        .rounded(t, Size::Pill)
        .states(t)
        .on_press(|s: &mut Workbench| s.run_command("shell.doctor"));
    let dot = col()
        .key("status-dot")
        .w(side)
        .h(side)
        .fill(tone(t, tn))
        .rounded(t, Size::Pill);
    let (chip, dot) = fx::live_chip(t, chip, dot, s.app.watch_live);
    chip.child(dot)
        .child(label(t, engine).color(color(t.colors.text)))
}

fn status(s: &Workbench) -> El {
    let t = &s.theme;
    let k = theme::kit(t);
    let dark = t.mode == Mode::Dark;
    let events = s.app.committed;
    let (mode_icon, mode_label) = if dark {
        (&icons::SUN, "Switch to light theme")
    } else {
        (&icons::MOON, "Switch to dark theme")
    };
    let (motion_icon, motion_label) = if t.motion.reduced {
        (&icons::ZAP_OFF, "Motion reduced — turn motion on")
    } else {
        (&icons::ZAP, "Motion on — reduce motion")
    };
    row()
        .key("status")
        .w_full()
        .h(t.spacing.row_h)
        .px(t.spacing.unit * 2.0)
        .gap(t.spacing.unit * 3.0)
        .items_center()
        .fill(color(t.colors.rail))
        .border_sides([0.5, 0.0, 0.0, 0.0], color(t.colors.border))
        .child(engine_chip(s))
        .child(
            RollingText::new("status-1", format!("{events} events"))
                .value(events as f64)
                .font(font(t.type_.label))
                .color(color(t.colors.text_dim))
                .spring(theme::spring(t.motion.pop))
                .build(&k),
        )
        .child(
            row()
                .key("status-2")
                .px(t.spacing.unit * 1.5)
                .fill(color(t.colors.pane_alt))
                .rounded(t, Size::Xs)
                .child(
                    text(format!("epoch {}", s.app.snapshot_epoch))
                        .font(font(t.type_.mono))
                        .color(color(t.colors.text_dim))
                        .single_line(),
                ),
        )
        .child(spacer())
        .child(label(t, "mod+K commands"))
        .child(controls::icon_button(
            t,
            "status-3",
            mode_icon,
            mode_label,
            |s: &mut Workbench| s.run_command("shell.theme.toggle"),
        ))
        .child(controls::icon_button(
            t,
            "status-4",
            motion_icon,
            motion_label,
            |s: &mut Workbench| s.run_command("shell.motion.toggle"),
        ))
}

/// Longest status text a toast shows. The toast grows upward from its
/// bottom pin with its close button on the first row, so an unbounded body
/// pushes the close button off the top of the window.
pub const TOAST_MAX_CHARS: usize = 160;

/// Title and optional body for a status toast: short messages are the
/// title; longer ones go under "Status" on one paragraph, cut to
/// [`TOAST_MAX_CHARS`] with "…".
pub fn toast_copy(status: &str) -> (String, Option<String>) {
    let flat = status.split_whitespace().collect::<Vec<_>>().join(" ");
    let n = flat.chars().count();
    if n <= 40 {
        return (flat, None);
    }
    let body = if n <= TOAST_MAX_CHARS {
        flat
    } else {
        let cut: String = flat.chars().take(TOAST_MAX_CHARS - 1).collect();
        format!("{}…", cut.trim_end())
    };
    ("Status".into(), Some(body))
}

/// Transient status messages, bottom-right above the status bar.
fn toasts(s: &Workbench) -> Option<El> {
    if s.status.is_empty() {
        return None;
    }
    let t = &s.theme;
    let k = theme::kit(t);
    let (title, body) = toast_copy(&s.status);
    let mut toast = Toast::new("status-msg", title);
    if let Some(b) = body {
        toast = toast.description(b);
    }
    toast = toast.on_close(|s: &mut Workbench| s.status.clear());
    Some(ToastRegion::new().toast(toast.build(&k)).build(&k))
}
/// The shell view (rune `view` fn).
pub fn view(s: &Workbench, cx: &mut Cx<Workbench>) -> El {
    let t = &s.theme;
    theme::motion_preference(cx, t);
    let lights = cx.titlebar_inset();
    let mut body = row().key("body").w_full().grow(1.0).min_h(0.0);
    body = body
        .child(rail(s, lights > 0.0))
        .child(nav(s))
        .child(list(s))
        .child(crate::reading::pane(s));
    if s.inspector_open {
        body = body.child(crate::inspector::pane(s));
    }
    let mut root = col()
        .key("shell")
        .w_full()
        .h_full()
        .fill(color(t.colors.bg));
    if let Some(b) = banner(s, lights) {
        root = root.child(b);
    }
    root = root.child(body).child(status(s));
    if let Some(t) = toasts(s) {
        root = root.child(t);
    }
    if s.palette.open {
        let w = cx.layout_size("shell").map(|[w, _]| w).unwrap_or(1280.0);
        root = root
            .child(crate::palette::scrim(s))
            .child(crate::palette::panel(s, w));
    }
    if let Some(d) = s.dialogs.open() {
        let mut layer = Vec::new();
        crate::dialog::build::<Workbench>(t, "dialog", d, &mut layer);
        root = root.children(layer);
    }
    root
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_status_is_the_toast_title() {
        assert_eq!(toast_copy("Note archived"), ("Note archived".into(), None));
    }

    #[test]
    fn long_status_is_cut_to_one_bounded_paragraph() {
        let long = format!(
            "boris not found\n{}",
            "/a/very/long/searched/path ".repeat(200)
        );
        let (title, body) = toast_copy(&long);
        assert_eq!(title, "Status");
        let body = body.unwrap();
        assert_eq!(body.chars().count(), TOAST_MAX_CHARS);
        assert!(body.starts_with("boris not found /a/"));
        assert!(body.ends_with('…'));
        assert!(!body.contains('\n'));
    }

    #[test]
    fn mid_length_status_is_shown_whole() {
        let s = "workspace: boris not found (searched 24 locations)";
        assert_eq!(toast_copy(s), ("Status".into(), Some(s.into())));
    }
}
