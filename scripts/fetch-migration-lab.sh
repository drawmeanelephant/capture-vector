#!/usr/bin/env bash
# fetch-migration-lab.sh — build boris-migration-lab at the pinned commit
# into vendor/boris-migration-lab (gitignored).
#
# The pin lives in fixtures/MANIFEST.toml [migration_lab]; the
# cv_engine::lab::PINNED mirror is checked against it by
# `cargo test -p cv-engine --test pin`. Needs git, a zig release in the
# pinned major.minor series, and network for the clone and zig's package
# fetch. The exact zig_version in the manifest is the compiler version
# recorded in the original tool pin; the capture pipeline does not record
# the compiler, so it is not an exact-patch requirement.
#
#   ./scripts/fetch-migration-lab.sh
#   ZIG=/path/to/zig ./scripts/fetch-migration-lab.sh   # POSIX shells
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

series_re='^(0|[1-9][0-9]*)[.](0|[1-9][0-9]*)$'
release_re='^(0|[1-9][0-9]*)[.](0|[1-9][0-9]*)[.](0|[1-9][0-9]*)$'
if [[ ! $ZIG_SERIES =~ $series_re ]]; then
  echo "fetch-migration-lab: migration-lab zig_series pin $ZIG_SERIES is not a major.minor series" >&2
  exit 1
fi
if [[ ! $ZIG_VERSION =~ $release_re ]]; then
  echo "fetch-migration-lab: migration-lab zig_version pin $ZIG_VERSION is not a release semver" >&2
  exit 1
fi
zig_version_series="${BASH_REMATCH[1]}.${BASH_REMATCH[2]}"
if [ "$zig_version_series" != "$ZIG_SERIES" ]; then
  echo "fetch-migration-lab: migration-lab zig_version pin $ZIG_VERSION is outside zig_series $ZIG_SERIES" >&2
  exit 1
fi

unsupported_zig() {
  echo "fetch-migration-lab: zig $1 is unsupported; need a release version in the zig $ZIG_SERIES.x series (tool pin recorded zig $ZIG_VERSION)" >&2
  exit 1
}

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

# A relative executable path is relative to the caller's cwd, not to the
# lab checkout this script cd's into for the build. Resolve it before the
# first cd; a bare command name such as ZIG=zig still uses PATH.
case $ZIG in
  /*) ;;
  */*)
    if [ -e "$ZIG" ]; then
      zig_dir=${ZIG%/*}
      zig_base=${ZIG##*/}
      zig_dir_abs=$(CDPATH="" cd -- "$zig_dir" 2>/dev/null && pwd -P) || {
        echo "fetch-migration-lab: could not resolve the ZIG executable directory" >&2
        exit 1
      }
      ZIG="$zig_dir_abs/$zig_base"
    fi
    ;;
esac

if ! found_zig=$("$ZIG" version 2>/dev/null); then
  echo "fetch-migration-lab: selected zig is not executable; need zig $ZIG_SERIES.x on PATH or in ZIG" >&2
  exit 1
fi
if [[ ! $found_zig =~ $release_re ]]; then
  unsupported_zig "$found_zig"
fi
found_zig_series="${BASH_REMATCH[1]}.${BASH_REMATCH[2]}"
if [ "$found_zig_series" != "$ZIG_SERIES" ]; then
  unsupported_zig "$found_zig"
fi

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
