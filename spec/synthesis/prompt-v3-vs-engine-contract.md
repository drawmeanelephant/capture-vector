# PROMPT-v3 vs. engine contract — recorded tensions

Places where the work order demands a check that the Boris 0.9.0
contract does not endorse — or actively contradicts. The rule still
blocks (PROMPT-v3 is normative for this product); this file keeps the
tension visible so the citation stays honest.

## `TGT-RAG-DRAFT-LEAK` (targets pack)

- **PROMPT-v3** (Audit section): "Draft/noindex leakage into
  sitemap/rss/llms/RAG is a blocking error."
- **Boris 0.9.0 contract** (`docs/contracts/frontmatter.md`, `status`
  row): "Standard.site, Nostr, RSS, sitemap, and search exclude drafts;
  IR and RAG keep every page."

The engine documents the opposite of the failure: RAG *keeps* drafts by
design, so a shipped RAG pack leaks them — the engine behavior is the
premise for the check, not its endorsement. The rule is cited to
PROMPT-v3 and tiered `NormativeSpec`; the remediation points at
`--scope` (`docs/contracts/rag-export.md`) or publishing the draft.
