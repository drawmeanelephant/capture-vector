//! Buttons on the app kit: one place decides how a text or icon button
//! looks, so views only name the action and how loud it is.

use crate::theme::kit;
use cv_model::theme::Theme;
use rune::{El, Icon, Tx, row, text};
use rune_kit::{
    Button, ButtonVariant, ControlSize, IconButton, IconButtonVariant, Keybind, KeybindVariant,
};

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

fn finish(key: &str, e: El, enabled: bool) -> El {
    let e = e.key(key.to_string());
    if enabled {
        e.press_scale(PRESS_SCALE)
    } else {
        e
    }
}

/// The tooltip and assistive-technology hint for a command's `chord`.
/// Rune Kit's own `shortcut` formats keys for the build target, which
/// headless runs cannot pin to one platform.
fn with_chord(e: El, label: &str, chord: &str) -> El {
    match crate::keys::chord_label(chord) {
        Some(keys) => e.help(format!("{label}  {keys}")).shortcut_hint(keys),
        None => e,
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
    finish(
        key,
        kit_button(label, v).on_press(on_press).build(&kit(t)),
        true,
    )
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
    finish(key, b.build(&kit(t)), enabled)
}

/// [`button_tx`] for a command with a shortcut: `chord` (`"mod+s"`) rides
/// in the label as keycaps and in the tooltip.
pub fn command_button<S: 'static>(
    t: &Theme,
    key: &str,
    label: &str,
    v: Variant,
    chord: &str,
    enabled: bool,
    on_press: impl Fn(&mut S, &mut Tx) + 'static,
) -> El {
    let k = kit(t);
    let mut b = kit_button(label, v);
    if let Some(caps) = crate::keys::keycaps(chord) {
        let (name, caps_kit) = (label.to_string(), k.clone());
        b = b.label_with(move |font, ink| {
            row()
                .gap(6.0)
                .center_y()
                .child(text(name).key("label").font(font).color(ink))
                .child(
                    Keybind::new(&caps)
                        .variant(KeybindVariant::Ghost)
                        .build(&caps_kit)
                        .key("keys")
                        .opacity(0.8),
                )
        });
    }
    let b = if enabled {
        b.on_press_tx(on_press)
    } else {
        b.disabled(true)
    };
    finish(key, with_chord(b.build(&k), label, chord), enabled)
}

fn kit_icon_button(icon: &'static Icon, label: &str) -> IconButton {
    IconButton::new(icon)
        .variant(IconButtonVariant::GhostMuted)
        .control_size(ControlSize::Small)
        .label(label)
        .help(label)
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
    let b = kit_icon_button(icon, label).on_press(on_press);
    finish(key, b.build(&kit(t)), true)
}

/// [`icon_button`] for a command with a shortcut (`"mod+z"`) in its
/// tooltip, which can be disabled: dimmed, unfocusable, with no handler.
pub fn icon_command<S: 'static>(
    t: &Theme,
    key: &str,
    icon: &'static Icon,
    label: &str,
    chord: &str,
    enabled: bool,
    on_press: impl Fn(&mut S) + 'static,
) -> El {
    let b = kit_icon_button(icon, label);
    let b = if enabled {
        b.on_press(on_press)
    } else {
        b.disabled(true)
    };
    finish(key, with_chord(b.build(&kit(t)), label, chord), enabled)
}
