//! The one diagnostic surface. Activity and the Pages inspector both
//! render a `cv_core::Diagnostic` through `card`, so the same value
//! produces the same subtree wherever it appears.

use crate::shell::{chip, tone_token};
use crate::theme::{Size, Styled, color, font, radius, tint};
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
                .child(
                    text(at.clone())
                        .key("diag-at")
                        .font(font(t.type_.mono))
                        .color(color(t.colors.text_dim))
                        .single_line()
                        .truncate_middle()
                        .help_when_truncated(at),
                )
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
