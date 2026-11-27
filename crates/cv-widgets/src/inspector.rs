//! The inspector pane: status chips, fields, diagnostic cards and
//! collapsible sections over the active module's `InspectorModel`.

use crate::shell::{chip, label, pane_header, pane_scroll};
use crate::theme::{self, Size, Styled, color, font};
use crate::{controls, diagnostic, icons};
use cv_model::module::Tone;
use cv_model::theme::{Theme, TypeStyle};
use cv_model::workbench::Workbench;
use rune::{Edge, El, Transition, col, icon, row, spacer, text};

/// Items a section lists before it stops; the count chip keeps the total.
const SECTION_MAX: usize = 12;

/// A field's value column sits beside its 72pt label in the fixed-width
/// pane: 144pt, twenty-odd mono glyphs at 11pt, fewer on a wide platform
/// face. Longer values can clip, so they earn the disclosure control
/// that expands them inline; a control on a value that happens to fit
/// costs nothing, so the count errs low.
const FIELD_FITS: usize = 20;
/// A list row spans the pane's whole width before it clips.
const ITEM_FITS: usize = 32;

pub(crate) fn pane(s: &Workbench) -> El {
    let t = &s.theme;
    let head = pane_header(t, "inspector-head", t.colors.bg)
        .px(t.spacing.unit * 2.0)
        .pt(t.spacing.unit * 2.0)
        .pb(t.spacing.unit)
        .child(
            text("Inspector")
                .font(font(t.type_.title))
                .color(color(t.colors.text)),
        );
    col()
        .key("inspector")
        .w(t.spacing.inspector_w)
        .h_full()
        .min_h(0.0)
        .fill(color(t.colors.bg))
        .border_sides([0.0, 0.0, 0.0, 0.5], color(t.colors.border))
        .transition(Transition::move_edge(Edge::Trailing))
        .motion(theme::spring(t.motion.panel))
        .child(pane_scroll("inspector-scroll").child(head).child(body(s)))
}

fn body(s: &Workbench) -> El {
    let t = &s.theme;
    let mut pane = col()
        .key("inspector-body")
        .w_full()
        .px(t.spacing.unit * 2.0)
        .pb(t.spacing.unit * 2.0)
        .gap(t.spacing.unit);
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
            pane = pane.child(field(s, k, &name, v));
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
                    pane = pane.child(item(s, k, &format!("list-{n}-{j}"), it));
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
        pane = pane.child(field(s, "windows", "primary", primary.into()));
        for c in &s.windows.companions {
            pane = pane.child(field(
                s,
                "windows",
                &format!("win{}", c.id.0),
                format!("{}{}", c.title, if c.state.dirty { " *" } else { "" }),
            ));
        }
    }
    pane
}

/// A field row: label beside value. A value short enough to fit the
/// column renders as one line; a longer one truncates behind a
/// disclosure control that expands it inline, wrapped over the pane's
/// full width, so evidence like paths, argv and digests is readable and
/// selectable without leaving the app.
fn field(s: &Workbench, noun: &str, k: &str, v: String) -> El {
    let t = &s.theme;
    let key = format!("field-{k}");
    if v.chars().count() <= FIELD_FITS {
        return row()
            .key(key)
            .w_full()
            .gap(t.spacing.unit * 2.0)
            .child(label(t, k.to_string()).w(72.0))
            .child(value_line(t, v).grow(1.0));
    }
    let state = format!("{noun}\u{1f}{key}");
    let open = s.value_expanded(&state);
    let middle = if open {
        spacer()
    } else {
        value_line(t, v.clone()).grow(1.0)
    };
    let mut f = col().key(key.clone()).w_full().gap(t.spacing.unit);
    f = f.child(
        row()
            .key(format!("{key}-head"))
            .w_full()
            .gap(t.spacing.unit * 2.0)
            .items_center()
            .child(label(t, k.to_string()).w(72.0))
            .child(middle)
            .child(expander(t, &format!("{key}-expand"), &state, open, Some(k))),
    );
    if open {
        f = f.child(value_full(t, v).key(format!("{key}-full")).w_full());
    }
    f
}

/// A section list item: bare text when it fits the pane, otherwise the
/// same disclosure a field gets — the full text wraps in place.
fn item(s: &Workbench, noun: &str, key: &str, v: String) -> El {
    let t = &s.theme;
    if v.chars().count() <= ITEM_FITS {
        return value_line(t, v).key(key.to_string());
    }
    let state = format!("{noun}\u{1f}{key}");
    let open = s.value_expanded(&state);
    let body = if open {
        value_full(t, v)
    } else {
        value_line(t, v)
    };
    row()
        .key(key.to_string())
        .w_full()
        .gap(t.spacing.unit)
        .items_start()
        .child(body.grow(1.0).min_w(0.0))
        .child(expander(t, &format!("{key}-expand"), &state, open, None))
}

/// The disclosure control a long value earns: a chevron mirroring the
/// section headers, rotating open while the full value shows inline.
fn expander(t: &Theme, key: &str, state: &str, open: bool, name: Option<&str>) -> El {
    let what = if open { "Hide" } else { "Show" };
    let label = match name {
        Some(n) => format!("{what} the full {n} value"),
        None => format!("{what} the full value"),
    };
    let state = state.to_string();
    row()
        .key(key.to_string())
        .label(label.clone())
        .help(label)
        .expanded(open)
        .w(18.0)
        .h(16.0)
        .items_center()
        .justify_center()
        .rounded(t, Size::Sm)
        .states(t)
        .on_press(move |s: &mut Workbench| s.toggle_value(&state))
        .child(
            icon(&icons::CHEVRON_RIGHT, 10.0)
                .color(color(t.colors.text_dim))
                .icon_rotation(if open {
                    std::f32::consts::FRAC_PI_2
                } else {
                    0.0
                })
                .motion(theme::spring(t.motion.snap)),
        )
}

/// A field or list value's collapsed shape: one line, mono, selectable,
/// cut to fit with the whole value on hover. Hashes keep both ends,
/// which is how people compare them; web addresses open in the browser.
fn value_line(t: &Theme, v: String) -> El {
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

/// A value's expanded shape: the whole text wrapped inside the pane,
/// still mono and selectable. `wrap_anywhere` breaks inside a path or
/// digest whose glyphs alone would overflow the line.
fn value_full(t: &Theme, v: String) -> El {
    let e = text(v.clone())
        .font(font(t.type_.mono))
        .wrap_anywhere()
        .min_w(0.0)
        .selectable(true);
    if is_url(&v) {
        return e.role("link").color(color(t.colors.accent)).on_press_tx(
            move |_: &mut Workbench, tx| {
                let _ = tx.open_url(&v);
            },
        );
    }
    e.color(color(t.colors.text))
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
