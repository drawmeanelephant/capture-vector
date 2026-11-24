//! Signature moments (issue #57): a few opt-in effects at the moments
//! that matter — a live watch session, a finished build, a publish
//! result — plus a faint aurora behind the dark rail.
//!
//! Everything here is gated by [`Theme::effects_on`] (the `effects` flag,
//! and never under reduced motion). With it off no element is emitted,
//! so the element tree, layout and pixels are exactly the plain shell's.
//! The pure-rune parts (pulse, glow gradient) need no GPU feature; the
//! rune-fx layers drawn at the effect slots come from [`gfx`], behind the
//! `effects` cargo feature. Without it the slots draw nothing.

use crate::theme::tint;
use cv_model::theme::{Mode, Rgba, Theme};
use rune::{Cycle, El, effect};

#[cfg(feature = "effects")]
pub mod gfx;

/// Effect-slot marks: which rune-fx layer the compositor draws.
pub const RAIL_AURORA: u32 = 1;
pub const BUILD_GLOW: u32 = 2;
pub const PUBLISH_OK: u32 = 3;
pub const PUBLISH_FAILED: u32 = 4;

/// How far a one-shot glow may paint past its element.
const BLEED: f32 = 18.0;

/// A colour token packed into one effect-data float (24 bits fit exactly).
pub fn pack(c: Rgba) -> f32 {
    ((u32::from(c.0) << 16) | (u32::from(c.1) << 8) | u32::from(c.2)) as f32
}

pub fn unpack(v: f32) -> u32 {
    v.max(0.0) as u32 & 0x00ff_ffff
}

/// Slow breathing for the live engine dot.
const LIVE_PULSE: Cycle = Cycle {
    period: 2.4,
    phase: 0.0,
    stops: &[(0.0, 1.0), (0.5, 0.35), (1.0, 1.0)],
    still: 1.0,
    fps: Cycle::DISPLAY_RATE,
};

/// Live watch: the engine chip's dot breathes and the chip carries a
/// faint accent glow from the dot side.
pub fn live_chip(t: &Theme, chip: El, dot: El, live: bool) -> (El, El) {
    if !(live && t.effects_on()) {
        return (chip, dot);
    }
    let a = t.colors.accent;
    let chip = chip.radial_gradient(
        [0.08, 0.5],
        [56.0, 22.0],
        [(0.0, tint(a, 0x3a)), (1.0, tint(a, 0x00))],
    );
    (chip, dot.cycle(LIVE_PULSE))
}

fn slot(mark: u32, key: String, data: [f32; 4]) -> El {
    effect(mark)
        .key(key)
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .effect_bleed(BLEED)
        .effect_data(data)
}

/// Rail background: a very low-intensity aurora, dark mode only.
pub fn rail_aurora(t: &Theme) -> Option<El> {
    (t.effects_on() && t.mode == Mode::Dark)
        .then(|| slot(RAIL_AURORA, "fx-rail".into(), [0.06, 0.18, 0.0, 0.0]))
}

/// Build finished: one glow over the Activity rail item, keyed by the
/// build count so each finished build plays once (on the theme's pop
/// spring, see [`envelope`]).
pub fn build_glow(t: &Theme, builds: u64) -> Option<El> {
    (t.effects_on() && builds > 0).then(|| {
        slot(
            BUILD_GLOW,
            format!("fx-build-{builds}"),
            [builds as f32, pack(t.colors.accent), 0.9, 0.0],
        )
    })
}

/// Publish result over its button: sparkles on success, an error pulse
/// on failure. `seq` identifies the run so each result plays once.
pub fn publish_burst(t: &Theme, ok: bool, seq: u64) -> Option<El> {
    t.effects_on().then(|| {
        let (mark, tok) = if ok {
            (PUBLISH_OK, t.colors.ok)
        } else {
            (PUBLISH_FAILED, t.colors.error)
        };
        slot(
            mark,
            format!("fx-publish-{seq}"),
            [seq as f32, pack(tok), 1.0, 0.0],
        )
    })
}

/// A one-shot's strength `age` seconds after it first showed: rises on
/// the pop spring, holds, then fades out. Zero once `ONE_SHOT` has passed.
pub fn envelope(pop: rune::Spring, age: f32) -> f32 {
    const HOLD: f32 = 0.45;
    if !(0.0..ONE_SHOT).contains(&age) {
        return 0.0;
    }
    let rise = pop.sample(0.0, 0.0, 1.0, age).0.max(0.0);
    let fade = 1.0 - ((age - HOLD) / (ONE_SHOT - HOLD)).clamp(0.0, 1.0);
    rise * fade
}

/// How long a one-shot plays, in seconds.
pub const ONE_SHOT: f32 = 1.4;

#[cfg(test)]
mod tests {
    use super::*;
    use cv_model::theme::Mode;

    fn on(mode: Mode) -> Theme {
        let mut t = Theme::for_mode(mode, false);
        t.effects = true;
        t
    }

    #[test]
    fn nothing_is_emitted_with_effects_off_or_reduced_motion() {
        let mut reduced = Theme::for_mode(Mode::Dark, true);
        reduced.effects = true;
        for t in [Theme::dark(), Theme::light(), reduced] {
            assert!(rail_aurora(&t).is_none());
            assert!(build_glow(&t, 3).is_none());
            assert!(publish_burst(&t, true, 1).is_none());
        }
    }

    #[test]
    fn aurora_is_dark_only_and_one_shots_need_a_result() {
        assert!(rail_aurora(&on(Mode::Dark)).is_some());
        assert!(rail_aurora(&on(Mode::Light)).is_none());
        assert!(build_glow(&on(Mode::Light), 0).is_none());
        assert!(build_glow(&on(Mode::Light), 2).is_some());
        assert!(publish_burst(&on(Mode::Light), false, 7).is_some());
    }

    #[test]
    fn colour_tokens_survive_packing() {
        let a = Theme::dark().colors.accent;
        assert_eq!(unpack(pack(a)), 0x00d1ff);
        assert_eq!(unpack(pack(Rgba::rgb(0xff, 0xff, 0xff))), 0xffffff);
    }

    #[test]
    fn one_shot_envelope_rises_overshoots_and_ends() {
        let pop = rune::Spring::new(0.25, 0.15);
        assert_eq!(envelope(pop, 0.0), 0.0);
        let peak = (1..40)
            .map(|i| envelope(pop, i as f32 * 0.01))
            .fold(0.0, f32::max);
        assert!(peak > 0.9, "rises to full strength, got {peak}");
        assert!(envelope(pop, 1.0) < peak);
        assert_eq!(envelope(pop, ONE_SHOT), 0.0);
    }
}
