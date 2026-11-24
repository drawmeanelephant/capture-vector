#!/usr/bin/env bash
# install-hooks.sh — enable the checked-in .githooks/pre-commit hook.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

git config core.hooksPath .githooks
chmod +x .githooks/pre-commit
for f in scripts/*.sh; do chmod +x "$f"; done
for f in scripts/*.py; do [ -e "$f" ] && chmod +x "$f"; done

echo "installed: core.hooksPath=.githooks"
echo "bypass a single commit with: git commit --no-verify"
