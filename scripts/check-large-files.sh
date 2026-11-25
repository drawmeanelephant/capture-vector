#!/usr/bin/env bash
# check-large-files.sh — fail on oversized tracked files.
#
#   scripts/check-large-files.sh              # all tracked files
#   scripts/check-large-files.sh a.rs b.json  # staged files from the hook
#
# Thresholds: 1200 lines for text, 2 MiB for any file. Fixture PNGs are
# well under the byte limit; generated/vendor trees are skipped.
set -euo pipefail
cd "$(dirname "$0")/.."

MAX_LINES="${CV_MAX_LINES:-1200}"
MAX_BYTES="${CV_MAX_BYTES:-2097152}"

files=()
if [ "$#" -gt 0 ]; then
  files=("$@")
else
  while IFS= read -r -d '' f; do files+=("$f"); done < <(git ls-files -z)
fi

fail=0
for f in "${files[@]}"; do
  [ -f "$f" ] || continue
  case "$f" in
    vendor/*|target/*|.git/*) continue ;;
  esac

  bytes=$(wc -c < "$f" | tr -d ' ')
  if [ "$bytes" -gt "$MAX_BYTES" ]; then
    echo "large file: $f is $bytes bytes (limit $MAX_BYTES)" >&2
    fail=1
    continue
  fi

  # Lockfiles and captured goldens are generated and legitimately long;
  # they still have to stay under the byte limit.
  line_check=1
  case "$f" in
    Cargo.lock|*/Cargo.lock|*.lock|fixtures/ui/golden/tree/*|fixtures/ui/golden/editor/*|fixtures/captures/*)
      line_check=0
      ;;
  esac

  # grep -I treats binary files as non-matching and keeps this portable.
  if [ "$line_check" -eq 1 ] && LC_ALL=C grep -Iq . "$f" 2>/dev/null; then
    lines=$(wc -l < "$f" | tr -d ' ')
    if [ "$lines" -gt "$MAX_LINES" ]; then
      echo "large file: $f has $lines lines (limit $MAX_LINES)" >&2
      fail=1
    fi
  fi
done

if [ "$fail" -ne 0 ]; then
  echo "large-files: split the file or add a documented exemption" >&2
  exit 1
fi
echo "large-files: ok"
