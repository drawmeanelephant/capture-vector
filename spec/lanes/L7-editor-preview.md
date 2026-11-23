# L7 — Editor (`cv-ui-editor` + editor companion window)

> Worker brief. The app's hardest component — hence the wave-0 spike.
> v3 killed the HTML-in-rune preview and the companion preview window;
> the preview is the user's own browser on `watch --serve`.

## Owns

- `crates/cv-ui-editor/`, `crates/cv-app/src/companions/editor/`,
  `fixtures/editor/`, `fixtures/ui/golden/editor/`

## Depends on

- L0 + `Engine` trait + L2's multi-window scope decision.

## Deliverables

1. **Buffer model**: rope-backed (`ropey`), undo/redo, dirty tracking,
   fingerprinted staged save via `ContentWriter` (`*.tmp`+rename,
   Windows retry). Diagnostics mapped by **build-version tag** with the
   specified stale policy (dim or diff-remap); span units per the wave-0
   contract pin (byte/char/UTF-16 — or ropey panics).
2. **Structured floor first**: frontmatter as a form (closed schema →
   field editing is legitimate UI, not semantic reimplementation) +
   plain body editor. Rich Markdown (`TextEdit::rich` + decorations +
   wikilink completion fed by `completion.json`) layered on — the
   wave-0 probe decided feasibility.
3. **"Draft sketch" lane**: `rune_kit::Markdown` renders the buffer in
   the reading pane, explicitly labeled advisory. After save, the
   authoritative view is Boris's next build (and the site preview is
   the external browser on `watch --serve`).
4. **Companion editor window** bound to a buffer entity; N buffers;
   per-window `Selection` scope + dirty-close per wave-0 contract.
5. **Parity check** (`xtask`): diff `rune_kit::Markdown` rendering vs
   Boris-emitted bodies on fixture docs — measures real user-visible
   divergence (wikilinks, `{{include}}`, `<Aside>`, `<Details>`). NOT
   Oliver-vs-Boris (that proves nothing).
6. **Problems rail**: diagnostics for the open file → gutter marks +
   list, click→span; arriving-from-build diagnostics while typing are
   stale-tagged.
7. **Goldens**: scripted edit session (type → diagnostic clears →
   draft sketch updates), N-buffer windows, degraded states.

## Done when

- Fixture edit→broken-wikilink→`EREFERENCEMISSING` at right line via
  `StubEngine` — zero real binaries.
- Structured editor ships even if rich editor is flagged off.
- Parity check runs and reports divergence honestly.

## Must not

- Webview or in-app themed preview. Companion *preview* window (dead in
  v3). Parse Markdown for preview semantics. `OliverRender` in the
  typing loop. Save/build logic in the window layer.
