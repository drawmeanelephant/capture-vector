# Final synthesis — what the model panel produced

Record of the multi-model refinement that produced `PROMPT-v3.md`.
Eight reviewers, four model families, three rounds.

## Round 1 — blind re-derivation (SWE-2 medium / high / max)

Three agents designed independently from the raw brief (no access to
the v1 draft). They **converged on the v1 spine** — subprocess Boris,
contract-first core, fixture lanes, headless rune verification — which
validated it, then out-designed it:

- `Noun`/`Mailbox`/`Selection` as open types — every artifact is
  routable mail (max)
- Everything-is-a-Finding unification + Findings mailbox + Preflight
  gate (high, max)
- Smart mailboxes = the Tinderbox gene (max)
- Watch NDJSON on stderr + `hello` handshake + exit-code mapping +
  `completion.json` + atomic saves (max — read the actual contracts)
- `.cv/` sidecar, `cv-ledger`, `cv-widgets` rune firewall, per-lane
  state slices + additive events, `cv-cli`, `xtask`, doctor surface
- Preview honesty: no HTML renderer in rune ("a second browser") —
  native noun view + external browser (high, max) — killed the v1 L7
  design
- Oliver subprocess default; FFI = optional optimization (max over
  medium/high's FFI-first)

## Round 2 — adversarial rulings (SWE-2 high + max)

- Forks ruled: ~10–13 crates; branded `NounKind`; domain rail (verb
  rail dead); bounded smart mailboxes v1; store contested
  (redb↔rusqlite — later settled rusqlite).
- Spine attacks: **fixture mechanism = serialized big-bang** (manifest,
  conformance suite, refresh mechanics); **additive events ≠ dataflow**
  (cascade spec demanded).
- Holes: `boris-editor` exists upstream (verified `boris-editor/0.1.0`);
  closed frontmatter kills frontmatter provenance; `ContentWriter` was
  reimplementing Boris's grammar (pillar violation); missing Clock/
  IdGen; `.cv/` versioning+lock; multi-window selection; scale
  envelope; diagnostic staleness tags; degraded-mode owners; L0 is the
  deepest item.

## Round 3 — frontier panel (Gemini Neon xhigh, Claude Opus 5.5, Kimi K3.1)

- **Opus** (went upstream): host watch always-false on Windows; host
  ring caps 100 events — CV must own the cursor; watch holds a
  `dist/` writer seat; **Preflight TOCTOU** — gate must build target
  profile and check those bytes; `OliverRender` has no consumer;
  vendored-binaries-not-optional *if* host adopted; SQLite in synced
  `.cv/` corrupts — JSONL truth + rebuildable index; lane-namespaced
  event variants; element-tree goldens as the real 3-OS gate; dedup
  needs a third component (target hash).
- **Kimi** (contract semantics): stale kit files would poison lanes
  (fixed); event envelope + atomic commit boundary must be concrete;
  capability-honest ports + conformance on all adapters; `CheckSpec`
  needs a real closed algebra; `ArtifactSet` needs final HTML;
  **`PreflightGrant`** — sealed hashes, approval persisted pre-dispatch,
  `unknown` reconciliation never auto-retry.
- **Gemini** (deepest structural pass): `cv-app`↔`cv-ui` cyclic dep;
  `cv-core` "no async" contradicted async ports (→ sync ports);
  cargo-deny can't police intra-crate boundaries; **reversed the host
  adoption** (RawSubprocess-only — steal mechanics, revisit later);
  **reversed single cv-ui** (cv-model + cv-widgets + peeled editor);
  hand-authored fixtures = self-confirming mocks → real captures in
  wave 0 or ban; `NounAttrRow` + StableId-never-path; no IR-in-SQLite;
  per-namespace migrations; parity check targets rune_kit::Markdown vs
  Boris (the real divergence); **`authority_tier` epistemics** —
  heuristic AEO folklore can never block publish; wave-0 rune probe +
  canary vertical slice; span-unit + Windows-rename contract pins.

## Overturned along the way

v1 HTML→rune mapper → dead. v2 `boris-editor` adoption → mechanics
stolen, host rejected. v2 single `cv-ui` → cv-model/cv-widgets split.
v2 async `cv-core` ports → synchronous. v2 staged/lane-contributed
fixtures → real captures only, wave 0. v2 deferred contract hardening →
checkpoint after first vertical slice. v2 frontmatter provenance →
`.cv/` sidecar. v2 OliverRender-in-typing-loop → out; Oliver serves
corpora + facts only. PNG-3-OS gating → element-tree gating.

## Standing discards

jq-predicate rule DSL; FFI on the critical path; in-app themed preview;
letter-grade SEO scores; BYO engine binary; sealed NounKind enum;
verb-oriented module rail; shipped `cv-cli` binary; live SERP APIs;
LLM in the scoring path.

## Deliverable

`PROMPT-v3.md` — self-contained groundwork prompt; `architecture.md`,
`contracts.md`, `lanes/L0–L7` rewritten to match. The remaining open
questions are owner decisions, enumerated at the bottom of PROMPT-v3.
