//! L7 editor surface, driven headless: typing, form, problems, completion,
//! N buffers/windows, save, IME, degraded states.

use cv_core::diagnostic::{Diagnostic, StderrDiagnostic};
use cv_core::engine::{Engine, Verb};
use cv_core::events::BuildId;
use cv_fixtures::stub_engine::StubEngine;
use cv_model::theme::Theme;
use cv_model::windows::{CloseOutcome, WindowId, Windows};
use cv_ui_editor::completion::CompletionIndex;
use cv_ui_editor::problems::Freshness;
use cv_ui_editor::save::{FsWriter, SaveStatus};
use cv_ui_editor::{BufferId, EditorSession};
use cv_widgets::editor::{self, EditorWindow};
use rune::accesskit::{Action, Role};
use rune::{Ime, Runtime};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn session() -> Rc<RefCell<EditorSession>> {
    Rc::new(RefCell::new(EditorSession::new()))
}

fn window(ses: &Rc<RefCell<EditorSession>>, id: u32, buf: BufferId) -> Runtime<EditorWindow> {
    editor::runtime(EditorWindow::new(
        WindowId(id),
        ses.clone(),
        buf,
        Theme::dark(),
    ))
}

/// Run frames until enter/exit transitions land (a fresh element is
/// unpickable at opacity 0).
fn settle(rt: &mut Runtime<EditorWindow>) {
    for _ in 0..40 {
        rt.frame(1.0 / 30.0);
    }
}

fn click(rt: &mut Runtime<EditorWindow>, key: &str) {
    settle(rt);
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no element {key}"));
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(rt);
}

fn texts(rt: &Runtime<EditorWindow>) -> Vec<String> {
    rt.elements().into_iter().filter_map(|e| e.text).collect()
}

/// Focus the body and put the caret at body byte `at`.
fn caret(rt: &mut Runtime<EditorWindow>, at: usize) {
    click(rt, "editor-body");
    rt.state.body.set_caret(at);
}

fn type_str(rt: &mut Runtime<EditorWindow>, s: &str) {
    rt.paste(s);
    settle(rt);
}

/// Diagnostics of one StubEngine verb run: the build id comes from the
/// emitted envelope, the diagnostics from the corpus's captured stderr.
fn stub_build(corpus: &str) -> (BuildId, Vec<Diagnostic>) {
    let engine = StubEngine::new(corpus);
    let seen = Arc::new(Mutex::new(None));
    let s = seen.clone();
    engine
        .run(
            &Verb::Build,
            Box::new(move |env| *s.lock().unwrap() = env.build_id),
        )
        .expect("stub run");
    let build = seen
        .lock()
        .unwrap()
        .clone()
        .expect("envelope carries a build id");
    let stderr = cv_fixtures::corpus(corpus).file_str("build/stderr.txt");
    let diags = stderr
        .lines()
        .filter_map(StderrDiagnostic::parse)
        .map(|d| Diagnostic {
            build_version: Some(build.clone()),
            ..Diagnostic::from(&d)
        })
        .collect();
    (build, diags)
}

fn broken_page() -> String {
    std::fs::read_to_string(repo().join("fixtures/corpus-src/broken-wikilink/page.md")).unwrap()
}

#[test]
fn broken_wikilink_reports_line_goes_stale_on_typing_and_clears() {
    let ses = session();
    let buf = ses.borrow_mut().open("page.md", &broken_page(), None);
    let mut rt = window(&ses, 1, buf);

    let (build, diags) = stub_build("broken-wikilink");
    ses.borrow_mut().ingest_build(build, &diags);
    rt.frame(0.0);
    let rail = rt.state.problems();
    assert_eq!(rail.len(), 1);
    assert_eq!(rail[0].diagnostic.code, "EREFERENCEMISSING");
    assert_eq!(rail[0].line, Some(5), "page.md:6 is 0-based line 5");
    assert_eq!(rail[0].freshness, Freshness::Fresh);
    assert!(rt.find("gutter-6").is_some());
    assert!(texts(&rt).iter().any(|t| t == "EREFERENCEMISSING:6"));

    // Click-to-span: the caret lands on line 6 of the file.
    click(&mut rt, "problem-0");
    let off = rt.state.body.range().0;
    let line6 = rt.state.body.text[off..]
        .lines()
        .next()
        .unwrap()
        .to_string();
    assert!(line6.starts_with("This page links to"), "{line6:?}");

    // Typing makes the old build's diagnostic stale: dimmed, never remapped.
    let target = "non-existent-page-target";
    let fix_at = rt.state.body.text.find(target).unwrap();
    click(&mut rt, "editor-body");
    rt.state.body.anchor = fix_at;
    rt.state.body.focus = fix_at + target.len();
    type_str(&mut rt, "broken-link-page");
    assert!(rt.state.body.text.contains("[[broken-link-page]]"));
    assert!(rt.state.is_dirty());
    let rail = rt.state.problems();
    assert_eq!(rail[0].freshness, Freshness::Stale);
    assert_eq!(rail[0].target, None, "stale spans are not remapped");
    assert!(rt.find("problems-stale").is_some());
    assert!(
        texts(&rt)
            .iter()
            .any(|t| t.contains("[[broken-link-page]]")),
        "sketch follows the buffer"
    );

    // The next (clean) build clears the rail.
    let (clean, none) = stub_build("happy");
    assert!(none.is_empty());
    ses.borrow_mut().ingest_build(clean, &none);
    rt.frame(0.0);
    assert!(rt.state.problems().is_empty());
    assert!(rt.find("problems-empty").is_some());
    assert!(rt.find("gutter-6").is_none());
}

#[test]
fn form_edits_frontmatter_and_undo_restores_it() {
    let ses = session();
    let buf = ses.borrow_mut().open("page.md", &broken_page(), None);
    let mut rt = window(&ses, 1, buf);
    let title = cv_ui_editor::frontmatter::SCHEMA
        .iter()
        .position(|k| k.key == "title")
        .unwrap();
    assert_eq!(rt.state.fields[title].text, "Page with Broken Wikilink");

    click(&mut rt, "fm-status");
    type_str(&mut rt, "draft");
    let text = ses.borrow().get(buf).unwrap().buffer.text();
    assert!(
        text.starts_with(
            "---\nid: broken-link-page\ntitle: Page with Broken Wikilink\nstatus: draft\n---\n"
        ),
        "{text}"
    );
    assert!(
        rt.state.body.text.starts_with("\nThis page links to"),
        "body editor shows only the body"
    );

    rt.state.undo();
    rt.frame(0.0);
    assert_eq!(ses.borrow().get(buf).unwrap().buffer.text(), broken_page());
    assert!(!rt.state.is_dirty());
    rt.state.redo();
    assert!(
        ses.borrow()
            .get(buf)
            .unwrap()
            .buffer
            .text()
            .contains("status: draft")
    );
}

#[test]
fn wikilink_completion_comes_from_captured_completion_json() {
    let json =
        std::fs::read_to_string(repo().join("fixtures/editor/captures/parity/completion.json"))
            .unwrap();
    let ses = session();
    ses.borrow_mut().completion = Some(CompletionIndex::parse(&json).unwrap());
    let buf = ses
        .borrow_mut()
        .open("content/new.md", "---\ntitle: New\n---\nSee ", None);
    let mut rt = window(&ses, 1, buf);
    let end = rt.state.body.text.len();
    caret(&mut rt, end);
    type_str(&mut rt, "[[gu");
    assert!(rt.find("completion").is_some());
    assert!(texts(&rt).iter().any(|t| t == "guide"));
    click(&mut rt, "complete-0");
    assert_eq!(rt.state.body.text, "See [[guide]]");
    assert!(rt.find("completion").is_none());
    assert!(
        ses.borrow()
            .get(buf)
            .unwrap()
            .buffer
            .text()
            .ends_with("See [[guide]]")
    );
}

#[test]
fn n_buffers_in_n_windows_keep_their_own_state_and_dirty_close_confirms() {
    let ses = session();
    let a = ses.borrow_mut().open("a.md", "alpha\n", None);
    let b = ses.borrow_mut().open("b.md", "beta\n", None);
    let mut wins = Windows::new();
    let wa = wins.open_companion(editor::MODULE, "a.md", "a");
    let wb = wins.open_companion(editor::MODULE, "b.md", "b");
    let mut ra = window(&ses, wa.0, a);
    let mut rb = window(&ses, wb.0, b);
    editor::host::bind(wa, a);
    editor::host::bind(wb, b);

    caret(&mut ra, 5);
    type_str(&mut ra, "!");
    caret(&mut rb, 0);
    assert_eq!(ra.state.body.text, "alpha!\n");
    assert_eq!(rb.state.body.text, "beta\n");
    assert_eq!(ra.state.body.range(), (6, 6));
    assert_eq!(rb.state.body.range(), (0, 0));

    editor::host::sync_dirty_from(&mut wins, &ses.borrow());
    assert_eq!(wins.close(wb, false), CloseOutcome::Closed(vec![wb]));
    assert_eq!(wins.close(wa, false), CloseOutcome::NeedsConfirm(vec![wa]));
    assert!(
        !ses.borrow_mut().close(a, false),
        "dirty buffer survives an unforced close"
    );
    ra.state.undo();
    editor::host::sync_dirty_from(&mut wins, &ses.borrow());
    assert_eq!(wins.close(wa, false), CloseOutcome::Closed(vec![wa]));
}

#[test]
fn save_stages_through_fs_writer_and_clears_dirty() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("p.md"), "one\n").unwrap();
    let ses = session();
    let base = cv_ui_editor::save::fingerprint_file(&dir.path().join("p.md")).unwrap();
    let buf = ses.borrow_mut().open("p.md", "one\n", base);
    let w = EditorWindow::new(WindowId(1), ses.clone(), buf, Theme::dark())
        .with_writer(Arc::new(FsWriter::new(dir.path())));
    let mut rt = editor::runtime(w);
    caret(&mut rt, 3);
    type_str(&mut rt, " two");
    assert!(rt.find("editor-dirty").is_some());
    rt.key(cv_widgets::keys::parse("mod+s").remove(0));
    rt.frame(0.0);
    rt.state.wait_save();
    rt.frame(0.0);
    assert!(
        matches!(rt.state.save_status(), SaveStatus::Saved { .. }),
        "{:?}",
        rt.state.save_status()
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("p.md")).unwrap(),
        "one two\n"
    );
    assert!(rt.find("editor-dirty").is_none());
    assert!(texts(&rt).iter().any(|t| t == "Saved"));
}

#[test]
fn save_without_writer_fails_honestly() {
    let ses = session();
    let buf = ses.borrow_mut().open("p.md", "x", None);
    let mut rt = window(&ses, 1, buf);
    caret(&mut rt, 1);
    type_str(&mut rt, "y");
    click(&mut rt, "editor-save");
    assert!(matches!(rt.state.save_status(), SaveStatus::Failed { .. }));
    assert!(rt.state.is_dirty());
    assert!(texts(&rt).iter().any(|t| t.starts_with("Not saved:")));
}

#[test]
fn ime_composition_commits_into_the_buffer() {
    let ses = session();
    let buf = ses.borrow_mut().open("p.md", "caf", None);
    let mut rt = window(&ses, 1, buf);
    caret(&mut rt, 3);
    rt.ime(Ime::Preedit("e".into(), Some((1, 1)), vec![]));
    rt.frame(0.0);
    assert_eq!(
        ses.borrow().get(buf).unwrap().buffer.text(),
        "caf",
        "preedit is not text yet"
    );
    rt.ime(Ime::Commit("é".into()));
    rt.frame(0.0);
    assert_eq!(ses.borrow().get(buf).unwrap().buffer.text(), "café");
    assert_eq!(ses.borrow().get(buf).unwrap().buffer.len_bytes(), 5);
}

#[test]
fn degraded_inputs_are_disclosed_not_fatal() {
    let ses = session();
    // Unknown key + unparsed line + no completion index.
    let buf = ses
        .borrow_mut()
        .open("p.md", "---\nparentEntry: x\n  nested\n---\n[[gu", None);
    let mut rt = window(&ses, 1, buf);
    assert!(rt.find("fm-note-0").is_some() && rt.find("fm-note-1").is_some());
    let end = rt.state.body.text.len();
    caret(&mut rt, end);
    rt.frame(0.0);
    assert!(
        rt.find("completion").is_none(),
        "no completion.json: no guesses"
    );
    // Body-only file: the form still offers every closed-schema key.
    let plain = ses.borrow_mut().open("q.md", "just text", None);
    let rq = window(&ses, 2, plain);
    assert!(rq.find("fm-title").is_some() && rq.find("fm-yield").is_some());
    assert_eq!(rq.state.body.text, "just text");
    assert!(texts(&rq).iter().any(|t| t == cv_ui_editor::sketch::LABEL));
}

#[test]
fn editor_accessibility_tree_names_each_editable_field() {
    for decorations in [false, true] {
        let ses = session();
        let buf = ses.borrow_mut().open("page.md", &broken_page(), None);
        let mut editor = EditorWindow::new(WindowId(1), ses, buf, Theme::dark());
        editor.decorations = decorations;
        let mut rt = editor::runtime(editor);
        settle(&mut rt);

        let tree = rt.accessibility_tree();
        let assert_field = |key: &str, name: &str, role: Role| {
            let (_, node) = tree
                .nodes
                .iter()
                .find(|(_, node)| node.author_id() == Some(key))
                .unwrap_or_else(|| panic!("{key} missing from the accessibility tree"));
            assert_eq!(node.role(), role, "{key}");
            assert_eq!(node.label(), Some(name), "{key}");
            assert!(
                node.supports_action(Action::Focus),
                "{key} cannot be focused"
            );
            assert!(
                node.supports_action(Action::ReplaceSelectedText),
                "{key} is not editable"
            );
        };

        for spec in cv_ui_editor::frontmatter::SCHEMA {
            assert_field(&format!("fm-{}", spec.key), spec.key, Role::TextInput);
        }
        assert_field("editor-body", "Markdown body", Role::MultilineTextInput);
    }
}

#[test]
fn editor_goldens() {
    let ses = session();
    let buf = ses.borrow_mut().open("page.md", &broken_page(), None);
    let mut rt = window(&ses, 1, buf);
    let (build, diags) = stub_build("broken-wikilink");
    ses.borrow_mut().ingest_build(build, &diags);
    settle(&mut rt);
    let dir = repo().join("fixtures/ui/golden/editor");
    let els = rt.elements();
    let mut errs = vec![];
    for (name, v) in [
        ("broken-wikilink.tree", cv_widgets::tree::element_tree(&els)),
        ("broken-wikilink.layout", cv_widgets::tree::layout(&els)),
    ] {
        if let Err(e) = cv_widgets::harness::check_golden(&dir, name, &v) {
            errs.push(e);
        }
    }
    assert!(errs.is_empty(), "{}", errs.join("\n"));
}

#[test]
fn first_body_edit_after_a_fence_at_eof_keeps_the_frontmatter() {
    let ses = session();
    let buf = ses.borrow_mut().open("p.md", "---\ntitle: P\n---", None);
    let mut rt = window(&ses, 1, buf);
    click(&mut rt, "editor-body");
    type_str(&mut rt, "hi");
    assert_eq!(
        ses.borrow().get(buf).unwrap().buffer.text(),
        "---\ntitle: P\n---\nhi"
    );
    assert_eq!(rt.state.body.text, "hi");
}

#[test]
fn form_field_keeps_a_typed_space_between_words() {
    let ses = session();
    let buf = ses.borrow_mut().open("p.md", "---\ntitle: P\n---\n", None);
    let mut rt = window(&ses, 1, buf);
    click(&mut rt, "fm-summary");
    type_str(&mut rt, "two words");
    assert_eq!(
        ses.borrow().get(buf).unwrap().buffer.text(),
        "---\ntitle: P\nsummary: two words\n---\n"
    );
}

#[test]
fn engine_builds_reach_the_editor_session_through_the_tee() {
    use cv_core::events::engine::{BuildPhase, EngineEvent};
    use cv_core::events::{AppEvent, Cursor, Envelope, EventId, SessionId, SnapshotEpoch};
    use cv_core::id::WorkspaceId;
    let seen = Arc::new(Mutex::new(0));
    let s = seen.clone();
    let mut sink = editor::host::tee_builds(Box::new(move |_| *s.lock().unwrap() += 1));
    sink(Envelope {
        workspace: WorkspaceId("w".into()),
        snapshot_epoch: SnapshotEpoch(0),
        build_id: Some(BuildId("tee-1".into())),
        cursor: Cursor {
            session: SessionId("s".into()),
            seq: 1,
        },
        event_id: EventId("e-1".into()),
        payload: AppEvent::Engine(EngineEvent::BuildFailed {
            phase: BuildPhase::Rebuild,
            diagnostics: vec![],
            recoverable: true,
        }),
    });
    assert_eq!(*seen.lock().unwrap(), 1, "the shell still gets every event");
    assert!(editor::host::ingest_builds());
    let ses = editor::host::session();
    assert_eq!(ses.borrow().current_build(), Some(&BuildId("tee-1".into())));
    assert!(!editor::host::ingest_builds());
}

#[test]
fn first_body_edit_closes_an_unclosed_frontmatter_block() {
    for (open, want) in [
        ("---", "---\n---\nhi"),
        ("---\ntitle: P\n", "---\ntitle: P\n---\nhi"),
    ] {
        let ses = session();
        let buf = ses.borrow_mut().open("p.md", open, None);
        let mut rt = window(&ses, 1, buf);
        click(&mut rt, "editor-body");
        type_str(&mut rt, "hi");
        assert_eq!(ses.borrow().get(buf).unwrap().buffer.text(), want);
        assert_eq!(rt.state.body.text, "hi");
    }
}

#[test]
fn return_in_the_body_breaks_the_line() {
    let ses = session();
    let buf = ses.borrow_mut().open("p.md", "one", None);
    let mut rt = window(&ses, 1, buf);
    caret(&mut rt, 3);
    rt.key(cv_widgets::keys::parse("enter").remove(0));
    settle(&mut rt);
    type_str(&mut rt, "two");
    assert_eq!(ses.borrow().get(buf).unwrap().buffer.text(), "one\ntwo");
}

fn dirty_closed_window() -> (
    Rc<RefCell<EditorSession>>,
    BufferId,
    Runtime<EditorWindow>,
    tempfile::TempDir,
) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("p.md"), "one\n").unwrap();
    let ses = session();
    let base = cv_ui_editor::save::fingerprint_file(&dir.path().join("p.md")).unwrap();
    let buf = ses.borrow_mut().open("p.md", "one\n", base);
    let w = EditorWindow::new(WindowId(7), ses.clone(), buf, Theme::dark())
        .with_writer(Arc::new(FsWriter::new(dir.path())));
    let mut rt = editor::runtime(w);
    caret(&mut rt, 3);
    type_str(&mut rt, " two");
    editor::host::on_closed(&mut rt.state);
    assert_eq!(editor::host::take_reopen(), vec![WindowId(7)]);
    assert!(editor::host::take_closed().is_empty());
    settle(&mut rt);
    assert!(rt.find("editor-close-prompt").is_some());
    (ses, buf, rt, dir)
}

#[test]
fn dirty_close_reopens_asking_and_discard_reverts_then_closes() {
    let (ses, buf, mut rt, dir) = dirty_closed_window();
    click(&mut rt, "editor-close-discard");
    assert_eq!(rt.shell_request, Some(rune::Shell::Close));
    assert!(!ses.borrow().is_dirty(buf));
    assert_eq!(ses.borrow().get(buf).unwrap().buffer.text(), "one\n");
    editor::host::on_closed(&mut rt.state);
    assert_eq!(editor::host::take_closed(), vec![WindowId(7)]);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("p.md")).unwrap(),
        "one\n"
    );
}

#[test]
fn dirty_close_save_and_close_writes_then_closes() {
    let (ses, buf, mut rt, dir) = dirty_closed_window();
    click(&mut rt, "editor-close-save");
    rt.state.wait_save();
    rt.frame(0.0);
    assert_eq!(rt.shell_request, Some(rune::Shell::Close));
    assert!(!ses.borrow().is_dirty(buf));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("p.md")).unwrap(),
        "one two\n"
    );
}

#[test]
fn dirty_close_keep_editing_stays_open_and_dirty() {
    let (ses, buf, mut rt, _dir) = dirty_closed_window();
    click(&mut rt, "editor-close-cancel");
    assert_eq!(rt.shell_request, None);
    assert!(ses.borrow().is_dirty(buf));
    assert!(rt.find("editor-close-prompt").is_none());
}

#[test]
fn clean_close_is_reported_closed() {
    let ses = session();
    let buf = ses.borrow_mut().open("p.md", "one\n", None);
    let mut rt = window(&ses, 9, buf);
    editor::host::on_closed(&mut rt.state);
    assert!(editor::host::take_reopen().is_empty());
    assert_eq!(editor::host::take_closed(), vec![WindowId(9)]);
}

#[test]
fn return_follows_crlf_from_the_frontmatter() {
    let ses = session();
    let buf = ses
        .borrow_mut()
        .open("p.md", "---\r\ntitle: P\r\n---\r\n", None);
    let mut rt = window(&ses, 1, buf);
    click(&mut rt, "editor-body");
    rt.key(cv_widgets::keys::parse("enter").remove(0));
    settle(&mut rt);
    assert_eq!(
        ses.borrow().get(buf).unwrap().buffer.text(),
        "---\r\ntitle: P\r\n---\r\n\r\n"
    );
}
