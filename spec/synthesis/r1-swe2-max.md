I have thorough grounding across all four repos now. Here's the design spec.

---

# Capture Vector — Technical Specification (design sub-agent draft)

## 0. What I grounded this in

Read before designing: Rune README + `crates/rune` + `crates/rune-kit` docs; Solipsist README, `docs/HARNESS.md`, `docs/ONBOARDING.md`; Boris README, `docs/contracts/cli.md`, `frontmatter.md`, `publication-model.md`, `documentation-intelligence.md`, `watch-json-recipe.md`; Oliver README. Nothing under `spec/` was touched.

---

## 1. Product shape

**Capture Vector is a desktop publishing workbench: Outlook for a local-first publication.** It treats everything Boris emits — pages, diagnostics, IR, plans, feeds, sitemaps, RAG packs, proof packs — as *mail*: first-class nouns you select, read in a pane, inspect in a drawer, and route to a destination. Notes/research is a second account type in the same shell. SEO/AEO is a deterministic rules engine that runs over the artifacts like Outlook Rules run over mail.

### The Mail → Outlook shift, mapped to architecture

Solipsist's Mail model was a *viewer*: chrome hosting foreign surfaces (WKWebView preview, Svelte editor companion), a deliberately thin reading pane, verbs in menus. The owner says that workflow isn't what's wanted. Outlook is a **PIM suite**, and the differences are architectural, not cosmetic:

| Outlook concept | Mail-era (Solipsist) | Outlook-era (Capture Vector) |
|---|---|---|
| One noun: message | Noun ≈ page/output | **`Noun` is an open type**: page, diagnostic, plan, finding, note, artifact file. One `Selection`, every surface reads it. |
| Rules engine | Check badges on rows | **Rules are the product.** Boris diagnostics + `cv-rules` findings attach to nouns; a Findings mailbox; a Preflight gate before publish. |
| Search folders | Mailbox = fixed list | **Smart mailboxes = Tinderbox agents**: saved predicates over nouns+findings ("drafts with stale_page", "pages missing summary"). |
| Reading pane renders the item | Reading pane was a summary stub + "open preview" verb | **Reading pane renders the noun natively** — page → document view, plan → plan view, finding → evidence view. The themed site preview is a separate concern. |
| Module switcher (mail/calendar/tasks) | Mailboxes under accounts | **Module rail** (Write / Verify / Publish / Inspect) + command palette. Ribbon without the ribbon. |
| Accounts | Sources in Settings | Same, plus a second account kind: **Research silo** (the DEVONthink gene). |

The children, minus the lumps:
- **Notion** → structured editing polish on *plain Markdown files*: frontmatter as a form, slash-commands for `{{include}}`/`<Aside>`/`<Details>`, wikilink completion fed by Boris's `completion.json`. The lump we remove is the proprietary block model — bytes stay Markdown.
- **Astro** → content collections already exist: Boris's frozen graph. Collections surface as mailbox grouping (by `parent`, `status`, `tags`, relation kinds).
- **DEVONthink** → the Research account: capture inbox, clippings, classify/cross-link to pages via wikilinks. Lives in a **separate silo**, not inside the Boris content root (keep the corpus clean; "promote to page" is an explicit verb).
- **Tinderbox** → smart mailboxes (persistent agents) + a native graph map over `graph.json`.

"Always publishing back" → the Publish lane models every machine output as having a **route home**: HTML → targets, sitemap → IndexNow/search-engine submission adapters, RAG/context → upload destinations, NIP-23 → relays, Standard.site → PDS — each delivery recorded in an evidence ledger using Boris's own artifacts → checks → claims vocabulary.

### One hard product boundary

Rune has no webview. Therefore **the app never renders themed site HTML in-process**. Two distinct surfaces result, and they must not be conflated:

1. **Native document view** — the reading pane renders the *noun* (body via `rune_kit::Markdown`, plus graph chrome: breadcrumbs, children, relations, backlinks — the slots Boris themes expose).
2. **Site preview companion** — the user's own browser pointed at `boris watch --serve`'s loopback URL (we learn the port from the `serve-started` watch event). Hosted, not authored — same as Solipsist, except the host is now any browser, not a WKWebView.

This is the honest split: we reimplement the *reading* surface; we refuse to reimplement the *theme renderer*.

---

## 2. Crate/module architecture

A cargo workspace. The spine is `cv-core` (types) + `cv-engine` (process boundary) + `cv-store` (workspace model). Every lane crate depends on those three (or their traits) and **on no other lane**. `cv-app` is the only crate that knows all lanes.

```
capture-vector/
  Cargo.toml                     # workspace members = the crate list below (wave-0 only)
  crates/
    cv-core/          # serde contract mirrors, Noun/Mailbox/Selection, Finding, events
    cv-fixtures/      # stunt corpora + golden JSON fixtures; dev-dep of every lane
    cv-engine/        # the ONLY Process owner: boris + oliver supervisors, NDJSON decoder
    cv-store/         # sources, project index, persistence, file-watch coalescing
    cv-rules/         # SEO/AEO rule engine + vendored ruleset (TOML data)
    cv-ledger/        # delivery/evidence ledger (artifacts→checks→claims→deliveries)
    cv-theme/         # the "Outlook did EDM" theme as data (Kit from PaletteSeed + motion)
    cv-widgets/       # app views on rune-kit; the rune-version firewall
    cv-shell/         # window chrome: module rail, mailbox tree, toolbar, inspector slot
    cv-play/          # reading-surface registry + dispatch per mailbox/noun kind
    cv-play-pages/    # pages mailbox: graph list + document view
    cv-play-findings/ # findings mailbox + per-finding evidence view
    cv-play-outputs/  # outputs mailbox: targets/editions/artifacts
    cv-play-publish/  # publish console surface
    cv-play-notes/    # notes/research surface
    cv-editor/        # native markdown editor (TextEdit::rich + frontmatter form + completion)
    cv-preview/       # document rendering + site-preview companion launcher
    cv-graph/         # native graph map over IR graph.json
    cv-publish/       # plan→sign→publish flows, stdin secrets, OS keychain
    cv-notes/         # capture inbox, research silo file model
    cv-app/           # AppState composition, event sum type, wake/drain loop
    capture-vector/   # thin main()
```

### `cv-core` — the contract everyone codes against (frozen in wave 0)

```rust
// Boris contract mirrors — serde only, no behavior beyond decode/encode.
pub mod boris {
    pub struct GraphDocument { /* ir-schema 0.2: nodes, edges, ... */ }
    pub struct CompletionDocument { /* completion.json: entity ids, relation
                                       kinds, parent targets, layout slots */ }
    pub struct HtmlBuildReport { /* html-build-report-0.2.0 */ }
    pub struct DocIntelReport { /* documentation-intelligence-0.2.0 */ }
    pub struct Diagnostic { severity, code, message, remediation,
                            source_path, line, column, id }   // shared shape
    pub struct WatchEvent { /* NDJSON: hello/build-*/watcher-started/
                               serve-started/watch-error/watch-stopped */ }
    pub struct PublicationPlan { /* `boris plan` stdout JSON */ }
    pub struct SignedBundle { /* nostr sign output */ }
    pub struct PublishReport { /* nostr publish / standard-site publish */ }
}

// App-domain vocabulary — open strings, never enums-to-extend.
pub struct Noun { pub kind: String, pub id: String, pub account: AccountId }
pub struct Mailbox { pub id: String, pub account: AccountId,
                     pub parent: Option<String> }
pub struct Selection { pub account: AccountId, pub mailbox: String,
                       pub noun: Option<Noun> }      // value type, one owner
pub struct Finding { pub source: FindingSource /* Engine | Ruleset */,
                     pub rule_id: String, pub severity: Severity,
                     pub endpoint: Endpoint, pub evidence: Evidence,
                     pub remediation: Option<String>,
                     pub citation: Option<Citation> }
pub enum AppEvent {                        // additive variants only
    Engine(EngineEvent), Store(StoreEvent), Editor(EditorEvent),
    Publish(PublishEvent), Rules(RulesEvent), /* …one per lane… */
}
```

### State model (rune's `view(&S, &mut Cx<S>)` shape)

- `AppState` is a struct of per-lane modules: `pub struct AppState { shell: ShellState, pages: PagesState, editor: EditorState, …, selection: Selection }`. Handlers mutate it via `&mut S` as rune intends.
- `cv-engine` runs a supervisor thread; decoded `EngineEvent`s are pushed through a channel; a `wake` handler drains the queue into `cv-store` each frame. UI never blocks on a process.
- `Selection` is written by chrome (account/mailbox) and play (noun); read by inspector, drawer, companions. Same rule as Solipsist's `WorkspaceSelection`.

### Filesystem discipline

- Boris's **workspace containment** is lexical against the child CWD → `cv-engine` always spawns `boris` with `cwd` = the publication root.
- Editor saves are **atomic**: write sibling `*.tmp`, rename over target — the Boris watcher ignores `.tmp` and treats rename as one clean change (per the watch recipe).
- App-owned project state lives in `<project>/.cv/` (ledger, notes-silo pointer, prefs). Machine state (accounts, pane widths, smart mailboxes) lives in the platform config dir. Never write into `content/`, `dist/`, `.boris/` except on explicit save.

---

## 3. Boris + Oliver integration boundary

### Boris: subprocess only, no exceptions

`cv-engine` is the only `Process` owner (Solipsist's rule, kept). Concretely:

- **Discovery/pin**: search order `CV_BORIS_BIN` → bundled per-OS/arch binary → PATH. On first use, gate on `boris --version` (exact `boris/<semver>` compare) and record `boris --build-info` (`vcsRevision`) into the ledger. Refuse to run a compiler outside the pinned major/minor band.
- **Sessions**: one long-lived `boris watch --watch-json` (+ `--serve` when preview is armed) per open account. **stderr is the NDJSON stream** — stdout stays empty; the #1 consumer bug is piping stdout, so the runner documents and enforces the fd wiring. First record must be `hello` with `watch_events_schema == 1`, else we kill the child and surface a typed error — never degrade silently.
- **Reports**: `check --format json --report`, `build|validate --report`, IR `--out` dir (`manifest.json`, `graph.json`, `completion.json`, `build-report.json`) are read as files and decoded into `cv-core`. Editor/inspector consume decoded structs, never raw JSON or re-parsed source.
- **Exit codes** `0/1/2/3` map to `Ok / ContentFailure / UsageError / IoFailure`. We never swallow a diagnostic; stderr prose (when not NDJSON) is captured into the activity log verbatim.
- **Never reimplemented in Rust**: frontmatter grammar, graph resolution, link audit, include expansion, publish semantics, theme/layout rendering. The normative escape hatch: if we need frontmatter-shaped validation in the editor before save, we run `boris validate` on the dirty buffer via stdin-to-tempfile, not a hand-rolled parser. (`completion.json` gives us entity ids/parent targets/relation kinds for completion legally — Boris explicitly designates it "the deterministic editor completion surface.")

### Oliver: two consumers, two transports

- **`cv-engine::OliverRunner` (subprocess)** — the default and the *authoritative* path. Used for "what will Boris emit" correctness renders (`render --from markdown --to xhtml` with the corpus's opt-in flags), and for notes/textile/cooklang rendering. Respect `--max-input-bytes`; decode explicit exit codes.
- **`cv-oliver-ffi` (optional, `oliver-sys`)** — the C ABI (`include/oliver.h`, `oliver_free`) bound in a thin unsafe-owning crate, for sub-16ms fragment renders in the editor's live preview. **Feature-flagged; not on the critical path.** Editor v1 uses subprocess+debounce (~100 ms), so the editor lane is never blocked on FFI review. This is where I part from "use Oliver the way Boris does": Boris embeds it in-process because it *is* a compiler; the app is a harness, and process isolation is the feature worth keeping. FFI is a latency optimization, not an architecture.

**Boundary in one line:** Boris owns publication semantics; Oliver owns fragment rendering; Capture Vector owns selection, presentation, and orchestration — and never parses content semantics itself.

---

## 4. SEO/AEO design — deterministic, citable, offline

### `cv-rules`: rules as data, engine as pure function

```toml
# ruleset/seo/title.toml — vendored, versioned, audited like code
[[rule]]
id = "seo.title.missing"
severity = "error"
scope = "page"
inputs = ["graph", "html"]                  # which artifacts it reads
check = { dom = { exists = "title" } }      # closed DSL, not code
remediation = "Add a <title> or author `title:` frontmatter."
[[rule.citation]]
source = "Google Search Central — Title links"
url = "https://developers.google.com/search/docs/appearance/title-link"
quote = "…frozen excerpt ≤200 chars…"
captured = "2026-11-01"
```

- **Inputs**: IR `graph.json` + `completion.json` + `build-report.json`, the `check` report, and rendered HTML parsed with `html5ever`/`scraper` (offline, deterministic).
- **Evaluation**: `fn eval(artifacts: &ArtifactSet, ruleset: &Ruleset) -> Report` — pure: no clock, no network, no RNG. Findings sorted canonically; byte-identical reports for identical inputs + ruleset version. Determinism is enforced by a test that evaluates twice and byte-compares.
- **Honesty vocabulary is borrowed from Boris's publication model on purpose**: findings are `checked`/`passed`/`failed`/`limited` with the check and scope named — never "verified" beyond scope, never a letter grade without enumerable findings. Scores are deterministic functions of finding counts/severities and every score decomposes to its findings.
- **Citable**: `ruleset/SOURCES.md` is *generated* from the citation records (the lever, not hand-maintained docs). Quotes are frozen at authoring time so justification travels offline.

### Two finding tiers, one surface

- **Engine findings**: Boris diagnostics + `boris check` findings (`unreferenced_page`, `unlinked_page`, `flat_graph`, `zero_includes`, `zero_relations`, `draft_page`, `stale_page`) rendered verbatim — we pass through codes, never reinterpret.
- **Ruleset findings**: `cv-rules` families:
  - **SEO**: title/meta-description presence & length, canonical/base_url consistency across projections (implements the conductor rule's "one `base_url`" invariant as a check on emitted artifacts), heading order, image alt text, sitemap↔graph coverage, RSS autodiscovery link, OG/Twitter cards, `robots`/indexability sanity, draft leakage into sitemap/feed (a *projection-consistency* check on emitted artifacts — legitimate, and never claimed as re-verifying Boris).
  - **AEO**: `llms.txt` presence+shape, JSON-LD presence + `@type` validity against a vendored schema.org subset, FAQ/HowTo structure, `summary`/`published_at` completeness (projection eligibility), entity/title consistency, answer-first paragraph heuristics expressed as bounded predicates.
- **UI**: a Findings mailbox (smart-filterable by severity/family/noun), a per-page Health section in the inspector, and a **Preflight** gate in the publish flow: `validate` + `check` + ruleset → a claims-style report mirroring the proof-pack vocabulary before any network action is armed.

---

## 5. Parallel-lane decomposition

The requirement is that agents improve aspects independently. Path/crate ownership *is* the contract — if two lanes need the same file, the cut was wrong; recut.

### Wave 0 — chassis (serial, deliberately short)

1. Workspace skeleton + `Cargo.toml` members (only wave-0 edits it; afterwards integration adds members).
2. `cv-core` contract types + `cv-fixtures` (golden JSON captured from a pinned Boris against stunt corpora — happy, broken-frontmatter, broken-parent, broken-wikilink shapes).
3. `cv-engine` traits + NDJSON decoder against *replayed* fixture streams (no real boris needed).
4. `cv-app` `AppState`/`AppEvent` skeleton; `cv-shell` empty SplitView + mailbox tree stub; `cv-theme` seed.
5. CI: `cargo test` (fixture decodes) + **headless rune render** (`--tree`/PNG) smoke per surface.

**Gate to wave 1**: app opens a folder, decodes `graph.json` *from a fixture*, shows mailbox tree + empty reading + empty inspector. Everything below codes only against `cv-core` types.

### Wave 1 lanes

| Lane | Owns | Consumes | Verifies independently via |
|---|---|---|---|
| Engine | `cv-engine` impl: discovery, version gate, watch session, report decoding, exit mapping | `cv-core` | fake-boris script emitting contract NDJSON; fixture streams |
| Store | `cv-store`: account registry, persistence, watcher→engine wiring, noun index | `cv-core`, engine traits | unit tests on tempdirs |
| Reading | `cv-play`, `cv-play-pages`, `cv-play-outputs`, `cv-preview` | `cv-core` decoded artifacts | headless PNG/element-tree on fixture nouns |
| Findings UI | `cv-play-findings` + inspector sections | `cv-core::Finding` | fixture findings → headless renders |
| Editor | `cv-editor` (TextEdit::rich, frontmatter form, completion popover, atomic save) | `cv-core::CompletionDocument`, engine trait | scripted headless text edits; golden-file saves on fixture trees |
| Rules/SEO | `cv-rules` + `ruleset/` TOML | `cv-core::ArtifactSet` only | golden reports; determinism byte-compare; no UI, no process |
| Publish | `cv-publish` + `cv-ledger` | plan/bundle/report types, engine trait | `plan`/`sign` offline vs fixtures; publish vs loopback fixture report — **never a real relay** |
| Notes | `cv-notes` + `cv-play-notes` | `cv-core` | file-model unit tests + headless renders |
| Graph map | `cv-graph` | `cv-core::GraphDocument` | headless layout on fixture graphs |
| Chrome/Theme | `cv-shell`, `cv-theme`, `cv-widgets` | all lanes read-only | headless PNG states (light/dark/reduced-motion) |

### Collision rules

- `cv-core` is **append-only** after wave 0. Breaking changes ride a serialized "contract change" PR all lanes rebase onto.
- `AppEvent` takes one additive variant module per lane. `AppState` takes one additive field per lane.
- Mailbox/noun vocabularies are **open strings** — a lane adds `mailbox="findings"` without editing a central enum (Solipsist's rule, kept).
- Theme keys are namespaced (`theme.pages.*`, `theme.findings.*`).
- Fixtures are additive new files only.
- Every lane must be verifiable **without another lane's runtime** — types via `cv-core`, behavior via `cv-fixtures`.

### What actually breaks if two lanes collide

- `Cargo.toml` members list: trivial merge conflict, wave-0-owned.
- `AppState`/`AppEvent`: additive-only so collisions are textually adjacent inserts, not semantic conflicts.
- `cv-shell` slot wiring: the chassis exposes slots; a lane fills a slot fn — two lanes filling the same slot is a *cut* error, caught by lane ownership review, not by the compiler.
- Fixture refreshes (boris version bump): serialized — the Engine lane owns fixture regeneration; other lanes pin fixture hashes in tests so a silent schema drift fails loudly in their CI.

---

## 6. Risks, non-goals, questions

### Top 5 hard parts

1. **The editor.** A credible Markdown editor — selection, IME, undo, decorations, wikilink completion, diagnostic squiggles — is the hardest component in any UI kit. `rune-kit`'s `TextEdit::rich` + `decorations`/`spans` + `Combobox` is a real foundation, but this is the lane to spike **first**. Fallback: structured editor (frontmatter form + plain body editor) ships while the rich editor matures.
2. **Rune is pre-release.** No crates.io; pin a `dev` rev (`dev` is never rewritten). All rune contact goes through `cv-widgets`/`cv-theme` so a rev bump is one PR; headless PNG tests catch renderer drift; upstream takes agent-filed issues, not PRs — plan for issue-roundtrip latency.
3. **No webview → the themed-site preview gap.** Decided above: external-browser companion on `watch --serve` + native document reading surface. Any "in-app site preview" expectation needs to die in wave 0 or be scheduled as a separate research lane.
4. **Contract drift across ~7 schemas** (IR 0.2, html-build-report 0.2.0, doc-intel 0.2.0, watch schema 1, frontmatter-1, plan/bundle/report shapes). Mitigations: compiler version + `vcsRevision` gate at session start; `schemaVersion` tolerance — degrade, never crash; per-schema fixture tests; a single Engine-owned "bump pinned boris" operation that regenerates all fixtures.
5. **Secrets crossing the boundary.** Nostr key and ATProto app-password must never hit argv, env, disk, or logs — pipe via child **stdin** at sign time (Boris's own model), sourced from the OS keychain, held only in a zeroizing buffer, and structured so the *network* child never sees the key (plan → sign → publish stays three operations in-app, not one).

### Non-goals

- Not a compiler/SSG; never reimplements Boris or Oliver semantics.
- Not a hosted SEO service: no live SERP data, no rank tracking, no keyword APIs. Offline and deterministic by design.
- No collaboration, sync, or cloud accounts. Local-first, single user.
- Not a site host. Publishing = orchestrating Boris targets + declared external adapters.
- No embedded web engine / in-app themed site preview in v1.
- Not a general notes app — the research silo exists to feed the publication graph.
- No LLM chat panel in v1. AEO means machine-readable outputs and checkable structure, not a sidebar chatbot.

### Where I disagree with the brief

- "Use Oliver the way Boris does" — Boris embeds it because Boris *is* the compiler. In an app shell, subprocess isolation is the better default; the C ABI is an optimization lane, not the architecture.
- The preview story in the brief is silent; a reader will assume "in-app site preview." It should not be assumed — the honest answer is companion-browser + native reading surface.
- "Highly parallelizable" is only true if `cv-core` freezes in wave 0 and mailbox/noun vocabularies stay open. If we let the noun set be an enum edited centrally, parallelization collapses — so: open strings, additive events, path ownership.

### Questions for the product owner before building

1. **Editor ambition**: live-decorated rich Markdown editor as the v1 bar, or structured editor (frontmatter form + body) first with rich editing in a later wave? This single answer moves the schedule more than anything else.
2. **"Back to the engines" — which engines?** Concretely: IndexNow/Bing URL submission, sitemap ping, Search Console, ATProto/Nostr (Boris-covered), RAG-pack upload destinations? I'd build a `Delivery` adapter registry (copy-to-folder, run-command, HTTP POST) — which adapters are v1?
3. **Notes silo placement**: parallel `.cv/research/` store with explicit "promote to page" (my recommendation), or notes as a reserved subtree inside `content/`?
4. **Binary distribution**: vendor pinned per-platform Boris+Oliver inside the app (deterministic, ~MBs, my recommendation) or BYO with search order?
5. **Signing/distribution budget**: macOS notarization and Windows signing need paid identities — Solipsist parked on exactly this. And: single window with popouts (my recommendation, Outlook-style) vs multi-window companions?