# Groundwork Prompt — Capture Vector (SUPERSEDED by PROMPT-v3.md)

> **Do not dispatch against this file.** It predates eight adversarial
> reviews; `PROMPT-v3.md` is the master. Kept for the trail only.
>
> Original note: Paste this to the agent (or team lead) that scaffolds the repo. It is
> self-contained: it names the upstreams, the constraints, the lane map,
> and the acceptance gates. Lane workers get their `lanes/L*.md` file
> plus this repo — nothing else.

---

You are scaffolding **Capture Vector**: a cross-platform (macOS, Windows,
Linux) Rust desktop application that wraps the Boris publication compiler
in an Outlook-style modular workbench, with a native SEO/AEO audit engine
and a research/notes module. Read `spec/vision.md`, `spec/architecture.md`,
and `spec/contracts.md` first — they are normative.

## Upstreams (read-only; never modify)

- `github.com/drawmeanelephant/boris` — the engine. Zig binary, JSON
  contracts, subprocess boundary. `docs/contracts/` is the spec of record.
- `github.com/drawmeanelephant/oliver` — markup renderer; stdin/stdout CLI
  used for editor preview.
- `github.com/drawmeanelephant/solipsist` — the macOS predecessor. Steal
  its `Stunts/` corpora concept and its boundary rules ("never reimplement
  semantics; never swallow diagnostics"), not its workflow.
- `github.com/anomalyco/rune` — the UI framework. Pin a `dev` rev. Every
  UI claim is verified headlessly (PNG render + element-tree dump).

## The job

Produce a compiling, fixture-green cargo workspace where **lanes L1–L7 can
all start at once** and integrate without touching each other's code.

1. Land **L0** first — but keep it thin: workspace skeleton, `cv-core`
   contract stubs from `spec/contracts.md`, `cv-fixtures` with captured
   Boris corpora, `StubEngine`, CI, and a `DemoModule` proving the module
   contract renders headlessly. L0 is done when `cargo test --workspace`
   is green *without boris, a display, or the network*.
2. Dispatch `spec/lanes/L1.md` through `spec/lanes/L7.md` as parallel
   work orders. Each lane file is the worker's entire brief. The
   dependency graph is in `spec/README.md`; the only serializer is L0.
3. Integration rule: if two lanes collide, the answer is a `cv-core`
   contract change — never a cross-lane import, never a shared file
   outside Lane 0's ownership list.

## Hard rules (violations are review-blockers)

- Only `cv-engine` may spawn processes. Everything else talks to the
  `Engine` trait / `StubEngine`.
- Boris diagnostics, stderr, and exit codes flow to the Activity module
  intact. Swallowing them is the canonical bug.
- No Boris/Oliver semantic reimplementation anywhere. Mirror contracts,
  don't compute them.
- Every lane's tests pass on fixtures alone. No binary, no display, no
  network in `cargo test`.
- Secrets via OS keychain + stdin pipes only. Never argv, env, or files.
- rune pinned to a `dev` rev; bump deliberately, record the rev in
  `spec/UPGRADES.md`.

## Acceptance gates for groundwork complete

- [ ] `cargo build --workspace` on macOS, Windows, Linux CI.
- [ ] `cargo test --workspace` green with `CAPTURE_VECTOR_BORIS_BIN`
      unset and no network.
- [ ] `cv-app` boots headless to the module switcher and renders a golden
      PNG of each registered module's empty state.
- [ ] `StubEngine` replays a watch session and the Pages module shows
      Boris diagnostics as its own — no code path treats them differently.
- [ ] Audit lane produces findings against fixture IR with stable codes.
- [ ] Each lane file's "Done when" is checkable by a reviewer who did not
      write the lane.

## Working agreements

- Small PRs per lane; contract changes are their own PRs and merge first.
- Fixtures are captured once from real Boris and checked in — record the
  Boris version and command in each fixture's sidecar `.meta.toml`.
- If a spec file is wrong, fix the spec file in the same PR. Drifting
  spec is worse than wrong spec.
