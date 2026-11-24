#!/usr/bin/env bash
# PNG goldens for the shell's named scenes, on the canonical rasterizer
# only (Linux + Lavapipe). Elsewhere this exits 0: the cross-OS gate is
# the element-tree/layout JSON (`cargo test -p cv-app --test goldens`).
#
#   scripts/check-ui-png.sh            # compare against fixtures/ui/golden/png
#   CV_BLESS=1 scripts/check-ui-png.sh # rewrite the goldens (review the diff!)
set -euo pipefail
cd "$(dirname "$0")/.."

if [ "$(uname -s)" != "Linux" ]; then
  echo "check-ui-png: not the canonical rasterizer ($(uname -s)); skipping"
  exit 0
fi
export WGPU_BACKEND="${WGPU_BACKEND:-vulkan}"

SCENES=(canary palette light store-corrupt pages outputs activity broken-parent activity-broken empty pages-watch-dead audit-empty audit audit-detail audit-degraded)
GOLD=fixtures/ui/golden/png
OUT=target/ui
mkdir -p "$GOLD" "$OUT"
cargo build -q -p cv-app -p xtask 2>/dev/null || cargo build -p cv-app -p xtask

fail=0
for s in "${SCENES[@]}"; do
  ./target/debug/cv-app --scene "$s" --snap "$OUT/$s.png" >/dev/null 2>&1
  if [ "${CV_BLESS:-}" = "1" ]; then
    cp "$OUT/$s.png" "$GOLD/$s.png"
    echo "blessed $GOLD/$s.png"
  elif ! ./target/debug/xtask png-diff "$GOLD/$s.png" "$OUT/$s.png" --tolerance 8 --max-ratio 0.002; then
    echo "  actual: $OUT/$s.png"
    fail=1
  fi
done
exit "$fail"
