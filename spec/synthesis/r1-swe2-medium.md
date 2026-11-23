I have enough grounding. Writing the spec directly in the reply below.

---

# Capture Vector — Technical Design Specification

**Status:** design draft, v0. Independent derivation; nothing here references the existing `spec/` tree.

---

## 1. Product shape

**What it is:** Capture Vector is a desktop "publication client" — an Outlook-style three-pane workbench where a Boris publication is the account, pages are the messages, and Boris's machine outputs (JSON IR, RAG packs, `llms.txt`, RSS, sitemap, ATProto/Nostr publications) are the outbox. The user authors Markdown in a native-feeling notes surface, compiles continuously, inspects SEO/AEO health per page and per site, and pushes machine-facing artifacts back out to their targets.

**The Mail → Outlook metaphor, mapped to architecture.** This is the load-bearing insight, so I'll be precise:

| Mail/Outlook concept | Solipsist (the predecessor) | Capture Vector |
|---|---|---|
| Account | one Boris source dir | a *Workspace* = `{ boris profile, sources, publish targets, SEO ruleset }` |
| Mailbox / folder | pages, outputs, publish mailboxes | same, plus **Outbox** (staged publication plans awaiting push) and **Clippings** (research notes not yet in the graph) |
| Message | a page's HTML render | a page's *compiled projection* — HTML, IR node, SEO verdict, publish status, all derived from one frozen graph node |
| Reading pane | preview host | preview + inspector showing compiled state, diagnostics, and per-target status |
| Drafts | compose buffer | native editor (Oliver-backed preview), dirty state reconciled against the frozen graph |
| Send | publish | explicit `plan → sign/commit → push` per target, echoing Boris's own plan/sign/publish discipline |

The key shift from Solipsist: Solipsist is a *harness* around `boris` the binary, Mail-style. Capture Vector is a *client* in the Outlook sense — it owns richer local state (notes, clippings, SEO findings, publish receipts) that lives **alongside** the Boris content tree but never inside the compiler's input domain.

Where the brief says "Google Notebook and Microsoft Outlook had a baby" — that means: **the notes/research surface is a first-class pane, not a second product.** Research notes (clippings) are Oliver-markdown documents stored under `.cv/notes/` in the workspace. They can be *promoted* into the content graph (become a Satellite) or cited by pages without ever becoming pages themselves. DEVONthink/Tinderbox shows up as a graph view and typed backlinks; Notion shows up as block-level editing feel — but we do **not** build a block editor (see risks).

**"Always publishing back engine shits to the engines"** = the Outbox is a real queue. Every Boris machine output that has a network home (RSS feed deployment, `llms.txt` to the site root, Standard.site records, Nostr events, GitHub Pages deploys) is staged as a plan artifact, then pushed with recorded evidence — reusing Boris's own evidence-chain model (artifacts → checks → claims → Proof Pack) as the UX's publish ledger.

---

## 2. Crate/module architecture

A cargo workspace. Design goal: **lanes compile independently; only `cv-core` types cross lane boundaries.**

```
capture-vector/
  Cargo.toml                    # workspace root
  crates/
    cv-core/        # THE contract crate: domain types, serde schemas, ports (traits). No deps on rune, tokio, or engine. <2k LOC.
    cv-engine/      # Boris + Oliver process/FFI boundary. Implements cv-core's EnginePort. Owns all subprocesses.
    cv-store/       # Local state: workspace DB (SQLite via rusqlite), notes, receipts, SEO findings cache.
    cv-seo/         # Deterministic SEO/AEO rules engine over frozen-graph IR. Pure: IR in → findings out.
    cv-publish/     # Outbox: target adapters (gh-pages, standard.site, nostr, static fs). Plan/commit/push ledger.
    cv-model/       # Application state machine: workspace load, compile loop, selection, dirty tracking. Pure Rust, headless-testable.
    cv-ui/          # rune app: panes, editor surface, graph view, outbox view, inspector. The ONLY rune-dependent crate.
    cv-cli/         # headless entry: same model as GUI; `cv compile`, `cv seo`, `cv outbox push` for CI and dogfooding.
    capture-vector/ # thin binary: wires cv-model → cv-ui; also runs cv-cli mode by argv.
  xtask/            # codegen: contract schema snapshots, SEO ruleset snapshots, rune rev pin check.
```

**Hard rules that make parallelism work:**

1. `cv-core` has zero async, zero IO, zero UI. Everything it defines is `serde` + plain types. It is the freeze-first crate: its public API is versioned and PRs to it require a schema-snapshot diff via `xtask`.
2. `cv-model` and `cv-seo` and `cv-publish` depend on `cv-core` only — they take `EnginePort`/`StorePort` as trait objects. Every non-UI lane is fully testable with a `StubEngine` in `cv-engine`'s `testkit` feature.
3. `cv-ui` is a pure view layer over `cv-model` state: `fn view(s: &AppState, cx: &mut Cx) -> El`, exactly the rune idiom. UI never calls `cv-engine` or `cv-store` directly — it dispatches intents into `cv-model`.
4. `cv-cli` exists so every lane can be exercised without a GPU or a display. This doubles as rune's own headless-verification story: `cv-cli` emits the element tree / rendered PNGs for UI review (rune supports headless render — use it in CI).

**Ports in `cv-core` (the three traits that gate everything):**

```rust
trait EnginePort {      // implemented by cv-engine only
    fn compile(&self, req: CompileRequest) -> Result<CompileEvent>;
    fn watch(&self, sink: EventSink) -> WatchHandle;      // NDJSON events, mirroring boris watch --watch-json
    fn validate(&self) -> Result<DiagnosticSet>;
    fn plan(&self, target: TargetId) -> Result<PublishPlan>;
}
trait StorePort {       // implemented by cv-store only
    fn get(&self, key: &Key) -> Result<Option<Doc>>;
    fn put(&self, doc: Doc) -> Result<()>;
    fn query(&self, q: Query) -> Result<Cursor<Doc>>;
}
trait Clock/IdGen {}    // determinism seam — every lane tests with FixedClock
```

---

## 3. Boris + Oliver integration boundary

**Draw the line at the process boundary, not the FFI boundary — with one exception.**

- **Boris**: invoke as a **pinned external binary**, never linked, never reimplemented. Rationale:
  - Solipsist proved this is the right seam: "subprocess isolation is a feature," "never reimplement Boris semantics in Swift." Same rule in Rust.
  - Boris is Zig, pinned by version (`boris --version` gives compiler id). We vendor per-platform binaries at install/package time — NOT in the repo. Search order: `CV_BORIS_BIN` env → app-bundled `Resources/boris` → workspace-local override → explicit settings. Missing engine = first-run setup screen, not a crash.
  - All Boris interaction is `--watch-json` NDJSON events + JSON artifacts (`--out`, `--rag`, `--context`, `--llms`, `--rss`, `--sitemap`, `nostr plan/sign/publish`, `standard-site plan/publish`). `cv-engine` owns the only `std::process::Command`; typed `serde` decoders in `cv-core` mirror Boris's published contracts.
  - **Pin discipline**: `Cargo.toml`/`xtask` carries `BORIS_PIN = "0.9.x"` + sha256 per platform; `cv-engine` refuses a binary whose `--version` doesn't match, with an upgrade path.

- **Oliver**: two surfaces.
  1. **Live editor preview** — link Oliver's **C ABI** (`include/oliver.h`, caller-allocates, `oliver_free`) into `cv-engine` via a small `oliver-sys` + safe wrapper. In-process because keystroke-rate rendering needs sub-millisecond calls and Oliver is already "no global state, deterministic." Boris embeds it in-process; we do the same.
  2. **Everything else** — when the question is "what will Boris emit," **never** render Markdown ourselves. Ask Boris (`compile` the frozen graph) and show that. Oliver-preview is explicitly labeled *approximate* in the UI gutter; the authoritative render is always the compiler's. This kills an entire class of "preview ≠ publish" bugs Solipsist had to fight.

- **Nostr key handling**: reuse Boris's plan→sign→publish three-command split verbatim. `cv-publish` never holds a secret; the *signing step* prompts and pipes the key via stdin to `boris nostr sign --key-stdin` exactly once. Do NOT implement signing in Rust — the secp256k1/BIP-340 work is already verified in Boris; duplicating it is pure risk.

- **Secrets policy**: all non-Nostr credentials (ATProto app password, GitHub token) live in the OS keychain via `keyring` crate, injected into Boris subprocess env at push time only, never persisted in `cv-store`, never logged.

---

## 4. SEO/AEO design — deterministic, citable, offline

`cv-seo` is a **rules engine over Boris's JSON IR**, not a linter over Markdown source. This matters: findings must cite the *compiled artifact* (what engines actually see), not the source.

**Architecture:**

```
frozen graph + JSON IR + emitted HTML + llms.txt + RSS + sitemap
                        │
                        ▼
                cv-seo RuleSet (TOML, versioned, snapshot-tested)
                        │
                        ▼
        Findings: { rule_id, severity, node_id, span?, evidence, fix_hint }
```

- **Inputs are Boris outputs.** Rules consume: IR nodes (title, description, headings, links, dates, canonical), emitted HTML (scraped for `<meta>`, OG tags, JSON-LD presence), `llms.txt` (chunk coverage, token budgets), RAG packs (are key pages in working-context packs?), sitemap/RSS (coverage, freshness).
- **Rulesets are data, not code.** Each rule: `{ id, version, description, source_citation, check = jq-like predicate or builtin, severity, fix_hint }`. Builtins cover structured checks (heading hierarchy, title length, JSON-LD validity against schema.org subset, answer-first paragraph detection for AEO, entity/FAQ markup, canonical consistency, llms.txt completeness). Ships a `default` ruleset compiled in; workspaces can layer `rules.d/*.toml`. Every rule carries a `source` field citing the practice it encodes — that's the "citable" requirement: findings say *why* and *according to whom*.
- **Deterministic by construction.** `cv-seo` is pure: same frozen-graph hash + same ruleset hash → byte-identical findings (sorted, stable IDs). No network calls, no LLM in the scoring path. An *optional* `cv-seo-llm` lane may add suggestions later, but it annotates findings, never generates them — and it's behind a workspace flag, default off.
- **Offline.** Everything runs against the local frozen graph. "Check against live SERP/answer-engine" is explicitly a non-goal for v1.
- **AEO specifics** (answer-engine optimization): rules check that pages have a self-contained answer block early (first ~60 words answer the page's question), FAQ/Q&A schema where appropriate, stable heading anchors, `llms.txt` covering all public trunks, RAG pack completeness, and entity consistency (same `sameAs`/org data across pages).
- **Where findings live:** per-node findings merge into `cv-model`'s page projection (inspector shows them inline), and site-level findings populate a dedicated **Health mailbox**. Each finding links to the exact IR node/HTML element — click → jump to source span.

---

## 5. Parallel-lane decomposition

The crowning requirement. The map:

| Lane | Crate | Owns | Depends on | Independent verification |
|---|---|---|---|---|
| **L0 Contracts** | `cv-core` | types, ports, serde schemas, error taxonomy | nothing | `cargo test` + `xtask schema-snapshot` diff; golden JSON fixtures from a real Boris run checked in |
| **L1 Engine** | `cv-engine` (+`oliver-sys`) | subprocess mgmt, NDJSON decode, version gate, Oliver FFI | L0 | `StubEngine` + integration tests vs real pinned `boris` on 3 OSes in CI; fixture corpora (steal Solipsist's stunt corpora idea: `happy/`, `broken-*`) |
| **L2 Store** | `cv-store` | workspace DB, notes, receipts | L0 | property tests, migration tests; no Boris needed |
| **L3 SEO** | `cv-seo` | ruleset format, rule builtins, finding emit | L0 (+ IR fixtures) | golden-file tests: fixture IR → expected findings; zero deps |
| **L4 Publish** | `cv-publish` | outbox queue, target adapters, keychain | L0, L1 port | fake-target adapter tests; real push tested by manual gate per target |
| **L5 Model** | `cv-model` | app state machine, projections, intent dispatch | L0, L2, L3, L4 (via ports) | headless `cargo test` with all-stub ports — the *entire app logic* is testable without UI or Boris |
| **L6 UI** | `cv-ui`, `capture-vector` | rune views, editor surface, graph view | L5 only | rune headless: render PNG, dump element tree, scripted input — per-scene golden screenshots |
| **L7 CLI/dogfood** | `cv-cli` | headless binary | L5 | e2e tests over fixture workspaces |

**Sequencing — what lands first:**

1. **Week 0 (single owner):** `cv-core` + L0 golden fixtures captured from real Boris (`boris --out`, `--watch-json`, `nostr plan`, diagnostic sets). *Everything else fans out only after this freeze.*
2. **Parallel wave 1:** L1 (engine), L2 (store), L3 (SEO) — mutually independent.
3. **Parallel wave 2:** L4 (publish; needs L1's port shape), L5 (model; needs L2/L3 contracts — already frozen so stubs suffice even before impls land), L6 shell (pane layout, theming, empty-state scenes on rune).
4. **Wave 3:** UI wiring to model, CLI, outbox push flows.

**Collision rules:**

- Two lanes may never edit `cv-core` in the same wave without a contract-diff review — schema snapshots in CI make silent drift a build failure.
- Lanes communicate through `cv-core` types or through `cv-model` intents — **never** through shared files, globals, or reaching into each other's crates.
- `cv-model`'s `AppState` is a single-writer state machine (message-passing, not shared mutable state). Lanes contribute *reducers*, not state.
- If two lanes genuinely need the same helper → it goes into `cv-core` via a dedicated PR, not duplicated.

**What breaks if lanes collide:** UI compiling against a stale `Intent` enum (CI: `cargo check -p cv-ui` on every L0 PR); stub/real behavior drift (mitigated by running the *same* test suite against `StubEngine` and real Boris in CI).

---

## 6. Risks, non-goals, questions

### Top 5 risks / hard parts

1. **Rune is pre-release.** Its API will move under us. Mitigation: pin `rev` (docs say `dev` is never rewritten), isolate all rune code in `cv-ui`, and keep `cv-model` runnable headless so a broken UI never blocks other lanes. Budget a "rune upgrade" lane every dependency bump.
2. **The native-feeling editor.** "Notion minus the lumps" is the hardest UX ask. Building a block editor in rune is a project by itself. **My opinionated call:** v1 is a Markdown source editor with Oliver in-process live preview (Typora-style, not block-style), syntax highlighting, and schema-aware completion borrowed from the Boris Editor's proven model. Defer block editing to v2 or never.
3. **Contract drift with Boris.** Boris is a separate moving project; its NDJSON/JSON contracts evolve. Mitigation: pinned binary + version gate + golden fixtures + a CI job that runs real Boris weekly against the pinned version.
4. **Preview ≠ publish divergence.** If the editor's Oliver preview drifts from what Boris emits, users lose trust. Mitigation (§3): preview is labeled approximate; authoritative pane always shows compiled output from `boris watch`.
5. **Publish-target secret handling + irreversibility.** Nostr events can't be unpublished; ATProto records have identity consequences. Mitigation: plan → review → sign → push is a non-compressible user flow (no one-click publish for Nostr), dry-run is the default, receipts recorded in `cv-store`.

### Non-goals (v1)

- Not a second compiler — **never** reimplement Boris semantics (validation, graph, templates) in Rust.
- No live SERP/answer-engine probing, no keyword research, no rank tracking — SEO/AEO is *structural*, against the frozen graph.
- No collaboration/multiplayer, no sync service, no cloud account. Local-first like Boris.
- No WYSIWYG block editor (v2 candidate).
- No plugin system. Rulesets (TOML data) are the extension surface.
- No migration labs in-app — Boris's migration-lab stays external; we *open* its output as a workspace.
- No web build even though rune supports it — scope discipline.

### Questions for the product owner

1. **Oliver preview fidelity:** acceptable that the typing-preview is labeled "approximate" and the authoritative render is always Boris's compiled output? (Strongly recommended; the alternative is a permanent fidelity chase.)
2. **Notes/clippings promotion:** when a research note becomes a Satellite page, does the note remain (with a link) or move? DEVONthink keeps both; Notion moves. Affects `cv-store` schema — decide before L2 freeze.
3. **Nostr scope:** is Nostr actually a v1 publish target for *this app* given it's not a verified target in Boris, or do we ship GitHub Pages + Standard.site first and treat Nostr UI as behind a flag?
4. **Ruleset provenance:** "best practices borrowed from a host of sources" — do we need a licensing audit on the sources we cite in rules (Google SEO docs, schema.org, llmstxt.org)? Rules cite them; do we also ship excerpts?
5. **Distribution:** rune gives us native binaries — but who signs/notarizes on macOS (Solipsist's release is parked on a paid Apple Developer ID), and does Windows matter for v1 or is macOS+Linux the honest first target?
6. **How ambitious is "graphically ambitious"?** Is there a concrete visual bar (demo video, shader effects like rune's liquid-metal, or restrained polish)? This changes whether L6 needs a dedicated GPU-effects lane or stays standard `rune-kit` components.

### Where I disagree with the brief

- "Uses Oliver the way Boris does" — Boris embeds Oliver *in-process in Zig*. For us, in-process is right **only** for the editor preview; using our own Oliver render for anything user-facing-authoritative would create a second, subtly different truth. The boundary is *approximate preview in-process, authoritative output via Boris subprocess*, full stop.
- "Wrapping what makes a great website into an application" is right, but the temptation will be to bake SEO opinions into code. Wrong — rules must be versioned TOML data so they can evolve without recompiling the app and so findings stay citable.
- The parallelizability requirement is achievable only if `cv-core` is *tiny and frozen early* — resist putting logic in it. Every convenience type added to `cv-core` is a merge conflict generator across lanes.

An engineer can start today with: `cargo new capture-vector`, the crate layout in §2, `cv-core`'s three traits sketched, and the Boris golden fixtures captured from `boris --out`/`--watch-json` on a `boris init --type docs` corpus.