#!/usr/bin/env bash
# check-toolchain.sh — refuse to build with a rustc other than the one
# pinned in rust-toolchain.toml.
#
# A cargo that is not a rustup proxy (e.g. Homebrew's `rust` formula)
# ignores rust-toolchain.toml and builds with whatever version it ships.
# CV_ALLOW_TOOLCHAIN_DRIFT=1 downgrades the failure to a warning.
set -euo pipefail
cd "$(dirname "$0")/.."

channel="$(sed -n 's/^[[:space:]]*channel[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' rust-toolchain.toml | head -n 1)"
if [ -z "$channel" ]; then
  echo "toolchain: no channel found in rust-toolchain.toml" >&2
  exit 1
fi

if ! actual_line="$(rustc --version 2>/dev/null)"; then
  echo "toolchain: rustc not found on PATH; install rustup and Rust $channel" >&2
  exit 1
fi
actual="$(printf '%s\n' "$actual_line" | awk '{print $2}')"

if [ "$actual" = "$channel" ]; then
  exit 0
fi

rustc_path="$(command -v rustc)"
msg="toolchain: rust-toolchain.toml pins $channel but $rustc_path is $actual.
  This rustc is not a rustup proxy, so the pin is ignored (common with Homebrew's rust).
  Fix: put rustup's proxies first on PATH, for example
    export PATH=\"\$(brew --prefix rustup)/bin:\$PATH\"   # Homebrew rustup
    export PATH=\"\$HOME/.cargo/bin:\$PATH\"              # rustup-init
  or run a single command with: rustup run $channel cargo ...
  Set CV_ALLOW_TOOLCHAIN_DRIFT=1 to continue anyway."

if [ "${CV_ALLOW_TOOLCHAIN_DRIFT:-0}" = "1" ]; then
  printf 'warning: %s\n' "$msg" >&2
  exit 0
fi
printf 'error: %s\n' "$msg" >&2
exit 1
