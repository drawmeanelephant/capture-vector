//! The one diagnostic surface. Activity and the Pages inspector both
//! render a `cv_core::Diagnostic` through `card`, so the same value
//! produces the same subtree wherever it appears.

use crate::shell::{chip, tone_token};
use crate::theme::{Size, Styled, color, font, radius, tint};
use crate::value;
use cv_core::diagnostic::Diagnostic;
use cv_model::module::Tone;
use cv_model::modules::activity::{location, severity_label, severity_tone};
use cv_model::theme::Theme;
use rune::{El, col, row, text};

/// A severity-tinted card: a rail in the severity colour, then severity
/// and code chips, the location (both ends kept), the message and the
/// build that reported it.
pub fn card(t: &Theme, d: &Diagnostic, n: usize) -> El {
    card_in(t, d, n, severity_tone(d.severity))
}

/// [`card`] tinted `tn` instead of its severity: the editor draws a
/// diagnostic from before the latest edits in `Tone::Dim`.
pub fn card_in(t: &Theme, d: &Diagnostic, n: usize, tn: Tone) -> El {
    card_render(t, d, n, tn, collapsed_location)
}

/// [`card`] whose location uses the inspector's complete-value disclosure
/// when the measured location cannot fit (or has more than one line).
/// `available` is the card's no-control text width in points; ownership of
/// `open` and the toggle stays with the caller's state type.
pub fn card_disclosed<S: 'static>(
    t: &Theme,
    d: &Diagnostic,
    n: usize,
    available: f32,
    open: bool,
    on_toggle: impl Fn(&mut S) + 'static,
) -> El {
    card_render(t, d, n, severity_tone(d.severity), move |t, at| {
        if !value::needs_disclosure(t, &at, available) {
            return collapsed_location(t, at);
        }
        let body = if open {
            value::full::<S>(t, at.clone()).key(format!("diag-{n}-at-full"))
        } else {
            value::line::<S>(t, at.clone()).key("diag-at")
        };
        let what = if open { "Hide" } else { "Show" };
        row()
            .key(format!("diag-{n}-at-row"))
            .w_full()
            .gap(t.spacing.unit)
            .items_start()
            .child(body.grow(1.0).min_w(0.0))
            .child(value::expander::<S>(
                t,
                &format!("diag-{n}-at-expand"),
                open,
                format!("{what} the full diagnostic location"),
                on_toggle,
            ))
    })
}

fn collapsed_location(t: &Theme, at: String) -> El {
    text(at.clone())
        .key("diag-at")
        .font(font(t.type_.mono))
        .color(color(t.colors.text_dim))
        .single_line()
        .truncate_middle()
        .help_when_truncated(at)
}

fn card_render(
    t: &Theme,
    d: &Diagnostic,
    n: usize,
    tn: Tone,
    location_el: impl FnOnce(&Theme, String) -> El,
) -> El {
    let fg = tone_token(t, tn);
    let r = radius(t, Size::Md);
    let build = d
        .build_version
        .as_ref()
        .map_or_else(|| "build —".to_string(), |b| format!("build {}", b.0));
    let at = location(d);
    row()
        .key(format!("diag-{n}"))
        .role("note")
        .w_full()
        .fill(tint(fg, 0x1a))
        .border(0.5, tint(fg, 0x4d))
        .rounded(t, Size::Md)
        .child(
            col()
                .key("diag-rail")
                .w(3.0)
                .h_full()
                .fill(color(fg))
                .corners([r, 0.0, 0.0, r]),
        )
        .child(
            col()
                .key("diag-body")
                .grow(1.0)
                .min_w(0.0)
                .padding(t.spacing.unit * 2.0)
                .gap(t.spacing.unit)
                .child(
                    row()
                        .key("diag-head")
                        .gap(t.spacing.unit)
                        .items_center()
                        .child(chip(t, "diag-severity", severity_label(d.severity), tn))
                        .child(chip(t, "diag-code", d.code.clone(), Tone::Normal)),
                )
                .child(location_el(t, at))
                .child(
                    text(d.message.clone())
                        .key("diag-msg")
                        .font(font(t.type_.body))
                        .color(color(t.colors.text))
                        .wrap(),
                )
                .child(
                    text(build)
                        .key("diag-build")
                        .font(font(t.type_.label))
                        .color(color(t.colors.text_dim)),
                ),
        )
}
