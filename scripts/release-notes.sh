#!/usr/bin/env bash
# release-notes.sh — Markdown release notes from conventional commits.
#
#   scripts/release-notes.sh                  # since the last tag
#   scripts/release-notes.sh v0.1.0..HEAD     # explicit range
#   CV_NOTES_OUT=CHANGELOG.md scripts/release-notes.sh
#
# No third-party release tooling: git log plus shell grouping.
set -euo pipefail
cd "$(dirname "$0")/.."

range="${1:-}"
if [ -z "$range" ]; then
  if last_tag="$(git describe --tags --abbrev=0 2>/dev/null)"; then
    range="$last_tag..HEAD"
  else
    range="HEAD"
  fi
fi

tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT

git log --no-merges --format='%h%x09%s' "$range" \
  | while IFS=$'\t' read -r sha subject; do
      section="Other"
      case "$subject" in
        feat:*|feat\(*\)) section="Features" ;;
        fix:*|fix\(*\)) section="Fixes" ;;
        docs:*|docs\(*\)) section="Documentation" ;;
        deps:*|ci:*|build:*|chore\(deps\)*) section="Build and dependencies" ;;
        chore:*|refactor:*|test:*|perf:*) section="Maintenance" ;;
      esac
      printf '%s\t%s\t%s\n' "$section" "$sha" "$subject" >> "$tmp"
    done

if [ ! -s "$tmp" ]; then
  echo "No commits in $range"
  exit 0
fi

{
  echo "# Release notes"
  echo
  echo "Range: \`$range\`"
  echo
  for section in "Features" "Fixes" "Documentation" "Build and dependencies" "Maintenance" "Other"; do
    if grep -q "^$section"$'\t' "$tmp"; then
      echo "## $section"
      echo
      grep "^$section"$'\t' "$tmp" | while IFS=$'\t' read -r _ sha subject; do
        echo "- $subject ($sha)"
      done
      echo
    fi
  done
} | if [ -n "${CV_NOTES_OUT:-}" ]; then tee "$CV_NOTES_OUT"; else cat; fi
