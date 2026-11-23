#!/usr/bin/env bash
# capture-editor-fixtures.sh — capture REAL Boris outputs for the editor
# lane (L7) into fixtures/editor/captures/. Hand-authored Boris JSON is
# banned; rerun this with the pinned boris/0.9.0 instead.
#   BORIS=/path/to/boris scripts/capture-editor-fixtures.sh
set -uo pipefail
BORIS="${BORIS:-boris}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
E="$ROOT/fixtures/editor"
rm -rf "$E/captures"
for src in "$E"/corpus-src/*/; do
  c="$(basename "$src")"
  cap="$E/captures/$c"; mkdir -p "$cap"
  work="$(mktemp -d)"; cp -R "$src/." "$work/"
  ( cd "$work" && "$BORIS" build --no-rag ) > /dev/null 2> "$cap/ir-stderr.txt"
  echo $? > "$cap/ir-exit-code.txt"
  [ -f "$work/.boris/completion.json" ] && cp "$work/.boris/completion.json" "$cap/completion.json"
  ( cd "$work" && "$BORIS" build ) > /dev/null 2> "$cap/html-stderr.txt"
  echo $? > "$cap/html-exit-code.txt"
  if [ -d "$work/dist" ]; then
    ( cd "$work/dist" && find . -name '*.html' -not -path './_boris/*' | sort | while read -r f; do
        mkdir -p "$cap/html/$(dirname "$f")"; cp "$f" "$cap/html/$f"; done )
  fi
  rm -rf "$work"
done
{
  echo "schema_version = 1"
  echo "engine_version = '$("$BORIS" --version)'"
  echo "capture_command = \"scripts/capture-editor-fixtures.sh\""
  echo "captured = \"$(date -u +%Y-%m-%dT%H:%M:%SZ)\""
  echo
  ( cd "$E" && find corpus-src captures -type f | sort | while read -r f; do
      echo "\"$f\" = \"$(sha256sum "$f" | cut -d' ' -f1)\""; done )
} > "$E/MANIFEST.toml"
echo "captured into $E"
