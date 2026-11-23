//! Rune capability probe for Capture Vector validation readiness.
//! Headless flags exercise the real Rune Runtime and renderer. Native mode opens
//! two independent windows with stable accessibility keys for OS-driver probes.

use rune::tok::{self, type_};
use rune::{AppSpec, Cx, El, TextEdit, WindowSpec, col, el, row, text, text_input};

#[derive(Default)]
pub struct Diag {
    pub code: &'static str,
    pub page: usize,
}

#[derive(Default)]
pub struct State {
    pub diagnostics: Vec<Diag>,
    pub rows: usize,
    pub note: TextEdit,
    pub presses: usize,
}

pub fn view(s: &State, _: &mut Cx<State>) -> El {
    let t = tok::theme(true);
    let mut list = col().key("mailbox").w_full();
    for (i, d) in s.diagnostics.iter().enumerate() {
        list = list.child(
            row().key(format!("diag-{i}")).gap(8.0).child(
                text(format!("{} on page {}", d.code, d.page)).color(t.text),
            ),
        );
    }
    col().key("probe-root").w_full().h_full().fill(t.bg).padding(16.0).gap(10.0).child(
        text(format!("Capture Vector probe — {} findings, {} rows, {} presses",
                     s.diagnostics.len(), s.rows, s.presses))
            .key("status").font(type_::TITLE).color(t.text),
    ).child(
        el().key("press").role("button").label("Probe button").focusable(true)
            .size(160.0, 32.0).fill(t.control_off).center()
            .on_press(|s: &mut State| s.presses += 1)
            .child(text("Probe button").color(t.text)),
    ).child(
        text_input(&s.note, type_::BODY).key("note").label("Probe note")
            .single_line().w_full().h(34.0).color(t.text)
            .on_edit(|s: &mut State| &mut s.note),
    ).child(list)
}

fn spec(title: &'static str) -> AppSpec<State> {
    AppSpec::<State> {
        scenes: &[
            ("diags", |s| {
                s.diagnostics = vec![
                    Diag { code: "EPARENTMISSING", page: 7 },
                    Diag { code: "EREFERENCEMISSING", page: 12 },
                ];
                s.rows = 5000;
            }),
        ],
        ..AppSpec::new(title, (900.0, 600.0), State::default, view)
    }
}

fn main() {
    rune::run_windows(|_| [
        WindowSpec::new("primary", spec("CV Probe Primary")),
        WindowSpec::new("secondary", spec("CV Probe Secondary")),
    ]);
}
