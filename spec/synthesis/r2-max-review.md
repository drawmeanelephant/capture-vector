# Round 2 — adversarial review (SWE-2 max)

## Fork rulings
1. Crates: ~13. All UI surfaces in ONE cv-ui crate; per-surface crates are
   permanent integration tax for a 30s blast-radius win. Peel out
   cv-ui-graph only if it drags unique GPU/layout deps.
2. Noun kind: branded open string (NounKind newtype + const well-knowns +
   mandatory unknown-kind fallback renderer). Sealed enum + Other() is
   worst option — exhaustiveness theater + serialized-data trap.
3. Module rail: domain-oriented, decisively. Module contract is
   noun-shaped; verb rail contradicts vision.md's own enumeration.
4. cv-cli: build it, scoped. Enforces ports-and-services layering (if
   `cv audit` can't drive services without cv-app, layering is a lie).
   No --render-png ever; developer/dogfood binary in v1.
5. Smart mailboxes: v1, bounded. field-op-value conjunctions over
   already-indexed attrs; no grammar, no joins, no regex. It's the
   Tinderbox gene — deferring ships the metaphor without the gene.
6. Store: redb. Flip rule: rusqlite only if a lane lands a real
   multi-namespace-join/aggregation need — decide BEFORE fixtures
   encode a schema (fixture shapes are the lock-in, not the engine).

## Two weakest points in the spine
A. Fixture mechanism is a serialized big-bang wearing a parallelism
   costume. Boris pin moves → Engine regenerates everything → all lane
   fixture hashes fail at once. Missing: per-schema versioning, decoder
   tolerance, StubEngine≡ProcessEngine conformance suite, and the
   fixture pin mechanism is backwards (manifest: Engine-owned
   fixtures/MANIFEST.toml; lanes assert by name).
B. "Additive AppEvent variants" is a merge strategy, not a dataflow
   design. Missing the cascade spec: event→reducer→emitted-intent
   ordering, idempotency, replay semantics; findings can arrive before
   the IR they reference. Needs a state-machine paragraph in cv-core
   before wave 1 or it's the first rewrite.

## Hole hunt
- Engine-side concurrency unspecified: watch (long-lived) vs one-shot
  verbs; need per-workspace scheduler (serialize mutating verbs,
  parallelize reads) or two lanes corrupt dist/ differently.
- No Clock/IdGen seam in frozen contracts — determinism pillar fails
  silently (stale_page rules, run timestamps, dedup all need "now").
- x-capture-vector-note provenance key contradicts closed frontmatter —
  verify Boris tolerates x-* keys or move provenance to .cv/ sidecar map.
- .cv/ has no schema versioning/migration/lock policy; single-instance
  enforcement unwritten; "gitignored by default" notes = silent
  data-loss story unless UI says so.
- Multi-window state is a shrug: one Selection, one AppState; L7 has
  companion windows — per-window vs shared state unaddressed, on L2's
  critical path.
- Scale only fixture-deep: list virtualization as hard requirement;
  where GraphDocument lives (Arc-share, don't clone into AppState);
  finding dedup stable IDs across rebuilds (code+subject).
- Degraded-mode UX ownerless: every module needs engine-absent/
  mismatched/dead-mid-watch golden states; headless-Linux-CI keychain
  path for publish lane.
- Golden-update policy unwritten: bless mechanic + who reviews visual
  diffs on rune rev bumps.
