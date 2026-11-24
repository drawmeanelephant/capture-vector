# Packaging & release

A packaged Capture Vector embeds **the** pinned Boris — `boris/0.9.0`,
vcs `66efcfb`, the same pin as `fixtures/MANIFEST.toml`
(`cv_engine::locate::PINNED`). There is no bring-your-own engine in a
release build.

## Prerequisites

- Rust 1.99.1 (`rust-toolchain.toml`) and `bash scripts/fetch-rune.sh`.
- Zig **0.17.x** on `PATH` (Boris `build.zig.zon` minimum).
- `git` and network access — only for `fetch-boris`.

## 1. Fetch and build the pinned engine

```sh
cargo run -p xtask -- fetch-boris            # clones into vendor/boris
cargo run -p xtask -- fetch-boris --src DIR  # or use an existing checkout
```

Checks out tag `v0.9.0`, refuses any HEAD that is not `66efcfb`, refuses a
non-0.17 Zig, then runs

```sh
zig build --release=safe -Dcpu=baseline -Dvcs-revision=66efcfb
```

`-Dcpu=baseline` keeps the binary portable across machines (a `native`
build SIGILLs on other runners). The built binary must pass the same
`--build-info` version gate the app uses before it is staged at
`vendor/engines/boris` (gitignored). This is the only step that touches
the network; `cargo test --workspace` never does.

## 2. Build the release layout

```sh
cargo run -p xtask -- release [--out DIR]    # default target/dist
```

Produces `capture-vector-<version>-<os>-<arch>/` plus `.tar.gz` and
`.tar.gz.sha256`:

```text
cv-app            release build, stamped with CV_BUILD_VCS
engines/boris     the staged pinned engine
VERSION           CARGO_PKG_VERSION of cv-app
build-info.json   output of `cv-app --build-info`
```

Before archiving, the pipeline runs the packaged `cv-app --build-info`
with `CV_BORIS_BIN`/`CV_BORIS_SIBLING` removed and fails unless the engine
resolves as `Bundled` from `engines/boris` at the pinned version/revision.

`cv-app --version` prints `cv-app/<version>`; `cv-app --build-info` prints
JSON with the app version, git revision (`-dirty` if the tree was dirty),
target, engine pin, and the resolved engine (source, path, identity) or
the resolution error.

## Engine lookup order

`CV_BORIS_BIN` → bundled → `PATH` → sibling checkout (`CV_BORIS_SIBLING=1`
only). Bundled means `<exe dir>/engines/`, then the macOS `.app`
`Contents/Resources/engines/`, then `<exe dir>`. Every candidate must pass
the version gate; a mismatched engine is skipped, not trusted.

## `.cv/` sidecar schema

Each `.cv/` namespace (`research`, `ledger`) carries its own
`schema.json` stamp (`{"format":"cv-sidecar","namespace":…,"schema_version":N}`)
and migrates independently (`cv_store::sidecar`):

- missing namespace → created and stamped at the current version;
- no stamp but existing data → treated as version 0 and upgraded;
- older stamp → each migration step runs in order, the stamp advances
  after every successful step; a missing step fails before anything runs;
- newer stamp, foreign/malformed stamp → typed error, files untouched.

A newer `research` schema does not block opening the `ledger`, and vice
versa.

## Deferred — needs the owner (issue #2 territory)

Not built, not verified:

- **macOS** `.app` bundle, code signing, notarization — needs an Apple
  Developer account (signing budget decision) and the owner's Mac.
- **Windows** installer (MSI/MSIX) and Authenticode signing — needs a
  signing-certificate decision and a Windows machine.
- **Real-OS verification** of the package on macOS and Windows. The
  `release` pipeline and lookup code are platform-neutral (`boris.exe`
  on Windows, `.app` Resources fallback on macOS) but have only been run
  on Linux x86_64.
