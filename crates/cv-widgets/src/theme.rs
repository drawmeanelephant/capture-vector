//! Theme tokens (data, in cv-model) → rune values.

use cv_model::theme::{Rgba, SpringTok, TypeStyle};
use rune::{Color, Family, Spring, TextStyle};

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
