#!/usr/bin/env bash
# check-unused-deps.sh — fail on Cargo's unused-dependency manifest lints.
#
# Cargo reports both `unused dependency` (a member's manifest) and
# `unused workspace dependency` (a `[workspace.dependencies]` entry no member
# uses). Rustc's dead_code lint is separate and already denied by clippy.
set -euo pipefail
cd "$(dirname "$0")/.."

log="$(mktemp)"
trap 'rm -f "$log"' EXIT

cargo check --workspace --all-targets 2>&1 | tee "$log" >/dev/null

if grep -qE 'unused (workspace )?dependency' "$log"; then
  grep -E -A3 'unused (workspace )?dependency' "$log" >&2
  echo "unused-deps: remove the declaration or wire the dependency" >&2
  exit 1
fi

echo "unused-deps: ok"
