//! Editor window view. Element keys are the golden/test contract.

use super::Closing;
use super::EditorWindow;
use crate::controls::{self, Variant};
use crate::diagnostic;
use crate::dialog::{self, DialogHost};
use crate::icons;
use crate::shell::chip;
use crate::theme::{self, Level, Size, Styled, color, font};
use cv_model::dialog::{ActionTone, DialogKind, DialogSpec, OpenDialog};
use cv_model::module::Tone;
use cv_model::modules::activity::severity_tone;
use cv_model::theme::Theme;
use cv_ui_editor::frontmatter::{self, Note, SCHEMA};
use cv_ui_editor::problems::Freshness;
use cv_ui_editor::save::SaveStatus;
use cv_ui_editor::sketch;
use rune::{
    Align, Cx, El, ScrollSpec, Side, Span, Transition, Tx, col, el, row, spacer, text, text_input,
};
use rune_kit::{Keybind, RollingText, Toolbar};

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

/// "Saving…" rolls into "Saved"; a failed save says why, in the error tone.
fn save_status(s: &EditorWindow) -> Option<El> {
    let t = &s.theme;
    let (msg, tone) = match s.save_status() {
        SaveStatus::Idle => return None,
        SaveStatus::Saving { .. } => ("Saving…".to_string(), t.colors.text_dim),
        SaveStatus::Saved { .. } => ("Saved".to_string(), t.colors.text_dim),
        SaveStatus::Conflict { .. } => (
            "Not saved: the file changed on disk".to_string(),
            t.colors.error,
        ),
        SaveStatus::Failed { message } => (format!("Not saved: {message}"), t.colors.error),
    };
    let shown = if tone == t.colors.error {
        label(t, msg.clone())
            .color(color(tone))
            .single_line()
            .truncate()
            .max_w(360.0)
            .help_when_truncated(msg)
    } else {
        // One roll memory per window: windows share the UI thread.
        RollingText::new(format!("editor-{}-save", s.id.0), msg)
            .font(font(t.type_.label))
            .color(color(tone))
            .build(&theme::kit(t))
    };
    Some(row().key("editor-save-status").items_center().child(shown))
}

fn header(s: &EditorWindow, lights: f32) -> El {
    let t = &s.theme;
    let path = s
        .session
        .borrow()
        .get(s.buffer)
        .map(|e| e.buffer.path().to_string())
        .unwrap_or_default();
    let mut bar = Toolbar::new();
    if lights > 0.0 {
        bar = bar.leading(el().key("editor-lights").w(lights).a11y_hidden());
    }
    bar = bar.leading(
        text(path.clone())
            .key("editor-path")
            .font(font(t.type_.title))
            .color(color(t.colors.text))
            .single_line()
            .truncate_middle()
            .help_when_truncated(path)
            .shrink()
            .min_w(0.0),
    );
    if s.is_dirty() {
        bar = bar.leading(
            chip(t, "editor-dirty", "Unsaved", Tone::Warn)
                .label("Unsaved edits")
                .transition(enter(t, Transition::scale(0.8)))
                .motion(theme::spring(t.motion.snap)),
        );
    }
    if let Some(st) = save_status(s) {
        bar = bar.trailing(st);
    }
    bar.trailing(controls::icon_command(
        t,
        "editor-undo",
        &icons::UNDO_2,
        "Undo",
        "mod+z",
        s.can_undo(),
        |s: &mut EditorWindow| {
            s.undo();
        },
    ))
    .trailing(controls::icon_command(
        t,
        "editor-redo",
        &icons::REDO_2,
        "Redo",
        "mod+shift+z",
        s.can_redo(),
        |s: &mut EditorWindow| {
            s.redo();
        },
    ))
    .trailing(controls::command_button(
        t,
        "editor-save",
        "Save",
        Variant::Primary,
        "mod+s",
        s.is_dirty(),
        |s: &mut EditorWindow, _| s.save(),
    ))
    .build(&theme::kit(t))
    .key("editor-header")
    .border_sides([0.0, 0.0, 0.5, 0.0], color(t.colors.border))
}

/// Why the window has no page, under the toolbar where it is read in full.
fn error_banner(s: &EditorWindow) -> Option<El> {
    let t = &s.theme;
    let e = s.error()?;
    Some(
        row()
            .key("editor-error-banner")
            .role("alert")
            .w_full()
            .px(t.spacing.unit * 3.0)
            .py(t.spacing.unit * 1.5)
            .fill(theme::tint(t.colors.error, 0x1a))
            .border_sides([0.0, 0.0, 0.5, 0.0], theme::tint(t.colors.error, 0x4d))
            .child(
                text(e.to_string())
                    .key("editor-error")
                    .font(font(t.type_.body))
                    .color(color(t.colors.text))
                    .wrap(),
            ),
    )
}

/// The unsaved-edits question on the shared dialog. Escape, the close
/// button and a press outside all keep editing.
fn close_dialog(s: &EditorWindow, layer: &mut Vec<El>) {
    let (title, msg) = match s.closing {
        Closing::Open => return,
        Closing::Confirm { stuck: false } => (
            "Close with unsaved edits?",
            "This page has unsaved edits. Save them, discard them, or keep editing.",
        ),
        Closing::Confirm { stuck: true } => (
            "These edits cannot be discarded",
            "The saved text is no longer in undo history. Save or keep editing.",
        ),
        Closing::SaveThenClose => ("Saving…", "Saving, then closing…"),
    };
    let mut spec = DialogSpec::new("editor.close", title, DialogKind::Confirm).body(msg);
    if let Closing::Confirm { stuck } = s.closing {
        if !stuck {
            spec = spec.action("discard", "Discard and close", ActionTone::Danger);
        }
        spec = spec
            .cancel("Keep editing")
            .action("save", "Save and close", ActionTone::Primary);
    }
    dialog::build::<EditorWindow>(&s.theme, "editor-close", &OpenDialog::new(spec), layer);
}

impl DialogHost for EditorWindow {
    fn act(&mut self, action: &str, tx: &mut Tx) {
        match action {
            "discard" => self.discard_and_close(tx),
            "save" => self.save_and_close(tx),
            _ => self.closing = Closing::Open,
        }
    }
    fn cancel(&mut self) {
        self.closing = Closing::Open;
    }
}

/// A text field's box: inset fill, hairline border, rounded, with a ring
/// while focus is inside it.
fn field_box(t: &Theme, key: String, field: El) -> El {
    row()
        .key(key)
        .grow(1.0)
        .min_w(0.0)
        .min_h(t.spacing.row_h + t.spacing.unit)
        .px(t.spacing.unit * 1.5)
        .items_center()
        .fill(color(t.colors.rail))
        .border(0.5, color(t.colors.border))
        .rounded(t, Size::Sm)
        .focus_within_ring(2.0, 0.0, color(t.colors.accent))
        .child(field)
}

fn note(t: &Theme, n: usize, note: &Note) -> El {
    let (what, detail) = match note {
        Note::UnknownKey { key, line } => (
            format!("unknown key `{key}`"),
            format!("line {line}: not a Boris frontmatter key — the next build judges"),
        ),
        Note::Unparsed { line, text } => (
            "not a field".to_string(),
            format!("line {line}: not a `key: value` field: {text}"),
        ),
        Note::Unclosed => (
            "unclosed".to_string(),
            "frontmatter has no closing `---`".to_string(),
        ),
    };
    row()
        .key(format!("fm-note-{n}"))
        .role("note")
        .gap(t.spacing.unit * 1.5)
        .items_center()
        .child(chip(t, "fm-note-chip", what, Tone::Warn))
        .child(label(t, detail).shrink().min_w(0.0))
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
                .child(field_box(t, format!("fm-box-{}", spec.key), field))
                .child(
                    label(t, spec.hint)
                        .single_line()
                        .truncate()
                        .help_when_truncated(spec.hint)
                        .w(200.0),
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
    for (n, nt) in form.notes.iter().enumerate() {
        f = f.child(note(t, n, nt));
    }
    f
}

/// Problem lines as small dots: filled for the last build, outlined once
/// edits have made it stale.
fn gutter(s: &EditorWindow) -> El {
    let t = &s.theme;
    let mut g = col()
        .key("editor-gutter")
        .w(56.0)
        .pt(t.spacing.unit)
        .gap(t.spacing.unit);
    let marks = {
        let ses = s.session.borrow();
        ses.get(s.buffer)
            .map(|e| e.problems.gutter(&e.buffer))
            .unwrap_or_default()
    };
    for m in marks {
        let line = m.line + 1;
        let dot = el().key("dot").size(8.0, 8.0).rounded(t, Size::Pill);
        let dot = if m.stale {
            dot.border(1.0, color(t.colors.text_dim))
        } else {
            dot.fill(color(t.colors.error))
        };
        let what = if m.stale {
            "Problem from before your edits"
        } else {
            "Problem"
        };
        g = g.child(
            row()
                .key(format!("gutter-{line}"))
                .role("note")
                .label(format!("{what} on line {line}"))
                .gap(t.spacing.unit)
                .items_center()
                .child(dot)
                .child(
                    text(line.to_string())
                        .font(font(t.type_.mono))
                        .color(color(t.colors.text_dim))
                        .a11y_hidden(),
                ),
        );
    }
    g
}

fn body(s: &EditorWindow) -> El {
    let t = &s.theme;
    let mut field = text_input(&s.body, font(t.type_.mono))
        .key("editor-body")
        .label_static("Markdown body")
        .grow(1.0)
        .min_h(240.0)
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
    row()
        .grow(1.0)
        .gap(t.spacing.unit)
        .child(gutter(s))
        .child(field)
}

/// Wikilink candidates in a floating list under the caret. The first row
/// is what Tab accepts, so it carries the highlight and a Tab keycap.
fn completion(s: &EditorWindow, caret: Option<[f32; 4]>, layer: &mut Vec<El>) {
    let t = &s.theme;
    let items = s.completions();
    if items.is_empty() {
        return;
    }
    let anchor = match caret {
        Some([x, y, w, h]) => {
            layer.push(
                el().key("completion-anchor")
                    .absolute(x, y)
                    .size(w.max(1.0), h)
                    .a11y_hidden(),
            );
            "completion-anchor"
        }
        None => "editor-body",
    };
    let k = theme::kit(t);
    let mut list = col()
        .key("completion")
        .role("listbox")
        .label("Wikilink completions")
        .w(320.0)
        .padding(t.spacing.unit * 0.5)
        .gap(1.0)
        .fill(color(t.colors.pane_alt))
        .border(0.5, color(t.colors.border))
        .rounded(t, Size::Md)
        .elevated(t, Level::Floating)
        .preserve_focus_on_press(true)
        .anchored_fit(anchor, Side::Bottom, Align::Start, 4.0)
        .transition(enter(t, Transition::opacity()))
        .motion(theme::spring(t.motion.snap));
    for (n, (id, title)) in items.into_iter().enumerate() {
        let mut r = row()
            .key(format!("complete-{n}"))
            .role("option")
            .label(id.clone())
            .w_full()
            .h(t.spacing.row_h + t.spacing.unit)
            .px(t.spacing.unit * 1.5)
            .gap(t.spacing.unit * 2.0)
            .items_center()
            .rounded(t, Size::Sm)
            .states(t)
            .on_press(move |s: &mut EditorWindow| {
                s.accept_completion(n);
            })
            .child(
                text(id)
                    .font(font(t.type_.mono))
                    .color(color(t.colors.text))
                    .single_line()
                    .a11y_hidden(),
            )
            .child(
                label(t, title)
                    .single_line()
                    .truncate()
                    .shrink()
                    .min_w(0.0)
                    .a11y_hidden(),
            )
            .child(spacer());
        if n == 0 {
            r = r
                .fill(color(t.colors.selection))
                .focus_selected(true)
                .child(Keybind::new(&["Tab"]).build(&k).key("complete-tab"));
        }
        list = list.child(r);
    }
    layer.push(list);
}

/// Problems as the shared diagnostic cards. A fresh one jumps to its line
/// when pressed; a stale one is dimmed and inert, since its span moved.
fn problems(s: &EditorWindow) -> El {
    let t = &s.theme;
    let rail = s.problems();
    let mut p = col().key("problems").gap(t.spacing.unit * 1.5).child(
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
        let tone = if stale {
            Tone::Dim
        } else {
            severity_tone(d.severity)
        };
        let mut card = diagnostic::card_in(t, d, n, tone).key(format!("problem-{n}"));
        if let (Some(line), Some(_)) = (pr.line, &pr.target) {
            card = card
                .role("button")
                .label(format!("{}: go to line {}", d.code, line + 1))
                .states(t)
                .press_scale(controls::PRESS_SCALE)
                .on_press(move |s: &mut EditorWindow| {
                    s.jump(n);
                });
        }
        p = p.child(card);
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
    let lights = cx.titlebar_inset();
    let panes = row()
        .key("editor-panes")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .padding(t.spacing.unit * 3.0)
        .gap(t.spacing.unit * 3.0)
        .child(
            col()
                .key("editor-main")
                .grow(1.0)
                .min_h(0.0)
                .gap(t.spacing.unit * 2.0)
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
        );
    let base = col()
        .key("editor")
        .w_full()
        .h_full()
        .fill(color(t.colors.bg))
        .child(header(s, lights))
        .children(error_banner(s))
        .child(panes);
    let mut layer = Vec::new();
    completion(s, cx.caret("editor-body"), &mut layer);
    close_dialog(s, &mut layer);
    rune_kit::root(base, layer)
}
