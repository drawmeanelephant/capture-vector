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

- `rune_kit::markdown::Markdown` — compiles (dep built), not exercised in a tree.
- Multi-window — tooling knows windows (`--window KEY` picks which window
  `--tree`/`--scene` describe), untested in the probe.
- IME composition — lives in rune-edit state machines; needs a per-OS real-window
  pass; cannot be probed headless meaningfully.
- Windows/Linux — all results above are macOS-only. Nothing here can be claimed
  for the other two OSes until run there.
- `TextEdit::rich` decorations — rune-edit compiles with `typing_tests`; a typed
  probe wasn't written.
- `rune-ref` — reference renderer exists in-repo (likely the canonical rasterizer
  for PNG goldens); not yet tested.

## Consumption details worth keeping

- `Env { system_dark, scale }` — per-runtime; theme via `tok::theme(dark)`.
- `virtual_rows` keys items by `key_fn(&S, usize) -> Cow<str>`; handlers must
  resolve items by key/id, never captured index (upstream warns `memo` can
  preserve stale handlers).
- Headless run prints perf line (gpu init, frame cpu p50/p95) — usable as a
  free smoke-benchmark gate.
- `Runtime::with_text` accepts a `TextSystem` — font-injection seam for
  deterministic goldens.
