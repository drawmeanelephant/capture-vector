//! Rune capability probe for Capture Vector wave-0.
//! `cargo run -- --snap out.png --tree out.json` exercises headless paths;
//! `cargo test` runs the in-process Runtime capability suite.

use rune::tok::{self, type_};
use rune::{AppSpec, Cx, El, col, el, row, text};

#[derive(Default)]
pub struct Diag {
    pub code: &'static str,
    pub page: usize,
}

#[derive(Default)]
pub struct State {
    pub diagnostics: Vec<Diag>,
    pub rows: usize,
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
    col().w_full().h_full().fill(t.bg).child(
        text(format!("Capture Vector probe — {} findings, {} rows",
                     s.diagnostics.len(), s.rows))
            .font(type_::TITLE).color(t.text),
    ).child(list)
}

fn main() {
    rune::run(AppSpec::<State> {
        scenes: &[
            ("diags", |s| {
                s.diagnostics = vec![
                    Diag { code: "EPARENTMISSING", page: 7 },
                    Diag { code: "EREFERENCEMISSING", page: 12 },
                ];
                s.rows = 5000;
            }),
        ],
        ..AppSpec::new("CV Probe", (900.0, 600.0), State::default, view)
    });
}
