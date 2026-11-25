# Runbook — Capture Vector desktop app

Capture Vector is a local desktop app. There is no server, no on-call
rotation, and no remote deploy. This runbook covers the failure modes an
agent or maintainer can hit on a workstation.

## First checks

Open the Doctor first: click the engine chip at the left of the status
bar, or run **Open Doctor** (`shell.doctor`) from the command palette
(`mod+K`). It answers most of the questions below without a terminal:

- **Engine**: which Boris binary this window runs, how it was found
  (`CV_BORIS_BIN`, bundled, `PATH`, sibling checkout), its `--version`
  and `--build-info` against the pin, and every path the search tried.
- **Capabilities**: what the probe saw on this OS (watch, serve,
  validate, content writes) and whether the watch is native or one-shot.
- **Watch session**: live or dead, the last cursor and exit, and the last
  non-recoverable failure with its diagnostics.
- **.cv/ store**: each namespace's schema stamp against what this build
  reads, who holds the writer locks, and where the rebuildable research
  index lives.
- **App build**: version, commit, target, toolchain, and logging state.

**Copy Doctor support bundle** (palette) puts a JSON report of every
section on the clipboard for an issue. It leaves out page bodies, the
engine's absolute paths, environment values, and secret-looking fields;
the workspace root and home directory become `<workspace>` and `~`.

If the app does not start at all, or you need a clean baseline:

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

- Doctor → Engine names the refused binary (or lists every searched path
  when none was found) and what the pin wants.
- The app browses the workspace without Boris; publishing and rebuilds
  need it.
- Put a real `boris/0.9.0` (vcs `66efcfb`) on `PATH`, or set
  `CV_BORIS_BIN=/path/to/boris`.
- Version mismatch is deliberate: the contract mirrors are keyed to the
  pinned engine. Do not "fix" it by editing the version string; build the
  pinned tag (`v0.9.0`) or point `CV_BORIS_BIN` at it.
- A sibling checkout is only considered with `CV_BORIS_SIBLING=1`.

## Symptom: watch dead / rebuilds never land

1. Doctor → Watch session shows whether the session is live, the last
   exit, and the last non-recoverable failure with its diagnostics.
   Doctor → Engine confirms the version gate passed (`--version` and
   `--build-info` against the pin).
2. Reproduce with the captured sessions:
   `cargo test -p cv-engine --test watch_resync`.
3. Run the opt-in live smoke locally only if a pinned binary exists:
   `cargo test -p cv-engine --test live_smoke -- --ignored --nocapture`.
4. Capture structured logs with `CV_LOG=debug CV_LOG_FILE=/tmp/cv.log`
   and include them in the issue.

## Symptom: corrupt research store

- Doctor → .cv/ store shows each namespace's schema stamp, the writer
  locks, and the research index path.
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

- the Doctor support bundle (palette: **Copy Doctor support bundle**);
- OS and version, `rustc --version`;
- the launch command (never include secrets);
- `CV_LOG=debug` JSONL;
- the headless `--tree` output if the UI is involved;
- whether the workspace opened from `--project`, `CV_PROJECT`, or recent.
