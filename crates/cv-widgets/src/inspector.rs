//! The inspector pane: status chips, fields, diagnostic cards and
//! collapsible sections over the active module's `InspectorModel`.

use crate::icons;
use crate::shell::{chip, label, pane_header, pane_scroll};
use crate::theme::{self, color, font};
use crate::{controls, diagnostic, value};
use cv_model::module::Tone;
use cv_model::theme::{Theme, TypeStyle};
use cv_model::workbench::Workbench;
use rune::{Edge, El, Transition, col, icon, row, spacer, text};

/// Items a section previews before the rest move behind an overflow
/// disclosure; the count chip always keeps the total.
const SECTION_MAX: usize = 12;
/// Legacy conservative affordance thresholds. Measurement is authoritative
/// for wide glyphs; these keep the PR #244 control on long-by-count values
/// even when a proportional face happens to fit them.
const FIELD_FITS: usize = 20;
const ITEM_FITS: usize = 32;
/// Width of a field row's label column.
const LABEL_W: f32 = 72.0;
/// Conservative chrome allowance (pane hairline and fractional borders).
const CHROME_SLACK: f32 = 2.0;

/// The inspector body width inside its horizontal padding.
fn content_width(t: &Theme) -> f32 {
    (t.spacing.inspector_w - t.spacing.unit * 4.0 - CHROME_SLACK).max(0.0)
}

/// Width a field value gets once its disclosure control is mounted. Using
/// the collapsed width is deliberately conservative: a value that fits
/// even without the control may keep the affordance, but a value that
/// would clip never goes without it.
fn field_disclosure_width(t: &Theme) -> f32 {
    (content_width(t) - LABEL_W - t.spacing.unit * 4.0 - value::EXPANDER_W).max(0.0)
}

/// Width a list value gets once its disclosure control is mounted.
fn item_disclosure_width(t: &Theme) -> f32 {
    (content_width(t) - t.spacing.unit - value::EXPANDER_W).max(0.0)
}

/// Width a diagnostic card's location gets inside the rail, border, card
/// padding and disclosure control.
fn diagnostic_disclosure_width(t: &Theme) -> f32 {
    (content_width(t) - 3.0 - t.spacing.unit * 5.0 - value::EXPANDER_W - CHROME_SLACK).max(0.0)
}

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
        .role("complementary")
        .label("Inspector")
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
                    let element = format!("diag-{n}-at");
                    let state = s.expansion_key(k, &element);
                    let open = s.value_expanded(&state);
                    let toggle = state.clone();
                    pane = pane.child(diagnostic::card_disclosed::<Workbench>(
                        t,
                        d,
                        n,
                        diagnostic_disclosure_width(t),
                        open,
                        move |s: &mut Workbench| s.toggle_value(toggle.clone()),
                    ));
                }
            }
        }
        for (n, (title, items)) in model.lists.into_iter().enumerate() {
            let open = s.section_open(&title, items.is_empty());
            let section_key = format!("list-{n}");
            pane = pane.child(header(t, &section_key, &title, items.len(), open));
            if open {
                let len = items.len();
                let mut entries = items.into_iter().enumerate();
                for (j, it) in entries.by_ref().take(SECTION_MAX) {
                    pane = pane.child(item(s, k, &format!("list-{n}-{j}"), it));
                }
                if len > SECTION_MAX {
                    let tail: Vec<_> = entries.collect();
                    pane = pane.child(overflow(s, k, &section_key, len - SECTION_MAX));
                    let overflow_state = s.expansion_key(k, &format!("{section_key}-overflow"));
                    if s.value_expanded(&overflow_state) {
                        for (j, it) in tail {
                            pane = pane.child(item(s, k, &format!("list-{n}-{j}"), it));
                        }
                    }
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

/// The overflow control that makes entries past the preview reachable.
fn overflow(s: &Workbench, noun: &str, section_key: &str, hidden: usize) -> El {
    let element = format!("{section_key}-overflow");
    let state = s.expansion_key(noun, &element);
    let open = s.value_expanded(&state);
    let toggle = state.clone();
    let label = if open {
        "Show fewer".to_string()
    } else {
        format!("Show {hidden} more")
    };
    value::text_disclosure::<Workbench>(
        &s.theme,
        &element,
        open,
        label,
        move |s: &mut Workbench| s.toggle_value(toggle.clone()),
    )
}

/// A field row: label beside value. A value measured to fit the column
/// renders as one line; a longer or multi-line one truncates behind a
/// disclosure control that expands it inline, wrapped over the pane's
/// full width, so evidence like paths, argv and digests is readable and
/// selectable without leaving the app.
fn field(s: &Workbench, noun: &str, k: &str, v: String) -> El {
    let t = &s.theme;
    let key = format!("field-{k}");
    if !(v.chars().count() > FIELD_FITS
        || value::needs_disclosure(t, &v, field_disclosure_width(t)))
    {
        return row()
            .key(key)
            .w_full()
            .gap(t.spacing.unit * 2.0)
            .child(label(t, k.to_string()).w(LABEL_W))
            .child(value::line::<Workbench>(t, v).grow(1.0));
    }
    let state = s.expansion_key(noun, &key);
    let open = s.value_expanded(&state);
    let middle = if open {
        spacer()
    } else {
        value::line::<Workbench>(t, v.clone()).grow(1.0)
    };
    let toggle = state.clone();
    let what = if open { "Hide" } else { "Show" };
    let mut f = col().key(key.clone()).w_full().gap(t.spacing.unit);
    f = f.child(
        row()
            .key(format!("{key}-head"))
            .w_full()
            .gap(t.spacing.unit * 2.0)
            .items_center()
            .child(label(t, k.to_string()).w(LABEL_W))
            .child(middle)
            .child(value::expander::<Workbench>(
                t,
                &format!("{key}-expand"),
                open,
                format!("{what} the full {k} value"),
                move |s: &mut Workbench| s.toggle_value(toggle.clone()),
            )),
    );
    if open {
        f = f.child(
            value::full::<Workbench>(t, v)
                .key(format!("{key}-full"))
                .w_full(),
        );
    }
    f
}

/// A section list item: bare text when it fits the pane, otherwise the
/// same disclosure a field gets — the full text wraps in place.
fn item(s: &Workbench, noun: &str, key: &str, v: String) -> El {
    let t = &s.theme;
    if !(v.chars().count() > ITEM_FITS || value::needs_disclosure(t, &v, item_disclosure_width(t)))
    {
        return value::line::<Workbench>(t, v).key(key.to_string());
    }
    let state = s.expansion_key(noun, key);
    let open = s.value_expanded(&state);
    let body = if open {
        value::full::<Workbench>(t, v)
    } else {
        value::line::<Workbench>(t, v)
    };
    let toggle = state.clone();
    let what = if open { "Hide" } else { "Show" };
    row()
        .key(key.to_string())
        .w_full()
        .gap(t.spacing.unit)
        .items_start()
        .child(body.grow(1.0).min_w(0.0))
        .child(value::expander::<Workbench>(
            t,
            &format!("{key}-expand"),
            open,
            format!("{what} the full value"),
            move |s: &mut Workbench| s.toggle_value(toggle.clone()),
        ))
}

/// A section's disclosure header: chevron, title, count chip. It is a
/// keyboard focus stop like the value disclosures; pressing it flips the
/// section, and an empty one starts closed.
fn header(t: &Theme, key: &str, title: &str, count: usize, open: bool) -> El {
    let name = title.to_string();
    let key_toggle = name.clone();
    let press = name.clone();
    let strong = TypeStyle {
        weight: 600.0,
        ..t.type_.body
    };
    row()
        .key(key.to_string())
        .role("button")
        .label(title.to_string())
        .expanded(open)
        .focusable(true)
        .w_full()
        .mt(t.spacing.unit * 2.0)
        .gap(t.spacing.unit)
        .items_center()
        .on_key(move |s: &mut Workbench, ev, tx| {
            value::activate_on_key(s, ev, tx, |s: &mut Workbench| s.toggle_section(&key_toggle))
        })
        .on_press(move |s: &mut Workbench| s.toggle_section(&press))
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
