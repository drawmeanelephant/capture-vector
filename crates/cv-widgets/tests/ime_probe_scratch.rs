//! Scratch probe: replicate the macOS IME window-script events and record
//! buffer/caret/preedit state at each step. Run:
//!   cargo test -p cv-widgets --test ime_probe_scratch -- --nocapture

use cv_model::theme::Theme;
use cv_model::windows::WindowId;
use cv_ui_editor::{BufferId, EditorSession};
use cv_widgets::editor::{self, EditorWindow};
use rune::{Ime, Key, KeyEvent, Runtime};
use std::cell::RefCell;
use std::rc::Rc;

fn session() -> Rc<RefCell<EditorSession>> {
    Rc::new(RefCell::new(EditorSession::new()))
}

/// The snapshot tests each spawn a real window; two live wgpu surfaces in
/// one process segfault on software D3D12 (Microsoft Basic Render Driver),
/// so windowed tests must not run in parallel.
static WINDOWED: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn window(ses: &Rc<RefCell<EditorSession>>, id: u32, buf: BufferId) -> Runtime<EditorWindow> {
    editor::runtime(EditorWindow::new(
        WindowId(id),
        ses.clone(),
        buf,
        Theme::dark(),
    ))
}

fn settle(rt: &mut Runtime<EditorWindow>) {
    for _ in 0..40 {
        rt.frame(1.0 / 30.0);
    }
}

fn state(rt: &Runtime<EditorWindow>, ses: &Rc<RefCell<EditorSession>>, buf: BufferId, tag: &str) {
    let (preedit, target) = rt
        .state
        .body
        .preedit()
        .map(|(t, _)| (t.to_owned(), rt.state.body.preedit_target()))
        .unwrap_or_default();
    let sel = rt.state.body.range();
    eprintln!(
        "{tag:24} body={:?} field_sel={sel:?} preedit={preedit:?} target={target:?} buffer={:?} dirty={}",
        rt.state.body.text,
        ses.borrow().get(buf).unwrap().buffer.text(),
        rt.state.is_dirty()
    );
}

fn key(rt: &mut Runtime<EditorWindow>, k: Key) {
    rt.key(KeyEvent {
        key: k,
        mods: Default::default(),
    });
    settle(rt);
}

#[test]
fn ime_probe_replication() {
    let ses = session();
    let body_src = "# Getting Started\n\nA page is one Markdown file.\n";
    let buf = ses.borrow_mut().open("p.md", body_src, None);
    let mut rt = window(&ses, 1, buf);
    settle(&mut rt);

    // tap editor-body: click centre -> caret where the click lands
    let (x, y) = rt.find("editor-body").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(&mut rt);
    state(&rt, &ses, buf, "after tap body");

    // type "AA" as real text keys
    for c in "AA".chars() {
        key(&mut rt, Key::Text(c.to_string()));
    }
    state(&rt, &ses, buf, "type AA");

    // mid-word reconversion: select [0,2] then preedit then commit
    rt.ime(Ime::Select(0..2));
    settle(&mut rt);
    state(&rt, &ses, buf, "ime select 0..2");
    rt.ime(Ime::Preedit("きょう".into(), Some((9, 9)), vec![]));
    settle(&mut rt);
    state(&rt, &ses, buf, "preedit kyou");
    rt.ime(Ime::Preedit("きょうは".into(), Some((12, 12)), vec![]));
    settle(&mut rt);
    state(&rt, &ses, buf, "preedit kyouha");
    rt.ime(Ime::Commit("今日は".into()));
    settle(&mut rt);
    state(&rt, &ses, buf, "commit kyouha");

    for c in "BB".chars() {
        key(&mut rt, Key::Text(c.to_string()));
    }
    state(&rt, &ses, buf, "type BB");

    // Esc cancels composition
    rt.ime(Ime::Preedit("けさ".into(), Some((6, 6)), vec![]));
    settle(&mut rt);
    state(&rt, &ses, buf, "preedit kesa");
    key(&mut rt, Key::Escape);
    state(&rt, &ses, buf, "esc in preedit");

    // backspace inside composition
    rt.ime(Ime::Preedit("あした".into(), Some((9, 9)), vec![]));
    settle(&mut rt);
    state(&rt, &ses, buf, "preedit ashita");
    key(&mut rt, Key::Backspace);
    state(&rt, &ses, buf, "bs in preedit");
    rt.ime(Ime::Preedit("あし".into(), Some((6, 6)), vec![]));
    settle(&mut rt);
    state(&rt, &ses, buf, "preedit ashi");
    rt.ime(Ime::Commit("明日".into()));
    settle(&mut rt);
    state(&rt, &ses, buf, "commit ashita");

    // What Kotoeri sends on Esc: an empty preedit ends the composition.
    rt.ime(Ime::Preedit("まつ".into(), Some((6, 6)), vec![]));
    settle(&mut rt);
    state(&rt, &ses, buf, "preedit matsu");
    rt.ime(Ime::Preedit("".into(), None, vec![]));
    settle(&mut rt);
    state(&rt, &ses, buf, "empty preedit (Esc)");
}

thread_local! {
    static PENDING_ED: RefCell<Option<EditorWindow>> = const { RefCell::new(None) };
}

fn pending_ed() -> EditorWindow {
    PENDING_ED.with(|p| p.borrow_mut().take()).unwrap()
}

/// Render the editor mid-composition to a PNG: real headless run over the
/// editor's own AppSpec, `Ime::Preedit` alive when `--at` snaps the mark.
#[test]
fn ime_marked_text_snapshot() {
    let _windowed = WINDOWED.lock().unwrap();
    let ses = session();
    let buf = ses.borrow_mut().open("p.md", "café body line\n", None);
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("ime.json");
    let snap = dir.path().join("marked.png");
    std::fs::write(
        &script,
        r#"{"events":[
            {"t":0.5,"type":"tap","key":"editor-body"},
            {"t":1.0,"type":"ime","preedit":"きょうは","cursor":[12,12],"clauses":[[0,9],[9,12,true]]},
            {"t":2.0,"type":"mark","name":"marked"}
        ]}"#,
    )
    .unwrap();
    let args = vec![
        "--script".to_string(),
        script.to_str().unwrap().to_string(),
        "--snap".to_string(),
        snap.to_str().unwrap().to_string(),
        "--size".to_string(),
        "1100x760".to_string(),
        "--scale".to_string(),
        "2".to_string(),
        "--at".to_string(),
        "mark:marked".to_string(),
    ];
    PENDING_ED.with(|p| {
        *p.borrow_mut() = Some(EditorWindow::new(
            WindowId(1),
            ses.clone(),
            buf,
            Theme::dark(),
        ))
    });
    rune::run_windows_with_args(rune::RunOptions::default(), args, move |_| {
        vec![rune::WindowSpec::new(
            "ed",
            rune::AppSpec {
                fonts: cv_widgets::fonts::FONTS,
                families: cv_widgets::fonts::FAMILIES,
                poll: Some(editor::poll),
                on_key: Some(editor::on_key),
                ..rune::AppSpec::new("ed", editor::SIZE, pending_ed, editor::view)
            },
        )]
    });
    eprintln!("SNAP={}", snap.display());
    assert!(snap.exists());
    std::fs::copy(&snap, "/tmp/cv-probe/marked.png").ok();
}

/// Dirty-close: the host's `on_closed` (what `.on_close` fires on a real
/// title-bar close) marks a dirty window `Confirm` and queues a reopen; the
/// reopened window renders `editor-close-prompt` with save/discard/cancel.
#[test]
fn dirty_close_confirm() {
    use cv_widgets::editor::Closing;
    use cv_widgets::editor::host;
    use rune::Shell;

    let ses = session();
    let buf = ses.borrow_mut().open("p.md", "clean body\n", None);
    let mut rt = window(&ses, 7, buf);
    settle(&mut rt);

    // Dirty it like a real edit.
    let (x, y) = rt.find("editor-body").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(&mut rt);
    key(&mut rt, Key::Text("x".into()));
    assert!(rt.state.is_dirty());

    // What `.on_close` fires on title-bar close / performClose:.
    host::on_closed(&mut rt.state);
    assert!(
        matches!(rt.state.closing, Closing::Confirm { stuck: false }),
        "dirty close asks to confirm, got {:?}",
        rt.state.closing
    );
    assert_eq!(
        host::take_reopen(),
        vec![WindowId(7)],
        "id queued to reopen"
    );
    assert!(host::take_closed().is_empty(), "not closed outright");

    // Reopened window: host::open sets closing=Confirm when ASK held the id.
    let mut w = EditorWindow::new(WindowId(7), ses.clone(), buf, Theme::dark());
    w.closing = Closing::Confirm { stuck: false };
    let mut rt = editor::runtime(w);
    settle(&mut rt);
    for k in [
        "editor-close-prompt",
        "editor-close-save",
        "editor-close-discard",
        "editor-close-cancel",
    ] {
        assert!(rt.find(k).is_some(), "prompt element {k} missing");
    }

    // Keep editing: prompt dismissed, edits intact.
    let (x, y) = rt.find("editor-close-cancel").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(&mut rt);
    assert!(matches!(rt.state.closing, Closing::Open));
    assert!(rt.state.is_dirty(), "keep editing preserves edits");

    // Discard and close: buffer reverted, window asked to close.
    rt.state.closing = Closing::Confirm { stuck: false };
    settle(&mut rt);
    let (x, y) = rt.find("editor-close-discard").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(&mut rt);
    assert_eq!(
        rt.shell_request.take(),
        Some(Shell::Close),
        "discard closes"
    );
    assert_eq!(
        ses.borrow().get(buf).unwrap().buffer.text(),
        "clean body\n",
        "discard reverts to disk"
    );

    // Clean window: closes outright, no confirm.
    let buf2 = ses.borrow_mut().open("q.md", "clean\n", None);
    let mut w = EditorWindow::new(WindowId(8), ses.clone(), buf2, Theme::dark());
    host::on_closed(&mut w);
    assert!(matches!(w.closing, Closing::Open), "clean close is silent");
    assert_eq!(host::take_closed(), vec![WindowId(8)]);
}

/// Snap the dirty-close prompt the reopened window shows.
#[test]
fn dirty_close_prompt_snapshot() {
    use cv_widgets::editor::Closing;

    let _windowed = WINDOWED.lock().unwrap();
    let ses = session();
    let buf = ses.borrow_mut().open("p.md", "clean body\n", None);
    let mut w = EditorWindow::new(WindowId(9), ses.clone(), buf, Theme::dark());
    w.body.insert("x");
    w.closing = Closing::Confirm { stuck: false };
    let dir = tempfile::tempdir().unwrap();
    let snap = dir.path().join("close.png");
    let args = vec![
        "--snap".to_string(),
        snap.to_str().unwrap().to_string(),
        "--size".to_string(),
        "1100x760".to_string(),
        "--scale".to_string(),
        "2".to_string(),
        "--at".to_string(),
        "60".to_string(),
    ];
    PENDING_ED.with(|p| *p.borrow_mut() = Some(w));
    rune::run_windows_with_args(rune::RunOptions::default(), args, move |_| {
        vec![rune::WindowSpec::new(
            "ed",
            rune::AppSpec {
                fonts: cv_widgets::fonts::FONTS,
                families: cv_widgets::fonts::FAMILIES,
                poll: Some(editor::poll),
                on_key: Some(editor::on_key),
                ..rune::AppSpec::new("ed", editor::SIZE, pending_ed, editor::view)
            },
        )]
    });
    assert!(snap.exists());
    std::fs::copy(&snap, "/tmp/cv-probe/close-prompt.png").ok();
}
