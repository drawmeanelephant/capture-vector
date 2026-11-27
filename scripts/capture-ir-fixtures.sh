#!/usr/bin/env bash
# capture-ir-fixtures.sh — capture REAL Boris output for the IR projection
# the live session runs beside its watch (`build --out .cv/ir`), over the
# pinned corpora in fixtures/corpus-src/, into fixtures/ir/. Hand-authored
# Boris output is banned; rerun this with the pinned boris/0.9.0 instead.
#   BORIS=/path/to/boris scripts/capture-ir-fixtures.sh
#
# Format-flagged corpora are captured twice, mirroring capture-fixtures.sh:
# the plain capture records what a modeless `build --out` does (an
# ECOOKLANG/ETEXTILE refusal for the flagged corpora), and a parallel
# `<corpus>-modeflag` capture records the real IR Boris emits in the
# corpus's declared input mode — cook-one-modeflag carries the IR 0.4
# recipe facet. CV_CAPTURE_ONLY="<name> …" recaptures only those and
# keeps every other capture byte-for-byte; the manifest still pins every
# file. Name the modeflag passes by their base corpus (`cook-one`), not
# the derived directory.
set -uo pipefail
BORIS="${BORIS:-boris}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
source "$ROOT/scripts/fixture-sha256.sh"
SRC="$ROOT/fixtures/corpus-src"
I="$ROOT/fixtures/ir"
# Must match cv_engine::verbs::IR_DIR.
OUT=".cv/ir"
if [ -z "${CV_CAPTURE_ONLY:-}" ]; then
  rm -rf "$I"
fi
mkdir -p "$I/captures"

capture_one() {
  # bash assignments are global without `local`; the caller's modeflag
  # lives in these same names.
  local c="$1" extra="$2" suffix="$3"
  local src="$SRC/$c" cap work
  cap="$I/captures/$c$suffix"; rm -rf "$cap"; mkdir -p "$cap"
  work="$(mktemp -d)"
  # A project root (boris.json) builds from its root; a bare content root
  # is opened the way the app opens one: as the workspace's content/.
  if [ -f "$src/boris.json" ]; then
    cp -R "$src/." "$work/"
    ( cd "$work" && "$BORIS" build $extra --out "$OUT" ) > "$cap/stdout.txt" 2> "$cap/stderr.txt"
  else
    mkdir -p "$work/content"; cp -R "$src/." "$work/content/"
    ( cd "$work" && "$BORIS" build $extra --out "$OUT" --input content ) > "$cap/stdout.txt" 2> "$cap/stderr.txt"
  fi
  echo $? > "$cap/exit-code.txt"
  [ -f "$work/$OUT/graph.json" ] && cp "$work/$OUT/graph.json" "$cap/graph.json"
  rm -rf "$work"
}

for src in "$SRC"/*/; do
  c="$(basename "$src")"
  if [ -n "${CV_CAPTURE_ONLY:-}" ]; then
    case " $CV_CAPTURE_ONLY " in
      *" $c "*) ;;
      *) continue ;;
    esac
  fi
  # Format-flagged corpora get a second pass with their declared mode,
  # same mapping as capture-fixtures.sh. The flag is an argument vector
  # of one, so word splitting is intentional here.
  extra=""
  case "$c" in
    happy-textile|broken-textile) extra="--textile" ;;
    cook-one) extra="--cooklang" ;;
  esac
  capture_one "$c" "" ""
  [ -n "$extra" ] && capture_one "$c" "$extra" "-modeflag"
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
