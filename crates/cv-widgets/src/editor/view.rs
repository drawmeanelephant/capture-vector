//! Editor window view. Element keys are the golden/test contract.

use super::Closing;
use super::EditorWindow;
use crate::controls::{self, Variant};
use crate::shell::{chip, tone_token};
use crate::theme::{self, Level, Size, Styled, color, font, spring, tint};
use crate::{diagnostic, icons};
use cv_model::module::Tone;
use cv_model::theme::Theme;
use cv_ui_editor::frontmatter::{self, Note, SCHEMA};
use cv_ui_editor::problems::Freshness;
use cv_ui_editor::save::SaveStatus;
use cv_ui_editor::sketch;
use rune::{
    Align, Cx, El, ScrollSpec, Side, Span, Transition, col, el, row, spacer, text, text_input,
};
use rune_kit::{Dialog, Keybind, KeybindVariant, Kit, RollingText, Toolbar};

/// Most completion rows shown; Tab takes the first, a press any of them.
const COMPLETION_ROWS: usize = 8;

fn label(t: &Theme, s: impl Into<String>) -> El {
    text(s.into())
        .font(font(t.type_.label))
        .color(color(t.colors.text_dim))
}

/// Reduced motion shows and hides at once instead of stretching an
/// entrance into a fade.
fn enter(t: &Theme, tr: Transition) -> Transition {
    if t.motion.reduced {
        Transition::none()
    } else {
        tr
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

/// Saving rolls into Saved; a failed save is an error line that keeps its
/// whole message in a tooltip.
fn save_status(s: &EditorWindow, k: &Kit) -> Option<El> {
    let t = &s.theme;
    let failed = |msg: String| {
        text(msg.clone())
            .key("editor-save-status")
            .font(font(t.type_.label))
            .color(color(t.colors.error))
            .single_line()
            .shrink()
            .min_w(0.0)
            .truncate()
            .help_when_truncated(msg)
    };
    let rolling = |msg: &str| {
        // Per window: a rolling label remembers its last text by key.
        row().key("editor-save-status").child(
            RollingText::new(format!("save-status-{}", s.id.0), msg)
                .font(font(t.type_.label))
                .color(color(t.colors.text_dim))
                .build(k),
        )
    };
    match s.save_status() {
        SaveStatus::Idle => None,
        SaveStatus::Saving { .. } => Some(rolling("Saving…")),
        SaveStatus::Saved { .. } => Some(rolling("Saved")),
        SaveStatus::Conflict { .. } => Some(failed("Not saved: the file changed on disk".into())),
        SaveStatus::Failed { message } => Some(failed(format!("Not saved: {message}"))),
    }
}

fn toolbar(s: &EditorWindow, k: &Kit) -> El {
    let t = &s.theme;
    let path = s
        .session
        .borrow()
        .get(s.buffer)
        .map(|e| e.buffer.path().to_string())
        .unwrap_or_default();
    let mut lead = row()
        .key("editor-doc")
        .gap(t.spacing.unit * 2.0)
        .items_center()
        .shrink()
        .min_w(0.0)
        .child(
            text(path.clone())
                .key("editor-path")
                .font(font(t.type_.title))
                .color(color(t.colors.text))
                .single_line()
                .shrink()
                .min_w(0.0)
                .truncate_middle()
                .help_when_truncated(path),
        );
    if let Some(e) = s.error() {
        let msg = e.to_string();
        lead = lead.child(
            text(msg.clone())
                .key("editor-error")
                .font(font(t.type_.label))
                .color(color(t.colors.error))
                .single_line()
                .shrink()
                .min_w(0.0)
                .truncate()
                .help_when_truncated(msg),
        );
    }
    if s.is_dirty() {
        lead = lead.child(
            chip(t, "editor-dirty", "Unsaved", Tone::Warn)
                .h(18.0)
                .role("status")
                .label("Unsaved edits")
                .transition(enter(t, Transition::scale(0.8)))
                .motion(spring(t.motion.pop)),
        );
    }
    lead = lead.children(save_status(s, k));
    Toolbar::new()
        .leading(lead)
        .trailing(controls::icon_button_tx(
            t,
            "editor-undo",
            &icons::UNDO_2,
            "Undo",
            Some("mod+z"),
            s.can_undo(),
            |s: &mut EditorWindow, _| {
                s.undo();
            },
        ))
        .trailing(controls::icon_button_tx(
            t,
            "editor-redo",
            &icons::REDO_2,
            "Redo",
            Some("mod+shift+z"),
            s.can_redo(),
            |s: &mut EditorWindow, _| {
                s.redo();
            },
        ))
        .trailing(el().w(t.spacing.unit).a11y_hidden())
        .trailing(controls::button_keys_tx(
            t,
            "editor-save",
            "Save",
            Variant::Primary,
            "mod+s",
            s.is_dirty(),
            |s: &mut EditorWindow, _| s.save(),
        ))
        .build(k)
        .key("editor-header")
        .fill(color(t.colors.pane))
        .border_sides([0.0, 0.0, 0.5, 0.0], color(t.colors.border))
}

/// Closing over unsaved edits: a modal dialog. Escape and its close
/// button keep editing; Return saves and closes.
fn close_dialog(s: &EditorWindow, k: &Kit, layer: &mut Vec<El>) {
    let t = &s.theme;
    let (title, msg) = match s.closing {
        Closing::Open => return,
        Closing::Confirm { stuck: false } => (
            "Close with unsaved edits?",
            "This page has edits that are not saved to disk yet.",
        ),
        Closing::Confirm { stuck: true } => (
            "These edits cannot be discarded",
            "The saved text is no longer in undo history. Save, or keep editing.",
        ),
        Closing::SaveThenClose => ("Saving…", "The window closes once the save lands."),
    };
    let mut actions = row()
        .key("editor-close-prompt")
        .w_full()
        .px(16.0)
        .pb(16.0)
        .gap(t.spacing.unit * 2.0)
        .items_center();
    if let Closing::Confirm { stuck } = s.closing {
        actions = actions
            .child(controls::button_tx(
                t,
                "editor-close-discard",
                "Discard and close",
                Variant::Danger,
                !stuck,
                |s: &mut EditorWindow, tx| s.discard_and_close(tx),
            ))
            .child(spacer())
            .child(controls::button_tx(
                t,
                "editor-close-cancel",
                "Keep editing",
                Variant::Ghost,
                true,
                |s: &mut EditorWindow, _| s.closing = Closing::Open,
            ))
            .child(
                controls::button_tx(
                    t,
                    "editor-close-save",
                    "Save and close",
                    Variant::Primary,
                    true,
                    |s: &mut EditorWindow, tx| s.save_and_close(tx),
                )
                .default_button(),
            );
    }
    Dialog::new("editor-close", title)
        .description(msg)
        .open(true)
        .fit()
        .width(440.0)
        .click_outside(false)
        .on_close(|s: &mut EditorWindow| s.closing = Closing::Open)
        .child(actions)
        .build(k, layer);
}

/// A text field's frame: an inset well with a hairline edge, ringed in
/// the focus colour while its field has focus.
fn field_frame(t: &Theme, key: String, field: El) -> El {
    row()
        .key(key)
        .grow(1.0)
        .min_w(0.0)
        .h(t.spacing.row_h)
        .px(t.spacing.unit * 2.0)
        .items_center()
        .fill(tint(t.colors.text, 0x08))
        .border(0.5, color(t.colors.border))
        .rounded(t, Size::Sm)
        .focus_within_ring(2.0, 0.0, color(t.states.focus_ring))
        .child(field)
}

fn note_chip(t: &Theme, key: String, msg: String) -> El {
    let fg = tone_token(t, Tone::Warn);
    row()
        .key(key)
        .role("note")
        .label(msg.clone())
        .shrink()
        .min_w(0.0)
        .px(t.spacing.unit * 2.0)
        .py(t.spacing.unit * 0.5)
        .items_center()
        .fill(tint(fg, 0x1f))
        .border(0.5, tint(fg, 0x4d))
        .rounded(t, Size::Sm)
        .child(
            text(msg.clone())
                .font(font(t.type_.label))
                .color(color(fg))
                .single_line()
                .shrink()
                .min_w(0.0)
                .truncate()
                .help_when_truncated(msg)
                .a11y_hidden(),
        )
}

fn form(s: &EditorWindow) -> El {
    let t = &s.theme;
    let mut f = col().key("editor-form").gap(t.spacing.unit);
    for (i, (spec, edit)) in SCHEMA.iter().zip(&s.fields).enumerate() {
        let caption_key = format!("fm-label-{}", spec.key);
        let field = text_input(edit, font(t.type_.body))
            .key(format!("fm-{}", spec.key))
            .labelled_by(caption_key.clone())
            .single_line()
            .grow(1.0)
            .color(color(t.colors.text))
            .on_edit(move |s: &mut EditorWindow| &mut s.fields[i])
            .on_input_change(move |s: &mut EditorWindow, _| s.field_changed(i));
        f = f.child(
            row()
                .gap(t.spacing.unit * 2.0)
                .items_center()
                .child(label(t, spec.key).key(caption_key).w(96.0))
                .child(field_frame(t, format!("fm-frame-{}", spec.key), field))
                .child(
                    text(spec.hint)
                        .font(font(t.type_.caption))
                        .color(color(t.colors.text_dim))
                        .single_line()
                        .w(220.0)
                        .truncate()
                        .help_when_truncated(spec.hint),
                ),
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
            row()
                .w_full()
                .child(note_chip(t, format!("fm-note-{n}"), msg)),
        );
    }
    f
}

/// A gutter mark: a dot in the error colour, an outline once stale.
fn gutter_mark(t: &Theme, line: usize, stale: bool) -> El {
    let dot = el().w(7.0).h(7.0).radius(3.5).a11y_hidden();
    let dot = if stale {
        dot.border(1.0, color(t.colors.text_dim))
    } else {
        dot.fill(color(t.colors.error))
    };
    let name = if stale {
        format!("Stale problem on line {line}")
    } else {
        format!("Problem on line {line}")
    };
    row()
        .key(format!("gutter-{line}"))
        .role("note")
        .label(name)
        .gap(t.spacing.unit * 1.5)
        .items_center()
        .child(dot)
        .child(
            text(line.to_string())
                .font(font(t.type_.mono))
                .color(color(if stale {
                    t.colors.text_dim
                } else {
                    t.colors.text
                }))
                .a11y_hidden(),
        )
}

fn body(s: &EditorWindow) -> El {
    let t = &s.theme;
    let marks = {
        let ses = s.session.borrow();
        ses.get(s.buffer)
            .map(|e| e.problems.gutter(&e.buffer))
            .unwrap_or_default()
    };
    let gutter = col()
        .key("editor-gutter")
        .w(48.0)
        .pt(t.spacing.unit * 2.0)
        .gap(t.spacing.unit)
        .children(
            marks
                .into_iter()
                .map(|m| gutter_mark(t, m.line + 1, m.stale)),
        );
    let mut field = text_input(&s.body, font(t.type_.mono))
        .key("editor-body")
        .label_static("Markdown body")
        .grow(1.0)
        .h_full()
        .color(color(t.colors.text))
        .on_edit(|s: &mut EditorWindow| &mut s.body)
        .on_input_change(|s: &mut EditorWindow, _| s.body_changed())
        // Plain Return submits a rune field; in a document it breaks lines.
        .on_submit(|s: &mut EditorWindow, _| {
            let nl = if s.text().contains("\r\n") {
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
    let frame = row()
        .key("editor-body-frame")
        .grow(1.0)
        .min_w(0.0)
        .h_full()
        .padding(t.spacing.unit * 2.0)
        .fill(color(t.colors.pane))
        .border(0.5, color(t.colors.border))
        .rounded(t, Size::Md)
        .focus_within_ring(2.0, 0.0, color(t.states.focus_ring))
        .child(field);
    row()
        .key("editor-text")
        .grow(1.0)
        .min_h(240.0)
        .gap(t.spacing.unit)
        .child(gutter)
        .child(frame)
}

/// Wikilink completions, anchored under the caret (or under the body
/// before the caret has been drawn). The first row is what Tab accepts.
/// The popup takes no focus, so typing stays in the body.
fn completion(s: &EditorWindow, cx: &mut Cx<EditorWindow>, layer: &mut Vec<El>) {
    let items = s.completions();
    if items.is_empty() {
        return;
    }
    let t = &s.theme;
    let anchor = match cx.caret("editor-body") {
        Some([x, y, w, h]) => {
            layer.push(
                el().key("completion-anchor")
                    .absolute(x, y)
                    .w(w.max(1.0))
                    .h(h)
                    .a11y_hidden(),
            );
            "completion-anchor"
        }
        None => "editor-body",
    };
    let tab = crate::keys::keycaps("tab").unwrap_or_else(|| vec!["Tab".into()]);
    let rows = items
        .into_iter()
        .take(COMPLETION_ROWS)
        .enumerate()
        .map(|(n, (id, title))| {
            let first = n == 0;
            let mut r = row()
                .key(format!("complete-{n}"))
                .role("option")
                .focus_selected(first)
                .label(id.clone())
                .w_full()
                .h(t.spacing.row_h)
                .px(t.spacing.unit * 2.0)
                .gap(t.spacing.unit * 2.0)
                .items_center()
                .rounded(t, Size::Sm)
                .on_press(move |s: &mut EditorWindow| {
                    s.accept_completion(n);
                })
                .child(
                    text(id)
                        .font(font(t.type_.mono))
                        .color(color(t.colors.text))
                        .single_line(),
                )
                .child(
                    text(title)
                        .font(font(t.type_.label))
                        .color(color(t.colors.text_dim))
                        .single_line()
                        .shrink()
                        .min_w(0.0)
                        .truncate(),
                )
                .child(spacer());
            if first {
                r = r.fill(color(t.colors.selection)).child(
                    Keybind::new(&tab)
                        .variant(KeybindVariant::Neutral)
                        .build(&theme::kit(t)),
                );
            } else {
                r = r.states(t);
            }
            r
        });
    layer.push(
        col()
            .key("completion")
            .role("listbox")
            .label("Wikilink completions")
            .anchored(anchor, Side::Bottom, Align::Start, 4.0)
            .w(360.0)
            .padding(t.spacing.unit)
            .gap(1.0)
            .fill(color(t.colors.pane))
            .border(0.5, color(t.colors.border))
            .rounded(t, Size::Md)
            .elevated(t, Level::Floating)
            .preserve_focus_on_press(true)
            .transition(enter(t, Transition::opacity()))
            .motion(spring(t.motion.snap))
            .children(rows),
    );
}

fn problems(s: &EditorWindow) -> El {
    let t = &s.theme;
    let rail = s.problems();
    let mut p = col().key("problems").gap(t.spacing.unit * 2.0).child(
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
        let at = pr
            .line
            .map(|l| format!(" on line {}", l + 1))
            .unwrap_or_default();
        let card = diagnostic::card(t, d, n)
            .key(format!("problem-{n}"))
            .role("button")
            .label(format!("{}{at}", d.code))
            .on_press(move |s: &mut EditorWindow| {
                s.jump(n);
            });
        // A stale card has no span to jump to: it recedes and stays still.
        p = p.child(if stale {
            card.opacity(0.6)
        } else {
            card.states(t).press_scale(controls::PRESS_SCALE)
        });
    }
    p
}

fn draft_sketch(s: &EditorWindow) -> El {
    let t = &s.theme;
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
        .child(label(t, sketch::DISCLAIMER).key("sketch-disclaimer").wrap())
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
                        .render(&crate::theme::markdown(t)),
                ),
        )
}

pub fn view(s: &EditorWindow, cx: &mut Cx<EditorWindow>) -> El {
    let t = &s.theme;
    theme::motion_preference(cx, t);
    let k = theme::kit(t);
    let mut layer = Vec::new();
    completion(s, cx, &mut layer);
    close_dialog(s, &k, &mut layer);
    let base = col()
        .key("editor")
        .w_full()
        .h_full()
        .fill(color(t.colors.bg))
        .child(toolbar(s, &k))
        .child(
            row()
                .key("editor-content")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .padding(t.spacing.unit * 3.0)
                .gap(t.spacing.unit * 3.0)
                .child(
                    col()
                        .key("editor-main")
                        .grow(1.0)
                        .min_w(0.0)
                        .min_h(0.0)
                        .gap(t.spacing.unit * 3.0)
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
                ),
        );
    rune_kit::root(base, layer)
}
