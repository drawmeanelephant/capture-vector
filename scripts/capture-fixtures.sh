#!/usr/bin/env bash
# capture-fixtures.sh — capture REAL Boris outputs into fixtures/.
# Spec rule: hand-authored Boris JSON is banned. Everything in fixtures/captures/
# comes from running the pinned binary over fixtures/corpus-src/.
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BORIS="${CV_BORIS_BIN:-boris}"
CORPUS_SRC="${1:?usage: capture-fixtures.sh <corpora-dir> — e.g. a solipsist Stunts/ or Tests/Fixtures checkout}"
FIX="$ROOT/fixtures"
rm -rf "$FIX/corpus-src" "$FIX/captures"
mkdir -p "$FIX/corpus-src" "$FIX/captures"

"$BORIS" --version > "$FIX/engine-version.txt" 2>&1
"$BORIS" --build-info >> "$FIX/engine-version.txt" 2>&1 || true

CORPORA="happy happy-textile cook-one dogfood broken-duplicate-id broken-frontmatter broken-parent broken-textile broken-wikilink"

for c in $CORPORA; do
  src="$CORPUS_SRC/$c"
  [ -d "$src" ] || { echo "skip $c (no corpus at $src)"; continue; }
  echo "== $c =="
  mkdir -p "$FIX/corpus-src/$c"
  cp -R "$src/." "$FIX/corpus-src/$c/"
  run="$FIX/corpus-src/$c"
  cap="$FIX/captures/$c"; mkdir -p "$cap"

  # project roots carry boris.json and build normally; bare content roots
  # (the broken stunts) are passed via --input
  if [ -f "$run/boris.json" ]; then
    for verb in build validate check; do
      mkdir -p "$cap/$verb"
      ( cd "$run" && "$BORIS" "$verb" ) > "$cap/$verb/stdout.txt" 2> "$cap/$verb/stderr.txt"
      echo $? > "$cap/$verb/exit-code.txt"
    done
    RUNV() { ( cd "$run" && "$BORIS" "$@" ); }
  else
    for verb in build validate check; do
      mkdir -p "$cap/$verb"
      "$BORIS" "$verb" --input "$run" > "$cap/$verb/stdout.txt" 2> "$cap/$verb/stderr.txt"
      echo $? > "$cap/$verb/exit-code.txt"
    done
    RUNV() { "$BORIS" "$@" --input "$run"; }
  fi

  # format-flagged corpora get a second pass with their declared mode
  EXTRA=""
  case "$c" in
    happy-textile|broken-textile) EXTRA="--textile" ;;
    cook-one) EXTRA="--cooklang" ;;
  esac
  if [ -n "$EXTRA" ]; then
    for verb in build validate; do
      mkdir -p "$cap/$verb-modeflag"
      if [ -f "$run/boris.json" ]; then
        ( cd "$run" && "$BORIS" "$verb" $EXTRA ) > "$cap/$verb-modeflag/stdout.txt" 2> "$cap/$verb-modeflag/stderr.txt"
      else
        "$BORIS" "$verb" $EXTRA --input "$run" > "$cap/$verb-modeflag/stdout.txt" 2> "$cap/$verb-modeflag/stderr.txt"
      fi
      echo $? > "$cap/$verb-modeflag/exit-code.txt"
    done
    if [ -d "$run/dist" ]; then
      ( cd "$run" && find dist -name '*.json' -type f ) | while read -r f; do
        mkdir -p "$cap/dist-modeflag/$(dirname "$f")"
        cp "$run/$f" "$cap/dist-modeflag/$f"
      done
    fi
  fi

  # harvest emitted machine artifacts from the build, if any
  if [ -d "$run/dist" ]; then
    ( cd "$run" && find dist -name '*.json' -type f ) | while read -r f; do
      mkdir -p "$cap/dist/$(dirname "$f")"
      cp "$run/$f" "$cap/dist/$f"
    done
  fi

  # watch session: NDJSON on stderr; touch a page to force one rebuild event
  mkdir -p "$cap/watch"
  ( cd "$run" && "$BORIS" watch --watch-json ) > "$cap/watch/stdout.txt" 2> "$cap/watch/session.ndjson" &
  WPID=$!
  sleep 2
  page=$(find "$run/content" -name '*.md' | head -1)
  [ -n "$page" ] && { sleep 1; printf '\n' >> "$page"; sleep 2; }
  kill $WPID 2>/dev/null; wait $WPID 2>/dev/null
  # keep corpus-src pristine: drop build artifacts the runs produced
  rm -rf "$run/dist" "$run/.boris-cache" "$run/content/../.boris-cache"
done

# manifest: sha256 of every captured artifact + engine version + commands
( cd "$FIX" && {
  echo 'schema_version = 1'
  # engine-version.txt is JSON — a TOML literal string keeps it valid
  echo "engine_version = '$(head -1 engine-version.txt)'"
  echo "capture_command = \"scripts/capture-fixtures.sh $CORPUS_SRC\""
  echo "captured = \"$(date -u +%Y-%m-%dT%H:%M:%SZ)\""
  echo ""
  find corpus-src captures -type f | sort | while read -r f; do
    h=$(shasum -a 256 "$f" | cut -d' ' -f1)
    echo "\"$f\" = \"$h\""
  done
} > MANIFEST.toml )
echo "done -> $FIX"
