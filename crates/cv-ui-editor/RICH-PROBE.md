# Rich-layer feasibility probe (L7)

Verdict: **rich layer ships as decorations + wikilink completion over the
plain body field. `TextEdit::rich` is not used.** The structured floor
(frontmatter form + plain body editor) does not depend on this verdict.

Probed against vendored rune `2a59b0f`, headless, in
`crates/cv-widgets/tests/editor.rs`.

| Capability | Verdict | Evidence |
|---|---|---|
| Plain `text_input` + `on_edit` as the body editor | Works | typing/paste fold into the ropey buffer; undo/redo, dirty, save |
| IME (`Ime::Preedit` then `Ime::Commit`) | Works headless; native US dead keys pass on macOS | `ime_composition_commits_into_the_buffer`; `spike/macos-probe/run.py` checks real macOS preedit and UTF-8 commit against an AppKit control |
| `El::decorations` byte spans (wikilink accent + underline) | Builds and lays out; macOS paint observed | The native probe's initial screenshot shows accented, underlined happy-corpus wikilinks. No cross-OS decoration paint gate exists |
| Wikilink completion from `.boris/completion.json` | Works | `wikilink_completion_comes_from_captured_completion_json` uses a real `boris/0.9.0` capture; no index → no guesses |
| `TextEdit::rich` (marks + HTML form) | Rejected | it models styled runs and serializes HTML; authoring Boris needs the literal Markdown/`[[…]]`/`{{include}}`/`<Aside>` source, and an HTML form would be an in-app HTML path |

Unproven, stated plainly:

- Decoration paint parity on Windows and Linux, and IME candidate-popup
  placement on every OS, remain unproven.
- A separate macOS Japanese conversion was observed, but broader automatic
  replay did not pass reliably. See `spike/FINDINGS.md` and the manual checklist
  in `spike/macos-probe/README.md`.
- Decorations do not move with a live IME preedit; rune draws the preedit
  at the caret and the spans are recomputed from the committed text only.
- Stale diagnostics are never drawn as decorations (their spans are not
  remapped); only the gutter/rail show them, dimmed.

Gate: `cv_widgets::editor::DECORATIONS` (on). Turning it off leaves the
floor intact.
