#!/usr/bin/env bash
# check-hygiene.sh — repository-wide documentation and file hygiene.
set -euo pipefail
cd "$(dirname "$0")/.."

python3 scripts/check-agents-docs.py
python3 scripts/check-version-drift.py
python3 scripts/check-duplicates.py
python3 scripts/test-agent-claim.py
scripts/check-large-files.sh
scripts/check-todos.sh
scripts/check-secrets.sh

echo "hygiene: ok"
