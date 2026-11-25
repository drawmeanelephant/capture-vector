---
name: fixture-refresh
description: Regenerate Capture Vector's Boris and boris-migration-lab fixture captures and verify the hash manifest without hand-authoring JSON.
---

# Fixture refresh

Use this when a Boris contract changes, a new corpus is added, or a fixture
test fails on a hash.

## Guardrails

- Hand-authored Boris JSON is banned. Regenerate with the capture script.
- Fixtures are byte-pinned in `fixtures/MANIFEST.toml`; commit the
  manifest and the captures together.
- Ordinary CI and tests must not need a Boris binary; only capture does.
- `[migration_lab]` near the end of the manifest is a tool pin, not a
  capture. The capture scripts carry it forward unchanged; change it
  only together with `cv_engine::lab::PINNED` and rebuild with
  `scripts/fetch-migration-lab.sh`.
- `fixtures/captures/lab/` and the `[lab_captures]` table (always last)
  belong to `scripts/capture-lab-fixtures.sh`. `capture-fixtures.sh`
  leaves both alone, and the lab script rewrites only that table.

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

## Lab captures

1. Build the pinned lab: `scripts/fetch-migration-lab.sh` (or point
   `CV_MIGRATION_LAB_BIN` at a build of the pinned commit inside its
   checkout). Set `CV_BORIS_BIN` to the pinned Boris; link-audit audits
   a Boris HTML build of the `hugo` capture.
2. Run `scripts/capture-lab-fixtures.sh`. It runs every row of
   `cv_fixtures::lab::LAB_CAPTURES` through `cv_engine::lab::run` over
   the lab checkout's own `fixtures/`, replaces each run's scratch root
   with `/CAPTURE_ROOT`, and rewrites `[lab_captures]`.
3. Run it a second time: `git status` must show no change (the capture
   is deterministic).
4. Run `cargo test -p cv-contracts -p cv-fixtures` and the live check
   `cargo test -p cv-engine --test lab_conformance -- --ignored` with
   both binaries set.
5. To capture another mode, add a row to `LAB_CAPTURES`, recapture, and
   mirror any new document in `cv_contracts::lab`.

## Failure handling

- `E*` diagnostic codes changing means the corpus or engine changed;
  update the lane that asserts the code, not the capture.
- A hash mismatch on an unchanged corpus means a line-ending conversion
  or a hand edit. Check `.gitattributes` (`fixtures/** -text`).
- A lab `DecodeError::UnsupportedSchemaVersion` after a recapture means
  the lab changed a contract: update `cv_contracts::lab` to the new
  version from the lab's `docs/contracts/`, never by guessing.
