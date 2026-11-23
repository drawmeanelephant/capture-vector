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
    pub radius: f32,
    pub palette_w: f32,
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
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Theme {
    pub name: String,
    pub mode: Mode,
    pub colors: Colors,
    #[serde(rename = "type")]
    pub type_: Type,
    pub spacing: Spacing,
    pub motion: Motion,
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
        }
    }
    /// Reduced motion: every spring collapses to instant.
    pub fn reduced() -> Self {
        Self {
            reduced: true,
            snap: INSTANT,
            panel: INSTANT,
        }
    }
}

fn type_ramp() -> Type {
    let s = |size, weight, mono| TypeStyle { size, weight, mono };
    Type {
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
        radius: 2.0,
        palette_w: 440.0,
    }
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
            motion: Motion::standard(),
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
            motion: Motion::standard(),
        }
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
        for t in [Theme::dark(), Theme::light()] {
            let json = serde_json::to_string(&t).unwrap();
            let back: Theme = serde_json::from_str(&json).unwrap();
            assert_eq!(t, back);
        }
    }

    #[test]
    fn reduced_motion_is_instant() {
        let t = Theme::for_mode(Mode::Light, true);
        assert!(t.motion.reduced);
        assert_eq!(t.motion.panel.duration, 0.0);
        assert_eq!(t.motion.snap.duration, 0.0);
        assert_eq!(t.mode, Mode::Light);
    }

    #[test]
    fn hex_parses_both_forms() {
        assert_eq!(Rgba::from_hex("#0e1014"), Some(Rgba::rgb(14, 16, 20)));
        assert_eq!(Rgba::from_hex("#00d1ff33"), Some(Rgba(0, 0xd1, 0xff, 0x33)));
        assert_eq!(Rgba::from_hex("nope"), None);
    }
}
