Grounded. I've read all four READMEs in full (Rune core + rune-kit, Solipsist, Boris including contracts/outputs/Nostr sections, Oliver including the C ABI and CLI contract). Workspace is empty except `spec/` (untouched, per instructions) and a stub README. Here is the spec.

---

# Capture Vector — Technical Specification (design agent draft)

## 1. Product shape

**Capture Vector is a local-first publication workbench**: a desktop app that owns the full lifecycle of a Boris publication — capture → author → validate → optimize → publish → receipt — as one continuous surface, not a pipeline you operate by hand.

Solipsist was "Radio UserLand's job in Mail's body": Boris publications rendered as mailboxes you *read*, with compose bolted on. The Mail→Outlook shift is the difference between a reader and a workbench:

| Mail (Solipsist) | Outlook (Capture Vector) | Architectural consequence |
|---|---|---|
| Mailboxes (left) | **Sources** — multiple Boris trees, git/GitHub sources, note corpora | Multi-root workspace model, not one site |
| Reading pane (middle) | **Editor + live preview** — compose is first-class, not a companion window | The editor is the center pane; preview is a lane of it |
| Inspector drawer (right) | **Properties + Findings drawer** — frontmatter form, diagnostics, SEO/AEO findings for the focused page | Inspector is data-driven off `cv-core` types |
| — | **Rules** — Outlook's rules engine maps to versioned SEO/AEO rule packs evaluated continuously | `cv-seo` as a deterministic rules engine, findings as first-class objects |
| — | **Calendar/queue** — publish schedule, outbound queue, receipts | `cv-publish` outbound queue with evidence chain surfaced as a ledger |
| Flags/follow-ups | **Findings inbox** — SEO/AEO + compiler diagnostics as triageable items | Findings have lifecycle states (open/fixed/ignored/wontfix) |
| Accounts | **Targets** — GitHub Pages, Standard.site, Nostr, static hosts | Target registry mirrors Boris's publication registry |

The Outlook metaphor's real content: *everything is a message you can act on.* A Boris diagnostic is a finding. An SEO violation is a finding. A stale `llms.txt` relative to last publish is a finding. An unpublished RAG pack is a finding. One `Finding` type, one inbox, one triage UX — that is the shared element that does the heavy lifting.

"Microsoft Outlook did EDM music" lands in `cv-theme` + the graph visualizer: OKLCH neon-on-dark theme data, spring-driven transitions (rune's motion palette), a live content-graph visualization as the signature screen. Visual ambition is isolated in theme data and dedicated scenes so it never blocks core lanes.

**What the app is NOT**: not a second compiler, not a host, not an editor with a site generator attached. Boris is the compiler; Capture Vector is the cockpit.

## 2. Crate/module architecture

Cargo workspace. Two hard rules that make the parallelism work: **(a) `cv-core` and `cv-contracts` freeze early and change only through contract PRs; (b) every crate below the `cv-app` line is usable and testable headlessly** (rune renders PNGs, dumps element trees, takes scripted input on a virtual clock — every UI lane self-verifies without a human).

```
crates/
  cv-core          # Domain types: Workspace, Source, Page, Note, Finding,
                   # Severity, TargetRef, Receipt. No IO, no rune, no serde_json
                   # of Boris payloads (that lives in cv-contracts).
  cv-contracts     # serde mirrors of Boris JSON: IR (--out), watch NDJSON,
                   # plan, check/diagnostics, RAG/context manifests, Proof Pack
                   # summary. Fixture-decode tests only. Boris-version-gated.
  cv-engine        # The ONLY crate that spawns `boris`. Engine actor:
                   # locate → version-gate → run verbs → NDJSON watch consumer.
                   # `trait Engine` + `ProcessEngine` + `StubEngine`.
  cv-oliver        # Oliver C ABI binding (static lib) + `OliverCli` fallback
                   # behind `trait Markup`. Source → typed render → HTML fragment.
                   # Also drives cooklang/textile note corpora.
  cv-store         # `.cv/` sidecar: redb for finding state, index cache,
                   # SEO history, publish receipts. Content tree stays the
                   # source of truth; cv-store never owns content.
  cv-seo           # Facts extraction + rule packs + scoring. Pure functions:
                   # Facts → Vec<Finding>. Zero UI, zero IO beyond pack loading.
  cv-publish       # Outbound queue: machine outputs → target push → receipts.
                   # Wraps boris publish verbs + direct engines (IndexNow etc).
                   # Secrets via keyring → boris stdin, never argv/env/log.
  cv-search        # Workspace index: titles, wikilinks, tags, full-text
                   # (tantivy or plain inverted index — pick at contract time).
  cv-theme         # Kit seed, theme JSON, glyph set, motion palette. Pure data.
  cv-ui-kit        # cv components on rune-kit: FindingRow, ScoreBadge,
                   # DiagnosticChip, TargetCard, ReceiptList.
  cv-ui-shell      # rune_kit::shell tri-pane: sources sidebar, work pane,
                   # inspector. Sidebar models + focus groups.
  cv-ui-editor     # Markdown editor: TextEdit::rich + spans highlight,
                   # frontmatter form, oliver-backed live preview lane.
  cv-ui-graph      # Content-graph visualization (the EDM canvas): trunks/
                   # satellites as a force-ish layout, diagnostics overlay.
  cv-ui-seo        # Findings inbox + score dashboard + per-page report.
  cv-ui-publish    # Targets, outbound queue, receipt/proof-pack browser.
  cv-ui-notes      # Capture surface: quick notes, research clips, cooklang/
                   # textile fragments — the Notebook baby.
  cv-app           # Composition root. AppState, Surface registry, ⌘K command
                   # palette, window specs, native menus. OWNED BY INTEGRATOR.
xtask/
                   # fetch-engines (pinned boris+oliver per target triple),
                   # codegen contract mirrors, stunt corpus runner, release.
app/
                   # thin bin: main.rs → cv_app::run()
```

**Dependency discipline** (enforced by a `xtask lint-deps` check): `cv-ui-*` crates may depend on `cv-core`, `cv-contracts`, `cv-theme`, `cv-ui-kit`, and *traits* from `cv-engine`/`cv-seo`/`cv-publish` — never on their implementations. `cv-app` wires implementations to surfaces. This is what lets a UI lane run entirely on `StubEngine` while the real engine lane is still landing.

**Async model**: rune handlers run `&mut State` between frames; Boris runs take seconds. So `cv-engine` is a dedicated thread owning all `std::process` handles; requests go in as `EngineRequest`, results come back through the `Tx`/message queue and merge into state in a queued handler. Watch mode is a long-lived `boris watch --watch-json` child whose NDJSON stream the actor forwards as `EngineEvent`s. The UI *never* blocks on a compiler run.

**App data layout**:

```
my-site/                 # the Boris tree (git-friendly, app doesn't own it)
  content/  boris.json  dist/  .boris/
  .cv/                   # Capture Vector sidecar (gitignored by default)
    state.redb           # finding lifecycle, index, history, receipts
    notes/               # uncaptured research (opt-in: stays out of boris)
    cache/               # oliver fragments, preview assets
    logs/
```

## 3. Boris + Oliver integration boundary

### Boris — subprocess only. Non-negotiable, and Solipsist already proved why.

- `cv-engine::ProcessEngine` is the sole `std::process::Command` owner for `boris`. Binary resolution order: `CV_BORIS_BIN` env → vendored in-app binary (per-triple, fetched by `xtask fetch-engines`, content-hash pinned) → user-configured path → `boris` on PATH. Version-gated at startup via `boris --version`; contract mirrors are keyed to a min/max engine version range.
- **Consumed surfaces**: `--out` JSON IR (the graph we visualize and analyze), `validate`/`check` (diagnostics → findings), `impact` (editor "what breaks if I change this"), `graph` (fallback structure), `--watch-json` NDJSON (live rebuild loop), `--rag`/`--context`/`--llms`/`--rss`/`--sitemap` (machine outputs), `plan`/`standard-site`/`nostr plan|sign|publish` (publish verbs), `init` (new-workspace archetypes).
- **Never**: reimplement graph validation, frontmatter semantics, or diagnostics in Rust; swallow exit codes; mutate the content tree outside file writes the user made. `cv-contracts` mirrors are hand-maintained serde types + fixture-decode tests with checked-in JSON fixtures (the Solipsist Stunts pattern — port `happy/`, `broken-*` corpora).
- **Deliberately rejected**: linking Boris's `compileBundle` in-process. It's shipped (native + Wasm), and it's tempting, but subprocess isolation buys crash isolation, zero Zig-arena-across-FFI lifetime bugs, hot-swappable engine versions, and a trivially stubbable `Engine` trait. Revisit only if measured compile latency for live preview actually hurts — the `trait Engine` boundary makes that a `cv-engine`-internal change.

### Oliver — in-process via the C ABI. This is the "use it the way Boris does" ask, honestly translated.

Boris embeds Oliver as a Zig library because Boris *is* Zig. The equivalent for a Rust host is Oliver's stable C ABI (`include/oliver.h`, explicit error codes, `oliver_free`) — same library, same deterministic output, different FFI. Two adapters behind `trait Markup`:

- `OliverLib` — dlopen/static-link `liboliver`; per-keystroke fragment rendering is cheap enough for live preview.
- `OliverCli` — stdin/stdout `oliver render` fallback (same semantics, ~process-spawn cost) for platforms where the FFI build is blocked (see risk 3).

**What Oliver is for in-app** — and this is a deliberate, opinionated scoping:

1. **SEO/AEO fact extraction on the authoritative DOM.** Live findings need the same HTML Boris will emit, not a second markdown implementation. Render the buffer through Oliver → parse fragment (html5ever/tl) → extract `DocFacts`. rune-kit's `Markdown` component is used *only* for the visual preview pane (it's streaming, CommonMark+GFM, syntax-highlighted — it's a better live-preview widget than an HTML fragment would be), clearly labeled "advisory preview"; the compiler-accurate preview remains `boris watch --serve` opened in the user's browser. **Do not build an HTML renderer in rune for v1** — that way lies a second browser.
2. **Non-Markdown corpora.** Notes/research in Textile and Cooklang (`oliver render --from textile|cooklang`, `scale`, `serialize`) — the DEVONthink/cookbook corpus support the brief gestures at, for free.
3. **Parity checks.** `xtask` can diff Oliver-fragment vs. Boris-emitted page bodies in fixtures.

Version discipline: pin the Oliver rev to the one Boris pins (`build.zig.zon` content hash); `xtask fetch-engines` asserts it. Mismatch policy: preview/facts remain advisory; Boris output is always authoritative — that sentence should be in the code as a doc comment on `trait Markup`.

## 4. SEO/AEO design — deterministic, citable, offline

Three-layer pipeline, all pure functions:

```
Facts  ──extract──▶  Findings  ──score──▶  Scores + History
```

**Facts** (`cv-seo::facts`): `DocFacts` per page + `SiteFacts` per graph. Two extraction paths, same output type:
- *Post-build (authoritative)*: parse Boris IR + `dist/` HTML → titles, meta descriptions, heading tree, canonical/OG/Twitter/JSON-LD presence, internal link graph, orphans, image alts, sitemap/RSS/llms.txt coverage, hreflang, status codes from evidence.
- *Live (advisory)*: current buffer → Oliver fragment → same `DocFacts` (subset flagged `provisional`) for the inspector while typing.

**Findings**: `Finding { rule_id, severity, scope(Page|Site|Output), page_id, evidence_spans, citation, fix_hint }`. Every finding carries a **citation** — `{ source_name, url, section }` pointing at the best-practice document the rule encodes (Google Search Central starter docs, schema.org types, llmstxt.org spec, IndexNow protocol, WCAG techniques, ATProto/Nostr target requirements). This is the "borrowed from a host of sources" requirement made auditable: the rule pack *is* the bibliography.

**Rule packs** (`cv-seo::packs`): versioned TOML data + a `Rule` trait. Ships with:
- `seo-core@1` — title/description lengths, single H1, heading hierarchy, canonical, meta robots, image alts, internal-link depth, orphan detection.
- `aeo-llm@1` — answer-engine heuristics: definitional first paragraph ("answer capsule"), FAQ/HowTo JSON-LD presence, llms.txt coverage of every trunk, `description` frontmatter on satellites, entity consistency, RAG-pack completeness vs. last export.
- `a11y-content@1` — alt text, link text quality ("click here"), heading skips, table headers.
- `targets@1` — per-target requirements (Standard.site record fields, NIP-23 allowlist coverage, RSS metadata completeness).

Rule packs are data + code pairs: declarative thresholds in TOML (citable, diffable), check functions in Rust. **Honest position**: classic SEO has real invariants and a lot of folklore; AEO is almost entirely heuristic. So severities are `Error|Warn|Info`, every Info finding shows its citation inline, and packs are user-editable — the app is a linter for citable practice, not an oracle. That framing is also the disagreement-with-the-brief answer: "validate against best practices" only works if the practices are versioned and cited, or we've built a horoscope generator.

**Scores**: per-page and per-site, computed deterministically from finding severities (weighted, config in pack). History in `cv-store` → trend sparkline on the dashboard. **Goldens**: fixture site + expected findings JSON per pack version — the `Finding` golden is what makes the whole SEO lane parallelizable (Lane C needs nothing else).

**LLM boundary**: core scoring is 100% deterministic and offline — non-negotiable for trust and for CI. An optional `cv-seo-advisory` lane (behind a feature flag, user-supplied key, output always badged "advisory, never scored") can suggest rewrites. It is a separate crate so it can be deleted without touching anything.

## 5. Parallel-lane decomposition

The design is parallelizable because the expensive things are shared contracts, and the contracts are small. Sequence:

**Phase 0 — Contract freeze (one PR each, block everything)**

1. `cv-core`: `Finding`, `PageId`, `Source`, `TargetRef`, `Receipt` + lifecycle enums. ~300 lines.
2. `cv-contracts`: serde mirrors + fixture-decode test harness + fixture corpus layout.
3. `trait Engine` / `EngineRequest` / `EngineEvent` / `StubEngine` (drives from fixtures). *This is the keystone* — with a stub engine, every UI lane is unblocked day one.
4. `trait Markup` + `DocFacts`/`SiteFacts`/`RulePack` schemas.
5. `cv-theme`: `Kit` seed + token names (theme *values* can churn later; names can't).
6. `cv-app`: `trait Surface { fn view(..) -> El; fn reduce(..) }` + `AppMsg` envelope + surface-registry shape. Integrator-owned forever.

**Phase 1+ — Independent lanes** (each lane owns its crate directory; each verifies alone):

| Lane | Crate(s) | Needs | Self-verification |
|---|---|---|---|
| A Engine | `cv-engine` | contracts 1–3 | stub-corpus runs; NDJSON replay tests from recorded `--watch-json` captures |
| B Store | `cv-store` | 1 | unit tests + crash-recovery property tests |
| C SEO | `cv-seo` | 1, 4 | golden findings per fixture per pack — zero IO, fastest lane |
| D Editor | `cv-oliver`, `cv-ui-editor` | 3–5 | headless `--snap`/`--tree`; Oliver conformance spot-checks |
| E Shell+graph | `cv-ui-shell`, `cv-ui-graph`, `cv-ui-kit` | 1, 5, 6 | headless scenes on `StubEngine`; 5k-row virtual fixtures |
| F Publish | `cv-publish` | 1–3 | offline plan/sign replay; receipts on fixtures; live targets explicitly out of scope for CI |
| G Theme/visuals | `cv-theme`, gallery scenes | 5 | scene goldens (`RUNE_BLESS`-style PNG bless) |
| H Notes/capture | `cv-ui-notes`, `cv-search` | 1, 4, 5 | unit + headless scenes |
| I Integrator | `cv-app` | all | end-to-end headless script: open fixture → find finding → fix → republish on stub |

**Collision rules**: lanes may only *depend on* contract crates, never edit them — a core change is a contract PR owned by the integrator, reviewed against the fixture corpus. If two lanes need the same type, that's a signal it belongs in `cv-core`, not that it should be defined twice.

**What breaks when lanes collide, concretely**: if Lane C and Lane B both declare `Finding`, the workspace fails to compile immediately (two crates, one name in core = core wins, nothing ambiguous can merge). If two UI lanes mutate the same `AppState` field, that's a merge conflict in `cv-app` only — per-surface state slices (`EditorState`, `SeoState`, …) inside `AppState` shrink the blast radius to one file. If a contract PR changes a serde shape, `cv-contracts`' fixture-decode tests fail before anything downstream can — that's the tripwire working as designed.

## 6. Top risks, non-goals, questions

### Top 5 risks / hard parts

1. **Rune is pre-release.** Pin a `dev` rev (`dev` is never rewritten), record the rev in `Cargo.toml` + `xtask`, upgrade deliberately. Windows GNU toolchain needs MinGW binutils on PATH — document MSVC as the tier-1 Windows path. Mitigation: headless verification catches rendering regressions mechanically.
2. **Oliver C ABI cross-platform linking.** Static-linking a Zig artifact into a Rust app on three OSes is the kind of thing that eats a week on Windows. Mitigation: `OliverCli` fallback behind the same trait ships day one; FFI lands when it lands.
3. **Contract drift.** Boris is alive. Contract mirrors keyed to engine version ranges + fixture tests + a `doctor` surface that says "your boris 0.10 speaks a contract I don't know" beats silent misparse.
4. **SEO credibility.** If findings aren't cited and pack-versioned, the app is snake oil and the whole pitch collapses. This is a *content* risk more than a code risk — someone must curate packs against real sources.
5. **State/event architecture under rune's sync-handler model.** Long-running compiles, watch streams, publish jobs all need to land in `&mut State` cleanly between frames. Get the `Tx`-injection pattern right in week 0 (a thin `bg → ui` channel the integrator owns) or every lane reinvents it badly.

(Honorable mention: scope. "Graphically ambitious" + Outlook parity is enormous — the lane map exists precisely so visuals never gate core.)

### Non-goals (v1, unless the owner says otherwise)

- No second compiler, no reimplemented Boris/Oliver semantics, no WYSIWYG block editor (Markdown + forms).
- No in-app HTML/webview preview renderer. Advisory preview = rune-kit Markdown; authoritative = `boris watch --serve` in a real browser.
- No hosting, no server, no collaboration/multi-user, no sync service.
- No general YAML frontmatter, no MDX/executable content (Boris's closed model is a feature).
- No LLM in the scoring path; advisory AI is a deletable flag-gated lane.
- No web target despite rune supporting it — desktop-first.
- Live network publish (Pages push, ATProto, Nostr) is explicit-user-action only; nothing publishes on a timer in v1.

### Questions for the product owner before building

1. **Where do notes/research live?** Inside the Boris tree as draft/unpublished pages (graph-aware, wikilinks work) or in `.cv/notes/` outside it (Boris never sees them)? This is the biggest fork — it changes `cv-store`, the graph lane, and what "promote note → page" means. My recommendation: outside by default, one-click promote.
2. **Vendored engine or BYO?** Ship pinned per-platform `boris`/`oliver` binaries in-app (size, notarization, update cadence) or require the user's own? Vendored matches Solipsist's direction and makes version-gating tractable.
3. **Secrets**: Nostr `nsec`, ATProto app passwords, GitHub tokens — OS keychain via `keyring`, piped to Boris on stdin only. Confirm that's the model and whether Touch ID gating is wanted.
4. **"Publishing back to the engines"**: which outbound engines are in v1 — Pages push, Standard.site, Nostr, IndexNow ping, RSS/llms.txt regeneration, webhook? And does "always" mean *on every successful build* or on explicit publish?
5. **How hard is the EDM requirement?** Is the graph visualizer a signature feature (budget real GPU/motion work) or garnish (defer behind a flag)? This decides whether Lane G is launch-blocking.
6. **Tier-1 OSes at launch** — all three equally, or macOS-first parity with Solipsist users as the migration path?
7. **Is a Solipsist import path in scope** — read its sources/settings format, or is this a clean break?

---

**One-line summary for the orchestrator**: Freeze six small contract surfaces (`cv-core` types, Boris contract mirrors, `trait Engine` + `StubEngine`, `trait Markup` + facts/rule schemas, `Kit` tokens, `Surface` registry), and nine lanes — engine, store, SEO, editor, shell/graph, publish, theme, notes, integration — proceed independently with headless verification each. Boris stays a subprocess; Oliver links in-process via C ABI; SEO is a citable rule-pack linter, not an oracle.