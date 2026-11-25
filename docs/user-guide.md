# User guide

Capture Vector is a pre-release native desktop workbench for Boris
content. It is local-first: workspaces are ordinary directories, Boris is
the compiler, and Capture Vector is the operator interface over the graph,
diagnostics, research, audit, and publishing evidence.

This guide describes what the current build does. Aspirational product
behavior belongs in `spec/`, not here.

## Install and launch

There is no signed installer yet. Run from a source checkout:

```sh
./scripts/fetch-rune.sh
cargo build --workspace
cargo run -p cv-app -- --project /path/to/workspace
```

For a safe first run, use the demo workspace:

```sh
./scripts/demo.sh
```

The demo copies `fixtures/corpus-src/happy` to a temporary directory, so
edits do not touch the repository fixture.

A full release layout can be built locally after fetching the pinned
engine; see `docs/PACKAGING.md`. Release packaging embeds Boris 0.9.0 and
does not use whatever `boris` happens to be on `PATH`.

## Workspaces and the engine

A workspace is a Boris project directory. Launch resolution is:

1. `--project <dir>`
2. `CV_PROJECT=<dir>`
3. the most recent workspace
4. the fixture demo fallback

Press `mod+shift+O` to open a different folder. On macOS `mod` is Command;
on Windows/Linux it is the platform primary modifier.

Capture Vector can browse a workspace without Boris. The status line then
discloses `engine absent`; builds, watch rebuilds, and publish operations
need the pinned engine. Discovery order is `CV_BORIS_BIN`, bundled engine,
`PATH`, then an explicitly opted-in sibling checkout. Version mismatch is
a hard disclosure, not a warning to patch around.

## The shell

The shell follows an Outlook-style arrangement:

- module rail on the left;
- mailbox/navigation column;
- item list;
- reading pane;
- inspector on the right.

Current modules include Pages, Outputs, Audit, Research, and Activity.
The fixture demo (no workspace open, running from a checkout) adds a Demo
module whose `Simulate: …` palette verbs fake degraded states for the
headless verification harness. Real workspaces do not show it; set
`CV_DEBUG=1` to keep it there for debugging. Pages and outputs
come from the Boris project snapshot. Research and audit state live in the
workspace's `.cv/` sidecar, never in Boris frontmatter.

Keyboard basics:

| Key | Action |
|---|---|
| `mod+K` | command palette |
| `mod+1` … `mod+9` | switch module |
| `Tab` / `shift+Tab` | cycle mailboxes |
| `Up`/`Down` or `k`/`j` | move in the list |
| `Enter` | open the selected noun in a companion window |
| `mod+O` | follow the selected linked noun |
| `mod+shift+O` | open workspace |
| `mod+I` | toggle inspector |
| `Esc` | clear selection / close palette |

While a dialog is open (a confirmation, the capture prompt, the filter
form, the recent-workspace chooser), it takes every key; the list behind
it does not move. Only one dialog shows at a time; the next one opens
when it closes.

| Key | Action in a dialog |
|---|---|
| `Enter` | run the highlighted button (Capture, Archive, Open, …) |
| `Esc` | cancel; clicking outside or the close button does the same |
| `Tab` / `shift+Tab` | next / previous field in a form |
| `Space` or `Down` / `Up` | next / previous option of a choice field |
| `Space` | flip a switch field |
| `Up`/`Down` | move in a chooser's list; typing filters it |

A secret dialog (an app password, a signing key) shows only dots. What
you type goes only to the app's secret store; it is never written to the
workspace, the log, or the element tree.

The command palette is the reliable inventory of module-specific verbs.
If a mouse action and the palette disagree, the palette/command behavior
is the contract to test against.

## Editing

Open a page in a companion window with `Enter`. Editing is plain source
editing, not WYSIWYG HTML editing.

- `mod+S` saves inside the workspace.
- `mod+Z` / `shift+mod+Z` (or `mod+Y`) undo/redo.
- `Tab` accepts the first visible completion when completions are open.
- Closing a dirty companion asks before discarding.

The save path is contained to the workspace. If the file changed on disk
after the buffer loaded, Capture Vector reports a fingerprint conflict
instead of silently overwriting it.

## Research, audit, and evidence

Research notes live under `.cv/research/`; JSONL records are the source
of truth and the SQLite index is rebuildable. Audit lifecycle state lives
under `.cv/audit/`. These namespaces carry independent schema stamps, so
one sidecar namespace can migrate without rewriting the others.

Boris output and diagnostics are evidence. Do not treat a disclosed
degraded state as a crash:

- `engine absent` means no pinned engine was found;
- `version mismatch` means the first discovered engine was the wrong one;
- `watch dead` means the watch subprocess ended and the UI is disclosing
  that fact.

The runbook has the diagnostic flow for each: `docs/runbooks/desktop-app.md`.

## Logging and support bundles

Structured logs are JSONL:

```sh
CV_LOG=debug CV_LOG_FILE=/tmp/cv.log cargo run -p cv-app -- --project /path/to/workspace
```

Keys that look secret (`token`, `password`, `secret`, `nsec`, and similar)
are dropped by the logger, but page bodies and local paths can still be
sensitive. Read logs before attaching them to an issue.

For UI problems, include the headless element tree:

```sh
cargo run -p cv-app -- --scene canary --tree target/qa.tree.json
```

Element-tree JSON is the cross-platform evidence format. PNG goldens are
only canonical on the Linux Lavapipe configuration.

## Platform status

- macOS is the primary development platform.
- Linux has real-window evidence; CJK/font fallback and GPU behavior still
  have open edges.
- Windows is explicitly unproven unless a linked issue/test says
  otherwise.

Do not infer support from "it compiles." `spike/FINDINGS.md` is the
capability evidence ledger.

## Troubleshooting

Start with `docs/runbooks/desktop-app.md`. The short version:

```sh
rustc --version
git status --short
./scripts/fetch-rune.sh
cargo test --workspace --locked
```

Then file the bug template with the exact command, revision, platform,
structured logs, and tree JSON for UI behavior.
