#!/usr/bin/env bash
# capture-lab-fixtures.sh — capture REAL boris-migration-lab outputs into
# fixtures/captures/lab/<mode>/. Hand-authored lab JSON is banned: every
# file there comes from the pinned lab, run over the lab checkout's own
# fixtures by the cv-engine `capture_lab` example, which goes through
# cv_engine::lab::run (so argv.txt is exactly what PreparedRun builds).
#
#   scripts/capture-lab-fixtures.sh
#
# Lab:   CV_MIGRATION_LAB_BIN, else vendor/boris-migration-lab
#        (scripts/fetch-migration-lab.sh builds it at the pin).
# Boris: CV_BORIS_BIN, else `boris` on PATH. Only link-audit needs it, to
#        build the HTML tree it audits.
# Lab fixtures: CV_MIGRATION_LAB_FIXTURES, else <lab checkout>/fixtures.
#
# Rewrites the [lab_captures] table at the end of fixtures/MANIFEST.toml
# and leaves everything above it alone. The run is deterministic: a
# rerun at the same pins leaves the tree and the manifest byte-identical.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
source "$ROOT/scripts/fixture-sha256.sh"
FIX="$ROOT/fixtures"
MANIFEST="$FIX/MANIFEST.toml"

LAB="${CV_MIGRATION_LAB_BIN:-$ROOT/vendor/boris-migration-lab/zig-out/bin/boris-migration-lab}"
BORIS="${CV_BORIS_BIN:-$(command -v boris || true)}"
[ -x "$LAB" ] || { echo "capture-lab-fixtures: no lab at $LAB (run scripts/fetch-migration-lab.sh or set CV_MIGRATION_LAB_BIN)" >&2; exit 1; }
[ -n "$BORIS" ] && [ -x "$BORIS" ] || { echo "capture-lab-fixtures: no boris (set CV_BORIS_BIN)" >&2; exit 1; }

pin() { sed -n '/^\[migration_lab\]$/,/^\[/p' "$MANIFEST" | sed -n "s/^$1 = \"\(.*\)\"$/\1/p" | head -1; }
pin_commit=$(pin commit)
pin_tool_id=$(pin tool_id)
[ -n "$pin_commit" ] && [ -n "$pin_tool_id" ] || { echo "capture-lab-fixtures: MANIFEST.toml has no [migration_lab] pin" >&2; exit 1; }

# The binary cannot report its commit; its checkout can.
checkout="$(cd "$(dirname "$LAB")/../.." && pwd)"
if head=$(git -C "$checkout" rev-parse HEAD 2>/dev/null); then
  [ "$head" = "$pin_commit" ] || { echo "capture-lab-fixtures: $checkout is at $head, pin is $pin_commit" >&2; exit 1; }
else
  echo "capture-lab-fixtures: $checkout is not a git checkout; cannot confirm commit $pin_commit" >&2
  echo "capture-lab-fixtures: capture through scripts/fetch-migration-lab.sh or a binary inside the pinned checkout" >&2
  exit 1
fi

DEST="$FIX/captures/lab"
rm -rf "$DEST"
mkdir -p "$DEST"
( cd "$ROOT" && CV_MIGRATION_LAB_BIN="$LAB" CV_BORIS_BIN="$BORIS" \
    cargo run --locked -q -p cv-engine --example capture_lab -- "$DEST" )

tool_id=$("$LAB" --version | head -1)
[ "$tool_id" = "$pin_tool_id" ] || { echo "capture-lab-fixtures: lab is $tool_id, pin is $pin_tool_id" >&2; exit 1; }
boris_build_info=$("$BORIS" --build-info | head -1)
case "$boris_build_info" in *"'"*) echo "capture-lab-fixtures: build-info has a quote" >&2; exit 1 ;; esac

# Keep everything above [lab_captures]; this script owns the table and it
# stays last, after [migration_lab] (capture-fixtures.sh carries both).
above=$(sed '/^\[lab_captures\]$/,$d' "$MANIFEST")
tmp="$MANIFEST.tmp"
{
  printf '%s\n\n' "$above"
  echo '[lab_captures]'
  echo '# Written by scripts/capture-lab-fixtures.sh; do not edit by hand.'
  echo '# Each run gets a fresh scratch root; every occurrence of that root in'
  echo '# the captured text is replaced with capture_root.'
  echo "tool_id = \"$tool_id\""
  echo "commit = \"$pin_commit\""
  echo "boris_build_info = '$boris_build_info'"
  echo 'capture_root = "/CAPTURE_ROOT"'
  echo 'capture_command = "scripts/capture-lab-fixtures.sh"'
  echo ''
  echo '[lab_captures.files]'
  ( cd "$FIX" && find captures/lab -type f | LC_ALL=C sort ) | while IFS= read -r f; do
    case "$f" in *'"'*|*\\*) echo "capture-lab-fixtures: unquotable path $f" >&2; exit 1 ;; esac
    h=$(fixture_sha256 "$FIX/$f") || exit 1
    echo "\"$f\" = \"$h\""
  done
} > "$tmp"
mv "$tmp" "$MANIFEST"
echo "done -> $DEST ($(find "$DEST" -type f | wc -l | tr -d ' ') files)"
