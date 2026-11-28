# Architecture overview

This is the readable map for people joining the codebase. The normative
contract is [`../spec/architecture.md`](../spec/architecture.md); when this
overview and the spec disagree, the spec wins.

## One-sentence shape

Capture Vector is a Rust desktop shell that treats Boris as the only
compiler, mirrors Boris's machine contracts across a narrow subprocess
boundary, projects the result into lane-owned view models, and stores app
evidence in the workspace `.cv/` sidecar.

## Process boundaries

```text
+----------------------------------------------------------+
| cv-app                                                   |
| composition root, native shell wiring, launch/recovery   |
+----------------------------------------------------------+
| cv-widgets                                               |
| the only crate allowed to import rune/rune-kit           |
+----------------------------------------------------------+
| cv-model                                                 |
| pure workbench state, module models, commands, reducers  |
+-------------+------------------------+-------------------+
| lane crates | services             | evidence          |
| cv-notes    | cv-publish, cv-rules | cv-ledger, store  |
+-------------+------------------------+-------------------+
| cv-engine                         | cv-store            |
| only process spawner              | sidecar/index paths |
+-----------------------------------+---------------------+
| cv-core / cv-contracts                                  |
| domain vocabulary / serde mirrors of Boris contracts    |
+---------------------------------------------------------+
                          |
                          v
                   pinned boris binary
```

The arrows point downward only in spirit: upper layers may depend on lower
vocabulary and ports, never the reverse. `cv-core` and `cv-contracts` stay
in-workspace-dependency-free so every lane can compile against them without
inheriting UI, storage, or process machinery.

## Boundary rules that matter day to day

1. **Boris is a subprocess.** `cv-engine` is the only crate allowed to
   spawn it. Do not reparse Boris IR semantics elsewhere and do not shell
   out from a lane crate.
2. **rune is firewalled.** `cv-widgets` is the only crate importing rune.
   `cv-model` must remain testable without a GPU, display, or window.
3. **Events are lane-namespaced.** `AppEvent::Audit(AuditEvent)` and
   similar variants let lanes add events without editing one central enum
   for every change.
4. **Stable IDs are not paths.** Paths can rename; nouns cannot. Selection,
   backlinks, snoozes, and lifecycle state key off stable identities.
5. **`.cv/` is evidence, not content metadata.** Notes, lifecycle, and
   provenance live in the sidecar. Boris frontmatter remains closed; adding
   app-owned keys there produces real engine failures.
6. **SQLite is a rebuildable index.** JSONL evidence is the source of
   truth. Do not persist Boris IR into SQLite.
7. **Degraded states are product states.** Engine absent, version mismatch,
   watch dead, and corrupt store must render honestly and appear in tests.
8. **Fixtures are captured, not imagined.** Hand-authored Boris JSON is
   banned because it teaches the decoders a fiction.

## Current crate map

| Crate | Responsibility |
|---|---|
| `cv-core` | Domain types, identifiers, sync ports, event vocabulary |
| `cv-contracts` | Serde mirrors of pinned Boris JSON contracts |
| `cv-fixtures` | Captured corpora plus stub implementations for tests, fixture-backed scenes, and the in-memory Store |
| `cv-engine` | Boris discovery, subprocess execution, watch sessions |
| `cv-store` | Sidecar schema/migrations, `.cv/` JSONL helpers, confined paths, app-data index location (the SQLite indexes themselves live in `cv-ledger` and `cv-notes`) |
| `cv-rules` | Deterministic audit rule evaluation |
| `cv-ledger` | Artifact/check/claim/delivery evidence model |
| `cv-publish` | Publish planning, preflight, target adapters |
| `cv-notes` | Research capture, notes, promote-to-page behavior |
| `cv-model` | Workbench state and pure module/view-model logic |
| `cv-widgets` | Native shell widgets and all rune contact |
| `cv-ui-editor` | Editor surface, text buffer behavior, completions |
| `cv-app` | Composition root and executable entrypoint |
| `xtask` | Fixture, codegen, engine-fetch, parity, and release automation |

## Parallel development model

Lanes integrate through contracts and fixtures, not by importing each
other's implementation details. That is why issue claims
([`agent-claims.md`](agent-claims.md)) operate at issue granularity while
file ownership stays lane-oriented: coordination happens visibly on GitHub,
integration happens through `cv-core`/`cv-contracts`, and shared contract
changes land as their own PR before dependent lane work.

## Verification model

The portable verification surface is behavior plus element-tree/layout
JSON, not screenshots:

- `cargo test --workspace --locked` runs without a Boris binary, network,
  GPU, or display.
- `cargo run -p cv-app -- --scene canary --tree target/canary.tree.json`
  emits the shell tree headlessly.
- PNG goldens are canonical only on the Linux Lavapipe rasterizer because
  native GPU/font stacks do not produce byte-identical pixels across OSes.

See [`../spike/FINDINGS.md`](../spike/FINDINGS.md) for what is proven per
platform and what remains open.
