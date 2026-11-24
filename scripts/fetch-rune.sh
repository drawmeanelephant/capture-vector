#!/usr/bin/env bash
# fetch-rune.sh — vendor rune at the pinned dev rev into vendor/rune.
# Why this exists (spike/FINDINGS.md): rune CANNOT be a git dependency —
# it patches parley_engine/winit/wgpu-hal/sctk-adwaita/accesskit_* via
# local vendor paths, and a dependency's [patch] never propagates.
# Consumers must vendor rune and replicate the six-entry [patch.crates-io]
# block in their workspace root Cargo.toml (see the block at the bottom).
set -eu
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
REV="${CV_RUNE_REV:-2a59b0f95b82972478a497c42151dbcb3ca5c69f}"
DEST="$ROOT/vendor/rune"

# .git is a file, not a dir, when the vendor is a linked worktree.
if ! git -C "$DEST" rev-parse --git-dir >/dev/null 2>&1; then
  git clone --no-checkout https://github.com/anomalyco/rune "$DEST"
fi
git -C "$DEST" fetch --depth 1 origin "$REV" || git -C "$DEST" fetch origin "$REV"
git -C "$DEST" checkout -q "$REV"
echo "rune @ $(git -C "$DEST" rev-parse HEAD) -> $DEST"
echo "consumer Cargo.toml needs:"
cat <<'TOML'
[patch.crates-io]
wgpu-hal = { path = "vendor/rune/vendor/wgpu-hal" }
winit = { path = "vendor/rune/vendor/winit" }
sctk-adwaita = { path = "vendor/rune/vendor/sctk-adwaita" }
parley_engine = { path = "vendor/rune/vendor/parley_engine" }
accesskit_windows = { path = "vendor/rune/vendor/accesskit_windows" }
accesskit_macos = { path = "vendor/rune/vendor/accesskit_macos" }
TOML
