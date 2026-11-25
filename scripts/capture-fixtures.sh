#!/usr/bin/env bash
# capture-fixtures.sh — capture REAL Boris outputs into fixtures/.
# Spec rule: hand-authored Boris JSON is banned. Everything in fixtures/captures/
# comes from running the pinned binary over fixtures/corpus-src/.
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
source "$ROOT/scripts/fixture-sha256.sh"
BORIS="${CV_BORIS_BIN:-boris}"
CORPUS_SRC="${1:?usage: capture-fixtures.sh <corpora-dir> — e.g. a solipsist Stunts/ or Tests/Fixtures checkout}"
FIX="$ROOT/fixtures"
# captures/lab/ belongs to capture-lab-fixtures.sh: keep it and its table.
# Corpora with a boris.json build as projects; the rest pass their root
# via --input. content-only is a bare content/ tree with no profile or
# theme, whose first build fails ELAYOUT.
ALL_CORPORA="happy happy-textile cook-one dogfood broken-duplicate-id broken-frontmatter broken-parent broken-textile broken-wikilink content-only"
# `init` is not a corpus: it captures `boris init` itself (see below).
# CV_CAPTURE_ONLY="<name> …" recaptures only those and keeps every other
# capture byte-for-byte; the manifest still pins every file.
CAPTURE="${CV_CAPTURE_ONLY:-$ALL_CORPORA init}"
mkdir -p "$FIX/corpus-src" "$FIX/captures"
if [ -n "${CV_CAPTURE_ONLY:-}" ]; then
  for c in $CAPTURE; do
    case " $ALL_CORPORA init " in
      *" $c "*) rm -rf "$FIX/corpus-src/$c" "$FIX/captures/$c" ;;
      *) echo "capture-fixtures: unknown capture '$c'" >&2; exit 2 ;;
    esac
  done
else
  rm -rf "$FIX/corpus-src"
  mkdir -p "$FIX/corpus-src"
  find "$FIX/captures" -mindepth 1 -maxdepth 1 ! -name lab -exec rm -rf {} +
fi

# one line of build-info JSON (version + vcs revision) is the pinned identity
"$BORIS" --build-info > "$FIX/engine-version.txt" 2>&1 || "$BORIS" --version > "$FIX/engine-version.txt" 2>&1
for c in $CAPTURE; do
  [ "$c" = init ] && continue
  src="$CORPUS_SRC/$c"
  [ -d "$src" ] || { echo "skip $c (no corpus at $src)"; continue; }
  echo "== $c =="
  mkdir -p "$FIX/corpus-src/$c"
  cp -R "$src/." "$FIX/corpus-src/$c/"
  run="$FIX/corpus-src/$c"
  cap="$FIX/captures/$c"; mkdir -p "$cap"

  # project roots (boris.json, or a content/ tree Boris defaults to) build
  # from the root as an author's folder would; bare page roots (the broken
  # stunts) are passed via --input
  if [ -f "$run/boris.json" ] || [ -d "$run/content" ]; then
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

  # harvest emitted machine artifacts from the build, if any
  if [ -d "$run/dist" ]; then
    ( cd "$run" && find dist -name '*.json' -type f ) | while read -r f; do
      mkdir -p "$cap/dist/$(dirname "$f")"
      cp "$run/$f" "$cap/dist/$f"
    done
    # ...plus every artifact the build's own index commits (pages, assets):
    # the audit lane evaluates rendered HTML, and the index's sha256 pins it
    idx="$run/dist/_boris/proof/artifacts.json"
    [ -f "$idx" ] && sed -n 's/^ *"path": "\(.*\)",$/\1/p' "$idx" | while read -r f; do
      [ -f "$run/dist/$f" ] || continue
      mkdir -p "$cap/dist/dist/$(dirname "$f")"
      cp "$run/dist/$f" "$cap/dist/dist/$f"
    done
  fi

  # the mode pass starts from a clean output tree so dist/ and
  # dist-modeflag/ each hold only their own pass's artifacts
  rm -rf "$run/dist" "$run/.boris-cache"

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

  # watch session: NDJSON on stderr; touch a page to force one rebuild event
  mkdir -p "$cap/watch"
  # exec so the kill below reaches boris itself (it then emits watch-stopped)
  # instead of orphaning it behind a subshell
  ( cd "$run" && exec "$BORIS" watch --watch-json ) > "$cap/watch/stdout.txt" 2> "$cap/watch/session.ndjson" &
  WPID=$!
  sleep 2
  page=$(find "$run/content" -name '*.md' | head -1)
  [ -n "$page" ] && { sleep 1; printf '\n' >> "$page"; sleep 2; }
  kill $WPID 2>/dev/null; wait $WPID 2>/dev/null
  [ -n "$page" ] && cp "$src/${page#"$run"/}" "$page"
  # keep corpus-src pristine: drop build artifacts the runs produced
  rm -rf "$run/dist" "$run/.boris-cache" "$run/content/../.boris-cache"
done

# boris init: the archetype list (`init --help`), each INIT_TYPES starter
# with the tree it writes, and the refusal over a target that is not empty.
INIT_TYPES="docs"
case " $CAPTURE " in *" init "*)
  echo "== init =="
  cap="$FIX/captures/init"
  mkdir -p "$cap/help"
  "$BORIS" init --help > "$cap/help/stdout.txt" 2> "$cap/help/stderr.txt"
  echo $? > "$cap/help/exit-code.txt"
  scratch=$(mktemp -d)
  for t in $INIT_TYPES; do
    mkdir -p "$cap/$t/tree" "$scratch/$t"
    ( cd "$scratch/$t" && "$BORIS" init site --type "$t" ) > "$cap/$t/stdout.txt" 2> "$cap/$t/stderr.txt"
    echo $? > "$cap/$t/exit-code.txt"
    [ -d "$scratch/$t/site" ] && cp -R "$scratch/$t/site/." "$cap/$t/tree/"
  done
  first=${INIT_TYPES%% *}
  mkdir -p "$cap/refuse"
  ( cd "$scratch/$first" && "$BORIS" init site --type "$first" ) > "$cap/refuse/stdout.txt" 2> "$cap/refuse/stderr.txt"
  echo $? > "$cap/refuse/exit-code.txt"
  rm -rf "$scratch"
  ;;
esac

# manifest: sha256 of every captured artifact + engine version + commands
# Quote one Bash argument for replay, then escape it for a TOML basic string.
printf -v capture_command 'scripts/capture-fixtures.sh %q' "$CORPUS_SRC"
[ -n "${CV_CAPTURE_ONLY:-}" ] && printf -v capture_command 'CV_CAPTURE_ONLY=%q %s' "$CV_CAPTURE_ONLY" "$capture_command"
capture_command=${capture_command//\\/\\\\}
capture_command=${capture_command//\"/\\\"}
# Carry the trailing tables forward verbatim: the [migration_lab] tool pin
# and capture-lab-fixtures.sh's [lab_captures]. Read them before the
# redirect below truncates the manifest.
lab_pin=$(sed -n '/^\[migration_lab\]$/,$p' "$FIX/MANIFEST.toml" 2>/dev/null || true)
( cd "$FIX" && {
  echo 'schema_version = 1'
  # engine-version.txt is JSON — a TOML literal string keeps it valid
  echo "engine_version = '$(head -1 engine-version.txt)'"
  printf 'capture_command = "%s"\n' "$capture_command"
  echo "captured = \"$(date -u +%Y-%m-%dT%H:%M:%SZ)\""
  echo ""
  find corpus-src captures -path captures/lab -prune -o -type f -print | sort | while IFS= read -r f; do
    h=$(fixture_sha256 "$f") || exit 1
    echo "\"$f\" = \"$h\""
  done || exit 1
  [ -z "$lab_pin" ] || printf '\n%s\n' "$lab_pin"
} > MANIFEST.toml ) || exit 1
echo "done -> $FIX"
