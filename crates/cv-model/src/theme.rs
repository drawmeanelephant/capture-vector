//! Theme as data — "Outlook did EDM": dense, keyboard-first, sharp
//! springs, dark + light, reduced-motion variant. Pure tokens; the
//! rune mapping lives in `cv-widgets::theme`.

use serde::{Deserialize, Serialize};

/// sRGB colour as `#rrggbbaa` hex in serialized form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgba(pub u8, pub u8, pub u8, pub u8);

impl Rgba {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self(r, g, b, 0xff)
    }
    pub fn hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}{:02x}", self.0, self.1, self.2, self.3)
    }
    pub fn from_hex(s: &str) -> Option<Self> {
        let s = s.strip_prefix('#')?;
        let b = |i: usize| u8::from_str_radix(s.get(i..i + 2)?, 16).ok();
        match s.len() {
            6 => Some(Self(b(0)?, b(2)?, b(4)?, 0xff)),
            8 => Some(Self(b(0)?, b(2)?, b(4)?, b(6)?)),
            _ => None,
        }
    }
}

impl Serialize for Rgba {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.hex())
    }
}

impl<'de> Deserialize<'de> for Rgba {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::from_hex(&s).ok_or_else(|| serde::de::Error::custom(format!("bad colour {s}")))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Dark,
    Light,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Colors {
    pub bg: Rgba,
    pub rail: Rgba,
    pub pane: Rgba,
    pub pane_alt: Rgba,
    pub border: Rgba,
    pub text: Rgba,
    pub text_dim: Rgba,
    pub accent: Rgba,
    pub accent_text: Rgba,
    pub selection: Rgba,
    pub warn: Rgba,
    pub error: Rgba,
    pub ok: Rgba,
    pub banner: Rgba,
    pub scrim: Rgba,
}

/// Type ramp: (size pt, weight). Dense — 12/13pt body.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TypeStyle {
    pub size: f32,
    pub weight: f32,
    pub mono: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Type {
    /// The reading pane's title: one step above every pane header.
    pub display: TypeStyle,
    pub title: TypeStyle,
    pub body: TypeStyle,
    pub label: TypeStyle,
    pub caption: TypeStyle,
    pub mono: TypeStyle,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Spacing {
    pub unit: f32,
    pub rail_w: f32,
    pub nav_w: f32,
    pub list_w: f32,
    pub inspector_w: f32,
    pub row_h: f32,
    pub palette_w: f32,
}

/// Corner radius scale (points). `continuous` asks for squircle corners.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Shape {
    pub xs: f32,
    pub sm: f32,
    pub md: f32,
    pub lg: f32,
    pub pill: f32,
    pub continuous: bool,
}

/// One box-shadow layer (CSS semantics: blur is the blur radius).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ShadowTok {
    pub x: f32,
    pub y: f32,
    pub blur: f32,
    pub spread: f32,
    pub color: Rgba,
    pub inset: bool,
}

/// Depth levels, each a stack of shadow layers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Elevation {
    /// Cards and chips resting on a pane.
    pub raised: Vec<ShadowTok>,
    /// Menus, toasts, popovers.
    pub floating: Vec<ShadowTok>,
    /// Modal surfaces (command palette).
    pub overlay: Vec<ShadowTok>,
}

/// Interaction overlays: translucent colours painted over a surface.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct States {
    pub hover: Rgba,
    pub press: Rgba,
    pub focus_ring: Rgba,
    /// Selection in a pane that does not have focus.
    pub selection_inactive: Rgba,
}

/// Two-stop gradient: `start` at the top (or left), `end` at the bottom (or right).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GradientTok {
    pub start: Rgba,
    pub end: Rgba,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Gradients {
    pub selection: GradientTok,
    pub rail_active: GradientTok,
    pub banner: GradientTok,
    /// Subtle top-lit sheen over a pane.
    pub pane_sheen: GradientTok,
}

/// A spring as (perceptual duration s, bounce). `duration == 0` = instant.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SpringTok {
    pub duration: f32,
    pub bounce: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Motion {
    pub reduced: bool,
    /// Selection/hover — sharp.
    pub snap: SpringTok,
    /// Panel open/close (inspector, palette).
    pub panel: SpringTok,
    /// Presses and confirmations — a little overshoot.
    pub pop: SpringTok,
    /// Larger panel travel.
    pub glide: SpringTok,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Theme {
    pub name: String,
    pub mode: Mode,
    pub colors: Colors,
    #[serde(rename = "type")]
    pub type_: Type,
    pub spacing: Spacing,
    pub shape: Shape,
    pub elevation: Elevation,
    pub states: States,
    pub gradients: Gradients,
    pub motion: Motion,
    /// Opt-in signature moments (GPU effects, glows, pulses). Off by
    /// default; reduced motion overrides it (see [`Theme::effects_on`]).
    #[serde(default)]
    pub effects: bool,
}

const INSTANT: SpringTok = SpringTok {
    duration: 0.0,
    bounce: 0.0,
};

impl Motion {
    pub fn standard() -> Self {
        Self {
            reduced: false,
            snap: SpringTok {
                duration: 0.12,
                bounce: 0.0,
            },
            panel: SpringTok {
                duration: 0.18,
                bounce: 0.05,
            },
            pop: SpringTok {
                duration: 0.25,
                bounce: 0.15,
            },
            glide: SpringTok {
                duration: 0.32,
                bounce: 0.0,
            },
        }
    }
    /// Reduced motion: every spring collapses to instant.
    pub fn reduced() -> Self {
        Self {
            reduced: true,
            snap: INSTANT,
            panel: INSTANT,
            pop: INSTANT,
            glide: INSTANT,
        }
    }
}

fn type_ramp() -> Type {
    let s = |size, weight, mono| TypeStyle { size, weight, mono };
    Type {
        display: s(16.0, 600.0, false),
        title: s(13.0, 600.0, false),
        body: s(12.0, 400.0, false),
        label: s(11.0, 500.0, false),
        caption: s(10.0, 400.0, false),
        mono: s(11.0, 400.0, true),
    }
}

fn spacing() -> Spacing {
    Spacing {
        unit: 4.0,
        rail_w: 88.0,
        nav_w: 180.0,
        list_w: 320.0,
        inspector_w: 240.0,
        row_h: 22.0,
        palette_w: 440.0,
    }
}

fn shape() -> Shape {
    Shape {
        xs: 2.0,
        sm: 4.0,
        md: 6.0,
        lg: 10.0,
        pill: 999.0,
        continuous: true,
    }
}

const fn shadow(y: f32, blur: f32, spread: f32, color: Rgba) -> ShadowTok {
    ShadowTok {
        x: 0.0,
        y,
        blur,
        spread,
        color,
        inset: false,
    }
}

/// Black drop shadows at per-level alpha `shade`, plus a 1pt hairline `ring`.
fn elevation(shade: [u8; 3], ring: Rgba) -> Elevation {
    let black = |a| Rgba(0, 0, 0, a);
    Elevation {
        raised: vec![shadow(1.0, 2.0, 0.0, black(shade[0]))],
        floating: vec![
            shadow(4.0, 12.0, 0.0, black(shade[1])),
            shadow(0.0, 0.0, 1.0, ring),
        ],
        overlay: vec![
            shadow(12.0, 32.0, 0.0, black(shade[2])),
            shadow(0.0, 0.0, 1.0, ring),
        ],
    }
}

const fn grad(start: Rgba, end: Rgba) -> GradientTok {
    GradientTok { start, end }
}

impl Theme {
    pub fn dark() -> Self {
        Self {
            name: "cv-dark".into(),
            mode: Mode::Dark,
            colors: Colors {
                bg: Rgba::rgb(0x0e, 0x10, 0x14),
                rail: Rgba::rgb(0x08, 0x09, 0x0c),
                pane: Rgba::rgb(0x14, 0x17, 0x1d),
                pane_alt: Rgba::rgb(0x1a, 0x1e, 0x26),
                border: Rgba::rgb(0x26, 0x2b, 0x35),
                text: Rgba::rgb(0xe6, 0xe9, 0xef),
                text_dim: Rgba::rgb(0x8a, 0x93, 0xa3),
                accent: Rgba::rgb(0x00, 0xd1, 0xff),
                accent_text: Rgba::rgb(0x00, 0x10, 0x18),
                selection: Rgba(0x00, 0xd1, 0xff, 0x33),
                warn: Rgba::rgb(0xff, 0xb0, 0x20),
                error: Rgba::rgb(0xff, 0x3d, 0x71),
                ok: Rgba::rgb(0x3d, 0xff, 0x9a),
                banner: Rgba::rgb(0x3a, 0x1a, 0x24),
                scrim: Rgba(0x00, 0x00, 0x00, 0x99),
            },
            type_: type_ramp(),
            spacing: spacing(),
            shape: shape(),
            elevation: elevation([0x66, 0x80, 0xa6], Rgba(0xff, 0xff, 0xff, 0x14)),
            states: States {
                hover: Rgba(0xff, 0xff, 0xff, 0x0f),
                press: Rgba(0xff, 0xff, 0xff, 0x1a),
                focus_ring: Rgba::rgb(0x00, 0xd1, 0xff),
                selection_inactive: Rgba(0xff, 0xff, 0xff, 0x14),
            },
            gradients: Gradients {
                selection: grad(Rgba(0x00, 0xd1, 0xff, 0x40), Rgba(0x00, 0xd1, 0xff, 0x26)),
                rail_active: grad(Rgba(0x00, 0xd1, 0xff, 0x47), Rgba(0x00, 0xd1, 0xff, 0x1f)),
                banner: grad(Rgba::rgb(0x5a, 0x1a, 0x2c), Rgba::rgb(0x3a, 0x1a, 0x24)),
                pane_sheen: grad(Rgba(0xff, 0xff, 0xff, 0x08), Rgba(0xff, 0xff, 0xff, 0x00)),
            },
            motion: Motion::standard(),
            effects: false,
        }
    }

    pub fn light() -> Self {
        Self {
            name: "cv-light".into(),
            mode: Mode::Light,
            colors: Colors {
                bg: Rgba::rgb(0xf4, 0xf5, 0xf7),
                rail: Rgba::rgb(0xe3, 0xe6, 0xeb),
                pane: Rgba::rgb(0xff, 0xff, 0xff),
                pane_alt: Rgba::rgb(0xf0, 0xf2, 0xf5),
                border: Rgba::rgb(0xd3, 0xd8, 0xe0),
                text: Rgba::rgb(0x12, 0x15, 0x1b),
                text_dim: Rgba::rgb(0x5a, 0x63, 0x72),
                accent: Rgba::rgb(0x00, 0x6e, 0xd6),
                accent_text: Rgba::rgb(0xff, 0xff, 0xff),
                selection: Rgba(0x00, 0x6e, 0xd6, 0x26),
                warn: Rgba::rgb(0xa8, 0x62, 0x00),
                error: Rgba::rgb(0xc8, 0x10, 0x44),
                ok: Rgba::rgb(0x0b, 0x8a, 0x4a),
                banner: Rgba::rgb(0xfd, 0xe4, 0xea),
                scrim: Rgba(0x00, 0x00, 0x00, 0x40),
            },
            type_: type_ramp(),
            spacing: spacing(),
            shape: shape(),
            elevation: elevation([0x1f, 0x29, 0x38], Rgba(0x00, 0x00, 0x00, 0x14)),
            states: States {
                hover: Rgba(0x00, 0x00, 0x00, 0x0a),
                press: Rgba(0x00, 0x00, 0x00, 0x14),
                focus_ring: Rgba::rgb(0x00, 0x6e, 0xd6),
                selection_inactive: Rgba(0x00, 0x00, 0x00, 0x0f),
            },
            gradients: Gradients {
                selection: grad(Rgba(0x00, 0x6e, 0xd6, 0x30), Rgba(0x00, 0x6e, 0xd6, 0x1c)),
                rail_active: grad(Rgba(0x00, 0x6e, 0xd6, 0x38), Rgba(0x00, 0x6e, 0xd6, 0x14)),
                banner: grad(Rgba::rgb(0xfb, 0xd0, 0xdb), Rgba::rgb(0xfd, 0xe4, 0xea)),
                pane_sheen: grad(Rgba(0xff, 0xff, 0xff, 0x80), Rgba(0xff, 0xff, 0xff, 0x00)),
            },
            motion: Motion::standard(),
            effects: false,
        }
    }

    /// Effects are drawn only when opted in and motion is not reduced.
    pub fn effects_on(&self) -> bool {
        self.effects && !self.motion.reduced
    }

    pub fn for_mode(mode: Mode, reduced_motion: bool) -> Self {
        let mut t = match mode {
            Mode::Dark => Self::dark(),
            Mode::Light => Self::light(),
        };
        if reduced_motion {
            t.motion = Motion::reduced();
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn themes_roundtrip_as_data() {
        let mut fx = Theme::dark();
        fx.effects = true;
        for t in [Theme::dark(), Theme::light(), fx] {
            let json = serde_json::to_string(&t).unwrap();
            let back: Theme = serde_json::from_str(&json).unwrap();
            assert_eq!(t, back);
            for key in ["shape", "elevation", "states", "gradients", "effects"] {
                assert!(json.contains(&format!("\"{key}\"")), "{key} missing");
            }
            assert_eq!(back.elevation.overlay.len(), 2);
            assert!(back.shape.continuous);
        }
    }

    #[test]
    fn reduced_motion_is_instant() {
        let t = Theme::for_mode(Mode::Light, true);
        assert!(t.motion.reduced);
        assert_eq!(t.motion.panel.duration, 0.0);
        assert_eq!(t.motion.snap.duration, 0.0);
        assert_eq!(t.motion.pop.duration, 0.0);
        assert_eq!(t.motion.glide.duration, 0.0);
        assert_eq!(t.mode, Mode::Light);
        let mut fx = Theme::for_mode(Mode::Dark, true);
        fx.effects = true;
        assert!(!fx.effects_on(), "reduced motion gates effects");
        fx.motion = Motion::standard();
        assert!(fx.effects_on());
    }

    #[test]
    fn hex_parses_both_forms() {
        assert_eq!(Rgba::from_hex("#0e1014"), Some(Rgba::rgb(14, 16, 20)));
        assert_eq!(Rgba::from_hex("#00d1ff33"), Some(Rgba(0, 0xd1, 0xff, 0x33)));
        assert_eq!(Rgba::from_hex("nope"), None);
    }
}
