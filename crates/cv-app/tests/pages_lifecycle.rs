//! Headless page lifecycle verbs against a temp copy of the happy corpus.
//! Dialogs are scripted through the shared dialog model; all writes go
//! through the real staged workspace writer behind the fake engine gate.

use cv_app::{WorkspaceEnv, init, open_workspace};
use cv_engine::locate::Locator;
use cv_model::dialog::{DialogKind, Input};
use cv_model::workbench::Workbench;
use std::path::Path;

const SHIM_SH: &str = include_str!("../../cv-engine/tests/fake-boris/boris");
const SHIM_BAT: &str = include_str!("../../cv-engine/tests/fake-boris/boris.bat");

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let dst = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_tree(&e.path(), &dst);
        } else {
            std::fs::copy(e.path(), dst).unwrap();
        }
    }
}

fn workspace() -> tempfile::TempDir {
    let t = tempfile::Builder::new()
        .prefix("cv-pages-")
        .tempdir()
        .unwrap();
    copy_tree(
        &cv_fixtures::fixtures_root().join("corpus-src/happy"),
        t.path(),
    );
    t
}

fn fake() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::Builder::new()
        .prefix("cv-bin-")
        .tempdir()
        .unwrap();
    let (bin, bytes) = if cfg!(windows) {
        (dir.path().join("boris.bat"), SHIM_BAT)
    } else {
        (dir.path().join("boris"), SHIM_SH)
    };
    std::fs::write(&bin, bytes).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let raw =
        std::fs::read_to_string(cv_fixtures::fixtures_root().join("engine-version.txt")).unwrap();
    let build_info = raw
        .lines()
        .find(|l| l.trim_start().starts_with('{'))
        .unwrap();
    std::fs::write(dir.path().join("version.txt"), "boris/0.9.0").unwrap();
    std::fs::write(dir.path().join("build-info.txt"), build_info).unwrap();
    let c = cv_fixtures::corpus("happy");
    std::fs::write(
        dir.path().join("watch.ndjson"),
        c.file("watch/session.ndjson"),
    )
    .unwrap();
    (dir, bin)
}

fn booted() -> (
    tempfile::TempDir,
    tempfile::TempDir,
    tempfile::TempDir,
    Workbench,
) {
    booted_with_outputs(false)
}

fn boot_workspace(
    ws: tempfile::TempDir,
    outputs: bool,
) -> (
    tempfile::TempDir,
    tempfile::TempDir,
    tempfile::TempDir,
    Workbench,
) {
    if outputs {
        copy_tree(
            &cv_fixtures::fixtures_root().join("captures/happy/dist/dist"),
            &ws.path().join("dist"),
        );
    }
    let (bin_dir, bin) = fake();
    let recent = tempfile::tempdir().unwrap();
    let env = WorkspaceEnv {
        locator: Some(Locator {
            env_override: Some(bin),
            ..Locator::default()
        }),
        recent_file: Some(recent.path().join("recent.json")),
    };
    let mut wb = init();
    open_workspace(&mut wb, ws.path().into(), &env)
        .unwrap()
        .join()
        .unwrap();
    wb.drain();
    (ws, bin_dir, recent, wb)
}

fn booted_with_outputs(
    outputs: bool,
) -> (
    tempfile::TempDir,
    tempfile::TempDir,
    tempfile::TempDir,
    Workbench,
) {
    boot_workspace(workspace(), outputs)
}

fn set_text(wb: &mut Workbench, field: usize, value: &str) {
    let d = wb.dialogs.open_mut().expect("dialog open");
    match &mut d.spec.kind {
        DialogKind::Form(fields) => {
            let Input::Text { value: v, .. } = &mut fields[field].input else {
                panic!("field {field} is not text")
            };
            *v = value.into();
        }
        DialogKind::Prompt(f) => {
            let Input::Text { value: v, .. } = &mut f.input else {
                panic!("prompt is not text")
            };
            *v = value.into();
        }
        other => panic!("unexpected dialog kind {other:?}"),
    }
}

fn activity_has(wb: &Workbench, text: &str) -> bool {
    let i = wb.registry.index_of("activity").unwrap();
    let m = wb.registry.get(i).unwrap();
    (0..m.list_len(None)).any(|i| m.row(None, i).title.contains(text))
}

fn wait_for(wb: &mut Workbench, what: &str, mut f: impl FnMut(&Workbench) -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        wb.drain();
        if f(wb) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {what}; status {:?}",
            wb.status
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[test]
fn new_page_form_writes_source_and_reports_activity() {
    let (ws, _bin, _recent, mut wb) = booted();
    wb.run_command("pages.new");
    set_text(&mut wb, 0, "Lifecycle Notes");
    wb.dialog_field(1, Some(1));
    set_text(&mut wb, 2, "guides/lifecycle.md");
    wb.dialog_act("create");
    let text = std::fs::read_to_string(ws.path().join("content/guides/lifecycle.md")).unwrap();
    assert!(text.contains("title: \"Lifecycle Notes\""));
    assert!(text.contains("parent: \"index\""));
    assert!(!text.contains("id:"));
    assert!(wb.status.contains("Page created"), "{}", wb.status);
    assert!(activity_has(&wb, "Page created"));

    wb.run_command("pages.new");
    set_text(&mut wb, 0, "Conflict");
    set_text(&mut wb, 2, "guides/lifecycle.md");
    wb.dialog_act("create");
    let d = wb.dialogs.open().expect("conflict re-asked");
    assert!(d.spec.error.as_deref().unwrap().contains("already exists"));
}

#[test]
fn rename_is_the_single_user_facing_rename_move_command() {
    let (_ws, _bin, _recent, wb) = booted();
    let commands = wb.commands();
    assert!(commands.iter().any(|c| c.id == "pages.rename"));
    assert!(commands.iter().all(|c| c.id != "pages.move"));
    assert!(
        commands
            .iter()
            .find(|c| c.id == "pages.rename")
            .is_some_and(|c| c.title.contains("Rename/move"))
    );
}

#[test]
fn degraded_research_storage_does_not_remove_page_lifecycle_writes() {
    let ws = workspace();
    std::fs::create_dir_all(ws.path().join(".cv")).unwrap();
    std::fs::write(ws.path().join(".cv/research"), b"not a dir").unwrap();
    let (ws, _bin, _recent, mut wb) = boot_workspace(ws, false);
    assert!(
        wb.status.contains("research notes not persisted"),
        "{}",
        wb.status
    );

    wb.run_command("pages.new");
    set_text(&mut wb, 0, "Lifecycle During Degraded Research");
    wb.dialog_field(1, Some(1));
    set_text(&mut wb, 2, "guides/degraded-research.md");
    wb.dialog_act("create");
    assert!(
        ws.path()
            .join("content/guides/degraded-research.md")
            .is_file()
    );
    assert!(wb.status.contains("Page created"), "{}", wb.status);
}

#[test]
fn rename_move_pins_stable_id_keeps_selection_and_invalidates_a_clean_editor() {
    let (ws, _bin, _recent, mut wb) = booted();
    let old = "guides/getting-started.md";
    let new = "guides/renamed.md";
    cv_app::editor::open_page(&mut wb, ws.path(), "content/guides/getting-started.md").unwrap();
    wb.select_key(old);
    let before = wb.windows.primary.selection.noun.clone().unwrap();
    wb.run_command("pages.rename");
    set_text(&mut wb, 0, new);
    wb.dialog_act("move");

    assert!(!ws.path().join("content").join(old).exists());
    let moved = std::fs::read_to_string(ws.path().join("content").join(new)).unwrap();
    assert!(moved.contains("id: \"guides/getting-started\""), "{moved}");
    assert_eq!(wb.selected_key(), Some(new));
    assert_eq!(wb.windows.primary.selection.noun, Some(before));
    assert!(wb.status.contains("Page moved"), "{}", wb.status);
    assert!(activity_has(&wb, "Page moved"));
    let ses = cv_widgets::editor::host::session();
    assert!(
        ses.borrow()
            .find("content/guides/getting-started.md")
            .is_none()
    );
}

#[test]
fn rename_preserves_a_snooze_through_the_pinned_stable_id() {
    let (ws, bin_dir, recent, mut wb) = booted_with_outputs(true);
    wait_for(&mut wb, "initial build", |wb| {
        activity_has(wb, "Build succeeded")
    });
    let audit = wb.registry.index_of("audit").unwrap();
    wb.activate(audit);
    let key = {
        let m = wb.registry.get(audit).unwrap();
        (0..m.list_len(Some("open")))
            .map(|i| m.row(Some("open"), i).key)
            .find(|k| k.starts_with("SEO-CANONICAL-PRESENT|page:guides/getting-started|"))
            .unwrap_or_else(|| {
                let m = wb.registry.get(audit).unwrap();
                let keys: Vec<_> = (0..m.list_len(Some("open")))
                    .map(|i| m.row(Some("open"), i).key)
                    .collect();
                panic!("getting-started canonical finding missing; open keys: {keys:?}")
            })
    };
    wb.select_key(&key);
    wb.run_command("audit.snooze");
    assert!(wb.status.ends_with("snoozed"), "{}", wb.status);

    let pages = wb.registry.index_of("pages").unwrap();
    wb.activate(pages);
    wb.select_key("guides/getting-started.md");
    wb.run_command("pages.rename");
    set_text(&mut wb, 0, "guides/renamed.md");
    wb.dialog_act("move");
    assert!(wb.status.contains("Page moved"), "{}", wb.status);

    // Real Boris writes the next HTML at the pinned stable entity id
    // (`guides/getting-started.html`), not at the renamed source path. The
    // captured output tree therefore needs no route rewrite here.
    let bin = bin_dir
        .path()
        .join(if cfg!(windows) { "boris.bat" } else { "boris" });
    let env = WorkspaceEnv {
        locator: Some(Locator {
            env_override: Some(bin),
            ..Locator::default()
        }),
        recent_file: Some(recent.path().join("recent.json")),
    };
    open_workspace(&mut wb, ws.path().into(), &env)
        .unwrap()
        .join()
        .unwrap();
    wait_for(&mut wb, "post-rename build", |wb| {
        activity_has(wb, "Build succeeded")
    });

    let audit = wb.registry.index_of("audit").unwrap();
    let m = wb.registry.get(audit).unwrap();
    assert!(
        (0..m.list_len(Some("snoozed"))).any(|i| m.row(Some("snoozed"), i).key == key),
        "snooze keyed by the pinned StableId did not survive rename"
    );
}

#[test]
fn reparent_choose_edits_only_the_parent_line() {
    let (ws, _bin, _recent, mut wb) = booted();
    let path = ws.path().join("content/guides/publishing.md");
    let before = std::fs::read_to_string(&path).unwrap();
    wb.select_key("guides/publishing.md");
    wb.run_command("pages.reparent");
    wb.dialog_choose(0);
    let after = std::fs::read_to_string(&path).unwrap();
    assert!(!after.contains("parent:"), "{after}");
    assert_eq!(after, before.replacen("parent: index\n", "", 1));
    assert!(wb.status.contains("Page reparented"), "{}", wb.status);
    assert!(activity_has(&wb, "Page reparented"));
}

#[test]
fn delete_confirm_lists_impact_and_removes_the_source() {
    let (ws, _bin, _recent, mut wb) = booted();
    let ir = ws.path().join(".cv/ir");
    std::fs::create_dir_all(&ir).unwrap();
    std::fs::copy(
        cv_fixtures::fixtures_root().join("ir/captures/happy/graph.json"),
        ir.join("graph.json"),
    )
    .unwrap();
    wb.load_snapshot(&cv_app::snapshot::workspace_snapshot(
        ws.path(),
        "dist",
        None,
        2,
    ));
    wb.select_key("guides/getting-started.md");
    wb.run_command("pages.delete");
    let body = &wb
        .dialogs
        .open()
        .unwrap()
        .spec
        .body
        .clone()
        .unwrap_or_default();
    assert!(body.contains("Inbound [[links]]"), "{body}");
    wb.dialog_act("delete");
    assert!(!ws.path().join("content/guides/getting-started.md").exists());
    assert!(wb.status.contains("Page deleted"), "{}", wb.status);
    assert!(activity_has(&wb, "Page deleted"));
}

#[test]
fn rename_target_exists_reasks_without_overwriting() {
    let (ws, _bin, _recent, mut wb) = booted();
    let old = "guides/getting-started.md";
    let target = "guides/publishing.md";
    let target_before = std::fs::read(ws.path().join("content").join(target)).unwrap();
    wb.select_key(old);
    wb.run_command("pages.rename");
    set_text(&mut wb, 0, target);
    wb.dialog_act("move");

    let d = wb.dialogs.open().expect("conflict re-asked");
    assert!(
        d.spec.error.as_deref().unwrap().contains("already exists"),
        "{:?}",
        d.spec.error
    );
    assert!(ws.path().join("content").join(old).exists());
    assert_eq!(
        std::fs::read(ws.path().join("content").join(target)).unwrap(),
        target_before
    );
}

#[test]
fn rename_stale_source_reasks_without_moving() {
    let (ws, _bin, _recent, mut wb) = booted();
    let old = "guides/getting-started.md";
    let new = "guides/stale-target.md";
    wb.select_key(old);
    wb.run_command("pages.rename");
    let disk = ws.path().join("content").join(old);
    std::fs::write(&disk, "changed elsewhere\n").unwrap();
    set_text(&mut wb, 0, new);
    wb.dialog_act("move");

    let d = wb.dialogs.open().expect("stale base re-asked");
    assert!(
        d.spec.error.as_deref().unwrap().contains("changed on disk"),
        "{:?}",
        d.spec.error
    );
    assert_eq!(
        std::fs::read_to_string(&disk).unwrap(),
        "changed elsewhere\n"
    );
    assert!(!ws.path().join("content").join(new).exists());
}

#[test]
fn delete_without_boris_ir_discloses_unknown_impact() {
    let (_ws, _bin, _recent, mut wb) = booted();
    wb.select_key("guides/getting-started.md");
    wb.run_command("pages.delete");
    let body = &wb
        .dialogs
        .open()
        .unwrap()
        .spec
        .body
        .clone()
        .unwrap_or_default();
    assert!(body.contains("does not cover this page yet"), "{body}");
    assert!(body.contains("unknown"), "{body}");
}

#[test]
fn a_dirty_editor_refuses_rename_without_touching_disk() {
    let (ws, _bin, _recent, mut wb) = booted();
    cv_app::editor::open_page(&mut wb, ws.path(), "content/guides/getting-started.md").unwrap();
    let ses = cv_widgets::editor::host::session();
    let id = ses
        .borrow()
        .find("content/guides/getting-started.md")
        .unwrap();
    ses.borrow_mut()
        .get_mut(id)
        .unwrap()
        .buffer
        .insert(0, "dirty")
        .unwrap();
    drop(ses);
    let path = ws.path().join("content/guides/getting-started.md");
    let before = std::fs::read(&path).unwrap();

    wb.select_key("guides/getting-started.md");
    wb.run_command("pages.rename");
    set_text(&mut wb, 0, "guides/dirty-rename.md");
    wb.dialog_act("move");

    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(!ws.path().join("content/guides/dirty-rename.md").exists());
    assert!(
        wb.status.contains("save or close the editor"),
        "{}",
        wb.status
    );
    assert!(activity_has(&wb, "Page rename failed"));
}

#[test]
fn dirty_editor_case_variant_refuses_rename_where_the_filesystem_aliases_case() {
    let (ws, _bin, _recent, mut wb) = booted();
    let aliased = ws.path().join("Content/guides/getting-started.md");
    if !aliased.exists() {
        // Linux and case-sensitive volumes correctly treat these as
        // distinct paths; there is no alias for this test to exercise.
        return;
    }
    cv_app::editor::open_page(&mut wb, ws.path(), "Content/guides/getting-started.md").unwrap();
    let ses = cv_widgets::editor::host::session();
    let id = ses
        .borrow()
        .find("Content/guides/getting-started.md")
        .unwrap();
    ses.borrow_mut()
        .get_mut(id)
        .unwrap()
        .buffer
        .insert(0, "dirty")
        .unwrap();
    drop(ses);
    let path = ws.path().join("content/guides/getting-started.md");
    let before = std::fs::read(&path).unwrap();

    wb.select_key("guides/getting-started.md");
    wb.run_command("pages.rename");
    set_text(&mut wb, 0, "guides/case-dirty-rename.md");
    wb.dialog_act("move");

    assert!(
        wb.status.contains("save or close the editor"),
        "{}",
        wb.status
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(
        !ws.path()
            .join("content/guides/case-dirty-rename.md")
            .exists()
    );
}

#[test]
fn clean_editor_case_variant_is_invalidated_by_rename_where_case_aliases() {
    let (ws, _bin, _recent, mut wb) = booted();
    if !ws.path().join("Content/guides/getting-started.md").exists() {
        return;
    }
    cv_app::editor::open_page(&mut wb, ws.path(), "Content/guides/getting-started.md").unwrap();
    wb.select_key("guides/getting-started.md");
    wb.run_command("pages.rename");
    set_text(&mut wb, 0, "guides/case-clean-rename.md");
    wb.dialog_act("move");
    assert!(wb.status.contains("Page moved"), "{}", wb.status);
    let ses = cv_widgets::editor::host::session();
    assert!(
        ses.borrow()
            .find("Content/guides/getting-started.md")
            .is_none()
    );
}

#[test]
fn clean_editor_case_variant_reloads_after_reparent_where_case_aliases() {
    let (ws, _bin, _recent, mut wb) = booted();
    if !ws.path().join("Content/guides/publishing.md").exists() {
        return;
    }
    cv_app::editor::open_page(&mut wb, ws.path(), "Content/guides/publishing.md").unwrap();
    wb.select_key("guides/publishing.md");
    wb.run_command("pages.reparent");
    wb.dialog_choose(0);
    assert!(wb.status.contains("Page reparented"), "{}", wb.status);
    let ses = cv_widgets::editor::host::session();
    let id = ses.borrow().find("Content/guides/publishing.md").unwrap();
    let text = ses.borrow().get(id).unwrap().buffer.text();
    assert!(!text.contains("parent: index"), "{text}");
}

#[test]
fn delete_stale_source_reasks_without_removing() {
    let (ws, _bin, _recent, mut wb) = booted();
    let rel = "guides/publishing.md";
    wb.select_key(rel);
    wb.run_command("pages.delete");
    let disk = ws.path().join("content").join(rel);
    std::fs::write(&disk, "changed elsewhere\n").unwrap();
    wb.dialog_act("delete");

    let d = wb.dialogs.open().expect("stale delete re-asked");
    assert!(
        d.spec.error.as_deref().unwrap().contains("changed on disk"),
        "{:?}",
        d.spec.error
    );
    assert_eq!(
        std::fs::read_to_string(&disk).unwrap(),
        "changed elsewhere\n"
    );
}

#[test]
fn a_dirty_editor_refuses_delete_without_touching_disk() {
    let (ws, _bin, _recent, mut wb) = booted();
    cv_app::editor::open_page(&mut wb, ws.path(), "content/guides/publishing.md").unwrap();
    let ses = cv_widgets::editor::host::session();
    let id = ses.borrow().find("content/guides/publishing.md").unwrap();
    ses.borrow_mut()
        .get_mut(id)
        .unwrap()
        .buffer
        .insert(0, "dirty")
        .unwrap();
    drop(ses);
    let path = ws.path().join("content/guides/publishing.md");
    let before = std::fs::read(&path).unwrap();

    wb.select_key("guides/publishing.md");
    wb.run_command("pages.delete");
    wb.dialog_act("delete");

    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(
        wb.status.contains("save or close the editor"),
        "{}",
        wb.status
    );
    assert!(activity_has(&wb, "Page delete failed"));
}

#[test]
fn a_dirty_editor_refuses_reparent_without_touching_disk() {
    let (ws, _bin, _recent, mut wb) = booted();
    cv_app::editor::open_page(&mut wb, ws.path(), "content/guides/publishing.md").unwrap();
    let ses = cv_widgets::editor::host::session();
    let id = ses.borrow().find("content/guides/publishing.md").unwrap();
    ses.borrow_mut()
        .get_mut(id)
        .unwrap()
        .buffer
        .insert(0, "dirty")
        .unwrap();
    drop(ses);
    let path = ws.path().join("content/guides/publishing.md");
    let before = std::fs::read(&path).unwrap();

    wb.select_key("guides/publishing.md");
    wb.run_command("pages.reparent");
    wb.dialog_choose(0);

    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(
        wb.status.contains("save or close the editor"),
        "{}",
        wb.status
    );
    assert!(activity_has(&wb, "Page reparent failed"));
}
