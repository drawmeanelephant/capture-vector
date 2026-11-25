# Bundled fonts

`DroidSansFallback.ttf.gz` is the Android Open Source Project's
`data/fonts/DroidSansFallback.ttf`, pinned at `android-15.0.0_r1` and
gzip-compressed with zeroed gzip metadata. The decompressed font is
3,451,900 bytes and has SHA-256
`21b96a0377f067833a93af3082eb28d4ffab7a8cd46bfd513286f1d64b7b0949`.
It is licensed under Apache License 2.0; see
`DroidSansFallback-LICENSE.txt` (the upstream NOTICE text).

Capture Vector embeds it as the last-resort CJK fallback for systems
with no CJK font installed. Measured coverage is deliberately narrow:
all 11,172 Hangul syllables; 20,902 of 20,992 BMP unified ideographs
(missing all of U+9FA6..U+9FFF); 2 of 6,592 Extension A ideographs; no
Extension B ideographs; 90 of 96 hiragana (missing U+3040,
U+3095..U+3098 and U+309F); 91 of 96 katakana (missing U+30F7..U+30FA
and U+30FF); no U+31F0..U+31FF katakana extension; and all halfwidth
kana. Its only GSUB feature is `vert`: there is no `locl`, so Han text
cannot select Japanese, Simplified Chinese, Traditional Chinese or
Korean regional glyph variants. Text in those gaps still needs a system
font. The full Noto CJK alternative costs roughly 12-16 MB per weight,
so this smaller Android fallback is the explicit size/coverage
trade-off, not a pan-CJK solution.

At the pinned Rune revision `AppSpec::fonts` replaces rather than extends
Rune's embedded Inter faces, so Capture Vector restates those three faces
(1,248,684 bytes) beside the fallback and builds Rune without its
`embedded-fonts` feature. The replacement is byte-neutral apart from
linker layout; expected font-data growth is therefore the fallback's
3,451,900 bytes, not a duplicate copy of Inter.

`build.rs` inflates at most 3,451,901 bytes and refuses the build unless
the decompressed length and SHA-256 above match exactly. Unit tests
verify the checked-in gzip against the embedded bytes and reject a
corrupted copy.

Regenerate, verifying both digests before committing, by downloading
`https://android.googlesource.com/platform/frameworks/base/+/android-15.0.0_r1/data/fonts/DroidSansFallback.ttf?format=TEXT`,
base64-decoding it, confirming the decompressed length and SHA-256
above, and writing `gzip.compress(raw, compresslevel=9, mtime=0)` to
`DroidSansFallback.ttf.gz`.
