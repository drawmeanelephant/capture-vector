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

Tree JSON is built from `Runtime::elements()` (layout + retained tree)
and `Runtime::presented_pose`. It never reads GPU output, so it must be
byte-identical on every OS. Floats are rounded to 0.1pt.

### Tree schema 2

Every node has `path`, `rect` and `opacity`. The other fields appear only
when they differ from the default, so a styling change shows up as added
or changed lines:

| Field | Meaning |
|---|---|
| `key`, `role`, `label`, `text` | Identity and content (schema 1). |
| `help` | Tooltip and accessibility help (`El::help`). |
| `fill`, `color` | Fill and text colour as `#rrggbbaa`, when not transparent (schema 1). |
| `radius` | Corner radius in points, when not 0. |
| `shadows` | `El::box_shadow` layers at their presented geometry: `[x, y, blur, spread, "#rrggbbaa", inset]`. Only on keyed nodes whose keyed path segments are unique, since Rune looks poses up by key. |
| `scale` | Effective uniform scale to 0.01, when not 1 (press scale, presence). |
| `hover`, `press` | Hover and press track amounts, when above 0. Resting goldens never have them; harness tests use `Harness::hover` and `Harness::hold`. |
| `focused`, `focus_visible` | Keyboard focus, and whether its ring shows. |
| `selected` | A selected list, tree or table item (`El::focus_selected`). |

Borders, gradients, outlines and blur are not in `ElementInfo`, so they
stay PNG-gated (Linux + Lavapipe). Gating them in JSON needs Rune to
expose them first.

Layout JSON (`<scene>.layout.json`) is still schema 1: keyed rects only.

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
