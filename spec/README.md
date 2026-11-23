# Capture Vector — Spec Kit (v3)

This directory is the product: the instructions for building the app,
designed so the build runs in parallel lanes from day one.

> **Status:** v3 — refined through eight adversarial reviews across four
> model families (SWE-2 ×3 re-derivations + ×2 adversaries, Gemini 4
> Neon, Claude Opus 5.5, Kimi K3.1 Pro). `PROMPT-v3.md` is the current
> master. `PROMPT-groundwork.md` and `PROMPT-v2.md` are **superseded**
> and kept only for the trail — do not dispatch against them.
> `spec/synthesis/` holds the review record.

## Files

| File | What it is |
|---|---|
| `PROMPT-v3.md` | **The kickoff prompt.** Self-contained; hand to the scaffolding agent/team. |
| `vision.md` | What Capture Vector is, what it refuses to be, non-negotiable pillars. |
| `architecture.md` | v3 crate map, engine boundary, state/identity, store discipline, parallelism rules. |
| `contracts.md` | v3 shared types and ports. Wave 0 turns these into compilable stubs. |
| `lanes/L*.md` | v3 self-contained work orders — each a complete prompt for a worker. |
| `synthesis/` | The multi-model review record: derivations, rulings, garbage bin. |

## How parallel work actually happens

1. **Wave 0 chassis** (L0): contracts as compilable stubs + **real
   captured fixtures** for all corpora (hand-authored Boris JSON is
   banned) + `StubEngine` + rune capability probe + canary vertical
   slice. Then every lane fans out.
2. **Contracts before code**: lanes code against `cv-core`/`cv-contracts`
   only. `cv-core` is append-only, with one contract-hardening
   checkpoint after the first vertical slice.
3. **Fixtures before engines**: `fixtures/MANIFEST.toml` (Engine-owned)
   pins every capture by hash + boris version; lanes assert by name;
   `StubEngine ≡ ProcessEngine` conformance runs continuously.
4. **Ownership by path**: each lane owns directories outright; shared
   files are L0's alone. `AppEvent` variants and `AppState` slices are
   lane-namespaced so additive changes don't collide.
5. **Headless verification**: element-tree + layout-geometry goldens
   gate on 3 OSes; PNG goldens gate one canonical software rasterizer.

## Dispatch

```
L0 wave-0 chassis ──gate──> L1 engine ─┬─ L5 publish ─┐
                          L2 shell ────┼─ L3 modules ─┤
                          L4 rules ────┼─ L6 research ─┼─ integration
                                       └─ L7 editor ──┘
```

All of L1–L7 are mutually parallel once the wave-0 gate passes.
A contract-hardening checkpoint lands after the first vertical slice.

## Upstreams

- [boris](https://github.com/drawmeanelephant/boris) — the engine.
  Subprocess only; never reimplement.
- [oliver](https://github.com/drawmeanelephant/oliver) — markup lib;
  non-Markdown corpora + advisory facts only.
- [solipsist](https://github.com/drawmeanelephant/solipsist) — macOS
  predecessor; boundary rules inherited.
- [rune](https://github.com/anomalyco/rune) — UI framework; pinned
  `dev` rev; all contact firewalled in `cv-widgets`.
