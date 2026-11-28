# Capture Vector documentation

The documentation is split by audience so the README stays short and the
spec remains normative.

## Use the app

- [`user-guide.md`](user-guide.md) — install/run, workspaces, shell,
  editing, evidence, platform status.
- [`runbooks/desktop-app.md`](runbooks/desktop-app.md) — symptom-first
  debugging and escalation.
- [`PACKAGING.md`](PACKAGING.md) — pinned-engine release layout and what
  is not yet signed or verified.
- [`.env.example`](../.env.example) — documented environment variables.

## Contribute

- [`../CONTRIBUTING.md`](../CONTRIBUTING.md) — setup, quality gates, PR
  expectations.
- [`../SECURITY.md`](../SECURITY.md) — private vulnerability reporting and
  security scope.
- [`../AGENTS.md`](../AGENTS.md) — machine/human agent orientation and
  hard architecture rules.
- [`TRIAGE.md`](TRIAGE.md) — issue grounding, priorities, labels, review
  pass.
- [`agent-claims.md`](agent-claims.md) — lease protocol that lets multiple
  agents work parallel issues without colliding.
- [`architecture-overview.md`](architecture-overview.md) — readable crate,
  boundary, and verification map before diving into the normative spec.
- [`runbooks/boris-cli-contract.md`](runbooks/boris-cli-contract.md) — static
  and black-box checks for every Boris argv shape the app emits.
- [`runbooks/process-boundary-tests.md`](runbooks/process-boundary-tests.md) —
  process-tree regression coverage and its remaining failure bounds.
- [`tech-debt.md`](tech-debt.md) — accepted debt and the conditions for
  paying it down.

## Design authority

- [`../spec/README.md`](../spec/README.md) — spec-kit index and dispatch
  model.
- [`../spec/PROMPT-v3.md`](../spec/PROMPT-v3.md) — current normative work
  order.
- [`../spec/vision.md`](../spec/vision.md) — product shape and non-goals.
- [`../spec/architecture.md`](../spec/architecture.md) — crate map and
  boundary rules.
- [`../spec/contracts.md`](../spec/contracts.md) — frozen contracts.
- [`../spike/FINDINGS.md`](../spike/FINDINGS.md) — verified platform/UI
  capability evidence.

If prose docs and the spec disagree, the spec wins for intended behavior
and the code/test evidence wins for current behavior. File a docs issue
when those three drift apart.
