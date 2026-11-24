#!/usr/bin/env bash
# check-secrets.sh — block high-confidence secrets from being committed.
#
#   scripts/check-secrets.sh              # all tracked files
#   scripts/check-secrets.sh a.rs .env    # staged files from the hook
#
# Short fake keys in tests are below the length thresholds; a deliberately
# fake long key needs the line comment `secret-scan: allow` and a reason.
set -euo pipefail
cd "$(dirname "$0")/.."

files=()
if [ "$#" -gt 0 ]; then
  files=("$@")
else
  while IFS= read -r -d '' f; do files+=("$f"); done < <(git ls-files -z)
fi

patterns=(
  '-----BEGIN [A-Z ]*PRIVATE KEY-----'
  'AKIA[0-9A-Z]{16}'
  'gh[pousr]_[A-Za-z0-9]{36,}'
  'github_pat_[A-Za-z0-9_]{50,}'
  'xox[baprs]-[A-Za-z0-9-]{10,}'
  'nsec1[0-9a-z]{50,}'
  'CV_NOSTR_NSEC=[^[:space:]]+'
  'CV_BSKY_APP_PASSWORD=[^[:space:]]+'
)

fail=0
for f in "${files[@]}"; do
  [ -f "$f" ] || continue
  case "$f" in
    vendor/*|target/*|.git/*|scripts/check-secrets.sh) continue ;;
  esac

  for pattern in "${patterns[@]}"; do
    while IFS= read -r line; do
      case "$line" in
        *"secret-scan: allow"*) continue ;;
      esac
      echo "possible secret: $f:$line" >&2
      fail=1
    done < <(grep -nEI -- "$pattern" "$f" 2>/dev/null || true)
  done
done

if [ "$fail" -ne 0 ]; then
  echo "secrets: remove the value; secrets reach the engine on stdin only" >&2
  exit 1
fi
echo "secrets: ok"
