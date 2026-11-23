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
4. [`spec/GAMEPLAN.md`](spec/GAMEPLAN.md) — wave-0 exit criteria and
   current verified state.
5. [`spike/FINDINGS.md`](spike/FINDINGS.md) — what is proven vs unproven.

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
| Rust | 1.99.1 floor (CI `rust-toolchain.toml` when it lands) |
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
- Element-tree/layout JSON is the cross-OS UI gate; PNG goldens only on
  one canonical rasterizer config.
- Windows/Linux behavior is UNPROVEN — don't claim it until run there.
