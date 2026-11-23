#!/usr/bin/env bash
# capture-research-fixtures.sh — REAL Boris/Oliver captures for L6 (research).
# Promotion drafts come from cv-notes' own `promote_with` (examples/
# research-fixture.rs); Boris judges them, Oliver renders note bodies.
# Never hand-edit fixtures/research/ — rerun this script.
set -eu
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BORIS="${CV_BORIS_BIN:-boris}"
OLIVER="${CV_OLIVER_BIN:-oliver}"
R="$ROOT/fixtures/research"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
rm -rf "$R"
mkdir -p "$R/corpus-src" "$R/captures" "$R/oliver"

( cd "$ROOT" && cargo run -q -p cv-notes --example research-fixture -- "$WORK" )

for c in promoted reparented orphan; do
  src="$R/corpus-src/$c"
  mkdir -p "$src"
  cp -R "$ROOT/fixtures/corpus-src/happy/." "$src/"
  cp -R "$WORK/$c/." "$src/"
  run="$WORK/run-$c"
  cp -R "$src" "$run"
  cap="$R/captures/$c"
  mkdir -p "$cap/validate" "$cap/build-ir"
  set +e
  ( cd "$run" && "$BORIS" validate ) > "$cap/validate/stdout.txt" 2> "$cap/validate/stderr.txt"
  echo $? > "$cap/validate/exit-code.txt"
  ( cd "$run" && "$BORIS" build --no-rag --out .boris ) > "$cap/build-ir/stdout.txt" 2> "$cap/build-ir/stderr.txt"
  echo $? > "$cap/build-ir/exit-code.txt"
  set -e
  [ -f "$run/.boris/graph.json" ] && cp "$run/.boris/graph.json" "$cap/build-ir/graph.json"
done

for f in markdown textile cooklang; do
  cp "$WORK/oliver/$f.in" "$R/oliver/$f.in"
  set +e
  "$OLIVER" render --from "$f" < "$R/oliver/$f.in" > "$R/oliver/$f.html" 2> "$R/oliver/$f.stderr.txt"
  echo $? > "$R/oliver/$f.exit-code.txt"
  set -e
done

sha() { if command -v shasum >/dev/null; then shasum -a 256 "$1"; else sha256sum "$1"; fi | cut -d' ' -f1; }
( cd "$R" && {
  echo 'schema_version = 1'
  echo "engine_version = '$("$BORIS" --build-info 2>/dev/null | head -1)'"
  echo "oliver_version = \"$("$OLIVER" --version | head -1)\""
  echo 'capture_command = "scripts/capture-research-fixtures.sh"'
  echo "captured = \"$(date -u +%Y-%m-%dT%H:%M:%SZ)\""
  echo ""
  find corpus-src captures oliver -type f | LC_ALL=C sort | while read -r f; do
    echo "\"$f\" = \"$(sha "$f")\""
  done
} > MANIFEST.toml )
echo "done -> $R"
