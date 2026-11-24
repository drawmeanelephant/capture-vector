# Issue triage

How maintainers review incoming issues and ground them before work
starts. Filers: the more of this you supply up front, the faster the
issue moves.

## Labels

Every triaged issue carries **one priority** and usually one area:

| Label | Meaning |
|---|---|
| `priority: critical` | Data loss, security, or unusable app — drop everything |
| `priority: high` | Blocks a lane or real users — next up |
| `priority: medium` | Scheduled work, not blocking |
| `priority: low` | Nice to have |
| `area: engine` | Boris process adapter, engine boundary |
| `area: ui` | Shell, widgets, editor, windows |
| `area: build` | Build, CI, packaging, release |
| `area: security` | Secrets, scanning, supply chain |
| `area: docs` | Docs, runbooks, AGENTS.md |
| `needs-info` | Waiting on the reporter for grounding evidence |

## Grounding checklist

An issue is *grounded* when a reviewer can re-derive the claim without
guessing. Ask for what is missing and apply `needs-info`:

1. **Revision and toolchain** — `git rev-parse --short HEAD`,
   `rustc --version`. Fixes and behavior shift fast pre-release.
2. **Platform** — macOS is the proven base; Linux real-window behavior
   has evidence; Windows is unproven (`spike/FINDINGS.md`). A report on
   an unproven platform is still valid — label it, do not close it.
3. **Reproduction** — the exact command or drive path. Prefer
   `scripts/demo.sh` or `--scene`/`--script` so it runs headless.
4. **Evidence** — `CV_LOG=debug` output (secrets stripped), and for UI
   bugs the `--tree`/`--layout` JSON diffed against
   `fixtures/ui/golden/tree/`. Tree JSON is the cross-OS gate; PNGs only
   count on the Lavapipe config.
5. **Expected vs disclosed** — check the status line and runbook first.
   A disclosed degraded state (`engine absent`, `watch dead`) is the
   contract working, not a bug. Link the spec section either way.

## Review pass

- Duplicates: search open issues before answering; close with
  `duplicate` and link the canonical one.
- Spec questions: quote `spec/` in the reply. If the spec is ambiguous
  or wrong, retitle the issue as a spec problem and label `area: docs`.
- Security: anything touching secret flow, process spawning outside
  `cv-engine`, or publishing gates goes private per `SECURITY.md`.
- Sizing: if a fix needs a spec change or spans lanes, say so in a
  comment and link the spec section — do not silently expand scope.

When grounding arrives, remove `needs-info` and set the priority.
