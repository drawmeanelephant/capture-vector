#!/usr/bin/env bash
# demo.sh — launch against a throwaway copy of the happy corpus.
#
# The app writes `.cv/` beside the workspace, so the demo never opens
# fixtures/ in place. Set CV_DEMO_DIR to reuse a directory.
set -euo pipefail
cd "$(dirname "$0")/.."

if [ -n "${CV_DEMO_DIR:-}" ]; then
  tmp="$CV_DEMO_DIR"
  mkdir -p "$tmp"
  cleanup=0
else
  tmp="$(mktemp -d)"
  cleanup=1
fi

if [ "$cleanup" -eq 1 ]; then
  trap 'rm -rf -- "$tmp"' EXIT
fi

if [ ! -f "$tmp/boris.json" ]; then
  cp -R fixtures/corpus-src/happy/. "$tmp/"
fi

echo "demo workspace: $tmp" >&2
./scripts/dev.sh --project "$tmp" "$@"
