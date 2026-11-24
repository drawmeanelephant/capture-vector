#!/usr/bin/env bash
# dev.sh — vendor rune on first use, then run the native app.
#
#   ./scripts/dev.sh --project <dir>
#   ./scripts/dev.sh --scene canary --tree target/canary.tree.json
#
# Extra arguments are forwarded to cv-app unchanged.
set -euo pipefail
cd "$(dirname "$0")/.."

if [ ! -d vendor/rune/.git ]; then
  echo "dev: vendor/rune missing; fetching the pinned rune rev" >&2
  ./scripts/fetch-rune.sh
fi

exec cargo run -p cv-app -- "$@"
