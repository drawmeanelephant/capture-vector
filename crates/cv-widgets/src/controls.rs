//! Buttons on the app kit: one place decides how a text or icon button
//! looks, so views only name the action.

use crate::theme::kit;
use cv_model::theme::Theme;
use rune::{El, Icon};
use rune_kit::{Button, ButtonVariant, ControlSize, IconButton, IconButtonVariant};

/// A compact text button. `key` is the element key tests click.
pub fn button<S: 'static>(
    t: &Theme,
    key: &str,
    label: &str,
    variant: ButtonVariant,
    on_press: impl Fn(&mut S) + 'static,
) -> El {
    Button::new(label)
        .variant(variant)
        .control_size(ControlSize::Small)
        .on_press(on_press)
        .build(&kit(t))
        .key(key.to_string())
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
}
