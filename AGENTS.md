# Capture Vector — agent orientation

Rust desktop app (macOS/Windows/Linux) for authoring, auditing, and
publishing Boris content. Outlook-style mailboxes over a graph compiler.
**Concurrent lane work is the central architectural property.**

## Read first

1. [`spec/PROMPT-v3.md`](spec/PROMPT-v3.md) — THE normative work order.
   `PROMPT-groundwork.md` and `PROMPT-v2.md` are superseded; do not build
   against them.
2. [`spec/architecture.md`](spec/architecture.md) — crate map, engine
   boundary, event/store model.
3. [`spec/contracts.md`](spec/contracts.md) — frozen contracts.
4. [`spec/GAMEPLAN.md`](spec/GAMEPLAN.md) — historical wave-0 plan, kept
   for the decision trail. Current state lives in FINDINGS, the
   tech-debt ledger, and open issues.
5. [`spike/FINDINGS.md`](spike/FINDINGS.md) — what is proven vs unproven.
6. [`docs/agent-claims.md`](docs/agent-claims.md) — claim the issue before
   writing code. The `pr-claims` CI check fails a PR whose `Fixes #N` has
   no linked claim, or duplicates another open PR.

## Claim before implementation

Before writing code for a GitHub issue, claim the issue and honor existing
claims. The canonical protocol is [`docs/agent-claims.md`](docs/agent-claims.md);
the shortest path is:

```sh
python3 scripts/agent-claim.py status <issue>
python3 scripts/agent-claim.py claim <issue> --agent <unique-agent-or-session-id> --branch <pr-branch>
```

Use the issue comment ledger as the source of truth even when labels or
assignees disagree. If `status` or `claim` reports an active claim, pick
another issue. Put `Fixes #<issue>` and `Claim: <claim-id>` in the PR
body, then release the claim with `--reason pr-opened` when the PR opens,
or release it when work stops. The `pr-claims` check prints the exact
fix when the link is missing. When `gh`
is unavailable or unauthenticated, post the manual claim format from
`docs/agent-claims.md` with the available GitHub issue tool instead of
skipping the ledger.

## Build, run, and QA

- Setup + build: `./scripts/fetch-rune.sh && cargo build --workspace`.
- Launch: `./scripts/dev.sh --project <dir>` (vendors rune on first use).
- Demo/QA: `./scripts/demo.sh` copies `fixtures/corpus-src/happy` to a
  temp dir and launches against it. `CV_EDIT=content/index.md
  ./scripts/demo.sh` opens the editor on the trunk page.
- Headless gate: `cargo run -p cv-app -- --scene canary --tree
  target/canary.tree.json`.
- Tests: `cargo test --workspace --locked`.
- The full agent-followable QA path (dependencies, no auth gate, drive
  steps) is in `README.md` under "Interactive QA".

Test layout and naming:
- Unit tests live beside the code in `#[cfg(test)] mod tests`.
- Integration tests live in `crates/<crate>/tests/<area>.rs`; shared
  helpers go in a `support/` or `common/` module under that `tests/` dir.
- Test functions are `snake_case` sentences describing the behavior
  (`gate_failures_map_to_disclosed_states`), one behavior per test.
- Goldens: `crates/cv-app/tests/goldens.rs`; PNGs only on the canonical
  Lavapipe config.

Local quality hooks:
- `./scripts/install-hooks.sh` sets `core.hooksPath=.githooks`.
- The hook runs fmt, secret/large-file/TODO/doc-link scans, and the
  version-drift check. `CV_HOOK_SLOW=1 git commit …` also runs
  `./scripts/lint.sh`.
- `./scripts/lint.sh` is the full local gate: fmt, clippy, hygiene, and
  the unused-dependency scan, plus cargo-deny when installed. CI adds
  platform- and pull-request-specific gates.

Dependency policy:
- All versions live in `[workspace.dependencies]`; members use
  `workspace = true`.
- Wait at least 7 days after a dependency release before merging a bump.
  Dependabot enforces this with `cooldown`; do not bypass it.
- No wildcard/`*` versions and no git dependencies. Rune is the only
  vendored path dependency.

Structured logs:
- `CV_LOG=info` emits JSONL to stderr; `CV_LOG_FILE=<path>` writes to a
  file. Levels: `error|warn|info|debug|trace`.
- Fields whose key looks like a secret (`secret`, `password`, `token`,
  `nsec`, …) are dropped. Never log argv, env values, or page bodies.

Release notes: `scripts/release-notes.sh [range]` groups conventional
commits; set `CV_NOTES_OUT=<file>` to write instead of printing.

Runbook: `docs/runbooks/desktop-app.md`.
Tech-debt ledger: `docs/tech-debt.md`.

## Proven facts (build on these)

- rune `dev` @ `2a59b0f95b82972478a497c42151dbcb3ca5c69f` works headless:
  `Runtime`/`frame`/`find`/`pointer_*`, `elements()` tree, `virtual_rows`
  on 5000 rows, `--snap` PNG (real GPU), `--tree` JSON, `--scene`.
  Probe: `spike/rune-probe` (`cargo test` to rerun).
- **rune cannot be a git dependency.** Run `scripts/fetch-rune.sh` and
  replicate the `[patch.crates-io]` block it prints. Vendor dir is
  gitignored on purpose.
## Ratified toolchain (mission-approved 2026-11-23)

| Component | Pin |
|---|---|
| Rust | 1.99.1 — pinned by `rust-toolchain.toml` |
| rune | `dev` @ `2a59b0f95b82972478a497c42151dbcb3ca5c69f`, vendored |
| boris | **0.9.0** (vcs `66efcfb`) — built w/ zig 0.17.x; 0.8.2 lacks native Windows support |
| oliver | 1.3.0 (`967bfdf`) |
| PNG goldens | Lavapipe software rasterizer on Linux CI = canonical config |
| signature graph | approved — boris nostr/atproto signing surface, secrets via stdin only |

- `fixtures/` contains REAL captures from `boris/0.9.0` (vcs `66efcfb`)
  over 9 corpora: green happy build + dist proof-pack JSON, 5 broken
  corpora with distinct diagnostic codes (`EDUPLICATEID`,
  `EFRONTMATTER`, `EPARENTMISSING`, `EREFERENCEMISSING`, `ETEXTILE`),
  `--textile`/`--cooklang` mode contracts, and 9 real `watch --watch-json`
  NDJSON sessions (hello handshake, schema 1, rebuild events).
  `fixtures/MANIFEST.toml` pins sha256 + engine version + capture command.
- Boris 0.9.0 builds from tag `v0.9.0` (`zig build --release=safe`;
  local copy at `~/.local/src/boris-0.9.0/zig-out/bin/boris`);
  cloud lanes should consume `fixtures/` and never need the engine.

## Hard rules (violations = wrong work)

- Never reimplement Boris or Oliver semantics in Rust.
- Hand-authored Boris JSON fixtures are banned — regenerate via
  `scripts/capture-fixtures.sh <corpora-dir>`.
- `cv-core`/`cv-contracts` depend on nothing in-workspace; no async, no
  I/O, no UI in `cv-core`. Ports are sync/channel-sink.
- Processes spawn only inside `cv-engine`.
- No webview, no in-app HTML renderer, no Oliver render in the typing
  loop. `rune_kit::Markdown` is a draft sketch; Boris output is
  authoritative.
- Notes/provenance live in `.cv/` sidecar, never in Boris frontmatter
  (closed schema — unknown keys hard-fail `EFRONTMATTER`).
- Heuristic SEO/AEO findings can never block publishing
  (`authority_tier`).
- Secrets: never in argv, env, logs, events, or persisted state.
  GH repo secrets exist for opt-in live-publish smoke only:
  `CV_BSKY_HANDLE`, `CV_BSKY_APP_PASSWORD`, `CV_NOSTR_NSEC` — ordinary
  CI/tests use a mock secret backend, never these.
- Element-tree/layout JSON is the cross-OS UI gate; PNG goldens only on
  one canonical rasterizer config.
- Windows behavior is UNPROVEN — don't claim it until run there. Linux
  claims must stay within the evidence in `spike/FINDINGS.md`.
