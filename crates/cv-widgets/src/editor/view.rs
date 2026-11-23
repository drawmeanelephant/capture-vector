//! Editor window view. Element keys are the golden/test contract.

use super::Closing;
use super::EditorWindow;
use crate::theme::{color, font};
use cv_model::theme::Theme;
use cv_ui_editor::frontmatter::{self, Note, SCHEMA};
use cv_ui_editor::problems::Freshness;
use cv_ui_editor::save::SaveStatus;
use cv_ui_editor::sketch;
use rune::{Cx, El, ScrollSpec, Span, Tx, col, row, text, text_input};

fn label(t: &Theme, s: impl Into<String>) -> El {
    text(s.into())
        .font(font(t.type_.label))
        .color(color(t.colors.text_dim))
}

fn button(t: &Theme, key: &str, s: &str, f: fn(&mut EditorWindow, &mut Tx)) -> El {
    row()
        .key(key)
        .padding(t.spacing.unit)
        .fill(color(t.colors.pane_alt))
        .child(
            text(s.to_string())
                .font(font(t.type_.label))
                .color(color(t.colors.text)),
        )
        .on_press_tx(move |s: &mut EditorWindow, tx: &mut Tx| f(s, tx))
}

fn save_text(st: &SaveStatus) -> String {
    match st {
        SaveStatus::Idle => String::new(),
        SaveStatus::Saving { .. } => "Saving…".into(),
        SaveStatus::Saved { .. } => "Saved".into(),
        SaveStatus::Conflict { .. } => "Not saved: the file changed on disk".into(),
        SaveStatus::Failed { message } => format!("Not saved: {message}"),
    }
}

/// Wikilink decorations over the body (display only — Boris resolves).
fn wikilink_spans(body: &str, t: &Theme) -> Vec<Span> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(open) = body[at..].find("[[") {
        let start = at + open;
        let Some(close) = body[start..].find("]]") else {
            break;
        };
        let end = start + close + 2;
        if !body[start..end].contains('\n') {
            out.push(Span {
                color: Some(color(t.colors.accent)),
                underline: Some(color(t.colors.accent)),
                ..Span::new(start, end)
            });
        }
        at = end;
    }
    out
}

fn header(s: &EditorWindow) -> El {
    let t = &s.theme;
    let path = s
        .session
        .borrow()
        .get(s.buffer)
        .map(|e| e.buffer.path().to_string())
        .unwrap_or_default();
    let mut r = row().key("editor-header").gap(t.spacing.unit * 2.0).child(
        text(path)
            .font(font(t.type_.title))
            .color(color(t.colors.text)),
    );
    if s.is_dirty() {
        r = r.child(label(t, "● unsaved").key("editor-dirty"));
    }
    let st = save_text(&s.save_status());
    if !st.is_empty() {
        r = r.child(label(t, st).key("editor-save-status"));
    }
    r.child(button(t, "editor-save", "Save", |s, _| s.save()))
        .child(button(t, "editor-undo", "Undo", |s, _| {
            s.undo();
        }))
        .child(button(t, "editor-redo", "Redo", |s, _| {
            s.redo();
        }))
}

fn close_prompt(s: &EditorWindow) -> Option<El> {
    let t = &s.theme;
    let msg = match s.closing {
        Closing::Open => return None,
        Closing::Confirm { stuck: false } => "This page has unsaved edits. Close it?",
        Closing::Confirm { stuck: true } => {
            "Cannot discard: the saved text is no longer in undo history. Save or keep editing."
        }
        Closing::SaveThenClose => "Saving, then closing…",
    };
    let mut r = row()
        .key("editor-close-prompt")
        .gap(t.spacing.unit * 2.0)
        .padding(t.spacing.unit)
        .fill(color(t.colors.pane_alt))
        .child(
            text(msg)
                .font(font(t.type_.body))
                .color(color(t.colors.text)),
        );
    if matches!(s.closing, Closing::Confirm { .. }) {
        r = r
            .child(button(t, "editor-close-save", "Save and close", |s, tx| {
                s.save_and_close(tx)
            }))
            .child(button(
                t,
                "editor-close-discard",
                "Discard and close",
                |s, tx| s.discard_and_close(tx),
            ))
            .child(button(t, "editor-close-cancel", "Keep editing", |s, _| {
                s.closing = Closing::Open
            }));
    }
    Some(r)
}

fn form(s: &EditorWindow) -> El {
    let t = &s.theme;
    let mut f = col().key("editor-form").gap(t.spacing.unit);
    for (i, (spec, edit)) in SCHEMA.iter().zip(&s.fields).enumerate() {
        let field = text_input(edit, font(t.type_.body))
            .key(format!("fm-{}", spec.key))
            .single_line()
            .grow(1.0)
            .color(color(t.colors.text))
            .on_edit(move |s: &mut EditorWindow| &mut s.fields[i])
            .on_input_change(move |s: &mut EditorWindow, _| s.field_changed(i));
        f = f.child(
            row()
                .gap(t.spacing.unit * 2.0)
                .child(label(t, spec.key).w(96.0))
                .child(field)
                .child(label(t, spec.hint)),
        );
    }
    let form = frontmatter::read(
        &s.session
            .borrow()
            .get(s.buffer)
            .map(|e| e.buffer.text())
            .unwrap_or_default(),
    );
    for (n, note) in form.notes.iter().enumerate() {
        let msg = match note {
            Note::UnknownKey { key, line } => format!(
                "line {line}: `{key}` is not a Boris frontmatter key — the next build judges"
            ),
            Note::Unparsed { line, text } => {
                format!("line {line}: not a `key: value` field: {text}")
            }
            Note::Unclosed => "frontmatter has no closing `---`".into(),
        };
        f = f.child(
            label(t, msg)
                .key(format!("fm-note-{n}"))
                .color(color(t.colors.warn)),
        );
    }
    f
}

fn body(s: &EditorWindow) -> El {
    let t = &s.theme;
    let mut gutter = col().key("editor-gutter").w(56.0).gap(t.spacing.unit * 0.5);
    let marks = {
        let ses = s.session.borrow();
        ses.get(s.buffer)
            .map(|e| e.problems.gutter(&e.buffer))
            .unwrap_or_default()
    };
    for m in marks {
        let c = if m.stale {
            t.colors.text_dim
        } else {
            t.colors.error
        };
        gutter = gutter.child(
            text(format!("● {}", m.line + 1))
                .key(format!("gutter-{}", m.line + 1))
                .font(font(t.type_.mono))
                .color(color(c))
                .opacity(if m.stale { 0.5 } else { 1.0 }),
        );
    }
    let mut field = text_input(&s.body, font(t.type_.mono))
        .key("editor-body")
        .grow(1.0)
        .min_h(240.0)
        .color(color(t.colors.text))
        .on_edit(|s: &mut EditorWindow| &mut s.body)
        .on_input_change(|s: &mut EditorWindow, _| s.body_changed())
        // Plain Return submits a rune field; in a document it breaks lines.
        .on_submit(|s: &mut EditorWindow, _| {
            let nl = if s.body.text.contains("\r\n") {
                "\r\n"
            } else {
                "\n"
            };
            s.body.insert(nl);
            s.body_changed();
        })
        .on_input_key(|s: &mut EditorWindow, ev, _| s.key(ev));
    if s.decorations {
        field = field.decorations(wikilink_spans(&s.body.text, t));
    }
    let mut c = col()
        .grow(1.0)
        .gap(t.spacing.unit)
        .child(row().gap(t.spacing.unit).child(gutter).child(field));
    let items = s.completions();
    if !items.is_empty() {
        let mut list = col()
            .key("completion")
            .gap(t.spacing.unit * 0.5)
            .fill(color(t.colors.pane_alt));
        for (n, (id, title)) in items.into_iter().enumerate() {
            list = list.child(
                row()
                    .key(format!("complete-{n}"))
                    .gap(t.spacing.unit * 2.0)
                    .child(
                        text(id)
                            .font(font(t.type_.mono))
                            .color(color(t.colors.text)),
                    )
                    .child(label(t, title))
                    .on_press(move |s: &mut EditorWindow| {
                        s.accept_completion(n);
                    }),
            );
        }
        c = c.child(list);
    }
    c
}

fn problems(s: &EditorWindow) -> El {
    let t = &s.theme;
    let rail = s.problems();
    let mut p = col().key("problems").gap(t.spacing.unit).child(
        text("Problems")
            .font(font(t.type_.label))
            .color(color(t.colors.text)),
    );
    if rail.is_empty() {
        return p.child(label(t, "No problems from the last build").key("problems-empty"));
    }
    let mut stale_header = false;
    for (n, pr) in rail.iter().enumerate() {
        let stale = pr.freshness == Freshness::Stale;
        if stale && !stale_header {
            stale_header = true;
            p = p.child(
                label(t, "Stale — from before your edits; rebuild to refresh")
                    .key("problems-stale"),
            );
        }
        let d = &pr.diagnostic;
        let at = pr.line.map(|l| format!(":{}", l + 1)).unwrap_or_default();
        let c = if stale {
            t.colors.text_dim
        } else {
            t.colors.error
        };
        p = p.child(
            col()
                .key(format!("problem-{n}"))
                .padding(t.spacing.unit)
                .fill(color(t.colors.pane))
                .opacity(if stale { 0.5 } else { 1.0 })
                .child(
                    text(format!("{}{at}", d.code))
                        .font(font(t.type_.mono))
                        .color(color(c)),
                )
                .child(
                    text(d.message.clone())
                        .font(font(t.type_.caption))
                        .color(color(t.colors.text_dim)),
                )
                .on_press(move |s: &mut EditorWindow| {
                    s.jump(n);
                }),
        );
    }
    p
}

fn draft_sketch(s: &EditorWindow) -> El {
    let t = &s.theme;
    let dark = t.mode == cv_model::theme::Mode::Dark;
    col()
        .key("draft-sketch")
        .grow(1.0)
        .min_h(0.0)
        .gap(t.spacing.unit)
        .child(
            text(sketch::LABEL)
                .key("sketch-label")
                .font(font(t.type_.label))
                .color(color(t.colors.warn)),
        )
        .child(label(t, sketch::DISCLAIMER).key("sketch-disclaimer"))
        .child(
            col()
                .key("sketch-body")
                .grow(1.0)
                .min_h(0.0)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .child(
                    rune_kit::Markdown::new(s.sketch.source.clone())
                        .build(&rune_kit::Kit::neutral(dark)),
                ),
        )
}

pub fn view(s: &EditorWindow, _cx: &mut Cx<EditorWindow>) -> El {
    let t = &s.theme;
    row()
        .key("editor")
        .w_full()
        .h_full()
        .fill(color(t.colors.bg))
        .padding(t.spacing.unit * 3.0)
        .gap(t.spacing.unit * 3.0)
        .child(
            col()
                .key("editor-main")
                .grow(1.0)
                .min_h(0.0)
                .gap(t.spacing.unit * 2.0)
                .child(header(s))
                .children(close_prompt(s))
                .child(form(s))
                .child(body(s)),
        )
        .child(
            col()
                .key("editor-side")
                .w(380.0)
                .h_full()
                .gap(t.spacing.unit * 3.0)
                .child(problems(s))
                .child(draft_sketch(s)),
        )
}
