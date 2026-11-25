//! Buttons on the app kit: one place decides how a text or icon button
//! looks, so views only name the action and how loud it is.

use crate::theme::kit;
use cv_model::theme::Theme;
use rune::{El, Icon, Tx};
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
    IconButton::new(icon)
        .variant(IconButtonVariant::GhostMuted)
        .control_size(ControlSize::Small)
        .label(label)
        .help(label)
        .on_press(on_press)
        .build(&kit(t))
        .key(key.to_string())
        .press_scale(PRESS_SCALE)
}
