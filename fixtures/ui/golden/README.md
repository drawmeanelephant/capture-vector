# UI goldens

Two kinds, one contract each:

| Kind | Where | Gate | Produced by |
|---|---|---|---|
| Element-tree + layout JSON | `tree/<scene>.{tree,layout}.json` | **cross-OS** (Linux, macOS, Windows) | `cargo test -p cv-app --test goldens` — in-process `rune::Runtime`, no GPU |
| PNG | `png/<scene>.png` | canonical rasterizer only: Linux + Lavapipe | `scripts/check-ui-png.sh` — `cv-app --scene <s> --snap` |

Scenes are named in `crates/cv-app/src/lib.rs` (`SCENES`): `canary`
(StubEngine replays `fixtures/captures/happy/watch/session.ndjson`),
`pages-5k` (5,000-row virtualized list), `palette`, `light`
(light + reduced motion), and one per degraded state: `engine-absent`,
`version-mismatch`, `watch-dead`, `store-corrupt`. Scripted input
(`key:`, `click:`, `cmd:` steps) lives in `cv_widgets::harness::Harness`.

Tree JSON is built from `Runtime::elements()` (layout + retained tree):
keys, roles, text, rects rounded to 0.1pt, colours. It never reads GPU
output, so it must be byte-identical on every OS.

## Bless

1. Change the UI; run the gate. A JSON mismatch writes
   `tree/<name>.actual.json` (gitignored) next to the golden; a PNG
   mismatch leaves the render in `target/ui/<scene>.png`.
2. Inspect: `diff tree/<name>.json tree/<name>.actual.json`; for PNGs
   open both and run `cargo run -p xtask -- png-diff <a> <b>`.
3. Bless: `CV_BLESS=1 cargo test -p cv-app --test goldens` and/or
   `CV_BLESS=1 scripts/check-ui-png.sh` (Linux + Lavapipe only).
4. Commit goldens in the same commit as the UI change, with one line per
   changed scene saying why.

## Reviewer

Golden changes need review by the **L2 shell lane owner** and approval
by the **integrator** (`@drawmeanelephant`) — they are the named visual
diff reviewers. PNG goldens are never blessed from macOS/Windows or a
hardware GPU; tree/layout goldens blessed anywhere must pass on all three
OSes in CI before merge.
