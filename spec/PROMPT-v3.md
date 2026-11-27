# Groundwork Prompt v3 — Capture Vector

> The self-contained kickoff prompt for scaffolding the repo. Supersedes
> `PROMPT-groundwork.md` and `PROMPT-v2.md`. Refined through eight
> adversarial reviews across four model families (SWE-2 ×3 + x2
> adversaries, Gemini 4 Neon, Claude Opus 5.5, Kimi K3.1 Pro). Every
> decision below either survived or was overturned by evidence — the
> trail lives in `spec/synthesis/`.

---

You are scaffolding **Capture Vector**: a cross-platform (macOS, Windows,
Linux) Rust desktop workbench that wraps the **Boris** publication
compiler in an **Outlook-style modular app** — write, research, audit
SEO/AEO, publish — engineered so multiple agents work independent lanes
from day one.

## Upstreams (read-only)

- `github.com/drawmeanelephant/boris` — the engine (Zig). Markdown →
  validated graph → frozen graph → HTML/IR/RAG/llms.txt/RSS/sitemap →
  publish targets. `docs/contracts/` is normative. **Frontmatter is a
  closed key set — unknown keys hard-fail `EFRONTMATTER`.** Watch NDJSON
  rides **stderr**; first record must be `hello` with
  `watch_events_schema==1`. Exit codes `0/1/2/3` →
  `Ok/ContentFailure/UsageError/IoFailure`.
- `github.com/drawmeanelephant/boris` → `docs/contracts/editor-host.md`
  (`boris-editor/0.1.0`) — a loopback authoring host. **We studied it and
  rejected it for v1**: watch/validate daemons are unsupported on
  Windows, its event ring caps at 100 events/4 MiB, it adds HTTP+token
  plumbing and a second conformance matrix for zero unique capability.
  **Steal its proven mechanisms instead** — fingerprinted saves,
  bounded event ring + resumption cursor, snapshot/recovery — into our
  own contract. Re-evaluate if upstream gains Windows watch + verb
  coverage.
- `github.com/drawmeanelephant/oliver` — markup lib; stdin/stdout CLI +
  C ABI. Used for non-Markdown corpora (Textile/Cooklang) and advisory
  fact extraction only.
- `github.com/drawmeanelephant/solipsist` — macOS predecessor. Boundary
  rules inherited: one process owner, never reimplement semantics, never
  swallow diagnostics.
- `github.com/anomalyco/rune` — GPU Rust UI framework. **Vendor at a pinned
  `dev` rev (git dep cannot resolve — see architecture.md §0 and
  `spike/FINDINGS.md`);** consumer workspace replicates rune's `[patch]` block.
  Headless-verifiable. **No webview.** Pre-release: all rune contact is
  firewalled behind one crate.

## The product model

A Boris publication is the account; **every artifact is a `Noun`** —
page, diagnostic, finding, plan, receipt, note, output file. One
`Selection` value type every surface reads. Modules are
**domain-oriented** (Pages / Research / Audit / Publish / Activity) on a
module rail; each supplies nav/list/reading/inspector providers. Verbs
live in the command palette, not the rail. The **Findings mailbox**
unifies Boris diagnostics + ruleset findings + staleness into one
triageable stream with lifecycle states. **Smart mailboxes** (saved
`field op value` conjunctions over indexed attributes — no grammar, no
joins, no regex) are the Tinderbox gene; v1. Publish requires a
**Preflight grant** bound to the exact artifact set it checked.

## Crates (~13) — split by privilege and dependency, not by lane

```
cv-core       domain types + SYNC ports + event/state vocab. NO IO, NO async, NO UI.
cv-contracts  serde mirrors of Boris JSON, version-keyed. Fixture-decode tests only.
cv-fixtures   captured corpora + StubEngine + StubStore. Dev-dep of every lane.
cv-engine     the ONLY Command owner: RawSubprocess adapter + EngineScheduler.
cv-store      JSONL evidence trail + `.cv/` sidecar migrations + app-data index paths.
              The rusqlite(bundled) indexes live in cv-ledger and cv-notes.
cv-rules      SEO/AEO: pure fn(ArtifactSet, Ruleset) -> Report.
cv-ledger     evidence: artifacts→checks→claims→deliveries.
cv-publish    target registry + PreflightGrant + delivery adapters.
cv-notes      capture inbox, research silo, promote-to-page.
cv-model      AppState slices + AppEvent + cascade dispatcher + pure *Model
              builders per lane directory. NO rune, NO IO. Millisecond tests.
cv-widgets    the ONLY crate importing rune/rune-kit (version firewall).
cv-ui-editor  rich editor — peeled day 1 (ropey, highlight, IME deps).
cv-app        composition root + rune shell wiring. Integrator-owned.
xtask         fetch-engines, fixtures refresh, codegen, parity checks, release.
```

`cv-model` holds state and pure view-model builders — so `cv-app`→
`cv-model`→`cv-widgets` is a DAG, no cycle. Modules self-register via an
xtask-generated list (integrator-owned) — no merge hotspot in lib.rs.

## Engine boundary — RawSubprocess, all three OSes

`cv-engine` is the only `std::process::Command` owner. One adapter;
the `Engine` trait exposes **capability ports** (`watch`, `authoring`,
`content_write`, `commands`) so a future `BorisEditorHost` adapter or
platform gaps are honest, not hidden.

- Search order `CV_BORIS_BIN` → bundled → PATH → sibling checkout;
  `--version` gate + `--build-info` recorded. **Vendor boris (and
  boris-editor if ever adopted) from the same pinned commit — one pin in
  `MANIFEST.toml`. BYO binary is not offered.**
- `EngineScheduler` per workspace: serialize mutating verbs, parallelize
  reads, one watch session, **a running watch holds the `dist/` writer
  seat** — mutating verbs either stop-watch first or write to their own
  output dir.
- **Resumable event cursor owned by cv-engine**: `(engine_session_id,
  seq)` over a normalized stream; session change → `Engine::snapshot()`
  full resync. Never fold a pipe into state forever; `broadcast` Lagged
  and malformed lines are handled, not tolerated silently.
- **Fingerprinted staged saves** (SHA-256+mtime, `*.tmp`+rename — which
  the Boris watcher treats as one change). `ContentWriter` does **not**
  validate frontmatter in Rust — that reimplements Boris's grammar. It
  writes staged + lets `boris validate` judge, or validates via tempfile.
- Secrets: OS keychain → stdin at sign time only (`nostr sign
  --key-stdin`). Never argv/env/disk/logs. Keychain mock for headless CI.
- Probe capability at startup per OS; Windows path is one-shot builds +
  file watcher if watch proves unsupported — decided by probe, recorded,
  surfaced in Doctor.

## Dataflow — specified in `cv-core` before wave 1

- **Event envelope**: `{workspace, snapshot_epoch, build_id, cursor,
  event_id, payload}` where payload enums are **lane-namespaced**
  (`AppEvent::Audit(AuditEvent)` — lane-owned files under
  `cv-core/src/events/`; additive still collides textually otherwise).
- **Commit boundary**: event + store mutation + derived-work outbox
  commit atomically; cursor advances only after commit. Derived work is
  idempotent by `event_id + input_hash`. Findings arriving before their
  IR: buffered and evicted per spec.
- **Cascade spec** (the paragraph everyone asked for): build-finished →
  persist IR → rules re-eval → findings refresh → staleness badges —
  ordering, idempotency, replay semantics written as a state-machine
  paragraph in `cv-core` docs.
- Ports are **synchronous/channel-sink** — matches rune's `&mut State` +
  wake-drain model. `cv-core` contains no async.
- `Clock` + `IdGen` determinism ports (stale_page, run timestamps, dedup
  all need "now"). Diagnostics carry **build-version tags**; editor
  stale-diagnostic policy (dim or diff-remap) is specified.

## Identity & store

- `NounId = (AccountId, NounKind, StableId)` — `NounKind` branded newtype
  with `pub const` well-knowns + noun-view registry + mandatory fallback
  renderer + a test that every produced kind has a view.
- **`StableId` is never a filesystem path** — renames/reparents must not
  break selection, backlinks, or snoozes.
- `NounAttrRow` projection contract in `cv-core`: every lane projects
  its nouns to uniform attribute rows — that's what smart-mailbox
  predicates evaluate (no central `match` on kind).
- **Do not persist Boris IR into SQLite** — `Arc<GraphDocument>` in
  memory + durable metadata; smart-mailbox conjunctions scan in memory.
- `.cv/` sidecar: **JSONL evidence trail is the source of truth;
  SQLite (in OS app-data, keyed by project id) is a rebuildable index** —
  `store-corrupt` recovery = delete + rebuild. No dual-write peers.
  Per-namespace migration tracks `(namespace, version)` — lanes migrate
  independently, no global integer contention. Store actor + WAL reader
  pool so sync rusqlite never blocks the rune loop.
- `.cv/` has schema_version + single-instance lock; **UI discloses it's
  gitignored by default.** Don't duplicate boris-editor's recovery
  snapshots — host owns dirty-buffer recovery if adopted; otherwise the
  fingerprinted-save protocol covers it.

## Editor & preview — honest split

- **Editor = wave-0 spike.** Structured floor (frontmatter form + body),
  rich goal (`TextEdit::rich` + decorations). Completion from Boris's
  `completion.json` — the sanctioned surface.
- **No in-app themed site preview. No companion preview window.**
  `rune_kit::Markdown` renders the draft *in the reading pane* labeled
  "draft sketch"; authoritative preview = user's own browser on
  `boris watch --serve`. `OliverRender` is out of the typing loop; Oliver
  serves non-Markdown corpora + advisory `DocFacts` extraction
  (html5ever → inspector).
- Parity check compares **`rune_kit::Markdown` output vs Boris-emitted
  bodies** — the real user-visible divergence (wikilinks, `{{include}}`,
  `<Aside>`, `<Details>`), not Oliver-vs-Boris (Boris embeds Oliver;
  that diff proves nothing).
- Multi-window (editor popouts): wave 0 defines per-window vs shared
  `Selection` scope and dirty-close semantics.
- Lock in wave-0 contracts: **diagnostic span units** (byte vs UTF-8
  char vs UTF-16, 0/1-based, CRLF — or `ropey` panics on multi-byte) and
  **Windows atomic-save retry** (`rename` fails under read handles).

## SEO/AEO — cited, tiered, honest

- `ArtifactSet` = content-hashed manifest: `{ IR, final HTML, exports,
  TargetDeployProfile (base_url, slash policy, expected robots.txt),
  missing-input states }`. canonical/robots/JSON-LD checks need **final
  HTML**, not IR.
- `CheckSpec` = closed versioned algebra: typed selectors + thresholds.
  No inventing DSLs per lane.
- Every rule carries `authority_tier: NormativeSpec | EngineDocumented |
  Heuristic` + `archetype_scope` (trunk/satellite/faq/article/changelog)
  + `citation{source, url, frozen-quote, captured}`.
  `ruleset/SOURCES.md` is generated from citations.
- **Preflight blocks only on `NormativeSpec`/`EngineDocumented` errors.**
  Heuristic findings (answer-first, entity consistency…) are advisory
  forever — folklore must never gate a publish or train users to
  mass-snooze.
- Honesty vocabulary `checked/passed/failed/limited`: anything requiring
  live HTTP headers, redirects, or a real browser (CWV) reports
  **`limited`** — never `passed`.
- Cross-artifact parity checks: HTML canonical ≡ sitemap ≡ RSS ≡
  llms.txt ≡ internal hrefs ≡ deploy profile. Draft/noindex leakage into
  sitemap/rss/llms/RAG is a blocking error.

## Preflight & publish — sealed, reconcilable

`PreflightGrant` binds `{ artifact_set_hash, frozen_graph_hash,
plan_hash, target_config_hash, ruleset_hash, boris_version }`. Preflight
builds the **target profile** into its own output dir (never the watch
build — its URLs are localhost-shaped). Publish delivers exactly the
sealed plan; hash mismatch = refuse. Approval is persisted **before**
dispatch. Run states: `dispatching → submitted | unknown | complete` —
a crash after network contact but before the report lands is
**`unknown` requiring reconciliation**, never auto-retry.

## Fixtures — the parallelism mechanism, hardened

- **Ban hand-authored Boris fixtures.** Wave 0 runs scripted capture
  against the pinned binary for all corpora (`happy/`, `broken-*`,
  `check`, `plan`, publish reports) *before* `cv-contracts` freezes —
  fixtures captured from reality or not at all.
- Engine-owned `fixtures/MANIFEST.toml` (hash + boris version + capture
  command per corpus); lanes assert by name. Fixture *format* has its
  own `schema_version`.
- `xtask fixtures refresh` + CI runs `#[ignore]`d live-smoke **on every
  pin bump**.
- **`StubEngine ≡ ProcessEngine` conformance suite** runs continuously
  from the moment the engine lane lands.
- Contract-hardening checkpoint lands **after the first vertical slice,
  before full wave-1 fan-out** — wave-0 guesses are not load-bearing.

## Verification & wave 0

- **Element-tree + layout-geometry JSON goldens are the 3-OS CI gate**
  (deterministic, agent-readable diffs). PNG goldens gate on ONE
  canonical software-rasterizer config (Linux Lavapipe + bundled TTF);
  advisory elsewhere — cross-OS pixels never match.
- Every module's golden set includes degraded states: engine-absent /
  version-mismatch / dead-mid-watch / store-corrupt (which is now
  "delete index, rebuild").
- Wave-0 additions: a **Rune capability probe** (headless multi-window,
  5k-row virtual list, `TextEdit::rich` decorations+IME on 3 OSes) and a
  **canary vertical slice** (StubEngine event → cascade → store write →
  wake redraw — proves the async pattern every lane will copy).
- Golden updates have a bless mechanic + named visual-diff reviewer.

## Non-goals v1

No second compiler; no Boris/Oliver semantic reimplementation. No
webview, no in-app themed preview, no companion preview window. No
WYSIWYG block editor. No live SERP/rank APIs. No sync/collab/accounts.
No shipped CLI binary (layering proven by compile-test; doctor lives
in-app). No LLM in scoring; advisory AI is flag-gated and deletable. No
publish on a timer — every network action is explicit user intent
through Preflight.

## Owner decisions (2026-11-27)

- Delivery adapters: `copy-to-folder` is the only v1 delivery adapter.
  IndexNow, webhook, and sitemap ping are post-v1 roadmap scope. Sitemap
  ping must be revalidated before implementation because Google's classic
  endpoint is deprecated and returns 404.
- Solipsist: clean break. Solipsist and Capture Vector are different
  products; no Solipsist import mode is in scope.

## Open questions for the owner

1. Editor bar: structured-first vs rich-editing-first?
2. macOS-first or 3-OS parity at launch? Signing/notarization budget?
3. Graph visualizer: signature feature or garnish?
4. Single window + popouts (recommended) vs multi-window?

## Acceptance gates

- `cargo test --workspace` green on 3 OSes, no boris, no display, no net.
- Element-tree goldens green on 3 OSes; PNG golden green on canonical
  rasterizer.
- `StubEngine ≡ ProcessEngine` conformance suite green.
- Resync proven: kill watch mid-session, restart → snapshot+cursor
  converges; no ghost diagnostics.
- Preflight refuses when sealed hashes don't match the build about to
  ship; `unknown` state demonstrably recoverable.
- A heuristic-tier finding demonstrably cannot block publish.
- Canary vertical slice passes: one `BuildFinished` → findings → snooze
  → redraw, scripted headless.
