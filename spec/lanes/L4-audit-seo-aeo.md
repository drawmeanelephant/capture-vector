# L4 — Audit: SEO/AEO Engine (`cv-rules` + Audit module)

> Worker brief. "What makes a great website" as deterministic, citable,
> *tiered* findings. You're a linter for citable practice, not an
> oracle — and folklore never blocks a publish.

## Owns

- `crates/cv-rules/`, `rulepacks/` (versioned TOML, cited),
  `crates/cv-model/src/modules/audit/` + `crates/cv-app/src/surfaces/audit/`,
  `fixtures/audit/`, `fixtures/ui/golden/audit/`

## Depends on

- L0 contracts (`ArtifactSet`, `CheckSpec`, `Finding`, `DedupKey`,
  `AuthorityTier`). Zero engine dependency — consume artifact manifests.

## Deliverables

1. **`cv-rules`**: `eval(&ArtifactSet, &Ruleset) -> Report` — pure,
   deterministic (byte-identical for identical inputs; a test runs it
   twice and byte-compares). Reports carry artifact+ruleset hashes and
   stable ordering. Unsupported/unavailable inputs → `limited`, never
   silent pass.
2. **`CheckSpec` closed algebra** (v1): typed selectors over IR nodes +
   final HTML DOM + exports + `TargetDeployProfile`. No per-lane DSLs.
   Canonical/robots/JSON-LD checks consume **final HTML**, not IR.
3. **Rule packs v1** (`rulepacks/*.toml`, each citing its source):
   `seo-core`, `aeo-llm`, `a11y-content`, `targets`. Every rule:
   `id, severity, tier, archetypes, check, remediation,
   citation{source,url,frozen-quote,captured}`.
   `rulepacks/SOURCES.md` is **generated** from citations.
4. **Tier enforcement**: `NormativeSpec`/`EngineDocumented` errors may
   feed Preflight blocking; `Heuristic` findings are advisory and can
   *never* block — a test proves a heuristic can't produce a blocking
   verdict. `archetype_scope` keeps narrative pages out of
   answer-first checks.
5. **Cross-artifact parity**: HTML canonical ≡ sitemap ≡ RSS ≡ llms.txt
   ≡ internal hrefs ≡ deploy profile; draft/noindex leakage into
   sitemap/rss/llms/RAG = blocking error. Anything needing live HTTP
   (redirects, X-Robots headers, CWV) = `limited`.
6. **Audit module**: Findings mailbox (severity rails, group by
   rule/pack/noun, lifecycle open/fixed/ignored/wontfix keyed by
   `DedupKey` = code+subject+hash(target)), per-page Health inspector,
   smart-mailbox saved filters (bounded conjunctions only).
7. **Goldens**: empty findings, findings table, detail, limited states,
   degraded states.

## Done when

- Fixture ArtifactSets with seeded violations → expected findings;
  clean → zero; tier test proves heuristics can't block.
- Remove a pack → exactly its findings disappear.
- `cargo test -p cv-rules` pure: fixtures only, no net, no engine.

## Must not

- Fetch live anything. Score beyond enumerable findings. Invent codes
  per-run. Re-derive Boris graph semantics — read the IR.
