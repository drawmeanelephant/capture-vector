---
name: ui-goldens
description: Verify and update Capture Vector's cross-OS element-tree/layout goldens and the canonical Lavapipe PNG gate.
---

# UI goldens

Use this after changing the shell, a module's view, theme tokens, or a
fixture that a scene loads.

## What gates what

- **Element tree + layout JSON** is the cross-OS gate. It runs in-process
  with `rune::Runtime`, needs no GPU, and must pass on macOS, Linux, and
  Windows.
- **PNG** is canonical only on Linux with the Lavapipe software
  rasterizer. Local macOS/Windows PNGs are diagnostic, never authoritative.

## Steps

1. Run the cross-OS gate:
   `cargo test -p cv-app --test goldens`.
2. For a deliberate tree change, rebless trees:
   `CV_BLESS=1 cargo test -p cv-app --test goldens`.
   Then review the JSON diff for unintended layout movement.
3. Only on Linux with Lavapipe, compare PNGs:
   `scripts/check-ui-png.sh`.
   Rebless with `CV_BLESS=1 scripts/check-ui-png.sh` and review the
   images before committing.
4. Run `cargo test -p cv-app --test probes -- --nocapture` when the
   change touches rendering or pointer handling.

## Guardrails

- Do not raise PNG tolerance to hide a layout regression; fix the layout
  or rebless deliberately.
- `--tree` is the source of truth for reviewers on non-Linux machines.
- A golden change and a behavior change belong in the same commit with an
  explanation in the PR.
