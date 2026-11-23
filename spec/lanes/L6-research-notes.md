# L6 — Research & Notes (`cv-notes` + Research module)

> Worker brief. Notebook's capture ease, Tinderbox's connective tissue,
> DEVONthink's place-for-everything — as a silo *beside* the Boris tree,
> feeding it.

## Owns

- `crates/cv-notes/`, `crates/cv-model/src/modules/research/` +
  surfaces dir, `fixtures/research/`, `fixtures/ui/golden/research/`

## Depends on

- L0 contracts (`Store`, `NounId`, `NounAttrRow`, `ContentWriter`).

## Deliverables

1. **Note model** in `research.*` namespace: `Note { id, title, body_md,
   captures, links, tags, created, modified, source_uri? }`; captures
   typed `Selection|Url|File|Image|Idea`. Textile/Cooklang corpora
   render through the `OliverRender` engine command (its one sanctioned
   interactive use).
2. **Link graph**: typed edges note↔note, note→page, note→finding;
   backlinks computed. `StableId` everywhere — paths never keys.
3. **Inbox → triage → promote**: quick-capture inbox (fastest path to
   saved); `promote(note) -> PageDraft` writes Boris-conformant Markdown
   via `ContentWriter` (fingerprinted staged write; `boris validate`
   judges).
4. **Provenance via `.cv/` sidecar map** (`note-id → page stable-id`) —
   **never frontmatter** (`x-*` keys hard-fail `EFRONTMATTER`,
   verified). Sidecar survives renames; page→note backlink resolved at
   read time.
5. **Smart mailboxes**: saved `field op value` conjunctions over
   `NounAttrRow` projections — nav entries + a small filter-builder UI.
   No grammar, joins, or regex.
6. **Research module**: nav = inbox, notebooks, saved views; list =
   notes; reading = rendered note + backlinks rail; inspector = links,
   captures, promote. Goldens incl. degraded states.
7. **FTS5 search** via `cv-store` namespace (titles/body/tags).

## Done when

- Promoted page passes `boris validate` semantics via fixture;
  sidecar link survives a reparent fixture.
- Inbox→promote→rebuild→backlink round-trip proven on `StubEngine`.
- Restart round-trip: all state recovered (JSONL truth + index rebuild).

## Must not

- Put notes inside `content/` or provenance inside frontmatter. Build a
  second renderer. Sync/service layer. Duplicate Boris's page model.
