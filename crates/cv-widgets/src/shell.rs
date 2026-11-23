//! The shell view: degraded banner, module rail, nav / virtualized list /
//! reading / inspector providers, status bar, command palette. Renders
//! `*Model` data only — no lane logic, no IO.

use crate::theme::{color, font};
use cv_core::events::shell::DegradedReason;
use cv_model::module::{ReadingModel, RowModel, Slot, Tone};
use cv_model::theme::Theme;
use cv_model::workbench::Workbench;
use rune::{Color, Cx, El, GrowthSpec, ScrollSpec, col, row, spacer, text};
use std::borrow::Cow;

fn tone(t: &Theme, tone: Tone) -> Color {
    let c = &t.colors;
    color(match tone {
        Tone::Normal => c.text,
        Tone::Dim => c.text_dim,
        Tone::Ok => c.ok,
        Tone::Warn => c.warn,
        Tone::Error => c.error,
    })
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
        DegradedReason::WatchDead => {
            "Watch session died — showing last snapshot; restart watch to resume.".into()
        }
        DegradedReason::StoreCorrupt => {
            "Store corrupt — journal writes failing; changes are not persisted.".into()
        }
    }
}

fn banner(s: &Workbench) -> Option<El> {
    let t = &s.theme;
    let r = s.app.degraded.as_ref()?;
    Some(
        row()
            .key("banner")
            .role("alert")
            .w_full()
            .px(t.spacing.unit * 3.0)
            .py(t.spacing.unit * 1.5)
            .gap(t.spacing.unit * 2.0)
            .fill(color(t.colors.banner))
            .child(
                text("DEGRADED")
                    .font(font(t.type_.label))
                    .color(color(t.colors.error)),
            )
            .child(
                text(degraded_text(r))
                    .font(font(t.type_.body))
                    .color(color(t.colors.text)),
            )
            .child(spacer())
            .child(label(t, "palette: Dismiss degraded banner")),
    )
}

fn rail(s: &Workbench) -> El {
    let t = &s.theme;
    let mut rail = col()
        .key("rail")
        .w(t.spacing.rail_w)
        .h_full()
        .fill(color(t.colors.rail))
        .py(t.spacing.unit * 2.0)
        .gap(t.spacing.unit);
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
        let active = idx == Some(s.active);
        let fg = match (idx, active) {
            (None, _) => t.colors.text_dim,
            (_, true) => t.colors.accent_text,
            _ => t.colors.text,
        };
        let mut item = row()
            .key(format!("rail-{key}"))
            .role("tab")
            .w_full()
            .px(t.spacing.unit * 3.0)
            .py(t.spacing.unit * 1.5)
            .fill(color(if active {
                t.colors.accent
            } else {
                t.colors.rail
            }))
            .child(
                text(title)
                    .font(font(t.type_.title))
                    .color(color(fg))
                    .single_line(),
            );
        if let Some(i) = idx {
            item = item
                .hover(color(t.colors.pane_alt))
                .on_press(move |s: &mut Workbench| s.activate(i));
        } else {
            item = item.label("not registered");
        }
        rail = rail.child(item);
    }
    rail
}

fn nav(s: &Workbench) -> El {
    let t = &s.theme;
    let mut nav = col()
        .key("nav")
        .w(t.spacing.nav_w)
        .h_full()
        .fill(color(t.colors.pane_alt))
        .padding(t.spacing.unit * 2.0)
        .gap(t.spacing.unit);
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
        nav = nav.child(
            row()
                .key(format!("nav-{}", item.key))
                .role("treeitem")
                .w_full()
                .px(t.spacing.unit * 2.0)
                .py(t.spacing.unit)
                .fill(color(if sel {
                    t.colors.selection
                } else {
                    t.colors.pane_alt
                }))
                .hover(color(t.colors.selection))
                .on_press(move |s: &mut Workbench| s.set_nav(&key))
                .child(
                    text(item.label)
                        .font(font(t.type_.body))
                        .color(color(t.colors.text)),
                )
                .child(spacer())
                .child(label(
                    t,
                    item.count.map(|c| c.to_string()).unwrap_or_default(),
                )),
        );
    }
    nav
}

fn list_row(s: &Workbench, i: usize) -> El {
    let t = &s.theme;
    let RowModel {
        key,
        title,
        detail,
        badge,
        tone: tn,
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
        .hover(color(t.colors.pane_alt))
        .on_press(move |s: &mut Workbench| s.select_key(&k))
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
    col()
        .key("list-pane")
        .w(t.spacing.list_w)
        .h_full()
        .min_h(0.0)
        .fill(color(t.colors.pane))
        .border(1.0, color(t.colors.border))
        .child(
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
            lines: vec![],
        });
    let mut pane = col()
        .key("reading")
        .grow(1.0)
        .h_full()
        .min_h(0.0)
        .padding(t.spacing.unit * 4.0)
        .gap(t.spacing.unit * 1.5)
        .fill(color(t.colors.bg))
        .child(
            text(model.title)
                .font(font(t.type_.title))
                .color(color(t.colors.text)),
        );
    for (n, l) in model.lines.into_iter().enumerate() {
        let style = if l.mono { t.type_.mono } else { t.type_.body };
        let mut r = row().key(format!("line-{n}")).gap(t.spacing.unit * 2.0);
        if let Some(lb) = l.label {
            r = r.child(label(t, lb).w(96.0));
        }
        pane = pane.child(r.child(text(l.text).font(font(style)).color(tone(t, l.tone))));
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
        for (name, v) in m.inspector(k).fields {
            pane = pane.child(field(&name, v));
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

fn status(s: &Workbench) -> El {
    let t = &s.theme;
    let engine = s.app.engine.clone().unwrap_or_else(|| "no engine".into());
    let parts = [
        engine,
        format!("{} events", s.app.committed),
        format!("epoch {}", s.app.snapshot_epoch),
        format!("{:?}", t.mode).to_lowercase(),
        if t.motion.reduced {
            "reduced motion"
        } else {
            "motion"
        }
        .into(),
    ];
    let mut bar = row()
        .key("status")
        .w_full()
        .h(t.spacing.row_h)
        .px(t.spacing.unit * 3.0)
        .gap(t.spacing.unit * 3.0)
        .items_center()
        .fill(color(t.colors.rail));
    for (i, p) in parts.into_iter().enumerate() {
        bar = bar.child(label(t, p).key(format!("status-{i}")));
    }
    bar.child(spacer())
        .child(label(t, s.status.clone()).key("status-msg"))
        .child(label(t, "mod+K commands"))
}

fn palette(s: &Workbench, width: f32) -> El {
    let t = &s.theme;
    let cmds = s.commands();
    let hits = s.palette.matches(&cmds);
    let left = ((width - t.spacing.palette_w) / 2.0).max(0.0);
    let mut list = col().key("palette-list").w_full().gap(1.0);
    for (i, c) in hits.iter().take(12).enumerate() {
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
    let mut body = row().key("body").w_full().grow(1.0).min_h(0.0);
    body = body
        .child(rail(s))
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
    if let Some(b) = banner(s) {
        root = root.child(b);
    }
    root = root.child(body).child(status(s));
    if s.palette.open {
        let w = cx.layout_size("shell").map(|[w, _]| w).unwrap_or(1280.0);
        root = root.child(palette(s, w));
    }
    root
}
