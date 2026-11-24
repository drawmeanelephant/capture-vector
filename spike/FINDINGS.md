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
- Multi-window — **PROVEN on macOS (2026-11-23) and Linux (2026-11-24)**,
  using real native windows. See both results below.
- IME composition — **US dead-key native path proven on macOS (2026-11-23)**
  and **full ibus/anthy composition proven on Linux X11 (2026-11-24)**; see
  below. One macOS Japanese conversion was observed; candidate placement,
  broader replay and all of Windows remain unproven.
- Windows/Linux — all results above are macOS-only. Nothing here can be claimed
  for the other two OSes until run there.
- `TextEdit::rich` decorations — rune-edit compiles with `typing_tests`; a typed
  probe wasn't written.
- `rune-ref` — **PROBED 2026-11-23 (Linux)**: works headless and reproducibly, but is
  not a drop-in Lavapipe replacement. See below.

IME: the macOS US dead-key path is proven below (issue #26), and the
Linux ibus/anthy path further down; Windows still needs a real OS input
method. Windows native multi-window behavior remains unproven.
The Linux real-window results below remain separate evidence.

## 2026-11-23 — issue #26 macOS native probes

Run on macOS aarch64, Darwin 27.3.0, rustc 1.100.0, rune `2a59b0f`.
The vendored Rune checkout was clean at that pin.
Probe: `python3 spike/macos-probe/run.py`.
The foreground run completed with **9 PASS checks**.
`cargo test -p cv-widgets --test editor --locked` passed **19 tests**.

The probe uses the real `cv-app`, an isolated copy of the happy corpus,
native accessibility actions, and generated macOS keyboard events.
It does not use `Runtime::ime` or `--window-script` for the IME check.
An independent AppKit `NSTextView` supplies the US Escape comparison.

| Native behavior | Observed result |
|---|---|
| Primary and editor windows | Both native windows open |
| US Option+E composition | `caf` stays committed during preedit; E commits `café` |
| US Escape during a dead key | Both Cocoa and Capture Vector produce `caf´`; subsequent typing works |
| Dirty native title-bar close | Editor reopens over the same edits with the confirmation controls |
| Keep editing | Prompt disappears; unsaved text survives |
| Save and close | Exact edited UTF-8 text reaches the copied file; editor closes |
| Pinned reading companion | Primary selection changes without retargeting the companion |
| Same noun opened again | No duplicate native companion |
| Native close followed by reopen | A fresh companion opens without a stale handle |

Screenshots and accessibility trees are generated under `target/macos-probe`.
They are local evidence, not cross-OS goldens.
The initial editor screenshot also showed painted wikilink accents and underlines
on the happy corpus. This is a macOS paint observation, not a decoration parity gate.
The host's discovered Boris 0.8.2 binary produced the expected version-mismatch
banner; native editing and saving still worked. No engine pin changed.

### Japanese conversion: observed, not a repeatable gate

A separate native Kotoeri Romaji/Hiragana run kept the committed body at `caf`
through `kyou` and Space. Enter committed `caf教`.
The standard Cocoa control produced the same converted text in a separate run.
The native screenshot showed the marked candidate at the caret before commit.

Broader automated replay did not pass reliably.
Some runs lost foreground focus to Factory and stopped before sending another key.
Other runs after automatic input-source enablement produced Latin text in the
Cocoa control instead of the expected Hiragana. Those runs are not passes and
do not isolate a Capture Vector bug.
The unreliable optional replay was removed rather than added as a CI gate.
The original input source and temporarily enabled sources were restored.

Candidate-popup placement, Japanese cancellation, reconversion, and composition
across window changes remain unproven on macOS; Windows native window behavior
and IME remain unproven on that OS. No app or vendored Rune behavior changed
on the basis of these probes.
The [manual IME and Windows VM checklist](macos-probe/README.md) covers the remaining checks.

### 2026-11-24 — complementary lane: `rune --window-script` on real macOS windows

Same host, same pin, a different mechanism: `--window KEY --window-script`
replays scripted input on the wall clock into one real window (window keys:
`main`, `editor-<id>-<n>`; transcript:
`spike/macos-probe/window-script-transcript.log`).

- **Boot companion — PASS.** `CV_EDIT` produced two real windows
  (CGWindowList: main 1280x800, editor 1100x760).
- **Independent editor input — PASS.** Scripted tap on `editor-body`, typed
  text and cmd+S; the exact bytes landed in the corpus file on disk.
- **Post-boot companion — PASS.** Tap a Pages row + Enter opened a 560x640
  reading companion mid-run.
- **Palette "Close companion windows" — PASS.** The companion was retired.
- **Clean quit — PASS.** `NSRunningApplication.terminate()` exited the
  process; no `boris watch` orphans (none was spawned: PATH boris 0.8.2
  hits the version gate here).
- Scripted `{"type":"ime"}` events DO land (`core.ime()` runs on real
  windows — `Kind::Ime` is in `script_input`), but that only exercises the
  app's side; the OS input-method path still needs real keys, which is why
  `macos-probe/run.py` is the authoritative IME lane on macOS.
- A11y warnings reproduce identically on macOS (fm-* fields, editor-body).

## 2026-11-24 — issue #26 multi-window probe (Linux, real windows)

Driven on a live X desktop (`DISPLAY=:0`, debug cv-app, `CV_PROJECT=<happy
corpus> CV_EDIT=content/guides/getting-started.md`). All real winit windows,
no headless harness:

- Boot companion (`CV_EDIT`) opens beside the main window at launch — two real
  windows, both rendering.
- Windows take input independently: typed into the editor companion (buffer
  marked unsaved, live preview updated) and switched modules in the main
  window while the companion stayed open.
- Dirty-close: native close on an unsaved editor destroys the window host and
  reopens a fresh window carrying the same buffer + the Save/Discard/Keep
  prompt (the known rune `on_close`-can't-veto path). `Discard and close`
  then closes the window; the main window keeps running.
- Post-boot companion: palette `Open selection in companion window` on a Pages
  row opens a reader companion mid-run; closing it natively leaves the app
  alive.
- Quit: closing the main window exits the process; no orphaned `boris watch`
  or window hosts.

Side observations: the engine version gate works on a real launch (PATH boris
0.8.2 → degraded banner "found boris 0.8.2, want 0.9.0"). rune logs a11y
warnings for the editor's `fm-*` fields and `editor-body` — focusable text
fields with no screen-reader name (`El::label`/`labelled_by` missing).

Unproven on this OS: native portal folder dialog (GTK portal was present but
unexercised), IME, and the Windows/macOS window behaviors (per-OS surface
creation, WARP second-surface race).

## 2026-11-24 — issue #26 IME probe (Linux X11, ibus/anthy)

Probe: `spike/rune-probe/src/bin/ime_probe.rs`, a real rune window with a focused
`text_input`, logging every `TextEdit` state change and app-level `Ime` event to
`out/ime-probe.log`. Driven on the live X desktop (`DISPLAY=:0`) with
`XMODIFIERS=@im=ibus`, `ibus-daemon -x`, engine `anthy` (hiragana mode); keys sent
by `xdotool`. Screenshots: `out/ime-preedit.png`, `out/ime-committed.png`.

- **IME enable roundtrip — PASS.** Focusing the field fired `Ime::Disabled` then
  `Ime::Enabled` once the window had OS focus (`set_ime_allowed(true)` through
  XIM). The field never became editable to IME while unfocused.
- **Preedit — PASS.** Every keystroke arrived as `Preedit` with text + caret byte
  range + clause ranges: `n` → `に` → `にほ` → `にほんご`. Committed text never
  appeared during composition — the buffer stayed empty until commit.
- **Conversion — PASS.** Space rewrote the preedit in place (`にほんご` → `日本語`),
  still flagged `target: false` over the whole range. Clause granularity did not
  follow anthy's segments (one clause per event via XIM).
- **Commit — PASS.** Return inserted the composed text at the caret:
  `text="日本語お"`, `anchor=focus=12` (UTF-8 byte offsets, not chars).
- **Cancel — PASS.** Escape cleared the preedit (`preedit=""`, buffer untouched);
  an `Ime::Disabled` followed as the field re-armed the input context.
- **Keys during composition — PASS.** The IM consumed every key first; no raw
  ASCII leaked into the buffer mid-composition.

Environment gotchas (not rune defects, worth a setup note):

- A fresh XIM context's anthy engine started in direct (Latin) input mode, so the
  first `nihongo` committed as raw ASCII. Hiragana needed
  `gsettings set org.freedesktop.ibus.engine.anthy.common input-mode 0`
  (or Ctrl+J inside the field). Check the engine's mode before judging the app.
- ibus's engine scripts exec `python3`; the box's PATH python3 (3.12, no `gi`)
  silently kills engine startup — spawn the daemon with system python on PATH.
- **Committed CJK text renders as tofu (□).** The bundled font set has no CJK
  glyphs on this Linux box; the buffer is correct (log proves the codepoints) but
  the field and status line show boxes. Font fallback for CJK is unproven on
  Linux — flag for follow-up, alongside the a11y-name warnings noted earlier.

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
