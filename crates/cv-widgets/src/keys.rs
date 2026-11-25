//! rune key events → toolkit-neutral `cv_model::workbench::KeyPress`, and
//! display chords → keycaps.

use cv_model::workbench::{KeyInput, KeyPress};
use rune::{Chord, Key, KeyEvent, KeyPlatform};
use std::sync::OnceLock;

pub fn map(ev: &KeyEvent) -> Option<KeyPress> {
    let key = match &ev.key {
        Key::Command(c) => KeyInput::Command(c.to_ascii_lowercase()),
        Key::Text(t) if ev.mods.cmd => KeyInput::Command(t.chars().next()?.to_ascii_lowercase()),
        Key::Text(t) => {
            let mut cs = t.chars();
            let c = cs.next()?;
            if cs.next().is_some() {
                return None;
            }
            KeyInput::Char(c)
        }
        Key::Up => KeyInput::Up,
        Key::Down => KeyInput::Down,
        Key::Enter => KeyInput::Enter,
        Key::Escape => KeyInput::Escape,
        Key::Backspace => KeyInput::Backspace,
        Key::Tab => KeyInput::Tab,
        _ => return None,
    };
    Some(KeyPress {
        key,
        shift: ev.mods.shift,
    })
}

/// The platform whose keycaps the shell draws. Headless runs pin macOS so
/// tree goldens and PNGs match on every OS, as they pin its titlebar.
static KEYCAPS: OnceLock<KeyPlatform> = OnceLock::new();

pub fn pin_keycaps(platform: KeyPlatform) {
    let _ = KEYCAPS.set(platform);
}

/// A display chord such as `"mod+shift+o"` as keycaps ("⇧", "⌘", "O" on
/// macOS); `None` when Rune cannot read it.
pub fn keycaps(chord: &str) -> Option<Vec<String>> {
    let platform = KEYCAPS.get().copied().unwrap_or_else(KeyPlatform::current);
    Chord::parse(chord).ok().map(|c| c.keycaps_on(platform))
}
/// Parse a script token: `"mod+k"`, `"down"`, `"enter"`, `"esc"`, `"tab"`,
/// `"shift+tab"`, `"bksp"`, or literal text (`"text:replay"`).
pub fn parse(token: &str) -> Vec<KeyEvent> {
    use rune::Mods;
    let ev = |key, mods| KeyEvent { key, mods };
    let none = Mods::default();
    if let Some(t) = token.strip_prefix("text:") {
        return t
            .chars()
            .map(|c| ev(Key::Text(c.to_string()), none))
            .collect();
    }
    let (mods, name) = match token.split_once('+') {
        Some(("mod", n)) => (Mods { cmd: true, ..none }, n),
        Some(("shift", n)) => (
            Mods {
                shift: true,
                ..none
            },
            n,
        ),
        _ => (none, token),
    };
    let key = match name {
        "up" => Key::Up,
        "down" => Key::Down,
        "enter" => Key::Enter,
        "esc" => Key::Escape,
        "tab" => Key::Tab,
        "bksp" => Key::Backspace,
        n if mods.cmd && n.chars().count() == 1 => Key::Command(n.chars().next().unwrap_or(' ')),
        n => Key::Text(n.into()),
    };
    vec![ev(key, mods)]
}
