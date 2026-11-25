//! The command palette: a modal panel over a scrim. `cv_model`'s
//! `Palette` stays the source of truth for the query and the cursor; the
//! panel only draws it and routes its keys back to the model.

use crate::theme::{self, Level, Size, Styled, color, font};
use crate::{icons, keys};
use cv_model::command::CommandSpec;
use cv_model::workbench::Workbench;
use rune::{El, Transition, col, el, icon, row, spacer, text};
use rune_kit::Keybind;

/// How far below the window top the panel sits.
const TOP: f32 = 48.0;

pub(crate) fn scrim(s: &Workbench) -> El {
    let t = &s.theme;
    rune_kit::scrim("palette-scrim", |s: &mut Workbench| s.palette.close())
        .fill(color(t.colors.scrim))
        .transition(enter(s, Transition::opacity()))
        .motion(theme::spring(t.motion.panel))
}

pub(crate) fn panel(s: &Workbench, width: f32) -> El {
    let t = &s.theme;
    let cmds = s.commands();
    let hits = s.palette.visible(&cmds);
    let left = ((width - t.spacing.palette_w) / 2.0).max(0.0);
    let list = col()
        .key("palette-list")
        .w_full()
        .gap(1.0)
        .children(hits.iter().enumerate().map(|(i, c)| option(s, i, c)));
    col()
        .key("palette")
        .role("dialog")
        .label("Command palette")
        .absolute(left, TOP)
        .w(t.spacing.palette_w)
        .padding(t.spacing.unit)
        .gap(t.spacing.unit)
        .fill(color(t.colors.pane))
        .rounded(t, Size::Lg)
        .continuous_corners()
        .elevated(t, Level::Overlay)
        .border_gradient(
            1.0,
            theme::tint(t.colors.text, 0x2e),
            theme::tint(t.colors.text, 0x0a),
        )
        .transition(enter(s, Transition::scale(0.96)))
        .motion(theme::spring(t.motion.panel))
        .modal(true, |s: &mut Workbench| s.palette.close())
        // The modal focuses the panel (its first Tab stop), so every key,
        // Tab included, reaches the model's palette keymap before Rune's
        // own focus traversal.
        .focusable(true)
        .no_focus_ring()
        .on_key(crate::app::on_key)
        .child(query(s))
        .child(list)
        .child(footer(s, hits.len(), cmds.len()))
}

/// Reduced motion opens and closes the palette at once instead of
/// stretching its entrance into a fade.
fn enter(s: &Workbench, t: Transition) -> Transition {
    if s.theme.motion.reduced {
        Transition::none()
    } else {
        t
    }
}

fn query(s: &Workbench) -> El {
    let t = &s.theme;
    let q = &s.palette.query;
    let shown = if q.is_empty() {
        text("Type a command").color(color(t.colors.text_dim))
    } else {
        text(q.clone()).color(color(t.colors.text))
    }
    .font(font(t.type_.body))
    .single_line();
    let caret = el()
        .key("palette-caret")
        .w(1.5)
        .h(t.type_.body.size + 4.0)
        .fill(color(t.colors.accent))
        .a11y_hidden();
    let line = if q.is_empty() {
        row().child(caret).child(shown.key("palette-query"))
    } else {
        row().child(shown.key("palette-query")).child(caret)
    };
    row()
        .key("palette-search")
        .w_full()
        .px(t.spacing.unit * 1.5)
        .h(t.spacing.row_h + t.spacing.unit * 2.0)
        .gap(t.spacing.unit * 1.5)
        .items_center()
        .border_sides([0.0, 0.0, 0.5, 0.0], color(t.colors.border))
        .child(icon(&icons::SEARCH, 14.0).color(color(t.colors.text_dim)))
        .child(line.items_center().gap(1.0))
}

fn option(s: &Workbench, i: usize, c: &CommandSpec) -> El {
    let t = &s.theme;
    let sel = i == s.palette.cursor;
    let id = c.id.clone();
    let mut r = row()
        .key(format!("cmd-{}", c.id))
        .role("option")
        .focus_selected(sel)
        .label(c.title.clone())
        .w_full()
        .px(t.spacing.unit * 1.5)
        .h(t.spacing.row_h)
        .gap(t.spacing.unit * 2.0)
        .items_center()
        .rounded(t, Size::Sm)
        .on_hover(move |s: &mut Workbench| s.palette.cursor = i)
        .on_press(move |s: &mut Workbench| {
            s.palette.close();
            s.run_command(&id);
        })
        .child(
            text(c.title.clone())
                .font(font(t.type_.body))
                .color(color(t.colors.text))
                .single_line()
                .shrink()
                .truncate(),
        )
        .child(spacer());
    if sel {
        r = r.fill(color(t.colors.selection));
    }
    if let Some(caps) = c.chord.as_deref().and_then(keys::keycaps) {
        r = r.child(Keybind::new(&caps).build(&theme::kit(t)));
    }
    r
}

fn footer(s: &Workbench, shown: usize, total: usize) -> El {
    let t = &s.theme;
    row()
        .key("palette-footer")
        .w_full()
        .px(t.spacing.unit * 1.5)
        .pt(t.spacing.unit)
        .border_sides([0.5, 0.0, 0.0, 0.0], color(t.colors.border))
        .child(
            text(format!("{shown} of {total} commands"))
                .font(font(t.type_.label))
                .color(color(t.colors.text_dim))
                .single_line(),
        )
}
