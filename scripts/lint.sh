#!/usr/bin/env bash
# lint.sh — local Rust quality gate. CI is not identical: it adds effects
# clippy, firewall scans, PNG goldens, and PR claims, and it makes
# cargo-deny mandatory.
set -euo pipefail
cd "$(dirname "$0")/.."
./scripts/check-toolchain.sh

# Check tracked workspace Rust only. `cargo fmt --all` would walk the
# vendored rune tree and spike/, and would pick up local scratch files that
# are not part of the repository.
tracked_rs="$(git ls-files -- crates xtask | grep -E '\.rs$' || true)"
if [ -n "$tracked_rs" ]; then
  printf '%s\n' "$tracked_rs" | xargs rustfmt --check --edition 2024
fi

cargo clippy --workspace --all-targets --no-deps -- \
  -D warnings -D clippy::cognitive_complexity
./scripts/check-unused-deps.sh
./scripts/check-hygiene.sh

if command -v cargo-deny >/dev/null 2>&1; then
  cargo deny check
else
  echo "lint: cargo-deny not installed; skipping dependency check" >&2
fi
