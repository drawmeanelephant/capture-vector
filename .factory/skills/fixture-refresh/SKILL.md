---
name: fixture-refresh
description: Regenerate Capture Vector's Boris fixture captures and verify the hash manifest without hand-authoring JSON.
---

# Fixture refresh

Use this when a Boris contract changes, a new corpus is added, or a fixture
test fails on a hash.

## Guardrails

- Hand-authored Boris JSON is banned. Regenerate with the capture script.
- Fixtures are byte-pinned in `fixtures/MANIFEST.toml`; commit the
  manifest and the captures together.
- Ordinary CI and tests must not need a Boris binary; only capture does.

## Steps

1. Confirm the engine identity: `boris --version` must be `boris/0.9.0`
   and `boris --build-info` must report vcs `66efcfb`.
2. Place the corpora tree to capture and run:
   `scripts/capture-fixtures.sh <corpora-dir>`.
   The script drives `boris` and records the exact command in the
   manifest.
3. Inspect the diff: every changed file must be a capture or the
   manifest. Never edit a capture by hand.
4. Run `cargo test -p cv-fixtures` and the lane tests that consume the
   changed corpus (`cargo test --workspace --locked` for full coverage).
5. If a UI golden depends on the changed watch session, rebless the
   element-tree/layout goldens with
   `CV_BLESS=1 cargo test -p cv-app --test goldens` and review the diff.

## Failure handling

- `E*` diagnostic codes changing means the corpus or engine changed;
  update the lane that asserts the code, not the capture.
- A hash mismatch on an unchanged corpus means a line-ending conversion
  or a hand edit. Check `.gitattributes` (`fixtures/** -text`).
