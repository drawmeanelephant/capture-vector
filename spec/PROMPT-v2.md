# Groundwork Prompt v2 — Capture Vector (SUPERSEDED by PROMPT-v3.md)

> **Do not dispatch against this file.** The frontier panel (Gemini
> Neon, Claude Opus, Kimi K3.1) overturned three of its locked
> decisions — the boris-editor host adoption, the single cv-ui crate,
> and deferred real-fixture capture. `PROMPT-v3.md` is the master.
>
> Original note: Self-contained kickoff prompt for scaffolding the
> repo. Supersedes `PROMPT-groundwork.md`. This version survives five adversarial reviews
> (three independent derivations + two attacks); its decisions encode
> their wins and the holes they found.

---

You are scaffolding **Capture Vector**: a cross-platform (macOS, Windows,
Linux) Rust desktop workbench that wraps the **Boris** publication
compiler in an **Outlook-style modular app** — write, research, audit
SEO/AEO, publish — with concurrency-by-construction so multiple agents
can work independent lanes simultaneously.

## Upstreams (read-only)

- `github.com/drawmeanelephant/boris` — the engine. Zig. Markdown →
  validated graph → frozen graph → HTML/IR/RAG/llms.txt/RSS/sitemap →
  publish targets. `docs/contracts/` is normative — including
  **`editor-host.md` (`boris-editor/0.1.0`)**, a conformance-tested
  loopback host with managed watch daemons, fingerprinted atomic saves,
  buffered events + `?after=` resumption, `/api/authoring`,
  `/api/recovery`. Also note: **Boris frontmatter is a closed key set —
  unknown keys hard-fail `EFRONTMATTER`.**
- `github.com/drawmeanelephant/oliver` — markup lib; stdin/stdout CLI +
  C ABI.
- `github.com/drawmeanelephant/solipsist` — macOS predecessor. Boundary
  rules to inherit: one process owner, never reimplement semantics,
  never swallow diagnostics.
- `github.com/anomalyco/rune` — GPU Rust UI framework. Pin a `dev` rev.
  Headless-verifiable (PNG, element tree, scripted input). **No webview.**

## The product model

A Boris publication is the account; **every artifact is a `Noun`** —
page, diagnostic, finding, plan, receipt, note, output file. One
`Selection` type; every surface reads it. Modules are **domain-oriented**
(Pages / Research / Audit / Publish / Activity) on a module rail —
each gets nav/list/reading/inspector providers; verbs live in the
command palette, not the rail. The Findings mailbox unifies Boris
diagnostics + ruleset findings + staleness (stale `llms.txt`,
unpublished outputs) into one triageable stream. Publish requires a
**Preflight gate** (validate + check + ruleset → claims-style report)
before any network action is armed. **Smart mailboxes** (saved
`field op value` conjunctions — no grammar, no joins, no regex) are the
Tinderbox gene and are v1.

## Architecture — the locked decisions

### Crates (~12)

```
cv-core       domain types + ports. NO IO, NO async, NO UI. Frozen after wave 0.
cv-contracts  serde mirrors of Boris JSON. Boris-version-keyed. Fixture tests only.
cv-fixtures   stunt corpora + golden JSON + StubEngine + StubStore. Dev-dep of all.
cv-engine     the ONLY Command owner. ProcessEngine + managed BorisEditorHost adapter.
cv-store      rusqlite (bundled) — predicates are the product; FTS5 for notes.
              JSONL append-only evidence trail inside .cv/. schema_version + migrations.
cv-rules      SEO/AEO engine: pure fn(ArtifactSet, Ruleset) -> Report.
cv-ledger     evidence: artifacts→checks→claims→deliveries (proof-pack vocabulary).
cv-publish    target registry + run orchestration + delivery adapters.
cv-notes      capture inbox, research silo, promote-to-page.
cv-theme      theme/motion as data. cv-widgets — the rune-version firewall.
cv-ui         ALL surfaces as directories: modules/, companions/, graph/, shell/.
cv-app        composition root: AppState slices, AppEvent bus, wake/drain loop.
xtask         fetch-engines, fixtures refresh, codegen, parity check, release.
```

UI surfaces live in **one crate** — per-surface crates are permanent
integration tax for a 30-second blast-radius win. `cargo-deny` bans +
ownership-by-path give the same discipline. Peel out `cv-ui-graph` only
if it drags unique GPU/layout deps.

### `Noun` — branded open string

`NounKind` newtype + `pub const` well-knowns in `cv-core` + a noun-view
registry with a **mandatory generic fallback renderer** + a test
asserting every produced kind has a registered view. NOT a sealed enum
(`Other(String)` is exhaustiveness theater + a serialization trap).

### Engine boundary — the corrected design

Two adapters behind one `trait Engine`:

1. **`BorisEditorHost`** — a managed `boris-editor` child per workspace,
   for watch sessions, fingerprinted file writes, event resumption, and
   `/api/authoring` completion. This exists, it's conformance-tested,
   and it already solved the problems an ad-hoc supervisor would
   reinvent badly.
2. **`RawSubprocess`** — direct verbs for everything the host's JSON API
   can't carry: secrets-over-stdin (`nostr sign --key-stdin`), exports,
   plan/sign/publish, `init`, and as the degraded fallback when the host
   can't run.

Contract requirements either way: **fingerprinted saves** (SHA-256+mtime,
the file-changed-under-buffer problem), a **resumable event cursor**
(steal the `?after=` model — the app must resync after sleep/restart,
not fold a stderr pipe into state forever), `Engine::snapshot()` as the
re-read-disk-truth authority, and an explicit `EngineScheduler`
per workspace: **serialize mutating verbs, parallelize reads, one watch
session**. `boris watch` NDJSON rides **stderr** (stdout stays empty);
first record must be `hello` with `watch_events_schema==1` or kill the
child — never degrade silently. Exit codes `0/1/2/3` map to
`Ok/ContentFailure/UsageError/IoFailure`.

**`ContentWriter` does NOT validate frontmatter in Rust** — that's
reimplementing Boris's grammar (pillar violation). It does a
fingerprinted staged write and lets `boris validate` judge, or pipes the
draft to `boris validate` on a tempfile. Note→page provenance lives in a
`.cv/` sidecar map — NOT frontmatter (closed key set, verified).

### Dataflow — write this paragraph into `cv-core` before wave 1

The additive `AppEvent` enum is a merge strategy, not a dataflow design.
`cv-core` must specify the cascade: event → reducer → emitted-intent
ordering (build-finished → store persists IR → rules re-eval → findings
refresh → staleness badges), idempotency (same event applied twice =
same state), replay semantics (snapshot + cursor), and who originates
derived work (the bus's dispatcher, not lane-chained intents). Findings
may arrive before the IR they reference — ordering and buffering are
specified, not discovered at integration.

### Determinism seams in frozen contracts

`Clock` + `IdGen` ports (stale_page rules, run timestamps, finding dedup
all need "now"; fixture replay must be byte-identical). Finding dedup
keys = stable `code + subject-id` (content-stable, survives reparenting —
path-keyed snoozes silently resurrect on page moves). Diagnostics carry
a **build-version tag**; the editor's stale-diagnostics policy (dim or
diff-remap) is specified, since spans are computed against the last
build while typing has moved on.

### `.cv/` sidecar discipline

`<project>/.cv/` — schema_version + migration chain from v1;
single-instance file lock; findings lifecycle, notes silo pointer,
ledger, prefs. Never write `content/`, `dist/`, `.boris/` except on
explicit save (atomic `*.tmp`+rename — Boris's watcher treats it as one
change). **UI must disclose that `.cv/` is gitignored by default** —
unprotected research is a data-loss story otherwise.

### Fixture mechanics — the parallelism mechanism, hardened

- Engine-owned `fixtures/MANIFEST.toml`: per-corpus hash + Boris version
  + capture command. Lanes assert **by name**, never embed hashes.
- Fixture *format* has its own `schema_version` — capture-format changes
  don't silently invalidate every lane's pins.
- `xtask fixtures refresh` regenerates; a CI job runs the `#[ignore]`d
  live-smoke tests **on every pin bump** — scheduled, not aspirational.
- **Conformance suite**: `StubEngine ≡ ProcessEngine` on identical input
  streams. Without it, lanes stay green for weeks on fixtures that no
  longer match the gated binary.
- L0 is **staged**: contract stubs + StubEngine + `happy/` corpus land
  first to unblock lanes; `broken-*` and publish corpora are contributed
  additively by the lanes that need them.

### Editor & preview — honest split

- Editor = the first spike (hardest component). Structured editor
  (frontmatter form + body) is the floor; live-decorated rich Markdown
  via `TextEdit::rich` + decorations is the goal; `completion.json` /
  `/api/authoring` is the legal completion surface.
- **No in-app themed site preview, ever.** Native noun view renders via
  `rune_kit::Markdown` (advisory); authoritative preview = user's own
  browser on `boris watch --serve` loopback. Advisory-vs-authoritative
  parity check (`xtask` diffs Oliver fragments vs Boris bodies) is a
  standing lane, budgeted.
- Oliver: **subprocess** default (`OliverRender` engine command,
  ~100 ms debounce). C-ABI `oliver-sys` is a feature-flagged optimization
  lane, never on the critical path.
- Companion windows (editor + preview share one buffer entity): wave 0
  must define per-window vs shared `Selection` scope and dirty-close
  semantics — it's an L2↔L7 collision waiting to happen otherwise.

### Scale envelope — write it down

Design for 5k pages, diagnostics rivers, rebuilds every few seconds:
virtualized lists mandatory, event-burst coalescing in the actor,
`Arc<GraphDocument>` (don't clone into AppState), and "Activity filters
nothing" means *nothing is dropped semantically* — the view virtualizes
or the bus drowns.

### Degraded modes — every lane's golden set must include

engine-absent / version-mismatch / dead-mid-watch / store-corrupt
states. Doctor surface ("your boris speaks a contract I don't know")
lives **in-app**. Keychain needs a mock backend for headless CI; the
publish lane's secret flow needs a defined degraded path.

### SEO/AEO rules — cited data, honest vocabulary

TOML rule packs with `{ id, severity, scope, inputs, check, remediation,
citation{source, url, frozen-quote, captured} }`. `ruleset/SOURCES.md` is
**generated** from citations. Honesty vocabulary borrows Boris's proof
model: `checked/passed/failed/limited` — scores decompose to findings,
never letter grades. Packs v1: seo-core, aeo-llm (llms.txt/answer-first/
JSON-LD), a11y-content, targets (per-target requirements). No LLM in the
scoring path ever; advisory AI is a flag-gated deletable lane.

### Parallelism rules (final)

- Wave 0 = chassis only: skeleton + contracts + StubEngine + `happy/`
  corpus + empty shell rendering a fixture mailbox headless. Then ALL
  lanes fan out.
- `cv-core` append-only after wave 0 — BUT plan an explicit
  **contract-hardening wave** between stub-integration and
  real-engine integration. Wave-0's guesses are not load-bearing forever;
  the amendment process is cheap on purpose.
- `AppEvent`/`AppState`: additive variants/fields per lane.
- Mailbox/noun vocabularies: open strings. Theme keys: namespaced.
  Fixtures: additive files. Workspace `Cargo.toml` members: wave-0/integrator-owned.
- Every lane verifies alone: types via `cv-core`, behavior via
  `cv-fixtures`, UI via headless goldens with a **bless mechanic** and a
  named reviewer for visual diffs on rune rev bumps.
- `cv-cli` does not ship. Layering is proven by a compile-test (service
  crates callable without `cv-app`), fixture regen lives in `xtask`,
  doctor lives in-app.

## Non-goals v1

No second compiler / no Boris-Oliver semantic reimplementation. No
webview or in-app themed preview. No WYSIWYG block editor. No live
SERP/rank APIs. No sync/collab/accounts. No shipped CLI binary. No LLM
in scoring. No publish on a timer — every network action is explicit
user intent through Preflight.

## Open questions for the owner (decide before wave 1)

1. Vendored pinned boris/oliver binaries in-app (recommended) vs BYO?
2. Editor bar: structured-first vs rich-editing-first?
3. Which delivery adapters are v1 beyond Boris targets: IndexNow ping,
   sitemap submit, webhook POST, copy-to-folder?
4. macOS-first or all-three-OS parity at launch? Signing/notarization
   budget (Solipsist parked here)?
5. Solipsist import path in scope or clean break?
6. How hard is the EDM bar — is the graph visualizer signature or garnish?

## Acceptance gates

- `cargo test --workspace` green on 3 OSes, no boris, no display, no net.
- App opens a fixture workspace headless → mailbox tree + noun list +
  inspector, golden PNG green.
- `StubEngine ≡ ProcessEngine` conformance suite green.
- Resync proven: kill watch mid-session, restart, state converges via
  snapshot+cursor — no ghost diagnostics.
- Preflight gate blocks a publish on fixture findings.
- Every module renders its degraded-mode goldens.
