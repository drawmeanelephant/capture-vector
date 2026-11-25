# Contributing

Thanks for helping. This project is pre-release and spec-driven: behavior
is defined by [`spec/`](spec/), not by taste. Read
[`AGENTS.md`](AGENTS.md) before a non-trivial change — it is the
orientation doc for humans and agents alike.

## Reporting bugs

Use the bug-report template. A good report is *grounded*: it carries the
evidence a reviewer can rerun — exact command, `git rev-parse` output,
platform, structured logs (`CV_LOG=debug`), and for UI issues the
`--scene` + `--tree` JSON. See [`docs/TRIAGE.md`](docs/TRIAGE.md) for what
maintainers look for.

Vulnerabilities go through private reporting — see
[`SECURITY.md`](SECURITY.md). Never post secrets, page bodies, or
personal data.

## Development setup

```sh
./scripts/fetch-rune.sh      # vendored rune at the pinned rev (once)
./scripts/install-hooks.sh   # fmt + secret/TODO/doc-link scans
cargo build --workspace
./scripts/demo.sh            # launches against a throwaway corpus
```

## Picking up an issue

Claim implementation work before writing code; parallel agents use the
issue comment ledger to avoid duplicating each other. Run
`python3 scripts/agent-claim.py status <issue>` and then
`python3 scripts/agent-claim.py claim <issue> --agent <unique-name>` when
it is free. The full lease, race, heartbeat, and release rules are in
[`docs/agent-claims.md`](docs/agent-claims.md).

## Before you open a PR

- `cargo test --workspace --locked`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --no-deps -- -D warnings`
- `./scripts/lint.sh` for the full local gate (fmt + clippy + cargo-deny
  + unused deps)

Hard rules that PRs are rejected for violating: no reimplementing
Boris/Oliver semantics in Rust; no hand-authored Boris JSON fixtures
(regenerate via `scripts/capture-fixtures.sh`); no webview or in-app HTML
renderer; secrets never in argv/env/logs/events/persisted state;
`.cv/` sidecar for notes, never Boris frontmatter.

Dependencies: versions live in `[workspace.dependencies]`; members use
`workspace = true`. New releases need 7 days of age before a bump merges
(Dependabot enforces the cooldown). No wildcard or git dependencies.

## Conventions

- Conventional-commit style messages (`fix:`, `feat:`, `chore:`,
  `docs:`…). `scripts/release-notes.sh` groups them.
- Unit tests beside the code; integration tests under
  `crates/<crate>/tests/`; one behavior per test.
- Windows/Linux claims require evidence — "should work" is not proof
  (see `spike/FINDINGS.md`).
