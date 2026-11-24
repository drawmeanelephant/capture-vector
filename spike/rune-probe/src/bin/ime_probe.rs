//! Live IME probe (issue #26): a real window with a focused `text_input`.
//!
//! Drive it with an OS input method — on Linux, ibus over XIM:
//!
//! ```sh
//! XMODIFIERS=@im=ibus IME_PROBE_LOG=out/ime-probe.log \
//!   cargo run --bin ime-probe
//! ```
//!
//! Every observed state change — committed buffer, preedit text + cursor,
//! conversion clauses, app-level `Ime` events (Enabled/Disabled) — is
//! appended to the log file and shown in the window, so xdotool+screenshots
//! give both programmatic and visual evidence.

use std::fs::OpenOptions;
use std::io::Write;

use rune::tok::{self, type_};
use rune::{App, AppSpec, Cx, El, TextEdit, Tx, col, text, text_input};

pub struct State {
    field: TextEdit,
    events: Vec<String>,
    last: String,
    asked_focus: bool,
    log_path: String,
}

fn init() -> State {
    let log_path = std::env::var("IME_PROBE_LOG")
        .unwrap_or_else(|_| "out/ime-probe.log".to_owned());
    State {
        field: TextEdit::new(""),
        events: Vec::new(),
        last: String::new(),
        asked_focus: false,
        log_path,
    }
}

fn snapshot(s: &State) -> String {
    let (preedit, cursor) = s
        .field
        .preedit()
        .map(|(t, c)| (t.to_owned(), c))
        .unwrap_or_default();
    format!(
        "text={:?} anchor={} focus={} preedit={:?} cursor={:?} clauses={:?}",
        s.field.text,
        s.field.anchor,
        s.field.focus,
        preedit,
        cursor,
        s.field.preedit_clauses(),
    )
}

fn log_line(s: &mut State, t: f64, line: String) {
    let stamped = format!("t={t:8.3} {line}");
    {
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&s.log_path)
            .expect("IME_PROBE_LOG writable");
        writeln!(f, "{stamped}").unwrap();
    }
    s.events.push(stamped);
    if s.events.len() > 14 {
        s.events.remove(0);
    }
}

fn poll(s: &mut State, t: f64, tx: &mut Tx) -> bool {
    if !s.asked_focus {
        s.asked_focus = true;
        tx.focus("ime-field", true);
        log_line(s, t, "boot: focus ime-field".into());
    }
    let snap = snapshot(s);
    if snap != s.last {
        s.last = snap.clone();
        log_line(s, t, snap);
    }
    true
}

fn on_ime(s: &mut State, ev: &rune::Ime, _: &mut Tx) -> bool {
    log_line(s, 0.0, format!("app-event: {ev:?}"));
    true
}

fn view(s: &State, _: &mut Cx<State>) -> El {
    let t = tok::theme(true);
    let (preedit, _) = s
        .field
        .preedit()
        .map(|(p, c)| (p.to_owned(), c))
        .unwrap_or_default();
    let mut events = col().key("events").gap(2.0);
    for e in &s.events {
        events = events.child(text(e.clone()).font(type_::LABEL).color(t.text_secondary));
    }
    col()
        .key("root")
        .w_full()
        .h_full()
        .fill(t.bg)
        .gap(12.0)
        .padding(24.0)
        .child(
            text("IME probe — type with an input method (ibus anthy)")
                .font(type_::TITLE)
                .color(t.text),
        )
        .child(
            text_input(&s.field, type_::BODY)
                .key("ime-field")
                .label("ime probe field")
                .grow(1.0)
                .min_h(96.0)
                .color(t.text)
                .on_edit(|s: &mut State| &mut s.field),
        )
        .child(
            text(format!("preedit: {preedit:?}   text: {:?}", s.field.text))
                .font(type_::BODY)
                .color(t.text_secondary),
        )
        .child(events)
}

fn main() {
    let spec = AppSpec::<State> {
        poll: Some(poll),
        ..AppSpec::new("rune IME probe", (640.0, 480.0), init, view)
    };
    App::new(spec).on_ime(on_ime).run();
}
