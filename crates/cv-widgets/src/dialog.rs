//! The shared modal dialog: one renderer over `cv_model::dialog` for the
//! shell's dialogs and the editor's own. Built on `rune_kit::Dialog`, so
//! Rune owns the scrim, the focus trap, Escape and focus return; the host
//! says what each button, field and key does.

use crate::controls::{self, Variant};
use crate::theme::{self, Size, Styled, color, font};
use crate::{icons, shell};
use cv_model::dialog::{ActionTone, DialogKind, Field, Input, OpenDialog};
use cv_model::module::Tone;
use cv_model::theme::Theme;
use cv_model::workbench::Workbench;
use rune::{El, KeyEvent, Tx, col, el, icon, row, spacer, text};
use rune_kit::{Dialog, Switch};

/// A view state that can show a dialog.
pub trait DialogHost: Sized + 'static {
    /// Whether the host currently has a dialog on screen.
    fn open(&self) -> bool;
    /// A button was pressed.
    fn act(&mut self, action: &str, tx: &mut Tx);
    /// Escape, the close button or a press outside.
    fn cancel(&mut self);
    /// A field was pressed; with `option`, one of its options or its switch.
    fn field(&mut self, _field: usize, _option: Option<usize>) {}
    fn browse(&mut self, _field: usize) {}
    /// A `Choose` row was pressed.
    fn choose(&mut self, _row: usize) {}
    /// The host's own keymap for the dialog, ahead of Rune's (which still
    /// handles Escape and the default button when this is `None`).
    const KEYS: Option<fn(&mut Self, &KeyEvent, &mut Tx) -> bool> = None;
}

impl DialogHost for Workbench {
    fn open(&self) -> bool {
        self.dialogs.is_open()
    }
    fn act(&mut self, action: &str, _tx: &mut Tx) {
        self.dialog_act(action);
    }
    fn cancel(&mut self) {
        self.dialog_cancel();
    }
    fn field(&mut self, field: usize, option: Option<usize>) {
        self.dialog_field(field, option);
    }
    fn browse(&mut self, field: usize) {
        self.dialog_browse(field);
    }
    fn choose(&mut self, row: usize) {
        self.dialog_choose(row);
    }
    const KEYS: Option<fn(&mut Self, &KeyEvent, &mut Tx) -> bool> = Some(crate::app::on_key);
}

/// The header close control's activation key when Rune lets it reach the
/// window keymap (a dialog with no default button). Dialogs with a default
/// answer it in their action handler instead, before this runs.
pub(crate) fn close_control_key<S: DialogHost>(s: &mut S, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if !s.open() || tx.focused() != Some("close") {
        return false;
    }
    crate::value::activate_on_key(s, ev, tx, |s, _| s.cancel())
}

fn caption(t: &Theme, s: impl Into<String>) -> El {
    text(s)
        .font(font(t.type_.label))
        .color(color(t.colors.text_dim))
}

fn caret(t: &Theme, key: String) -> El {
    el().key(key)
        .w(1.5)
        .h(t.type_.body.size + 4.0)
        .fill(color(t.colors.accent))
        .a11y_hidden()
}

/// A one-line text box drawn from the model: the value (or its dim
/// placeholder) and, while it is the field keys edit, a caret.
fn text_box(t: &Theme, key: &str, label: &str, shown: &str, placeholder: &str, active: bool) -> El {
    let line = if shown.is_empty() {
        text(placeholder.to_string()).color(color(t.colors.text_dim))
    } else {
        text(shown.to_string()).color(color(t.colors.text))
    }
    .key(format!("{key}-text"))
    .font(font(t.type_.body))
    .single_line()
    .truncate_head()
    .shrink()
    .min_w(0.0);
    let mut inner = row().items_center().gap(1.0).shrink().min_w(0.0);
    inner = match (active, shown.is_empty()) {
        (true, true) => inner.child(caret(t, format!("{key}-caret"))).child(line),
        (true, false) => inner.child(line).child(caret(t, format!("{key}-caret"))),
        (false, _) => inner.child(line),
    };
    let edge = if active {
        t.colors.accent
    } else {
        t.colors.border
    };
    row()
        .key(key.to_string())
        .role("textbox")
        .label(label.to_string())
        .grow(1.0)
        .min_w(0.0)
        .min_h(t.spacing.row_h + t.spacing.unit)
        .px(t.spacing.unit * 1.5)
        .items_center()
        .fill(color(t.colors.rail))
        .border(if active { 1.0 } else { 0.5 }, color(edge))
        .rounded(t, Size::Sm)
        .child(inner)
}

fn options<S: DialogHost>(t: &Theme, key: &str, i: usize, opts: &[String], selected: usize) -> El {
    let mut r = row()
        .key(format!("{key}-options"))
        .role("radiogroup")
        .w_full()
        .flex_wrap()
        .gap(t.spacing.unit)
        .items_center();
    for (o, label) in opts.iter().enumerate() {
        let sel = o == selected;
        let fg = if sel {
            t.colors.accent_text
        } else {
            t.colors.text
        };
        let mut chip = row()
            .key(format!("{key}-option-{o}"))
            .role("radio")
            .checked(sel)
            .label(label.clone())
            .px(t.spacing.unit * 1.5)
            .h(t.spacing.row_h)
            .items_center()
            .rounded(t, Size::Pill)
            .states(t)
            .on_press(move |s: &mut S| s.field(i, Some(o)))
            .child(
                text(label.clone())
                    .font(font(t.type_.label))
                    .color(color(fg))
                    .single_line()
                    .a11y_hidden(),
            );
        chip = if sel {
            chip.fill(color(t.colors.accent))
        } else {
            chip.fill(theme::tint(t.colors.text, 0x12))
        };
        r = r.child(chip);
    }
    r
}

fn field_el<S: DialogHost>(t: &Theme, key: &str, i: usize, f: &Field, active: bool) -> El {
    let fkey = format!("{key}-field-{}", f.id);
    let control = match &f.input {
        Input::Text { value, placeholder } => text_box(
            t,
            &format!("{fkey}-box"),
            &f.label,
            value,
            placeholder,
            active,
        ),
        Input::Path { value, .. } => row()
            .w_full()
            .gap(t.spacing.unit * 2.0)
            .items_center()
            .child(text_box(
                t,
                &format!("{fkey}-box"),
                &f.label,
                value,
                "",
                active,
            ))
            .child(controls::button(
                t,
                &format!("{fkey}-browse"),
                "Browse…",
                Variant::Neutral,
                move |s: &mut S| s.browse(i),
            )),
        Input::Choice {
            options: opts,
            selected,
        } => options::<S>(t, &fkey, i, opts, *selected),
        Input::Toggle(on) => Switch::new(*on)
            .label(f.label.clone())
            .hide_label()
            .on_toggle(move |s: &mut S| s.field(i, Some(0)))
            .build(&theme::kit(t))
            .key(format!("{fkey}-switch")),
    };
    let mut head = row().w_full().items_center().child(
        caption(t, f.label.clone())
            .key(format!("{fkey}-label"))
            .color(color(if active {
                t.colors.text
            } else {
                t.colors.text_dim
            })),
    );
    if matches!(f.input, Input::Toggle(_)) {
        head = head.child(spacer()).child(control);
        return col()
            .key(fkey)
            .w_full()
            .on_press(move |s: &mut S| s.field(i, None))
            .child(head);
    }
    let mut c = col()
        .key(fkey)
        .w_full()
        .gap(t.spacing.unit)
        .on_press(move |s: &mut S| s.field(i, None))
        .child(head)
        .child(control);
    if let Some(h) = &f.hint {
        c = c.child(caption(t, h.clone()).wrap().w_full());
    }
    c
}

fn choose_list<S: DialogHost>(t: &Theme, key: &str, d: &OpenDialog) -> El {
    let q = &d.choose.query;
    let shown = if q.is_empty() {
        text("Type to filter").color(color(t.colors.text_dim))
    } else {
        text(q.clone()).color(color(t.colors.text))
    }
    .key(format!("{key}-query"))
    .font(font(t.type_.body))
    .single_line();
    let line = if q.is_empty() {
        row().child(caret(t, format!("{key}-caret"))).child(shown)
    } else {
        row().child(shown).child(caret(t, format!("{key}-caret")))
    };
    let search = row()
        .key(format!("{key}-search"))
        .w_full()
        .h(t.spacing.row_h + t.spacing.unit)
        .px(t.spacing.unit * 1.5)
        .gap(t.spacing.unit * 1.5)
        .items_center()
        .fill(color(t.colors.rail))
        .border(0.5, color(t.colors.border))
        .rounded(t, Size::Sm)
        .child(icon(&icons::SEARCH, 14.0).color(color(t.colors.text_dim)))
        .child(line.items_center().gap(1.0));
    let rows = d.visible_choices();
    let mut list = col()
        .key(format!("{key}-list"))
        .role("listbox")
        .label(d.spec.title.clone())
        .w_full()
        .gap(1.0);
    if rows.is_empty() {
        list = list.child(caption(t, "Nothing matches").key(format!("{key}-none")));
    }
    for (n, (id, label)) in rows.into_iter().enumerate() {
        let sel = n == d.choose.cursor;
        let mut r = row()
            .key(format!("{key}-choice-{id}"))
            .role("option")
            .focus_selected(sel)
            .label(label.clone())
            .w_full()
            .h(t.spacing.row_h)
            .px(t.spacing.unit * 1.5)
            .items_center()
            .rounded(t, Size::Sm)
            .states(t)
            .on_press(move |s: &mut S| s.choose(n))
            .child(
                text(label)
                    .font(font(t.type_.body))
                    .color(color(t.colors.text))
                    .single_line()
                    .truncate_head()
                    .shrink()
                    .min_w(0.0)
                    .a11y_hidden(),
            );
        if sel {
            r = r.fill(color(t.colors.selection));
        }
        list = list.child(r);
    }
    col().w_full().gap(t.spacing.unit).child(search).child(list)
}

/// What sits between the title and the buttons, if anything.
fn content<S: DialogHost>(t: &Theme, key: &str, d: &OpenDialog) -> Option<El> {
    let mut parts: Vec<El> = Vec::new();
    match &d.spec.kind {
        DialogKind::Confirm => {}
        DialogKind::Prompt(f) => parts.push(field_el::<S>(t, key, 0, f, true)),
        DialogKind::Form(fs) => {
            for (i, f) in fs.iter().enumerate() {
                parts.push(field_el::<S>(t, key, i, f, i == d.focus));
            }
        }
        DialogKind::Choose(_) => parts.push(choose_list::<S>(t, key, d)),
        DialogKind::Secret(f) => {
            let mask = "•".repeat(d.secret_chars());
            parts.push(
                col()
                    .key(format!("{key}-secret"))
                    .w_full()
                    .gap(t.spacing.unit)
                    .child(caption(t, f.label.clone()).color(color(t.colors.text)))
                    .child(text_box(
                        t,
                        &format!("{key}-secret-box"),
                        &f.label,
                        &mask,
                        "",
                        true,
                    ))
                    .child(caption(
                        t,
                        format!("Stored as {}. It is never shown or logged.", f.name),
                    )),
            );
        }
        DialogKind::Progress(p) => {
            let value = p.permille.map(|v| f32::from(v.min(1000)) / 1000.0);
            parts.push(
                rune_kit::Progress::new(value)
                    .label(p.label.clone())
                    .show_value()
                    .build(&theme::kit(t))
                    .key(format!("{key}-progress")),
            );
        }
    }
    if let Some(e) = &d.spec.error {
        parts.push(
            text(e.clone())
                .key(format!("{key}-error"))
                .role("alert")
                .font(font(t.type_.body))
                .color(shell::tone(t, Tone::Error))
                .wrap()
                .w_full(),
        );
    }
    if parts.is_empty() {
        return None;
    }
    let mut c = col()
        .key(format!("{key}-content"))
        .w_full()
        .px(16.0)
        .pb(t.spacing.unit * 3.0)
        .gap(t.spacing.unit * 3.0)
        .children(parts);
    if let Some(keys) = S::KEYS {
        // The dialog opens with focus here, so its keys, Tab included,
        // reach the host's keymap before Rune's focus traversal. Inside
        // the modal is the only place that hears them: keys do not
        // bubble past a modal.
        c = c.focusable(true).autofocus().no_focus_ring().on_key(keys);
    }
    Some(c)
}

/// Danger actions lead on the left; the rest sit on the right in order.
fn actions<S: DialogHost>(t: &Theme, key: &str, d: &OpenDialog, focus_primary: bool) -> El {
    let mut r = row()
        .key(format!("{key}-prompt"))
        .w_full()
        .px(16.0)
        .pb(16.0)
        .gap(t.spacing.unit * 2.0)
        .items_center();
    let primary = d.spec.primary().map(|a| a.id.clone());
    let (danger, rest): (Vec<_>, Vec<_>) = d
        .spec
        .actions
        .iter()
        .partition(|a| a.tone == ActionTone::Danger);
    let button = |a: &cv_model::dialog::DialogAction| {
        let v = match a.tone {
            ActionTone::Primary => Variant::Primary,
            ActionTone::Ghost => Variant::Ghost,
            ActionTone::Danger => Variant::Danger,
        };
        let id = a.id.clone();
        let default = primary.as_deref() == Some(a.id.as_str());
        let b = controls::button_tx(
            t,
            &format!("{key}-{}", a.id),
            &a.label,
            v,
            true,
            move |s: &mut S, tx| {
                // Rune Kit's header close button cannot carry this crate's
                // control key handler. When Return falls through to the
                // dialog's default button while close is focused, the close
                // is the control being activated; answer it as a cancel,
                // never as the action Return happened to reach.
                if default && tx.by_keys() && tx.focused() == Some("close") {
                    s.cancel();
                } else {
                    s.act(&id, tx);
                }
            },
        );
        if default {
            let b = b.default_button();
            if focus_primary { b.autofocus() } else { b }
        } else {
            b
        }
    };
    for a in danger {
        r = r.child(button(a));
    }
    if !d.spec.actions.is_empty() {
        r = r.child(spacer());
    }
    for a in rest {
        r = r.child(button(a));
    }
    r
}

fn width(kind: &DialogKind) -> f32 {
    match kind {
        DialogKind::Confirm => 440.0,
        _ => 480.0,
    }
}

/// Mount dialog `d` into `layer` (`rune_kit::root`'s overlays, or the
/// shell root's children). `key` prefixes every element key:
/// `{key}-dialog`, `{key}-scrim`, `{key}-prompt` (the button row),
/// `{key}-<action id>`.
pub fn build<S: DialogHost>(t: &Theme, key: &str, d: &OpenDialog, layer: &mut Vec<El>) {
    let k = theme::kit(t);
    let mut dlg = Dialog::new(key, d.spec.title.clone())
        .width(width(&d.spec.kind))
        .fit()
        .open(true)
        .on_close(|s: &mut S| s.cancel());
    if let Some(b) = &d.spec.body {
        dlg = dlg.description(b.clone());
    }
    let body = content::<S>(t, key, d);
    let focus_primary = S::KEYS.is_some() && body.is_none();
    if let Some(c) = body {
        dlg = dlg.child(c);
    }
    dlg = dlg.child(actions::<S>(t, key, d, focus_primary));
    dlg.build(&k, layer);
}
