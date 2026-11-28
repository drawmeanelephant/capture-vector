#!/usr/bin/env bash
# fetch-migration-lab.sh — build boris-migration-lab at the pinned commit
# into vendor/boris-migration-lab (gitignored).
#
# The pin lives in fixtures/MANIFEST.toml [migration_lab]; the
# cv_engine::lab::PINNED mirror is checked against it by
# `cargo test -p cv-engine --test pin`. Needs git, a zig in the pinned
# major.minor series, and network for the clone and zig's package fetch.
# The exact zig_version in the manifest is capture provenance, not an
# exact-patch requirement.
#
#   ./scripts/fetch-migration-lab.sh
#   ZIG=/path/to/zig ./scripts/fetch-migration-lab.sh
#   export CV_MIGRATION_LAB_BIN="$PWD/vendor/boris-migration-lab/zig-out/bin/boris-migration-lab"
set -eu
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MANIFEST="$ROOT/fixtures/MANIFEST.toml"
DEST="$ROOT/vendor/boris-migration-lab"
ZIG="${ZIG:-zig}"

pin() {
  sed -n '/^\[migration_lab\]$/,/^\[/s/^'"$1"' = "\(.*\)"$/\1/p' "$MANIFEST"
}
REPO=$(pin repo)
COMMIT=$(pin commit)
TOOL_ID=$(pin tool_id)
ZIG_SERIES=$(pin zig_series)
ZIG_VERSION=$(pin zig_version)
if [ -z "$REPO" ] || [ -z "$COMMIT" ] || [ -z "$TOOL_ID" ] || [ -z "$ZIG_SERIES" ] || [ -z "$ZIG_VERSION" ]; then
  echo "fetch-migration-lab: no complete [migration_lab] pin in $MANIFEST" >&2
  exit 1
fi

case $ZIG in
  [A-Za-z]:[/\\]*)
    if command -v cygpath >/dev/null 2>&1; then
      ZIG=$(cygpath -u "$ZIG") || {
        echo "fetch-migration-lab: could not convert the ZIG executable path with cygpath" >&2
        exit 1
      }
    fi
    ;;
esac

if ! found_zig=$("$ZIG" version 2>/dev/null); then
  echo "fetch-migration-lab: selected zig is not executable; need zig $ZIG_SERIES.x on PATH or in ZIG" >&2
  exit 1
fi
case $found_zig in
  "$ZIG_SERIES".*) ;;
  *)
    echo "fetch-migration-lab: zig $found_zig is unsupported; need zig $ZIG_SERIES.x (canonical capture used $ZIG_VERSION)" >&2
    exit 1
    ;;
esac

# .git is a file, not a dir, when the checkout is a linked worktree.
if ! git -C "$DEST" rev-parse --git-dir >/dev/null 2>&1; then
  git clone --no-checkout "$REPO" "$DEST"
fi
git -C "$DEST" fetch --depth 1 origin "$COMMIT" || git -C "$DEST" fetch origin
git -C "$DEST" checkout -q --detach "$COMMIT"
( cd "$DEST" && "$ZIG" build --release=safe )

BIN="$DEST/zig-out/bin/boris-migration-lab"
[ -f "$BIN.exe" ] && BIN="$BIN.exe"
found=$("$BIN" --version)
if [ "$found" != "$TOOL_ID" ]; then
  echo "fetch-migration-lab: built $found, pinned $TOOL_ID" >&2
  exit 1
fi
echo "boris-migration-lab @ $(git -C "$DEST" rev-parse HEAD) -> $BIN"
echo "export CV_MIGRATION_LAB_BIN=\"$BIN\""
