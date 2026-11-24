#!/usr/bin/env bash
# check-todos.sh — every TODO/FIXME/HACK/XXX must link to an issue.
#
#   scripts/check-todos.sh              # all tracked source
#   scripts/check-todos.sh a.rs b.sh    # staged files from the hook
#
# Accepted: TODO(#123), FIXME(#45). Rejected: TODO, TODO: later, FIXME.
set -euo pipefail
cd "$(dirname "$0")/.."

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
    vendor/*|target/*|.git/*|scripts/check-todos.sh) continue ;;
    *.rs|*.sh|*.toml|*.yml|*.yaml) ;;
    *) continue ;;
  esac

  while IFS= read -r line; do
    case "$line" in
      *TODO\(\#*|*FIXME\(\#*|*HACK\(\#*|*XXX\(\#*) continue ;;
    esac
    echo "untracked marker: $f:$line" >&2
    fail=1
  done < <(grep -nE '(^|[^A-Za-z])(TODO|FIXME|HACK|XXX)([^A-Za-z]|$)' "$f" || true)
done

if [ "$fail" -ne 0 ]; then
  echo "todos: use TODO(#issue) and record it in docs/tech-debt.md" >&2
  exit 1
fi
echo "todos: ok"
