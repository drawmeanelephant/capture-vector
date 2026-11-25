#!/usr/bin/env bash
# capture-ir-fixtures.sh — capture REAL Boris output for the IR projection
# the live session runs beside its watch (`build --out .cv/ir`), over the
# pinned corpora in fixtures/corpus-src/, into fixtures/ir/. Hand-authored
# Boris output is banned; rerun this with the pinned boris/0.9.0 instead.
#   BORIS=/path/to/boris scripts/capture-ir-fixtures.sh
set -uo pipefail
BORIS="${BORIS:-boris}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
source "$ROOT/scripts/fixture-sha256.sh"
SRC="$ROOT/fixtures/corpus-src"
I="$ROOT/fixtures/ir"
# Must match cv_engine::verbs::IR_DIR.
OUT=".cv/ir"
rm -rf "$I"
mkdir -p "$I/captures"
for src in "$SRC"/*/; do
  c="$(basename "$src")"
  cap="$I/captures/$c"; mkdir -p "$cap"
  work="$(mktemp -d)"
  # A project root (boris.json) builds from its root; a bare content root
  # is opened the way the app opens one: as the workspace's content/.
  if [ -f "$src/boris.json" ]; then
    cp -R "$src/." "$work/"
    ( cd "$work" && "$BORIS" build --out "$OUT" ) > "$cap/stdout.txt" 2> "$cap/stderr.txt"
  else
    mkdir -p "$work/content"; cp -R "$src/." "$work/content/"
    ( cd "$work" && "$BORIS" build --out "$OUT" --input content ) > "$cap/stdout.txt" 2> "$cap/stderr.txt"
  fi
  echo $? > "$cap/exit-code.txt"
  [ -f "$work/$OUT/graph.json" ] && cp "$work/$OUT/graph.json" "$cap/graph.json"
  rm -rf "$work"
done
{
  echo "schema_version = 1"
  echo "engine_version = '$("$BORIS" --version)'"
  echo "capture_command = \"scripts/capture-ir-fixtures.sh\""
  echo "captured = \"$(date -u +%Y-%m-%dT%H:%M:%SZ)\""
  echo
  ( cd "$I" && find captures -type f | sort | while IFS= read -r f; do
      h=$(fixture_sha256 "$f") || exit 1
      echo "\"$f\" = \"$h\""
    done )
} > "$I/MANIFEST.toml" || exit 1
echo "captured into $I"
