//! The shell view: degraded banner, module rail, nav / virtualized list /
//! reading / inspector providers, status bar, command palette. Renders
//! `*Model` data only — no lane logic, no IO.

use crate::diagnostic;
use crate::theme::{self, Level, Size, Styled, color, font};
use crate::{controls, fx, icons};
use cv_core::events::shell::DegradedReason;
use cv_model::module::{ReadingModel, RowModel, Slot, Tone};
use cv_model::theme::{Mode, Rgba, Theme, TypeStyle};
use cv_model::workbench::Workbench;
use rune::{
    Color, Cx, Edge, El, GrowthSpec, Icon, ScrollSpec, Span, Transition, col, icon, row, spacer,
    text,
};
use rune_kit::{Markdown, RollingText, Toast, ToastRegion};
use std::borrow::Cow;

pub(crate) fn tone(t: &Theme, tn: Tone) -> Color {
    color(tone_token(t, tn))
}

fn label(t: &Theme, s: impl Into<String>) -> El {
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
        | DegradedReason::BuildFailed => Tone::Error,
    }
}

fn tone_token(t: &Theme, tn: Tone) -> Rgba {
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
fn chip(t: &Theme, key: &str, label: impl Into<String>, tn: Tone) -> El {
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
            .child(
                text(degraded_text(r))
                    .font(font(t.type_.body))
                    .color(color(t.colors.text)),
            )
            .child(spacer())
            .children((*r == DegradedReason::WatchDead).then(|| {
                controls::button(
                    t,
                    "banner-restart",
                    "Restart watch",
                    controls::Variant::Neutral,
                    |s: &mut Workbench| s.run_command("shell.watch.restart"),
                )
            }))
            .child(controls::button(
                t,
                "banner-dismiss",
                "Dismiss",
                controls::Variant::Ghost,
                |s: &mut Workbench| s.run_command("shell.degraded.dismiss"),
            )),
    )
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
    let mut nav = col()
        .key("nav")
        .w(t.spacing.nav_w)
        .h_full()
        .fill(color(t.colors.pane_alt))
        .padding(t.spacing.unit * 2.0)
        .gap(t.spacing.unit)
        .motion(theme::spring(t.motion.snap));
    let Some(m) = s.registry.get(s.active) else {
        return nav.child(label(t, "no modules registered"));
    };
    nav = nav.child(
        text(m.spec().title)
            .font(font(t.type_.title))
            .color(color(t.colors.text)),
    );
    for item in m.nav().items {
        let sel = s.active_nav() == Some(item.key.as_str());
        let key = item.key.clone();
        // Up/down always drive the list, so the nav never holds keyboard
        // focus: its selection wears the inactive colour, as Outlook's folder
        // pane does while the message list has focus.
        let mut r = row()
            .key(format!("nav-{}", item.key))
            .role("treeitem")
            .w_full()
            .px(t.spacing.unit * 2.0)
            .py(t.spacing.unit)
            .gap(t.spacing.unit)
            .items_center()
            .rounded(t, Size::Sm)
            .fill(color(if sel {
                t.states.selection_inactive
            } else {
                t.colors.pane_alt
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
        nav = nav.child(r);
    }
    nav
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
    } = s.row(i);
    let sel = s.selected_key() == Some(key.as_str());
    let k = key.clone();
    let mut r = row()
        .key(format!("row-{key}"))
        .role("listitem")
        .w_full()
        .h(t.spacing.row_h)
        .px(t.spacing.unit * 2.0)
        .gap(t.spacing.unit * 2.0)
        .items_center()
        .fill(color(if sel {
            t.colors.selection
        } else {
            t.colors.pane
        }))
        .states(t)
        .on_press(move |s: &mut Workbench| s.select_key(&k));
    if let Some(rt) = rl {
        r = r.child(
            col()
                .key("rail")
                .w(3.0)
                .h(t.spacing.row_h * 0.6)
                .fill(tone(t, rt)),
        );
    }
    r = r
        .child(
            text(title)
                .font(font(t.type_.body))
                .color(tone(t, tn))
                .single_line()
                .truncate(),
        )
        .child(spacer())
        .child(label(t, detail).truncate());
    if let Some(b) = badge {
        r = r.child(
            text(b)
                .font(font(t.type_.mono))
                .color(color(t.colors.accent)),
        );
    }
    r
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
    let mut pane = col()
        .key("list-pane")
        .w(t.spacing.list_w)
        .h_full()
        .min_h(0.0)
        .fill(color(t.colors.pane))
        .border(1.0, color(t.colors.border));
    if s.list_len() == 0 {
        pane = pane.child(
            label(t, "Empty mailbox")
                .key("list-empty")
                .px(t.spacing.unit * 3.0)
                .py(t.spacing.unit * 2.0),
        );
    }
    pane.child(
        col()
            .key("list-viewport")
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .scroll(ScrollSpec {
                stick_bottom: false,
            })
            .child(rows),
    )
}

fn reading(s: &Workbench) -> El {
    let t = &s.theme;
    let model = s
        .selected_key()
        .and_then(|k| s.registry.get(s.active).map(|m| m.reading(k)))
        .unwrap_or_else(|| ReadingModel {
            title: "Nothing selected".into(),
            ..Default::default()
        });
    let mut pane = col()
        .key("reading")
        .grow(1.0)
        .min_w(0.0)
        .h_full()
        .min_h(0.0)
        .padding(t.spacing.unit * 4.0)
        .gap(t.spacing.unit * 1.5)
        .fill(color(t.colors.bg));
    if !model.breadcrumbs.is_empty() {
        let mut crumbs = row().key("breadcrumbs").gap(t.spacing.unit);
        for (n, c) in model.breadcrumbs.into_iter().enumerate() {
            if n > 0 {
                crumbs = crumbs.child(label(t, "›"));
            }
            crumbs = crumbs.child(label(t, c).key(format!("crumb-{n}")));
        }
        pane = pane.child(crumbs);
    }
    pane = pane.child(
        text(model.title)
            .font(font(t.type_.title))
            .color(color(t.colors.text))
            .wrap(),
    );
    for (n, l) in model.lines.into_iter().enumerate() {
        let style = if l.mono { t.type_.mono } else { t.type_.body };
        let mut r = row().key(format!("line-{n}")).gap(t.spacing.unit * 2.0);
        if let Some(lb) = l.label {
            r = r.child(label(t, lb).w(96.0));
        }
        pane = pane.child(
            r.child(
                text(l.text)
                    .font(font(style))
                    .color(tone(t, l.tone))
                    .wrap()
                    .grow(1.0)
                    .min_w(0.0),
            ),
        );
    }
    if let Some(md) = model.markdown {
        // Draft sketch only: Boris output stays authoritative.
        pane = pane.child(label(t, "Draft sketch").key("sketch-label"));
        pane = pane.child(
            col()
                .key("sketch")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .child(Markdown::new(md).render(&theme::markdown(t))),
        );
    }
    pane
}

fn inspector(s: &Workbench) -> El {
    let t = &s.theme;
    let mut pane = col()
        .key("inspector")
        .w(t.spacing.inspector_w)
        .h_full()
        .padding(t.spacing.unit * 2.0)
        .gap(t.spacing.unit)
        .fill(color(t.colors.pane_alt))
        .child(
            text("Inspector")
                .font(font(t.type_.title))
                .color(color(t.colors.text)),
        );
    let field = |k: &str, v: String| {
        row()
            .key(format!("field-{k}"))
            .gap(t.spacing.unit * 2.0)
            .child(label(t, k.to_string()).w(72.0))
            .child(
                text(v)
                    .font(font(t.type_.mono))
                    .color(color(t.colors.text))
                    .truncate(),
            )
    };
    if let Some(k) = s.selected_key()
        && let Some(m) = s.registry.get(s.active)
    {
        let model = m.inspector(k);
        for (name, v) in model.fields {
            pane = pane.child(field(&name, v));
        }
        if !model.diagnostics.is_empty() {
            pane = pane.child(
                label(t, "Diagnostics")
                    .key("diagnostics")
                    .mt(t.spacing.unit * 2.0),
            );
            for (n, d) in model.diagnostics.iter().enumerate() {
                pane = pane.child(diagnostic::card(t, d, n));
            }
        }
        for (n, (title, items)) in model.lists.into_iter().enumerate() {
            pane = pane.child(
                label(t, format!("{title} ({})", items.len()))
                    .key(format!("list-{n}"))
                    .mt(t.spacing.unit * 2.0),
            );
            for (j, it) in items.into_iter().take(12).enumerate() {
                pane = pane.child(
                    text(it)
                        .key(format!("list-{n}-{j}"))
                        .font(font(t.type_.mono))
                        .color(color(t.colors.text))
                        .truncate(),
                );
            }
        }
        if let Some(l) = s.link_for(k) {
            pane = pane.child(
                controls::button(
                    t,
                    "follow-link",
                    &l.label,
                    controls::Variant::Primary,
                    |s: &mut Workbench| s.follow_link(),
                )
                .mt(t.spacing.unit * 2.0),
            );
        }
    }
    pane = pane.child(label(t, "Windows").mt(t.spacing.unit * 2.0));
    pane = pane.child(field(
        "primary",
        if s.windows.primary.dirty {
            "dirty"
        } else {
            "clean"
        }
        .into(),
    ));
    for c in &s.windows.companions {
        pane = pane.child(field(
            &format!("win{}", c.id.0),
            format!("{}{}", c.title, if c.state.dirty { " *" } else { "" }),
        ));
    }
    pane
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
        .h(t.spacing.row_h - t.spacing.unit)
        .px(t.spacing.unit * 2.0)
        .gap(t.spacing.unit * 1.5)
        .items_center()
        .fill(color(t.colors.pane_alt))
        .rounded(t, Size::Pill);
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

/// Transient status messages, bottom-right above the status bar.
fn toasts(s: &Workbench) -> Option<El> {
    if s.status.is_empty() {
        return None;
    }
    let t = &s.theme;
    let k = theme::kit(t);
    let short = s.status.chars().count() <= 40;
    let mut toast = if short {
        Toast::new("status-msg", s.status.clone())
    } else {
        Toast::new("status-msg", "Status").description(s.status.clone())
    };
    toast = toast.on_close(|s: &mut Workbench| s.status.clear());
    Some(ToastRegion::new().toast(toast.build(&k)).build(&k))
}
fn palette(s: &Workbench, width: f32) -> El {
    let t = &s.theme;
    let cmds = s.commands();
    let hits = s.palette.visible(&cmds);
    let left = ((width - t.spacing.palette_w) / 2.0).max(0.0);
    let mut list = col().key("palette-list").w_full().gap(1.0);
    for (i, c) in hits.iter().enumerate() {
        let sel = i == s.palette.cursor;
        let id = c.id.clone();
        list = list.child(
            row()
                .key(format!("cmd-{}", c.id))
                .role("option")
                .w_full()
                .px(t.spacing.unit * 2.0)
                .h(t.spacing.row_h)
                .items_center()
                .fill(color(if sel { t.colors.accent } else { t.colors.pane }))
                .on_press(move |s: &mut Workbench| {
                    s.palette.close();
                    s.run_command(&id);
                })
                .child(
                    text(c.title.clone())
                        .font(font(t.type_.body))
                        .color(color(if sel {
                            t.colors.accent_text
                        } else {
                            t.colors.text
                        })),
                )
                .child(spacer())
                .child(label(t, c.chord.clone().unwrap_or_default())),
        );
    }
    col()
        .key("palette")
        .role("dialog")
        .absolute(left, 48.0)
        .w(t.spacing.palette_w)
        .padding(t.spacing.unit * 2.0)
        .gap(t.spacing.unit * 2.0)
        .fill(color(t.colors.pane))
        .border(1.0, color(t.colors.accent))
        .rounded(t, Size::Md)
        .elevated(t, Level::Overlay)
        .child(
            text(format!("> {}", s.palette.query))
                .key("palette-query")
                .font(font(t.type_.mono))
                .color(color(t.colors.text)),
        )
        .child(list)
        .child(label(
            t,
            format!("{} of {} commands", hits.len(), cmds.len()),
        ))
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
        .child(reading(s));
    if s.inspector_open {
        body = body.child(inspector(s));
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
        root = root.child(palette(s, w));
    }
    root
}
