# Round 1 synthesis — merged best-of-three

Three independent SWE-2 derivations (thought levels medium/high/max), run
blind against this spec. All three converged on the same spine; this file
records the merge, the contested forks, and the discards.

## Converged spine (validated by 3/3 independent derivations)

- **Boris = subprocess only.** One process owner crate. Binary pinned +
  `--version`-gated (+ `--build-info`/`vcsRevision` recorded). Search:
  `CV_BORIS_BIN` → bundled → PATH → sibling checkout. Exit codes
  `0/1/2/3` → `Ok/ContentFailure/UsageError/IoFailure`. Diagnostics and
  stderr are sacred data, surfaced verbatim.
- **Contract-first core + fixtures = the parallelism mechanism.** Small
  frozen contract crate; `cv-fixtures` stunt corpora (happy/, broken-*);
  `StubEngine` replays captured NDJSON; every lane's tests run with no
  boris, no display, no network.
- **Headless rune verification.** PNG + element-tree goldens per surface
  state; scripted input drives tests.
- **SEO/AEO = deterministic offline rule engine.** Rules as versioned,
  *cited* data (TOML packs); findings with stable codes; no LLM in the
  scoring path; "a linter for citable practice, not an oracle."
- **No webview, ever.** rune_kit::Markdown renders the *noun* natively;
  themed site preview is the user's own browser on `boris watch --serve`
  loopback. Advisory (typing) vs authoritative (compiler) preview are
  labeled as such.
- **Secrets**: OS keychain → stdin pipe at sign time only; never argv,
  env, disk, logs. Boris's plan→sign→publish split kept verbatim.
- **Notes live outside the Boris tree** in an app silo; promote-to-page
  is an explicit verb carrying provenance.
- **Per-lane state slices + additive `AppEvent` variants; append-only
  core after wave 0; ownership-by-path.**

## Adopted upgrades over the v1 spec draft

| Upgrade | Source | Why |
|---|---|---|
| `Noun`/`Mailbox`/`Selection` as open strings | max | Real Outlook model; every artifact is routable "mail"; lanes add nouns without editing central enums |
| Findings mailbox + Preflight gate | max, high | Boris diagnostics + ruleset findings → one triage surface; publish requires a claims-style gate |
| Smart mailboxes = Tinderbox agents | max | Saved predicates over nouns+findings; the honest Tinderbox gene |
| Module rail by verb (Write/Verify/Publish/Inspect) | max | Alternative to per-domain modules — resolves "dedicated concepts get freedom" differently |
| Kill HTML→rune element mapper | high, max | "That way lies a second browser." Native noun view + external site preview instead |
| Oliver subprocess default; FFI = optional optimization lane | max (vs medium/high FFI-first) | Process isolation is the feature; ~100ms debounced preview is fine; FFI never on critical path |
| `.cv/` sidecar; never write `content/`, `dist/`, `.boris/` | high, max | Filesystem discipline; atomic `*.tmp`+rename saves tuned to Boris's watcher |
| `cv-ledger` evidence crate | max | artifacts→checks→claims→deliveries as first-class nouns |
| `cv-widgets` = rune-version firewall | max | One crate absorbs rune API churn per rev bump |
| `cv-cli` headless entry | medium | Every lane exercisable in CI; dogfooding surface |
| `xtask fetch-engines` + doctor surface | high, max | Pinned binary fetch; "your boris speaks a contract I don't know" |
| Fixture regeneration serialized via Engine lane; lanes pin fixture hashes | max | Silent schema drift fails loudly in the right CI |
| `ruleset/SOURCES.md` generated from citation records | max | The bibliography is generated, not maintained |
| Editor = first spike | max | Hardest component in the app; decide structured-vs-rich early |
| Delivery adapter registry (copy/run-command/HTTP POST) | max | "Back to the engines" = extensible adapters, IndexNow et al. later |
| Watch NDJSON on **stderr**, `hello` schema handshake first | max | Boris contract detail: stdout stays empty; kill child on bad handshake |

## Contested forks → round-2 questions

1. **Crate granularity.** ~10 crates (v1 draft/medium) vs ~17–20 crates
   (high/max, incl. per-surface `cv-play-*` and per-domain `cv-ui-*`).
   Does per-surface crate-split pay for its integration cost, or is
   one-module-per-crate-dir inside fewer crates enough parallelism?
2. **`Noun` as open string vs typed enum.** Open strings maximize lane
   independence (no central enum edits) but sacrifice exhaustiveness.
   Is the trade worth it, or should noun kinds be a sealed enum with
   `Other(String)`?
3. **Module rail decomposition.** Verb-oriented (Write/Verify/Publish/
   Inspect) vs domain-oriented (Pages/Research/Audit/Publish/Activity)?
4. **`cv-cli`** — real deliverable (medium's pick) or redundant given
   rune's own headless mode + StubEngine tests (high/max didn't need it)?
5. **Smart mailboxes v1?** Signature differentiator vs. scope creep —
   does the saved-predicate engine land in wave 1 or is it v2?
6. **Store tech**: redb vs rusqlite vs plain JSONL+index. Pick with
   criteria, or defer to lane 0 with a stated decision rule?

## Discards (garbage bin)

- **jq-like predicate DSL for rules** (medium) — hand-wavy; the closed
  TOML `check` DSL (max) or TOML-thresholds+Rust-checks (high) are both
  more honest and auditable.
- **In-process Oliver FFI on the critical path** (medium/high) — max's
  counter-argument wins: it contradicts the subprocess-isolation spine
  for a latency problem that a 100ms debounce already solves.
- **In-app themed site preview** — dead on arrival (no webview); any
  design that implies it is now explicitly a non-goal.
- **Letter grades / single SEO scores** — Boris's checked/passed/failed/
  limited honesty vocabulary wins; scores must decompose to findings.
