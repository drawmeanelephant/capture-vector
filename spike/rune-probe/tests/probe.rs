//! Wave-0 capability probe: the facts the spec bet on, tested in-process.

use rune::tok::{self, type_};
use rune::{Cx, El, Env, Runtime, col, el, text};

#[derive(Default)]
struct S {
    diags: usize,
    built: bool,
}

#[test]
fn headless_runtime_drives_pointer_and_reads_tree() {
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let t = tok::theme(true);
        col().fill(t.bg).w_full().h_full().child(
            el().key("btn").role("button").focusable(true).size(80.0, 24.0)
                .fill(t.control_off)
                .on_press(|s: &mut S| s.diags += 1)
                .child(text(format!("count {}", s.diags)).color(t.text)),
        )
    }
    let env = Env { system_dark: true, scale: 2.0 };
    let mut rt = Runtime::new(S::default(), view, (400.0, 300.0), env);
    rt.frame(0.0);
    let (x, y) = rt.find("btn").expect("button present in element tree");
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.diags, 1, "scripted pointer drove the handler");
}

#[test]
fn virtual_list_mounts_bounded_rows_of_five_thousand() {
    use rune::{GrowthSpec, ScrollSpec};
    fn view(_: &S, _: &mut Cx<S>) -> El {
        let t = tok::theme(true);
        let rows = col()
            .key("vlist")
            .growth(GrowthSpec::default())
            .virtual_rows::<S>(5000, 24.0,
                |_s: &S, i| format!("r{i}").into(),
                move |_s: &S, i| text(format!("row {i}")).color(t.text));
        col().w_full().h_full().min_h(0.0).fill(t.bg).child(
            el().w_full().h_full().min_h(0.0).scroll(ScrollSpec { stick_bottom: false }).child(rows),
        )
    }
    let env = Env { system_dark: true, scale: 1.0 };
    let mut rt = Runtime::new(S::default(), view, (800.0, 480.0), env);
    rt.frame(0.0);
    // If virtualize is real, the tree holds far fewer than 5000 text nodes.
    let info = rt.elements();
    let count = info.iter().filter(|e| e.text.as_deref().map_or(false, |t| t.starts_with("row "))).count();
    assert!(count > 0 && count < 500, "mounted {count} rows of 5000 — virtualization bounded");
}
