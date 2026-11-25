<img src="assets/icon.svg" width="64" height="64" alt="">

# CAPTURE-VECTOR

Outlook-style mailboxes for authoring, auditing, and publishing Boris
content. A native Rust desktop app: rune for the UI, Boris and Oliver as
subprocess engines.

**Status:** pre-release. macOS and Linux window/IME behavior is proven;
Windows is unproven (see [`spike/FINDINGS.md`](spike/FINDINGS.md)). The
normative work order is [`spec/PROMPT-v3.md`](spec/PROMPT-v3.md); the full
spec kit lives in [`spec/`](spec/README.md). Packaging and release:
[`docs/PACKAGING.md`](docs/PACKAGING.md).

## What works today

- Browse Pages, Outputs, Audit, Research, and Activity in one native
  Outlook-style shell.
- Edit source pages in companion windows with undo, completion, and
  conflict-aware workspace-contained saves.
- Run engine-backed builds/watch sessions when pinned Boris is present;
  absent/mismatched/dead engine states are disclosed instead of hidden.
- Verify the shell headlessly through element-tree/layout JSON goldens.

Still ahead: signed installers, Windows proof, and the fully wired publish
module. The user-facing guide is [`docs/user-guide.md`](docs/user-guide.md).

## Prerequisites

- Rust 1.99.1 — [`rust-toolchain.toml`](rust-toolchain.toml) pins it;
  rustup selects it automatically. Homebrew's `rust` formula puts its own
  `cargo`/`rustc` first on PATH and ignores the pin; put rustup's proxies
  first (`$(brew --prefix rustup)/bin` or `~/.cargo/bin`). The scripts
  refuse to run on drift (`scripts/check-toolchain.sh`).
- `git` (the pinned rune checkout is vendored by script).
- A Boris `0.9.0` binary is optional at launch. Without one the app still
  browses a workspace and discloses `engine absent` in the status line.
- Linux: `libvulkan-dev`, `mesa-vulkan-drivers`, `libxkbcommon-dev`.
- macOS: Xcode command line tools.

## Quick start — one command

```sh
./scripts/demo.sh
```

`scripts/demo.sh` copies `fixtures/corpus-src/happy` to a throwaway
directory and launches `cv-app` against it. `scripts/dev.sh` vendors rune
at the pinned rev when `vendor/rune` is missing, then runs the app. The
first run fetches rune and compiles the workspace; later runs are
incremental.

## Build

```sh
./scripts/fetch-rune.sh     # once per clone; vendor/ is gitignored
cargo build --workspace
```

## Run

```sh
cargo run -p cv-app -- --project <workspace-dir>
# or
CV_PROJECT=<workspace-dir> cargo run -p cv-app
```

`--project` wins over `CV_PROJECT`; with neither, the most recent
workspace reopens, then the fixture demo. `mod+shift+O` opens the native
folder picker.

Headless exports (no window, no click):

```sh
cargo run -p cv-app -- --scene canary --tree target/canary.tree.json
cargo run -p cv-app -- --scene canary --snap target/canary.png
```

`--tree` is the cross-OS UI gate. `--snap` needs a GPU and is only
canonical on Linux with Lavapipe.

## Test

```sh
cargo test --workspace --locked
cargo test -p cv-app --test goldens          # element-tree/layout goldens
cargo test -p cv-engine --test conformance   # stub == process engine
```

The suite needs no Boris binary, display, GPU, or network.

## Interactive QA

The app has no login gate and no external service. This path works on
macOS (proven) and should work on Linux/Windows once the UI is verified
there.

1. Launch against a throwaway copy of the happy corpus:

   ```sh
   CV_EDIT=content/index.md ./scripts/demo.sh
   ```

2. Expect a native window titled **Capture Vector** with the Pages
   mailbox selected and `index.md` listed. The status line names the
   workspace. Without a Boris binary it also says the engine is absent;
   browsing still works.

3. Drive at least one interaction:
   - `mod+K` opens the command palette; run **Go to Outputs**.
   - `mod+1` … `mod+9` switch the first nine mailboxes.
   - `mod+O` follows the selected linked noun.
   - Click a row or press `Enter` to open it in a companion window.
   - With `CV_EDIT`, type in the editor and press `mod+S`; the status
     line confirms the save.

4. Run a Boris verb from the palette (needs Boris 0.9.0; set
   `CV_BORIS_BIN` when it is not on `PATH`):
   - `mod+K`, type `build`, run **Boris: Build site**. A progress
     dialog with **Cancel** shows `boris build` until Boris exits.
   - Expect the status line to read `boris build finished (exit 0, … ms)
     · watch resumed`, and Activity › Builds to list **build finished**
     with its command, exit code, duration and stderr in the reading pane.
   - **Boris: Validate** and **Boris: Check (findings to Audit)** report on
     the status line; check's findings appear in Audit under
     **Pack · boris-check**. Without a located engine, every **Boris:**
     command is listed dimmed with the reason.

5. Headless fallback (no display): skip step 1 and run
   `cargo run -p cv-app -- --scene canary --tree target/qa.tree.json`,
   then confirm the JSON contains the shell tree.

Environment variables are documented in [`.env.example`](.env.example).
Runbooks live in [`docs/runbooks/desktop-app.md`](docs/runbooks/desktop-app.md).

## Repository map

| Path | What it is |
|---|---|
| `crates/cv-core` | Domain types and sync ports; no I/O, async, or UI |
| `crates/cv-contracts` | Serde mirrors of Boris contracts |
| `crates/cv-engine` | The only process owner (Boris adapter) |
| `crates/cv-app` | Composition root and native shell |
| `crates/cv-widgets` | The only crate importing rune |
| `crates/cv-model`, `cv-ui-editor`, `cv-notes` | State, editor, research |
| `crates/cv-ledger`, `cv-publish`, `cv-rules`, `cv-store` | Publish and audit lanes |
| `fixtures/` | Real Boris captures, hash-pinned |
| `spec/` | Normative spec and lane work orders |
| `spike/` | Proven Rune capabilities and probes |
| `scripts/` | Setup, fixture capture, goldens, hooks |
| `xtask/` | Codegen, parity, release helpers |

## Documentation

Start at [`docs/README.md`](docs/README.md). The short map:

- using the app: [`docs/user-guide.md`](docs/user-guide.md)
- troubleshooting: [`docs/runbooks/desktop-app.md`](docs/runbooks/desktop-app.md)
- packaging/release: [`docs/PACKAGING.md`](docs/PACKAGING.md)
- contributing: [`CONTRIBUTING.md`](CONTRIBUTING.md)
- parallel agent coordination: [`docs/agent-claims.md`](docs/agent-claims.md)
- architecture overview: [`docs/architecture-overview.md`](docs/architecture-overview.md)
- normative design: [`spec/README.md`](spec/README.md)

## Contributing

Read [`AGENTS.md`](AGENTS.md) first. Install the local quality hooks with
`./scripts/install-hooks.sh`. Before implementing a GitHub issue, claim it
with `python3 scripts/agent-claim.py claim <issue>` as described in
[`docs/agent-claims.md`](docs/agent-claims.md); the issue comment ledger is
what keeps parallel agents from duplicating work.

CI runs `cargo test --workspace`, `cargo fmt --check`,
`cargo clippy -D warnings`, and `cargo-deny`.

Created in [T3 Code](https://t3.codes).
