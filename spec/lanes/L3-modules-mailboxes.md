# L3 — Content Modules (Pages, Outputs, Activity)

> Worker brief. The Boris-facing modules. Pure `*Model` builders in
> `cv-model` + surface dirs; you never see a process or a display.

## Owns

- `crates/cv-model/src/modules/{pages,outputs,activity}/` +
  `crates/cv-app/src/surfaces/{pages,outputs,activity}/`,
  `fixtures/ui/golden/{pages,outputs,activity}/`

## Depends on

- L0 + `StubEngine`. Deliverable against fixtures only.

## Deliverables

1. **Pages module**: Trunk/Satellite graph as the mailbox tree (nested
   parents); list = children; reading = native noun view (`rune_kit::
   Markdown` "draft sketch" + breadcrumbs/children/relations);
   inspector = frontmatter fields + this page's diagnostics (build-tag
   aware) + impact lists.
2. **Outputs module**: IR, RAG packs, context bundles, llms.txt, RSS,
   sitemap, dist artifacts as nouns — provenance in inspector (build,
   profile, freshness vs last publish).
3. **Activity module**: the diagnostic/event river. "Nothing dropped"
   means *semantically* — the view virtualizes and the actor coalesces
   bursts; rivers don't drown the bus. Severity rails, group by file,
   click-through to Pages.
4. **Selection integration**: modules read/write the shared `Selection`;
   findings in Activity and Pages inspector are the same `Diagnostic`
   values — no remapping.
5. **NounAttrRow projections** for each module's nouns (feeds smart
   mailboxes uniformly).
6. **Goldens**: happy corpus, each broken-* corpus, empty project,
   mid-watch rebuild, every degraded state — element-tree JSON on all
   OSes, PNG on canonical.

## Done when

- Fixture watch replay drives all modules through documented states;
  goldens green.
- Same `Diagnostic` value renders identically in Activity and Pages.
- 5k-row fixture lists scroll at frame budget (virtualized).

## Must not

- Implement Audit/Publish/Research/Editor modules. Filter Activity
  semantically. Call `Engine` from render paths — state from the last
  event, commands as intents.
