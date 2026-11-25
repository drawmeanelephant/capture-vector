//! The inspector pane: status chips, fields, diagnostic cards and
//! collapsible sections over the active module's `InspectorModel`.

use crate::shell::{chip, label};
use crate::theme::{self, color, font};
use crate::{controls, diagnostic, icons};
use cv_model::module::Tone;
use cv_model::theme::{Theme, TypeStyle};
use cv_model::workbench::Workbench;
use rune::{El, col, icon, row, text};

/// Items a section lists before it stops; the count chip keeps the total.
const SECTION_MAX: usize = 12;

pub(crate) fn pane(s: &Workbench) -> El {
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
    if let Some(k) = s.selected_key()
        && let Some(m) = s.registry.get(s.active)
    {
        let model = m.inspector(k);
        if !model.badges.is_empty() {
            pane =
                pane.child(
                    row()
                        .key("inspector-badges")
                        .w_full()
                        .flex_wrap()
                        .gap(t.spacing.unit)
                        .children(
                            model.badges.iter().enumerate().map(|(i, b)| {
                                chip(t, &format!("badge-{i}"), b.text.clone(), b.tone)
                            }),
                        ),
                );
        }
        for (name, v) in model.fields {
            pane = pane.child(field(t, &name, v));
        }
        if !model.diagnostics.is_empty() {
            let open = s.section_open("Diagnostics", false);
            pane = pane.child(header(
                t,
                "diagnostics",
                "Diagnostics",
                model.diagnostics.len(),
                open,
            ));
            if open {
                for (n, d) in model.diagnostics.iter().enumerate() {
                    pane = pane.child(diagnostic::card(t, d, n));
                }
            }
        }
        for (n, (title, items)) in model.lists.into_iter().enumerate() {
            let open = s.section_open(&title, items.is_empty());
            pane = pane.child(header(t, &format!("list-{n}"), &title, items.len(), open));
            if open {
                for (j, it) in items.into_iter().take(SECTION_MAX).enumerate() {
                    pane = pane.child(value(t, it).key(format!("list-{n}-{j}")));
                }
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
    if s.debug_windows {
        let n = 1 + s.windows.companions.len();
        pane = pane.child(header(t, "windows", "Windows", n, true));
        let primary = if s.windows.primary.dirty {
            "dirty"
        } else {
            "clean"
        };
        pane = pane.child(field(t, "primary", primary.into()));
        for c in &s.windows.companions {
            pane = pane.child(field(
                t,
                &format!("win{}", c.id.0),
                format!("{}{}", c.title, if c.state.dirty { " *" } else { "" }),
            ));
        }
    }
    pane
}

fn field(t: &Theme, k: &str, v: String) -> El {
    row()
        .key(format!("field-{k}"))
        .w_full()
        .gap(t.spacing.unit * 2.0)
        .child(label(t, k.to_string()).w(72.0))
        .child(value(t, v).grow(1.0))
}

/// A field or list value: mono, selectable, cut to fit with the whole
/// value on hover. Hashes keep both ends, which is how people compare
/// them; web addresses open in the browser.
fn value(t: &Theme, v: String) -> El {
    let e = text(v.clone())
        .font(font(t.type_.mono))
        .single_line()
        .min_w(0.0)
        .selectable(true)
        .help_when_truncated(v.clone());
    if is_url(&v) {
        return e
            .role("link")
            .color(color(t.colors.accent))
            .truncate()
            .on_press_tx(move |_: &mut Workbench, tx| {
                let _ = tx.open_url(&v);
            });
    }
    let e = e.color(color(t.colors.text));
    if is_hashy(&v) {
        e.truncate_middle()
    } else {
        e.truncate()
    }
}

pub(crate) fn is_url(v: &str) -> bool {
    (v.starts_with("https://") || v.starts_with("http://")) && !v.contains(char::is_whitespace)
}

/// A value whose meaning sits at both ends: a long hex digest, or a list
/// of them.
fn is_hashy(v: &str) -> bool {
    v.split(|c: char| !c.is_ascii_hexdigit())
        .any(|run| run.len() >= 16)
}

/// A section's disclosure header: chevron, title, count chip. Pressing it
/// flips the section; an empty one starts closed.
fn header(t: &Theme, key: &str, title: &str, count: usize, open: bool) -> El {
    let name = title.to_string();
    let strong = TypeStyle {
        weight: 600.0,
        ..t.type_.body
    };
    row()
        .key(key.to_string())
        .role("button")
        .label(title.to_string())
        .expanded(open)
        .w_full()
        .mt(t.spacing.unit * 2.0)
        .gap(t.spacing.unit)
        .items_center()
        .on_press(move |s: &mut Workbench| s.toggle_section(&name))
        .child(
            icon(&icons::CHEVRON_RIGHT, 12.0)
                .color(color(t.colors.text_dim))
                .icon_rotation(if open {
                    std::f32::consts::FRAC_PI_2
                } else {
                    0.0
                })
                .motion(theme::spring(t.motion.snap)),
        )
        .child(
            text(title.to_string())
                .font(font(strong))
                .color(color(if count == 0 {
                    t.colors.text_dim
                } else {
                    t.colors.text
                }))
                .single_line(),
        )
        .child(chip(
            t,
            &format!("{key}-count"),
            count.to_string(),
            Tone::Dim,
        ))
}

#[cfg(test)]
mod tests {
    use super::{is_hashy, is_url};

    #[test]
    fn digests_and_lists_of_digests_are_hashy_but_words_are_not() {
        assert!(is_hashy("a8105e30998561e7db41ecb682ff2fef"));
        assert!(is_hashy("sha256:9e18bb8a9e912b15, 0011223344556677"));
        assert!(!is_hashy("stub-build-2"));
        assert!(!is_hashy("deadbeef cafe"));
    }

    #[test]
    fn only_whole_web_addresses_are_links() {
        assert!(is_url("https://llmstxt.org/"));
        assert!(is_url("http://example.org/a?b=c"));
        assert!(!is_url("see https://llmstxt.org/"));
        assert!(!is_url("https://a.org/ and more"));
        assert!(!is_url("file:///etc/hosts"));
    }
}
