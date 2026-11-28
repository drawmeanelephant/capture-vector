//! Complete-value disclosure primitives shared by inspector fields,
//! evidence lists and diagnostic locations.

use crate::icons;
use crate::theme::{self, Size, Styled, color, font};
use cv_model::theme::Theme;
use rune::{El, Key, KeyEvent, Mods, Tx, icon, row, text};
use std::rc::Rc;

pub(crate) const EXPANDER_W: f32 = 18.0;
pub(crate) const EXPANDER_H: f32 = 16.0;

fn line_break(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

/// Whether `v` has more than one logical line for display purposes.
pub(crate) fn multiline(v: &str) -> bool {
    v.contains(line_break)
}

/// The intentionally defined collapsed shape of a multi-line value: its
/// first line only. Expansion is the one place every line appears. A
/// value that opens with blank lines previews its first non-empty one —
/// an empty preview would look like nothing at all.
pub(crate) fn first_line(v: &str) -> &str {
    v.split(line_break).find(|s| !s.is_empty()).unwrap_or("")
}

/// Width `v` takes on one line in the inspector's mono style, measured by
/// the same text system that will render it. During view construction Rune
/// lends the window's text system to `measure`, so CJK, emoji and fallback
/// glyphs are counted by their rendered advances rather than by `chars()`.
pub(crate) fn measured_width(t: &Theme, v: &str) -> f32 {
    rune::text::measure(v, &[], font(t.type_.mono), None).0
}

/// Whether a value needs the complete-value disclosure in `available`
/// points. Any hard line break earns one even when the first line fits:
/// otherwise lines after it would be silently unreachable.
pub(crate) fn needs_disclosure(t: &Theme, v: &str, available: f32) -> bool {
    multiline(v) || measured_width(t, v) > available
}

/// Activate a button-like disclosure on unmodified Space or Enter. Rune's
/// own focus key handling presses Space but deliberately passes Enter on
/// ordinary buttons to the window; consuming both here keeps the shell's
/// Enter-for-companion keymap from firing while a disclosure has focus.
/// Repeats are consumed without toggling repeatedly.
pub(crate) fn activate_on_key<S>(
    s: &mut S,
    ev: &KeyEvent,
    tx: &mut Tx,
    activate: impl FnOnce(&mut S, &mut Tx),
) -> bool {
    let activation =
        matches!(ev.key, Key::Enter) || matches!(&ev.key, Key::Text(text) if text == " ");
    if !activation || ev.mods != Mods::default() {
        return false;
    }
    if !tx.key_repeat() {
        activate(s, tx);
    }
    true
}

fn activation_handler<S: 'static>(
    toggle: Rc<dyn Fn(&mut S)>,
) -> impl Fn(&mut S, &KeyEvent, &mut Tx) -> bool {
    move |s, ev, tx| activate_on_key(s, ev, tx, |s, _| toggle(s))
}

/// The compact complete-value disclosure control: a chevron mirroring the
/// section headers, rotating open while the full value shows inline. It is
/// a real keyboard focus stop, not just a pointer target.
pub(crate) fn expander<S: 'static>(
    t: &Theme,
    key: &str,
    open: bool,
    label: String,
    on_toggle: impl Fn(&mut S) + 'static,
) -> El {
    let toggle = Rc::new(on_toggle);
    let press = toggle.clone();
    row()
        .key(key.to_string())
        .role("button")
        .label(label.clone())
        .help(label)
        .expanded(open)
        .focusable(true)
        .w(EXPANDER_W)
        .h(EXPANDER_H)
        .items_center()
        .justify_center()
        .rounded(t, Size::Sm)
        .states(t)
        .on_key(activation_handler(toggle))
        .on_press(move |s: &mut S| press(s))
        .child(chevron(t, open, 10.0))
}

/// A row-wide disclosure command, used for overflow inside a long section.
/// It shares the compact control's keyboard activation and focus behavior.
pub(crate) fn text_disclosure<S: 'static>(
    t: &Theme,
    key: &str,
    open: bool,
    label: String,
    on_toggle: impl Fn(&mut S) + 'static,
) -> El {
    let toggle = Rc::new(on_toggle);
    let press = toggle.clone();
    row()
        .key(key.to_string())
        .role("button")
        .label(label.clone())
        .help(label.clone())
        .expanded(open)
        .focusable(true)
        .w_full()
        .px(t.spacing.unit)
        .py(t.spacing.unit * 0.5)
        .gap(t.spacing.unit)
        .items_center()
        .rounded(t, Size::Sm)
        .states(t)
        .on_key(activation_handler(toggle))
        .on_press(move |s: &mut S| press(s))
        .child(chevron(t, open, 10.0))
        .child(
            text(label)
                .font(font(t.type_.label))
                .color(color(t.colors.text_dim))
                .single_line()
                .truncate(),
        )
}

fn chevron(t: &Theme, open: bool, size: f32) -> El {
    icon(&icons::CHEVRON_RIGHT, size)
        .color(color(t.colors.text_dim))
        .a11y_hidden()
        .icon_rotation(if open {
            std::f32::consts::FRAC_PI_2
        } else {
            0.0
        })
        .motion(theme::spring(t.motion.snap))
}

pub(crate) fn is_url(v: &str) -> bool {
    (v.starts_with("https://") || v.starts_with("http://")) && !v.contains(char::is_whitespace)
}

/// A value whose meaning sits at both ends: a long hex digest, or a list
/// of them.
pub(crate) fn is_hashy(v: &str) -> bool {
    v.split(|c: char| !c.is_ascii_hexdigit())
        .any(|run| run.len() >= 16)
}

/// A field or list value's collapsed shape: one line, mono, selectable,
/// cut to fit with the whole value on hover. Multi-line values show their
/// first line only; the disclosure control is how every line is reached.
/// Hashes keep both ends, which is how people compare them; web addresses
/// open in the browser.
pub(crate) fn line<S: 'static>(t: &Theme, v: String) -> El {
    let preview = first_line(&v).to_string();
    let mut e = text(preview)
        .font(font(t.type_.mono))
        .single_line()
        .min_w(0.0)
        .selectable(true);
    e = if multiline(&v) {
        e.help(v.clone())
    } else {
        e.help_when_truncated(v.clone())
    };
    if is_url(&v) {
        return e
            .role("link")
            .color(color(t.colors.accent))
            .truncate()
            .on_press_tx(move |_: &mut S, tx| {
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
pub(crate) fn full<S: 'static>(t: &Theme, v: String) -> El {
    let e = text(v.clone())
        .font(font(t.type_.mono))
        .wrap_anywhere()
        .min_w(0.0)
        .selectable(true);
    if is_url(&v) {
        return e
            .role("link")
            .color(color(t.colors.accent))
            .on_press_tx(move |_: &mut S, tx| {
                let _ = tx.open_url(&v);
            });
    }
    e.color(color(t.colors.text))
}

#[cfg(test)]
mod tests {
    use super::{first_line, is_hashy, is_url, measured_width, multiline, needs_disclosure};
    use cv_model::theme::Theme;

    #[test]
    fn collapsed_multiline_preview_is_the_first_line() {
        assert!(multiline("one\ntwo"));
        assert!(multiline("one\r\ntwo"));
        assert!(multiline("one\u{2028}two"));
        assert_eq!(first_line("one\ntwo"), "one");
        assert_eq!(first_line("\ntwo"), "two");
        assert_eq!(first_line("\n\n"), "");
        assert_eq!(first_line(""), "");
        assert!(!multiline("one"));
    }

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

    #[test]
    fn width_eligibility_counts_rendered_wide_glyphs_not_characters() {
        let t = Theme::dark();
        let ascii = measured_width(&t, "aaaaaaaaaa");
        let cjk = measured_width(&t, "日日日日日日日日日日");
        assert!(
            cjk > ascii * 1.5,
            "ten wide glyphs must measure much wider than ten ascii glyphs: {ascii} vs {cjk}"
        );
        assert!(!needs_disclosure(&t, "short", 100.0));
        assert!(needs_disclosure(&t, "short\nlines", 100.0));
    }
}
