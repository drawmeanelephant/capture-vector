//! The app's embedded typeface set, shared by every native and headless
//! runtime. Rune's `AppSpec::fonts` replaces its built-in Inter set, so
//! the same three Inter faces must be named here before the CJK fallback.

use rune::text::TextSystem;

static INTER_REGULAR: &[u8] =
    include_bytes!("../../../vendor/rune/crates/rune/assets/Inter-Regular.ttf");
static INTER_MEDIUM: &[u8] =
    include_bytes!("../../../vendor/rune/crates/rune/assets/Inter-Medium.ttf");
static INTER_SEMIBOLD: &[u8] =
    include_bytes!("../../../vendor/rune/crates/rune/assets/Inter-SemiBold.ttf");
static DROID_SANS_FALLBACK: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/DroidSansFallback.ttf"));

/// Keep launch typography identical to Rune's default slots. The last
/// registered family is not primary, so Rune uses it as the app's CJK
/// fallback before asking the platform for one.
pub const FAMILIES: (&str, &str) = ("Inter", "Inter");
pub static FONTS: &[&[u8]] = &[
    INTER_REGULAR,
    INTER_MEDIUM,
    INTER_SEMIBOLD,
    DROID_SANS_FALLBACK,
];

/// A text system with the app's complete embedded font set. Headless
/// runtimes must use this rather than `Runtime::new`, whose default text
/// system omits the fallback because custom `AppSpec::fonts` replace it.
pub fn text_system() -> TextSystem {
    TextSystem::new(FONTS, FAMILIES)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::TextStyle;
    use rune::text::parley;

    fn shaped(sample: &str) -> Vec<(u32, bool)> {
        let mut ts = text_system();
        let layout = ts.layout_rich(sample, &[], TextStyle::new(16.0, 400.0), 1.0, None);
        let mut out = Vec::new();
        for line in layout.lines() {
            for item in line.items() {
                let parley::PositionedLayoutItem::GlyphRun(run) = item else {
                    continue;
                };
                let bundled: &[u8] = run.run().font().data.as_ref();
                let fallback = bundled == DROID_SANS_FALLBACK;
                out.extend(run.glyphs().map(|g| (g.id, fallback)));
            }
        }
        out
    }

    #[test]
    fn bundled_cjk_fallback_shapes_real_glyphs_for_japanese_chinese_and_korean() {
        for sample in ["日本語お", "中文漢字", "한국어"] {
            let glyphs = shaped(sample);
            assert!(!glyphs.is_empty(), "{sample} shaped no glyphs");
            assert!(
                glyphs.iter().all(|(id, _)| *id != 0),
                "{sample} shaped .notdef tofu: {glyphs:?}"
            );
            assert!(
                glyphs.iter().all(|(_, fallback)| *fallback),
                "{sample} did not resolve to the bundled fallback: {glyphs:?}"
            );
        }
    }

    #[test]
    fn latin_text_still_resolves_outside_the_cjk_fallback() {
        let glyphs = shaped("Capture Vector");
        assert!(!glyphs.is_empty());
        assert!(glyphs.iter().all(|(id, _)| *id != 0));
        assert!(
            glyphs.iter().all(|(_, fallback)| !*fallback),
            "Latin prose must keep the primary typeface"
        );
    }
}
