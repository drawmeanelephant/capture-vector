//! Buttons on the app kit: one place decides how a text or icon button
//! looks, so views only name the action and how loud it is.

use crate::theme::kit;
use cv_model::theme::Theme;
use rune::{El, Icon, Tx, row, text};
use rune_kit::{Button, ButtonVariant, ControlSize, IconButton, IconButtonVariant};

/// How loud a button is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    /// The one action a surface exists for, on the accent.
    Primary,
    Neutral,
    /// Secondary actions that should not compete with content.
    Ghost,
    /// Destroys work: the error tone.
    Danger,
}

/// Held buttons shrink this much; Rune keeps them at 1 under reduced motion.
pub const PRESS_SCALE: f32 = 0.97;

fn kit_button(label: &str, v: Variant) -> Button {
    let kv = match v {
        Variant::Primary => ButtonVariant::Contrast,
        Variant::Neutral => ButtonVariant::Neutral,
        Variant::Ghost => ButtonVariant::Ghost,
        Variant::Danger => ButtonVariant::Danger,
    };
    Button::new(label)
        .variant(kv)
        .control_size(ControlSize::Small)
}

fn finish(t: &Theme, key: &str, b: Button, enabled: bool) -> El {
    let e = b.build(&kit(t)).key(key.to_string());
    if enabled {
        e.press_scale(PRESS_SCALE)
    } else {
        e
    }
}

/// A compact text button. `key` is the element key tests click.
pub fn button<S: 'static>(
    t: &Theme,
    key: &str,
    label: &str,
    v: Variant,
    on_press: impl Fn(&mut S) + 'static,
) -> El {
    finish(t, key, kit_button(label, v).on_press(on_press), true)
}

/// [`button`] whose handler needs the transaction (closing a window), and
/// which can be disabled: dimmed, unfocusable, with no handler.
pub fn button_tx<S: 'static>(
    t: &Theme,
    key: &str,
    label: &str,
    v: Variant,
    enabled: bool,
    on_press: impl Fn(&mut S, &mut Tx) + 'static,
) -> El {
    let b = kit_button(label, v);
    let b = if enabled {
        b.on_press_tx(on_press)
    } else {
        b.disabled(true)
    };
    finish(t, key, b, enabled)
}

/// A small ghost icon button; `label` names it for assistive tech and is
/// its tooltip.
pub fn icon_button<S: 'static>(
    t: &Theme,
    key: &str,
    icon: &'static Icon,
    label: &str,
    on_press: impl Fn(&mut S) + 'static,
) -> El {
    icon_button_tx(t, key, icon, label, None, true, move |s: &mut S, _| {
        on_press(s)
    })
}

/// [`icon_button`] with an optional shortcut hint in its tooltip, which
/// can be disabled: dimmed, unfocusable, with no handler.
pub fn icon_button_tx<S: 'static>(
    t: &Theme,
    key: &str,
    icon: &'static Icon,
    label: &str,
    chord: Option<&str>,
    enabled: bool,
    on_press: impl Fn(&mut S, &mut Tx) + 'static,
) -> El {
    let mut b = IconButton::new(icon)
        .variant(IconButtonVariant::GhostMuted)
        .control_size(ControlSize::Small)
        .label(label)
        .help(label);
    b = if enabled {
        b.on_press_tx(on_press)
    } else {
        b.disabled(true)
    };
    let e = with_shortcut(b.build(&kit(t)), label, chord).key(key.to_string());
    if enabled {
        e.press_scale(PRESS_SCALE)
    } else {
        e
    }
}

/// `label  ⌘S` as the tooltip, and the chord for assistive tech.
fn with_shortcut(e: El, label: &str, chord: Option<&str>) -> El {
    match chord.and_then(crate::keys::shortcut_text) {
        Some(keys) => e.help(format!("{label}  {keys}")).shortcut_hint(keys),
        None => e,
    }
}

/// [`button_tx`] that shows its shortcut: the chord's keycaps after the
/// label, also named in its tooltip and to assistive tech.
pub fn button_keys_tx<S: 'static>(
    t: &Theme,
    key: &str,
    label: &str,
    v: Variant,
    chord: &str,
    enabled: bool,
    on_press: impl Fn(&mut S, &mut Tx) + 'static,
) -> El {
    let caps = crate::keys::shortcut_text(chord).unwrap_or_default();
    let words = label.to_string();
    let b = kit_button(label, v).label_with(move |font, ink| {
        row()
            .gap(6.0)
            .center_y()
            .child(text(words).key("label").font(font).color(ink))
            .child(text(caps).key("keys").font(font).color(ink.alpha(0.7)))
    });
    let b = if enabled {
        b.on_press_tx(on_press)
    } else {
        b.disabled(true)
    };
    with_shortcut(finish(t, key, b, enabled), label, Some(chord))
}
