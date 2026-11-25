//! The reading pane: the raised content surface between the list and the
//! inspector. A sticky header carries the breadcrumbs and display title;
//! the draft sketch sits in a dashed advisory frame.

use crate::icons;
use crate::shell::{chip, empty_glow, empty_state, label, pane_header, pane_scroll, tone};
use crate::theme::{self, Size, Styled, color, font};
use cv_model::module::{ReadingModel, Tone};
use cv_model::theme::Theme;
use cv_model::workbench::Workbench;
use rune::{El, col, row, text};
use rune_kit::Markdown;

pub(crate) fn pane(s: &Workbench) -> El {
    let t = &s.theme;
    let pane = col()
        .key("reading")
        .grow(1.0)
        .min_w(0.0)
        .h_full()
        .min_h(0.0)
        .fill(color(t.colors.pane))
        .motion(theme::spring(t.motion.panel));
    let model = s
        .selected_key()
        .and_then(|k| s.registry.get(s.active).map(|m| m.reading(k)));
    match model {
        None => empty_glow(t, pane, 480.0).child(empty_state(
            t,
            "reading-empty",
            &icons::FILE_TEXT,
            "Nothing selected",
            "Pick an item in the list to read it here".into(),
        )),
        Some(m) => pane.child(
            pane_scroll("reading-scroll")
                .scroll_edge_fade(16.0)
                .child(head(t, &m))
                .child(content(t, m)),
        ),
    }
}

/// A reading model as the pane draws it; reading companions share the
/// surface under their own key.
pub(crate) fn frame(t: &Theme, key: &str, model: ReadingModel) -> El {
    col()
        .key(key.to_string())
        .grow(1.0)
        .min_w(0.0)
        .h_full()
        .min_h(0.0)
        .fill(color(t.colors.pane))
        .child(
            pane_scroll("reading-scroll")
                .scroll_edge_fade(16.0)
                .child(head(t, &model))
                .child(content(t, model)),
        )
}

fn head(t: &Theme, m: &ReadingModel) -> El {
    let mut head = pane_header(t, "reading-head", t.colors.pane)
        .px(t.spacing.unit * 4.0)
        .pt(t.spacing.unit * 4.0)
        .pb(t.spacing.unit * 2.0)
        .gap(t.spacing.unit * 1.5);
    if !m.breadcrumbs.is_empty() {
        let mut crumbs = row().key("breadcrumbs").gap(t.spacing.unit);
        for (n, c) in m.breadcrumbs.iter().enumerate() {
            if n > 0 {
                crumbs = crumbs.child(label(t, "›"));
            }
            crumbs = crumbs.child(label(t, c.clone()).key(format!("crumb-{n}")));
        }
        head = head.child(crumbs);
    }
    head.child(
        text(m.title.clone())
            .key("reading-title")
            .font(font(t.type_.display))
            .color(color(t.colors.text))
            .wrap(),
    )
}

fn content(t: &Theme, m: ReadingModel) -> El {
    let mut body = col()
        .key("reading-body")
        .w_full()
        .px(t.spacing.unit * 4.0)
        .pb(t.spacing.unit * 4.0)
        .gap(t.spacing.unit * 1.5);
    for (n, l) in m.lines.into_iter().enumerate() {
        let style = if l.mono { t.type_.mono } else { t.type_.body };
        let mut r = row().key(format!("line-{n}")).gap(t.spacing.unit * 2.0);
        if let Some(lb) = l.label {
            r = r.child(label(t, lb).w(96.0));
        }
        let mut line = text(l.text.clone())
            .font(font(style))
            .color(tone(t, l.tone))
            .wrap()
            .grow(1.0)
            .min_w(0.0);
        if crate::inspector::is_url(&l.text) {
            let url = l.text;
            line = line.role("link").color(color(t.colors.accent)).on_press_tx(
                move |_: &mut Workbench, tx| {
                    let _ = tx.open_url(&url);
                },
            );
        }
        body = body.child(r.child(line));
    }
    if let Some(md) = m.markdown {
        body = body.child(sketch(t, md));
    }
    body
}

/// Draft sketch only: Boris output stays authoritative, so the frame and
/// its chip say so.
fn sketch(t: &Theme, md: String) -> El {
    col()
        .key("sketch")
        .w_full()
        .mt(t.spacing.unit * 2.0)
        .padding(t.spacing.unit * 3.0)
        .gap(t.spacing.unit * 2.0)
        .rounded(t, Size::Md)
        .fill(color(t.colors.pane))
        .border(1.0, theme::tint(t.colors.warn, 0x66))
        .border_dash(4.0, 3.0)
        .child(row().child(chip(
            t,
            "sketch-label",
            "Draft sketch · advisory",
            Tone::Warn,
        )))
        .child(Markdown::new(md).render(&theme::markdown(t)))
}
