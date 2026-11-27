# Vision

**Capture Vector** is a cross-platform Rust workbench that turns Boris's
validated content graph into an Outlook-class native application: write,
research, audit for SEO/AEO, and publish back to every engine — from one
deterministic core.

## The shape

Solipsist proved the model on macOS with a Mail metaphor: sources in
Settings, mailboxes on the left, one reading place, inspector on the right.
Capture Vector keeps the chrome grammar but changes the metaphor.

**Mail has one mailbox. Outlook has modules.** Pages, Research, Audit,
Publish, and Activity are first-class peers in a module switcher — the way
Mail, Calendar, and Tasks coexist in Outlook. Every module gets the shared
nav / list / reading / inspector arrangement for free, and each is free to
specialize inside it. The dedicated concepts get room to do their thing.

And it should look like it means it: Rune is GPU-rendered with springs,
motion, and themes. The bar is "Outlook did EDM" — information-dense,
keyboard-fast, and unapologetically polished. Not a gray enterprise grid,
not a toy.

## What it does

- **Wraps Boris completely.** Init, build, validate, watch, check, impact,
  graph, every export (`--out`, `--rag`, `--context`, `--llms`, `--rss`,
  `--sitemap`), and every publish target — driven through Boris's JSON
  contracts over a subprocess boundary. The app is the best seat in the
  house for the compiler, never a second compiler.
- **Makes the content graph navigable at publication scale.** Graph
  visualization is a signature surface, not garnish: users can browse and
  inspect libraries of thousands of pages, trunks with dozens of
  satellites, and dense interconnections without the view collapsing into
  an unusable hairball. Corpus-scale browsing uses level-of-detail,
  viewport culling, search/filtering, and selection-linked navigation.
  Small Mermaid diagrams may explain a neighborhood, but a static diagram
  of the whole corpus is not the graph product.
- **Makes SEO/AEO a working surface, not a checklist.** A rules engine
  scores the frozen graph and `dist/` artifacts against a versioned
  rulebook compiled from a host of sources — Google Search Central,
  schema.org, Open Graph, Core Web Vitals, llms.txt and answer-engine
  practice. Findings are diagnostics with stable codes and fix-its, shown
  next to Boris's own. "What makes a great website" becomes a feature of
  the application.
- **Treats research as native material.** Captures, clippings, and notes
  live in their own module with real linking — Google Notebook's capture
  ease inside Outlook's structure, with the Tinderbox/DEVONthink instinct
  for connections and none of the ceremony. Notes promote into Boris
  pages; pages cite notes.
- **Always publishes back to the engines.** The machine-facing outputs —
  llms.txt, sitemap, RSS, RAG/context bundles, IR — are not leftovers.
  Every publish run ships them to their targets and brings the evidence
  home: plans, reports, proof packs, per-target status, all first-class
  objects in the app. Nothing rots in `dist/`. The first v1 acceptance
  slice proves the preflight, approval, dispatch, and reconciliation state
  loop with `copy-to-folder`; it does not yet prove machine-output
  delivery, and it does not decide Boris-native target publishing scope.
  IndexNow and webhook are post-v1 delivery extensions. Sitemap ping is
  post-v1 desired in principle and ships only after evidence of a live
  useful consumer.

## Pillars — non-negotiable, every lane

1. **Boris is the compiler.** Never reimplement Boris or Oliver semantics.
   Locate the binary, run it, mirror its contracts, honor its diagnostics.
2. **Diagnostics are sacred.** Exit codes and stderr are data. They reach
   the Activity module intact or the lane is wrong.
3. **Deterministic core.** Given the same frozen graph and fixtures, every
   view-model, audit finding, and publish plan is reproducible.
4. **Parallel by contract.** Lanes integrate through `cv-core` traits and
   fixtures only. If two lanes need to talk, the contract changes first.
5. **Headless-verifiable.** Every UI claim is backed by a PNG or element
   tree produced without a display.

## Non-goals (v1)

- No WYSIWYG HTML editing — the editor edits Markdown, preview renders it.
- No live SERP scraping or search-engine APIs — the audit engine is
  offline, deterministic, and cites its sources in the rulebook.
- No sync backend or accounts — local-first, files and OS keychain.
- No Electron, no embedded web shell for app chrome — Rune or nothing.
- No second content model — Boris's Trunk/Satellite graph is the truth;
  research and audit hang off it, they don't fork it.
