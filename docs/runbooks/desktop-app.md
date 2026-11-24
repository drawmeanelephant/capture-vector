# Runbook — Capture Vector desktop app

Capture Vector is a local desktop app. There is no server, no on-call
rotation, and no remote deploy. This runbook covers the failure modes an
agent or maintainer can hit on a workstation.

## First checks

```sh
rustc --version                 # expect 1.99.1
git status --short              # do not debug on top of unknown edits
./scripts/fetch-rune.sh         # vendor/rune present?
cargo test --workspace --locked # fixture-only gate
```

If the app starts but the workspace is wrong, check the launch order:
`--project` → `CV_PROJECT` → most recent workspace. `mod+shift+O` opens
the folder picker; the recent list lives under the platform config dir
(`CV_CONFIG_DIR` overrides it).

## Symptom: app exits immediately

1. Run it in a terminal, not from a bundle:
   `CV_LOG=debug ./scripts/dev.sh --project <dir>`.
2. `vendor/rune` missing or at the wrong rev: rerun
   `./scripts/fetch-rune.sh`; it prints the expected
   `[patch.crates-io]` block.
3. GPU/windowing failure: try the headless gate
   `cargo run -p cv-app -- --scene canary --tree target/canary.tree.json`.
   If that passes, the failure is in native window creation, not the app
   model.
4. Linux: install `libvulkan-dev`, `mesa-vulkan-drivers`,
   `libxkbcommon-dev`.

## Symptom: "engine absent" / "version mismatch"

- The app browses the workspace without Boris; publishing and rebuilds
  need it.
- Put a real `boris/0.9.0` (vcs `66efcfb`) on `PATH`, or set
  `CV_BORIS_BIN=/path/to/boris`.
- Version mismatch is deliberate: the contract mirrors are keyed to the
  pinned engine. Do not "fix" it by editing the version string; build the
  pinned tag (`v0.9.0`) or point `CV_BORIS_BIN` at it.
- A sibling checkout is only considered with `CV_BORIS_SIBLING=1`.

## Symptom: watch dead / rebuilds never land

1. Confirm the engine version gate passes: `boris --version` and
   `boris --build-info`.
2. Reproduce with the captured sessions:
   `cargo test -p cv-engine --test watch_resync`.
3. Run the opt-in live smoke locally only if a pinned binary exists:
   `cargo test -p cv-engine --test live_smoke -- --ignored --nocapture`.
4. Capture structured logs with `CV_LOG=debug CV_LOG_FILE=/tmp/cv.log`
   and include them in the issue.

## Symptom: corrupt research store

- JSONL under `<workspace>/.cv/research/` is the source of truth; the
  SQLite index is rebuildable.
- Close the app, move the index aside (not the JSONL), and reopen:
  `mv <workspace>/.cv/research/index.sqlite <workspace>/.cv/research/index.sqlite.bak`.
  The app rebuilds it.
- If the JSONL itself is torn, restore from your backup or version
  control; do not hand-edit provenance records.

## Symptom: editor cannot save

- The save path is rooted at the workspace. A page outside the workspace
  is refused by design.
- Conflicts are reported when the on-disk fingerprint changed; reopen the
  page to reload the clean buffer, or keep the dirty buffer and copy the
  text out.
- Closing a dirty companion asks save/discard; a crash loses unsaved
  buffer text.

## Signature effects (opt-in)

The GPU effects (rail aurora in dark mode, a glow on Activity when a
build finishes, a breathing engine chip while a watch session is live)
are off by default and never run under reduced motion. To look at them:

```sh
cargo run -p cv-app --features effects -- --scene effects   # scripted
CV_EFFECTS=1 cargo run -p cv-app --features effects           # live app
```

Without `--features effects` the flag still adds the chip pulse and glow
gradient, but the rune-fx layers draw nothing. If an effect misbehaves on
a GPU, build without the feature or unset `CV_EFFECTS`; the plain shell
is unchanged.

## Rollback a bad build

This app is not deployed, so rollback is a local checkout operation:

```sh
git switch main
git revert <bad-commit>        # prefer a revert over rewriting history
./scripts/fetch-rune.sh
cargo test --workspace --locked
```

Fixtures are byte-pinned. If a fixture change is suspect, restore
`fixtures/` from the last known-good commit and rerun
`cargo test -p cv-fixtures`.

## Escalation

Open an issue with:

- OS and version, `rustc --version`;
- the launch command (never include secrets);
- `CV_LOG=debug` JSONL;
- the headless `--tree` output if the UI is involved;
- whether the workspace opened from `--project`, `CV_PROJECT`, or recent.
