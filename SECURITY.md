# Security policy

## Reporting a vulnerability

**Do not open a public issue for a vulnerability.** Use GitHub's private
vulnerability reporting:

1. Open the repository's **Security** tab.
2. Choose **Advisories → Report a vulnerability**.
3. Describe the impact, the affected surfaces (crate, module, scene), and
   a minimal reproduction.

Private reporting is enabled on this repository, so the report stays
between you and the maintainers until a fix is ready. If you cannot use
the advisory flow, say so in a public issue **without details** and a
maintainer will arrange a private channel.

Expect an acknowledgement within a few days. This is a pre-release,
single-maintainer project; fixes land on `main` and are called out in the
release notes.

## Scope

In scope:

- Secret handling: publish secrets must never reach argv, environment,
  logs, events, or persisted state (`AGENTS.md` hard rules). Any path
  that leaks `CV_BSKY_*`, `CV_NOSTR_NSEC`, or a keychain item is a
  vulnerability.
- Process boundary: `cv-engine` is the only crate allowed to spawn
  processes. Argument injection through project content, workspace
  paths, or frontmatter is a vulnerability.
- Filesystem confinement: reads/writes outside the opened workspace
  root or the `.cv/` sidecar that the app did not disclose.
- Supply chain: a compromised dependency or a path around the vendored
  rune pin / `[patch.crates-io]` firewall.
- Publishing: content published that failed a normative rule, or draft /
  `noindex` material leaking into published output, RSS, or sitemaps.

Out of scope:

- Heuristic SEO/AEO findings — they can never block publishing by
  design (`authority_tier` in `spec/contracts.md`).
- Boris or Oliver engine internals — report those upstream; Capture
  Vector must not reimplement their semantics.
- Attacks that require the ability to run arbitrary code on the host
  already.

## What helps a report

- `git rev-parse --short HEAD` and `rustc --version`.
- Platform and OS version. Windows and Linux behavior is unproven
  (`spike/FINDINGS.md`); a report that only reproduces there is still
  welcome, just label the platform.
- Whether `--scene` / `--tree` reproduces it headlessly.

Never include a real secret, session token, or page body in a report.
Redact paths containing personal names.
