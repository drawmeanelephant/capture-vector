# Architecture (v3 — normative)

Supersedes all prior architecture docs. The master prompt is
`PROMPT-v3.md`; this file is its structural companion. Decisions here
survived eight adversarial reviews — the rationale trail is in
`spec/synthesis/`.

## Stack

- Rust, edition 2024, cargo workspace. macOS / Windows / Linux.
- rune for all UI — **vendored, not a git dep.** Wave-0 probe proved a git
  dep cannot resolve: rune patches `parley_engine`/`winit`/`wgpu-hal`/
  `sctk-adwaita`/`accesskit_{windows,macos}` via local `vendor/` paths and a
  dependency's `[patch]` does not propagate to dependents. Rune lives at
  `vendor/rune` (submodule or `xtask`-managed checkout at a pinned `dev` rev
  recorded in `spec/UPGRADES.md`); dependents use `path` deps and replicate the
  six-entry `[patch.crates-io]` block in the workspace root `Cargo.toml`.
- rusqlite (bundled) for durable app state. redb rejected: predicates
  are the product (smart mailboxes, backlinks, triage) and FTS5 is free.
- Boris + Oliver as external binaries — subprocess, never linked in v1.
- No tokio requirement in contracts: ports are synchronous/channel-sink
  to match rune's `&mut State` + wake-drain model.

## Crate map — split by privilege and dependency, not by lane

```
cv-core       domain types + SYNC ports + event/state vocab. NO IO, NO async, NO UI.
cv-contracts  serde mirrors of Boris + migration-lab JSON, version-keyed. Fixture-decode tests only.
cv-fixtures   captured corpora + lab runs, StubEngine + StubStore + StubLab.
              Test fixtures for lanes. cv-app is the production exception for
              fixture-backed scenes; the Store port is cv_store::docs.
cv-engine     the ONLY Command owner: RawSubprocess + EngineScheduler.
cv-store      JSONL evidence trail + per-namespace migration tracks + app-data
              index paths. rusqlite indexes live in cv-ledger and cv-notes.
cv-rules      SEO/AEO: pure fn(ArtifactSet, Ruleset) -> Report.
cv-ledger     evidence: artifacts→checks→claims→deliveries.
cv-publish    target registry + PreflightGrant + delivery adapters.
cv-notes      capture inbox, research silo, promote-to-page.
cv-model      AppState slices + AppEvent + cascade dispatcher + pure *Model
              builders per lane directory. NO rune, NO IO.
cv-widgets    the ONLY crate importing rune/rune-kit (version firewall).
cv-ui-editor  rich editor — peeled day 1 (ropey, highlight, IME).
cv-app        composition root + rune shell wiring. Integrator-owned.
xtask         fetch-engines, fixtures refresh, codegen, parity checks, release.
```

DAG: `cv-app` → `cv-model`/`cv-widgets`/lane services → `cv-engine`/
`cv-store` → `cv-core`/`cv-contracts`. `cv-core` and `cv-contracts`
depend on nothing in-workspace. One `cv-ui` crate was explicitly
rejected — it creates a `cv-app`↔`cv-ui` cycle, defeats `cargo-deny`
(it can't see intra-crate module boundaries), and forces every lane's
tests to link the editor/GPU stack.

## Engine boundary — RawSubprocess only, capability-honest

`cv-engine` is the sole `std::process::Command` owner. `boris-editor`
(the upstream loopback host) was **evaluated and rejected for v1**:
watch/validate daemons unsupported on Windows, 100-event/4 MiB ring,
HTTP+token overhead, second conformance matrix — zero unique
capability. Its proven mechanisms are adopted into our contract
instead (fingerprinted saves, bounded ring + resumption cursor,
snapshot recovery). Re-evaluate if upstream gains Windows watch.

- `Engine` trait exposes **capability ports**: `watch`, `authoring`,
  `content_write`, `commands`. Platform gaps are honest, not hidden.
- Binary: `CV_BORIS_BIN` → bundled → PATH → sibling. `--version` gate +
  `--build-info` recorded. **Vendored from the same pinned commit as
  any future host — one pin in `MANIFEST.toml`. No BYO.**
- `EngineScheduler` per workspace: serialize mutating verbs,
  parallelize reads, one watch session; a running watch holds the
  `dist/` writer seat — mutating verbs stop-watch first or write to a
  separate output dir.
- Watch NDJSON rides **stderr**; `hello` + `watch_events_schema==1`
  handshake required or kill the child. Exit `0/1/2/3` →
  `Ok/ContentFailure/UsageError/IoFailure`.
- **CV-owned cursor** `(engine_session_id, seq)`; session change →
  `Engine::snapshot()` full resync. `broadcast` Lagged + malformed lines
  handled explicitly.
- Per-OS capability probe at startup; Windows fallback = one-shot
  builds + file watcher, decided by probe not assumption.
- Secrets: keychain → stdin at sign time only. Mock backend for CI.
- Custom Boris verbs fail closed when they would cross an authority
  boundary Capture Vector does not own. Standard.site custom access is
  limited to the offline artifact verbs `plan`, `records`, and `verify`;
  session-capable `login`, `sessions`, `logout`, `publish`, `smoke`,
  unknown future subcommands, and every `--session-root` spelling are
  refused before spawn rather than allowed to read or mutate Boris's
  default `$HOME` session store.

## State & identity

- `NounId = (AccountId, NounKind, StableId)`; `NounKind` branded newtype
  + `pub const` well-knowns + noun-view registry + mandatory fallback +
  a test asserting every produced kind has a view.
- `StableId` is **never a filesystem path** — renames/reparents must not
  break selection, backlinks, snoozes.
- `NounAttrRow` projection contract: lanes project nouns to uniform
  attribute rows; smart-mailbox conjunctions evaluate those, never a
  central `match`.
- `AppState` = per-lane slices in `cv-model`; `AppEvent` =
  **lane-namespaced** variants (`AppEvent::Audit(AuditEvent)`), payload
  enums in lane-owned files under `cv-core/src/events/`.
- Event envelope `{workspace, snapshot_epoch, build_id, cursor,
  event_id, payload}`; commit boundary = event + store mutation +
  derived-work outbox atomically; cursor advances after commit; derived
  work idempotent by `event_id + input_hash`.
- `Clock` + `IdGen` ports. Diagnostics carry build-version tags;
  stale-diagnostic policy is specified (dim or diff-remap).

## Store discipline

- `<project>/.cv/` — schema_version, single-instance lock, JSONL
  evidence trail (**source of truth**), notes silo pointer, prefs.
- SQLite lives in **OS app-data keyed by project id** — a rebuildable
  index (`.cv` WAL files under iCloud/Dropbox sync corrupt; this removes
  the hazard and makes `store-corrupt` recovery = delete + rebuild).
- Never persist Boris IR into SQLite — `Arc<GraphDocument>` in memory.
- Per-namespace migration tracks `(namespace, version)` — no global
  migration-integer contention across lanes.
- Atomic `*.tmp`+rename saves (Boris watcher treats rename as one clean
  change); Windows rename-retry semantics specified (sharing violations
  under reader handles).
- Note→page provenance lives in the `.cv/` sidecar map — **never
  frontmatter** (closed key set, hard `EFRONTMATTER`, verified).

## Parallelism rules (the crown jewels)

1. Wave 0 = chassis: skeleton, contracts, real-captured fixtures for
   ALL corpora (hand-authored Boris fixtures are **banned**),
   `StubEngine`, empty shell on fixture mailbox, **rune capability
   probe**, **canary vertical slice** (event → cascade → store →
   redraw). Then all lanes fan out.
2. `cv-core` append-only after wave 0, with an explicit
   **contract-hardening checkpoint after the first vertical slice,
   before full fan-out**.
3. `fixtures/MANIFEST.toml` Engine-owned; lanes assert by name; fixture
   format carries `schema_version`; `xtask fixtures refresh`; live-smoke
   CI on every pin bump; `StubEngine ≡ ProcessEngine` conformance runs
   continuously.
4. Ownership by path; workspace `Cargo.toml` members wave-0/integrator-owned.
5. **Element-tree + layout-geometry JSON goldens gate on 3 OSes**; PNG
   goldens gate one canonical software-rasterizer config, advisory
   elsewhere. Bless mechanic + named visual reviewer.
6. Every lane's golden set includes degraded states (engine-absent,
   version-mismatch, dead-mid-watch, store-corrupt).
7. Scale envelope is written down: 5k pages, diagnostics rivers,
   rebuild bursts — virtualized lists, event coalescing, `Arc` shared
   graph, stable-ID finding dedup `(code, subject, target-hash)`.
