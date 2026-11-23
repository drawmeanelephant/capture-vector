# Concrete gameplan — post wave-0 Rune probe

Status inputs (facts, not assumptions):
- rune `dev` @ 2a59b0f — PROVEN on macOS: headless `Runtime`, scripted pointer,
  `elements()` tree, 5000-row `virtual_rows` (bounded mounts), `--snap` PNG via
  real GPU, `--tree` JSON (keys+roles+text+bounds), `--scene`, `Env{dark,scale}`.
  Results: `spike/FINDINGS.md`, `spike/rune-probe`, `spike/vendor/rune`.
- rune consumption REQUIRED to be vendored path dep + replicated `[patch]` —
  git dep cannot resolve. Spec updated (architecture.md §0, PROMPT-v3).
- boris **0.9.0** (vcs `66efcfb`) is the ratified pin — built from tag
  `v0.9.0` w/ zig 0.17.1, fixtures re-captured (0.8.2 lacks native Windows
  support). Oliver 1.3.0 matches the pin. Rust floor 1.99.1.
  Lavapipe = canonical PNG gate on Linux CI.
- Windows/Linux: nothing proven. The rune probe must eventually run in the
  3-OS CI matrix; IME needs real windows per OS.

## Wave 0 — remaining (all doable on this machine)

| # | Task | Exit |
|---|---|---|
| 1 | Workspace skeleton: vendored `vendor/rune`, root `[patch]` block, crate map stubbed (cv-core/cv-contracts no-dep leaves) | `cargo check --workspace` clean |
| 2 | `cv-core` types + sync ports compile; event envelope + lane-namespaced `AppEvent` skeleton | unit tests pass |
| 3 | `cv-contracts` serde mirrors compile against real captures | — |
| 4 | Fixture capture: script drives `boris` over happy/broken-* corpora → `fixtures/` + `MANIFEST.toml` (hashes, `boris --version`, capture cmds) | manifest verifies; no hand-authored JSON |
| 5 | `StubEngine`/`StubStore` reading fixtures | conformance scaffold exists (even if ProcessEngine tests are `#[ignore]`) |
| 6 | Canary: `BuildFinished` → event cascade → store mutation → `Runtime` wake → element-tree assertion, in one `#[test]` | test green |
| 7 | Watch NDJSON capture: run `boris watch --watch-json` on a corpus, record real session (hello handshake, seq) to fixtures | fixtures + session-id resync note |
| 8 | Extend probe: `rune_kit::markdown` in a tree; `--window-script` multi-window smoke | findings updated |

## Wave 1 — parallel lanes can start when

- Wave-0 items 1–6 green AND fixture manifest verifies.
- Then lanes per spec/lanes/*, L0 integrator owns mod/wire lists.

## Known-unknown list (owner decisions / OS gates)

- ~~Boris pin~~ resolved: 0.9.0 / `66efcfb`, in MANIFEST.toml.
- Windows/Linux probe + CI runners (no containers allowed locally — CI-hosted
  runners or physical machines; cannot be faked from macOS).
- `rune-ref` as canonical PNG rasterizer — test in wave 1 before picking the
  golden config.
- IME/TextEdit::rich per-OS passes — real windows required.
