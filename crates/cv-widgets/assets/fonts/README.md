# Bundled fonts

`DroidSansFallback.ttf.gz` is the Android Open Source Project's
`data/fonts/DroidSansFallback.ttf`, pinned at `android-15.0.0_r1` and
gzip-compressed with zeroed gzip metadata. The decompressed font is
3,451,900 bytes and has SHA-256
`21b96a0377f067833a93af3082eb28d4ffab7a8cd46bfd513286f1d64b7b0949`.
It is licensed under Apache License 2.0; see
`DroidSansFallback-LICENSE.txt` (the upstream NOTICE text).

Capture Vector embeds it as the last-resort CJK fallback for systems
with no CJK font installed. It covers the common BMP CJK surface used by
Japanese, Chinese and Korean text (kana, the main unified ideograph
block and Hangul syllables). It is not a pan-Unicode font: CJK Extension
A/B and other rare ideographs still need a system font. The full Noto
CJK alternative costs roughly 12-16 MB per weight, so this smaller
Android fallback is the deliberate size/coverage trade-off.

At the pinned Rune revision `AppSpec::fonts` replaces rather than extends
Rune's embedded Inter faces, so Capture Vector restates those three faces
(1,248,684 bytes) beside the fallback and builds Rune without its
`embedded-fonts` feature. The replacement is byte-neutral apart from
linker layout; expected font-data growth is therefore the fallback's
3,451,900 bytes, not a duplicate copy of Inter.

Regenerate, verifying both digests before committing, by downloading
`https://android.googlesource.com/platform/frameworks/base/+/android-15.0.0_r1/data/fonts/DroidSansFallback.ttf?format=TEXT`,
base64-decoding it, confirming the decompressed length and SHA-256
above, and writing `gzip.compress(raw, compresslevel=9, mtime=0)` to
`DroidSansFallback.ttf.gz`.
