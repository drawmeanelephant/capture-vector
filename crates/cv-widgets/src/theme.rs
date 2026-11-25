//! Theme tokens (data, in cv-model) → rune values. Views style through
//! this module (and [`Styled`]) instead of picking radii, shadows or
//! hover colours themselves.

use cv_model::theme::{Rgba, ShadowTok, SpringTok, Theme, TypeStyle};
use rune::{Color, Cx, El, Family, Shadow, Spring, TextStyle};
use rune_kit::{Kit, MarkdownStyle, PaletteFocus, PaletteSeed};
use std::cell::RefCell;
use std::rc::Rc;

pub fn color(c: Rgba) -> Color {
    rune::color::hex(&c.hex()).unwrap_or(Color::TRANSPARENT)
}

pub fn font(t: TypeStyle) -> TextStyle {
    let mut s = TextStyle::new(t.size, t.weight);
    if t.mono {
        s.family = Family::Mono;
    }
    s
}

pub fn spring(s: SpringTok) -> Spring {
    if s.duration <= 0.0 {
        Spring::NONE
    } else {
        Spring::new(s.duration, s.bounce)
    }
}

pub fn shadow(s: &ShadowTok) -> Shadow {
    Shadow {
        x: s.x,
        y: s.y,
        blur: s.blur,
        spread: s.spread,
        color: color(s.color),
        inset: s.inset,
    }
}

pub fn shadows(layers: &[ShadowTok]) -> Vec<Shadow> {
    layers.iter().map(shadow).collect()
}

/// `c` at `alpha` (0–255), for tints derived from a token colour.
pub fn tint(c: Rgba, alpha: u8) -> Color {
    color(Rgba(c.0, c.1, c.2, alpha))
}

/// A step on the theme's radius scale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Size {
    Xs,
    Sm,
    Md,
    Lg,
    Pill,
}

pub fn radius(t: &Theme, s: Size) -> f32 {
    let sh = &t.shape;
    match s {
        Size::Xs => sh.xs,
        Size::Sm => sh.sm,
        Size::Md => sh.md,
        Size::Lg => sh.lg,
        Size::Pill => sh.pill,
    }
}

/// A depth level from the theme's elevation tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Raised,
    Floating,
    Overlay,
}

pub fn elevation(t: &Theme, l: Level) -> &[ShadowTok] {
    let e = &t.elevation;
    match l {
        Level::Raised => &e.raised,
        Level::Floating => &e.floating,
        Level::Overlay => &e.overlay,
    }
}

/// Token-driven styling for any element.
pub trait Styled: Sized {
    /// Corner radius from the shape scale (continuous corners when the theme asks).
    fn rounded(self, t: &Theme, s: Size) -> Self;
    /// Shadow stack for a depth level.
    fn elevated(self, t: &Theme, l: Level) -> Self;
    /// Hover and press overlays over the element's own fill.
    fn states(self, t: &Theme) -> Self;
}

impl Styled for El {
    fn rounded(self, t: &Theme, s: Size) -> Self {
        let e = self.radius(radius(t, s));
        if t.shape.continuous {
            e.continuous_corners()
        } else {
            e
        }
    }

    fn elevated(self, t: &Theme, l: Level) -> Self {
        elevation(t, l)
            .iter()
            .fold(self, |e, s| e.box_shadow(shadow(s)))
    }

    fn states(self, t: &Theme) -> Self {
        self.hover(color(t.states.hover))
            .press(color(t.states.press))
    }
}

/// Apply the theme's reduced-motion switch to Rune's own motion (press
/// scale, transitions, springs). It can only add reduction: the OS
/// preference still holds when the theme leaves motion on.
pub fn motion_preference<S>(cx: &mut Cx<S>, t: &Theme) {
    if t.motion.reduced {
        cx.reduced_motion(true);
    }
}

/// rune-kit's tokens rebuilt from the app theme, so kit components
/// (Markdown sketch, buttons, toasts) wear our palette, radii, depth and
/// springs. Cached: the theme only changes on the mode/motion toggles.
pub fn kit(t: &Theme) -> Rc<Kit> {
    thread_local! {
        static KIT: RefCell<Option<(Theme, Rc<Kit>)>> = const { RefCell::new(None) };
    }
    KIT.with(|cell| {
        let mut cell = cell.borrow_mut();
        if let Some((theme, k)) = cell.as_ref()
            && theme == t
        {
            return k.clone();
        }
        let k = Rc::new(build_kit(t));
        *cell = Some((t.clone(), k.clone()));
        k
    })
}

fn build_kit(t: &Theme) -> Kit {
    let c = &t.colors;
    let seed = PaletteSeed {
        bg: color(c.bg),
        fg: color(c.text),
        accent: color(c.accent),
    };
    let mut k = Kit::from_palette_focus(seed, PaletteFocus::Custom(color(t.states.focus_ring)));
    let p = &mut k.c;
    p.page = color(c.bg);
    p.bg_base = color(c.bg);
    p.bg_deep = color(c.rail);
    p.layer_01 = color(c.pane_alt);
    p.layer_02 = color(c.pane);
    p.bg_accent = color(c.accent);
    // Kit "contrast" buttons are our primary buttons: they sit on the accent.
    p.bg_contrast = color(c.accent);
    p.text_contrast = color(c.accent_text);
    p.contrast_hover = tint(c.accent_text, 0x1f);
    p.contrast_pressed = tint(c.accent_text, 0x38);
    p.text_base = color(c.text);
    p.text_strong = color(c.text);
    p.text_muted = color(c.text_dim);
    p.text_accent = color(c.accent);
    p.icon_base = color(c.text);
    p.icon_muted = color(c.text_dim);
    p.icon_accent = color(c.accent);
    p.border_muted = color(c.border);
    p.border_base = color(c.border);
    p.border_weak = color(c.border);
    p.border_focus = color(t.states.focus_ring);
    p.hover = color(t.states.hover);
    p.pressed = color(t.states.press);
    p.scrim = color(c.scrim);
    for (s, tok) in [
        (&mut p.success, c.ok),
        (&mut p.warning, c.warn),
        (&mut p.danger, c.error),
        (&mut p.info, c.accent),
    ] {
        s.fg = color(tok);
        s.icon = color(tok);
        s.border = color(tok);
        s.bg = tint(tok, 0x26);
    }
    k.radius.xs = t.shape.xs;
    k.radius.sm = t.shape.sm;
    k.radius.md = t.shape.md;
    k.radius.lg = t.shape.lg;
    k.radius.xl = t.shape.lg;
    k.elevation.raised = shadows(&t.elevation.raised);
    k.elevation.floating = shadows(&t.elevation.floating);
    k.elevation.overlay = shadows(&t.elevation.overlay);
    k.motion.control = spring(t.motion.snap);
    k.motion.overlay = spring(t.motion.panel);
    k.motion.disclosure = spring(t.motion.glide);
    k
}

/// The draft-sketch Markdown look on the app kit, set in the shell's type
/// ramp so the sketch never outranks the reading title. Code frames sit
/// on the app's alternate pane with its hairline border.
pub fn markdown(t: &Theme) -> MarkdownStyle {
    let leaded = |s: TypeStyle| TextStyle {
        line_height: (s.size * 1.5).round(),
        ..font(s)
    };
    let SketchType {
        body,
        headings,
        code,
    } = sketch_type(t);
    let mut md = MarkdownStyle::from_kit(&kit(t))
        .code_block(color(t.colors.pane_alt), color(t.colors.border))
        .body_font(leaded(body))
        .code_block_font(leaded(code));
    for (level, h) in (1..).zip(headings) {
        md = md.heading_font(level, leaded(h));
    }
    md
}

/// The sketch's faces on the shell ramp: 13 pt body, headings from the
/// display size down to the title size.
struct SketchType {
    body: TypeStyle,
    headings: [TypeStyle; 3],
    code: TypeStyle,
}

fn sketch_type(t: &Theme) -> SketchType {
    let ty = &t.type_;
    SketchType {
        body: TypeStyle {
            weight: 400.0,
            ..ty.title
        },
        headings: [
            ty.display,
            TypeStyle {
                size: 14.0,
                ..ty.title
            },
            ty.title,
        ],
        code: TypeStyle {
            size: 12.0,
            ..ty.mono
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cv_model::theme::{Mode, Theme};

    #[test]
    fn kit_wears_the_app_palette_and_shape() {
        let t = Theme::dark();
        let k = kit(&t);
        assert!(k.dark);
        assert_eq!(k.c.layer_01, color(t.colors.pane_alt));
        assert_eq!(k.c.text_base, color(t.colors.text));
        assert_eq!(k.radius.md, t.shape.md);
        assert_eq!(k.elevation.overlay.len(), t.elevation.overlay.len());
        assert!(!kit(&Theme::light()).dark);
    }

    #[test]
    fn the_sketch_is_set_in_the_shell_ramp() {
        let t = Theme::dark();
        let s = sketch_type(&t);
        assert_eq!(s.body.size, t.type_.title.size);
        assert_eq!(s.headings[0], t.type_.display);
        assert!(s.headings.windows(2).all(|w| w[0].size >= w[1].size));
        assert!(s.headings.iter().all(|h| h.size >= s.body.size));
    }

    #[test]
    fn kit_is_rebuilt_only_when_the_theme_changes() {
        let t = Theme::dark();
        assert!(Rc::ptr_eq(&kit(&t), &kit(&t)));
        let reduced = Theme::for_mode(Mode::Dark, true);
        let k = kit(&reduced);
        assert_eq!(k.motion.control, Spring::NONE);
        assert!(!Rc::ptr_eq(&k, &kit(&t)));
    }
}
