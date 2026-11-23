//! The one diagnostic surface. Activity and the Pages inspector both
//! render a `cv_core::Diagnostic` through `card`, so the same value
//! produces the same subtree wherever it appears.

use crate::shell::tone;
use crate::theme::{color, font};
use cv_core::diagnostic::Diagnostic;
use cv_model::modules::activity::{location, severity_label, severity_tone};
use cv_model::theme::Theme;
use rune::{El, col, row, text};

pub fn card(t: &Theme, d: &Diagnostic, n: usize) -> El {
    let fg = tone(t, severity_tone(d.severity));
    let build = d
        .build_version
        .as_ref()
        .map_or_else(|| "build —".to_string(), |b| format!("build {}", b.0));
    row()
        .key(format!("diag-{n}"))
        .role("note")
        .w_full()
        .gap(t.spacing.unit)
        .fill(color(t.colors.pane))
        .child(col().key("diag-rail").w(3.0).h_full().fill(fg))
        .child(
            col()
                .key("diag-body")
                .grow(1.0)
                .min_w(0.0)
                .gap(2.0)
                .child(
                    row()
                        .key("diag-head")
                        .gap(t.spacing.unit)
                        .child(
                            text(severity_label(d.severity))
                                .font(font(t.type_.label))
                                .color(fg),
                        )
                        .child(
                            text(d.code.clone())
                                .font(font(t.type_.mono))
                                .color(color(t.colors.text)),
                        ),
                )
                .child(
                    text(location(d))
                        .key("diag-at")
                        .font(font(t.type_.mono))
                        .color(color(t.colors.text_dim))
                        .truncate(),
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
