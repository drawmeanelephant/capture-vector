# Wave 0 — Rune capability probe findings

Run: 2026-11-23, macOS aarch64, rustc 1.100. Probe: `spike/rune-probe` against
rune `dev` @ `2a59b0f95b82972478a497c42151dbcb3ca5c69f` (vendored at `spike/vendor/rune`).

## PROVEN (ran, artifacts exist)

| Spec assumption | Result |
|---|---|
| Headless `Runtime` + scripted input | PASS — `Runtime::new`/`frame`/`find`/`pointer_down`/`pointer_up`; pointer drove a real handler mutation. `tests/probe.rs` |
| `Runtime::elements()` element tree | PASS — `ElementInfo` carries `key`, `role`, `text`, `rect`, `opacity`, focus/exiting flags. The in-process assertion surface the spec needs. |
| Virtualized 5,000-row list | PASS — `El::virtual_rows::<S>(count, est_h, key_fn, build_fn)`; element tree showed bounded mounted rows of 5000. API calls `virtualize` internally; doc example also shows `.virtualize()` + `.growth(GrowthSpec)` alone. |
| `--snap` PNG headless render | PASS — real GPU render, 1800×1200 RGBA (scale 2.0), gpu init ~213ms, 84 draw instances. |
| `--tree` element-tree JSON | PASS — keys + roles + names + bounds, incl. accessibility roles/states. This is the 3-OS golden gate. |
| `--scene` canned-state switching | PASS — `AppSpec::scenes` populated 2 findings + "5000 rows" label into the snap/tree run. |
| `&mut State` + wake/drain state model | PASS — handler took `&mut S`, mutated, next `frame()` reflected it. Confirms `cv-core`'s sync/channel-sink port choice. |

## CRITICAL SPEC CORRECTION — rune cannot be a git dependency

`cargo fetch` on `rune = { git = "...", rev = "..." }` fails:
`rune-text` needs `parley_engine` feature `line-dictionaries`; crates.io
`parley_engine 0.12.0` lacks it. Rune's own root `Cargo.toml` says it plainly:

> "a dependency's `[patch]` doesn't apply to the workspace that depends on it.
> An app that builds Rune patches it too."

Vendored forks required at `<rune>/vendor/`: `parley_engine`, `winit`, `wgpu-hal`,
`sctk-adwaita`, `accesskit_windows`, `accesskit_macos`.

**Prescription (tested, works):** vendor rune as a submodule (or copied tree) at
`vendor/rune`, depend by `path`, and replicate the six-entry `[patch.crates-io]`
block. `xtask` should own the pin (clone/checkout at `MANIFEST.toml`'s rev) so
lanes and CI don't each improvise the checkout.

## NOT YET PROVEN

- `rune_kit::markdown::Markdown` — **PROVEN 2026-11-23 (Linux)**, see below.
- Multi-window — tooling knows windows (`--window KEY` picks which window
  `--tree`/`--scene` describe), untested in the probe.
- IME composition — lives in rune-edit state machines; needs a per-OS real-window
  pass; cannot be probed headless meaningfully.
- Windows/Linux — all results above are macOS-only. Nothing here can be claimed
  for the other two OSes until run there.
- `TextEdit::rich` decorations — rune-edit compiles with `typing_tests`; a typed
  probe wasn't written.
- `rune-ref` — **PROBED 2026-11-23 (Linux)**: works headless and reproducibly, but is
  not a drop-in Lavapipe replacement. See below.

Multi-window and IME stay open: both need a real window on macOS/Windows, which the
Linux VM and headless harness can't give (issue #26).

## 2026-11-23 — issue #26 Linux probes

Run on Linux x86_64, rustc 1.99.1, same rune rev. Probe:
`crates/cv-app/tests/probes.rs` (`cargo test -p cv-app --test probes -- --nocapture`);
part of `cargo test --workspace`. No network, display, GPU or Boris needed.

### `rune_kit::Markdown` in the real shell tree — PASS

The dogfood scene with the Pages module, nav `all`, and a page row selected mounts
`sketch-label` + the `sketch` scroller in the reading pane (`shell.rs`). The texts
under `sketch` are what a reader would expect for `guides/asides.md`: h1/h2/h3
headings, paragraphs with inline code, the fenced code body, a 5×2 GFM table, list
bullets (`•`), in document order. `getting-started.md` and the long
`guides/apex-markdown.md` gallery (374 text nodes) also render.

Expected divergences. They are asserted, not fixed, because Boris is authoritative:

| Construct | Sketch shows | Boris does |
|---|---|---|
| `[[target\|label]]` wikilinks | raw `[[…]]` text | resolves to a link |
| `{{include path}}` | raw marker text; fragment not expanded | inlines the fragment |
| `<Aside kind=…>` | tag lines dropped (CommonMark HTML block); body renders as plain paragraphs, no kind label or callout box | semantic callout |
| `<Details summary=…>` | same as Aside: no summary/disclosure (covered by `crates/cv-widgets/tests/parity.rs` on `fixtures/editor` `index.md`; no dogfood page uses Details) | `<details>` element |

Detail: inline code spans come out wrapped in U+202F (narrow no-break space) padding,
e.g. `"\u{202f}\u{202f}tip"`. Text assertions should match with `contains`.

### `rune-ref` as the PNG-golden rasterizer — WORKS, NOT A DROP-IN

`cv_widgets::reference` (feature `reference`, test-only) builds a
`rune_ref::Scene` from `Runtime::instances` (`Primitive::from_instance`, which needs
rune-ref's `gpu` feature), glyph atlas page 0, and `Runtime::clear_color()`. It then
renders on the scalar CPU oracle and encodes the PNG.

- **Headless:** yes. No wgpu device, window or display. `Scene::validate()` accepts
  `canary` (373 instances) and `pages` (1,417). Both are only rect + glyph kinds, on
  one atlas page, with no images or native surfaces.
- **Reproducible:** yes, within a machine. Two fresh `init()`→scene→render runs give
  identical BLAKE3 hashes. At scale 1: canary
  `33eadf7a79f637aa3d1c09004001225e7a0055a0da43fba26b856d8cad2ccbe7`, pages
  `666aa330159e009792071505f96727b278d83383457b94339c4362b69e5ecad0`. The
  design (strict f32, soft libm) targets cross-host bytes, but we haven't compared
  hashes across hosts.
- **Speed:** slow. With `opt-level = 3` for rune-ref in dev (root `Cargo.toml`), a
  1280×800 frame takes ≈4 s (canary) to ≈9 s (pages). At the `--snap` 2560×1600 size
  it takes ≈15 s to ≈32 s, compared with sub-second on Lavapipe.
- **Does not match the Lavapipe goldens.** With `CV_REF_SCALE=2`, `xtask png-diff
  --tolerance 8` against `fixtures/ui/golden/png` gives 1.34% (canary) and 3.40%
  (pages) of pixels differing. The gate allows 0.2%. The differences are translucent
  fills (the selected-row tint is visibly brighter) and glyph edges. Cause: rune-ref
  only models *linear* blending into an sRGB target. The `--snap`/window path uses
  `App::srgb_blend` (on by default; it premultiplies in sRGB encoding and blends
  gamma-space). The headless `Runtime` defaults to linear. Turning `set_srgb_blend(true)`
  on before replaying makes 100% of pixels differ, because rune-ref has no gamma-blend
  mode.

**Verdict:** rune-ref can produce PNG goldens, but only for a *linear-blend* rendering
of the app, which is not what users see on screen. Replacing Lavapipe needs:

1. Upstream: a gamma-space (`srgb_blend`) mode in rune-ref. Or accept linear-blend
   goldens as the canonical config, which means a spec decision and re-blessing
   `fixtures/ui/golden/png`.
2. Replay of every glyph atlas page and the image atlas. The probe copies page 0
   only and reports `off_page_glyphs`/`images`; rune-ref's `Scene` has a single atlas.
   It also needs a hard fail on kinds that `validate()` rejects (layers, blur, surfaces).
3. A `--snap-ref` path, or a test-side writer like `CV_REF_OUT`, so that
   `scripts/check-ui-png.sh` produces PNGs without Vulkan. The `png-diff` gate
   stays the same.
4. Budget for ≈1 min CPU per scene set at 2×. Alternatively, rune's own
   `render::cpu_boot` (multi-threaded, has `srgb_blend`) may be the better CPU
   candidate. It is not probed here.

Until then Lavapipe stays canonical.

## Consumption details worth keeping

- `Env { system_dark, scale }` — per-runtime; theme via `tok::theme(dark)`.
- `virtual_rows` keys items by `key_fn(&S, usize) -> Cow<str>`; handlers must
  resolve items by key/id, never captured index (upstream warns `memo` can
  preserve stale handlers).
- Headless run prints perf line (gpu init, frame cpu p50/p95) — usable as a
  free smoke-benchmark gate.
- `Runtime::with_text` accepts a `TextSystem` — font-injection seam for
  deterministic goldens.

## 2026-11-23 update — engine pin moved to boris 0.9.0

Mission gameplan ratified: boris **0.9.0** (vcs `66efcfb`, tag `v0.9.0`,
built w/ zig 0.17.1 — min is 0.17.0) because 0.8.2 lacks native Windows
support. Fixtures re-captured: same exit matrix, watch hello now
`"compiler":"boris/0.9.0"`. oliver 1.3.0 verified installed. Rust floor
1.99.1; Lavapipe = canonical PNG rasterizer; signature-graph (nostr/
atproto signing surface) approved — secrets stdin-only per spec.
