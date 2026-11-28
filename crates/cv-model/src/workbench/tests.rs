use super::*;
use crate::modules::migrate::{self, ImportRun, Imports, MemoryPort, RunStatus};

#[test]
fn palette_enablement_tracks_row_selection_before_any_command_runs() {
    let mut run = ImportRun::new("20261127-hugo", RunStatus::Ok);
    run.mode = "hugo".into();
    run.source = "mini-hugo".into();
    migrate::install(Box::new(MemoryPort {
        imports: Imports {
            runs: vec![run],
            ..Imports::default()
        },
        records: Vec::new(),
    }));
    let mut wb = Workbench::new(Registry::with_all());
    let migrate_index = wb.registry.index_of(migrate::ID).unwrap();
    wb.activate(migrate_index);
    wb.select_row(0);

    let adopt = wb
        .commands()
        .into_iter()
        .find(|c| c.id == migrate::ADOPT)
        .unwrap();
    assert_eq!(
        adopt.disabled, None,
        "the palette must use the row selection visible in the list"
    );
}

#[test]
fn watch_restart_is_offered_only_while_the_watch_is_dead() {
    let mut wb = Workbench::new(Registry::with_all());
    assert!(wb.commands().iter().all(|c| c.id != "watch.restart"));
    wb.disclose(DegradedReason::WatchDead);
    assert!(wb.commands().iter().any(|c| c.id == "watch.restart"));
    wb.apply(Intent::Recover);
    assert!(wb.commands().iter().all(|c| c.id != "watch.restart"));
}

#[test]
fn watch_restart_reopens_the_workspace() {
    let mut wb = Workbench::new(Registry::with_all());
    wb.workspace = Some(PathBuf::from("/ws"));
    wb.run_command("watch.restart");
    assert_eq!(
        wb.requests,
        vec![ShellRequest::OpenWorkspace(PathBuf::from("/ws"))]
    );
}

#[test]
fn watch_restart_without_a_workspace_only_notes_the_status() {
    let mut wb = Workbench::new(Registry::with_all());
    wb.run_command("watch.restart");
    assert!(wb.requests.is_empty());
    assert_eq!(wb.status, "no workspace open");
}

struct DropWatch {
    session: SessionId,
    dropped: Arc<std::sync::atomic::AtomicBool>,
}

impl WatchHandle for DropWatch {
    fn session(&self) -> &SessionId {
        &self.session
    }
    fn stop(&mut self) {}
}

impl Drop for DropWatch {
    fn drop(&mut self) {
        self.dropped
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

#[test]
fn failed_try_send_returns_the_watch_message_instead_of_dropping_it() {
    let mut wb = Workbench::new(Registry::with_all());
    let inbox = wb.inbox();
    wb.reset_project(Registry::default());
    let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let message = inbox
        .try_send(Inbound::Watch(Box::new(DropWatch {
            session: SessionId("dead-generation".into()),
            dropped: dropped.clone(),
        })))
        .expect_err("the reset generation refuses the send");
    assert!(
        !dropped.load(std::sync::atomic::Ordering::SeqCst),
        "try_send must not drop the cleanup-owning message inside the caller's critical section"
    );
    drop(message);
    assert!(dropped.load(std::sync::atomic::Ordering::SeqCst));
}

fn primary_modifiers() -> Modifiers {
    Modifiers::default().with_primary(true)
}

fn command(c: char, modifiers: Modifiers) -> KeyPress {
    KeyPress::with_modifiers(KeyInput::Command(c), modifiers)
}

fn open_palette_in_primary_hold(wb: &mut Workbench) -> Modifiers {
    let primary = primary_modifiers();
    wb.modifiers(primary);
    wb.key(&command('k', primary));
    assert!(wb.palette.open);
    primary
}

#[test]
fn continuous_primary_hold_rollover_types_leading_palette_chars() {
    let mut wb = Workbench::new(Registry::with_all());
    let primary = open_palette_in_primary_hold(&mut wb);
    wb.key(&command('b', primary));
    wb.key(&command('u', primary));
    wb.modifiers(Modifiers::default());
    for c in "ild".chars() {
        wb.key(&KeyPress::new(KeyInput::Char(c)));
    }
    assert_eq!(wb.palette.query, "build");
}

#[test]
fn release_then_repress_makes_a_later_command_chord_inert_in_the_palette() {
    let mut wb = Workbench::new(Registry::with_all());
    let primary = open_palette_in_primary_hold(&mut wb);
    wb.modifiers(Modifiers::default());
    wb.modifiers(primary);
    let inspector = wb.inspector_open;
    assert!(!wb.key(&command('i', primary)));
    assert_eq!(wb.palette.query, "");
    assert_eq!(wb.inspector_open, inspector);
    assert!(wb.palette.open);
}

#[test]
fn alt_or_counterpart_command_modifier_never_turns_a_chord_into_palette_text() {
    for extra in [true, false] {
        let mut wb = Workbench::new(Registry::with_all());
        let mut modified = open_palette_in_primary_hold(&mut wb);
        if extra {
            modified.alt = true;
        } else {
            modified.ctrl = true;
            modified.logo = true;
        }
        assert!(!wb.key(&command('i', modified)));
        assert_eq!(wb.palette.query, "", "extra modifier case {extra}");
    }
}

#[test]
fn shifted_command_chords_do_not_type_lowercase_during_rollover() {
    let mut wb = Workbench::new(Registry::with_all());
    let mut shifted = open_palette_in_primary_hold(&mut wb);
    shifted.shift = true;
    assert!(!wb.key(&command('i', shifted)));
    assert_eq!(wb.palette.query, "");
}

#[test]
fn command_k_still_closes_the_palette_during_a_rollover_hold() {
    let mut wb = Workbench::new(Registry::with_all());
    let primary = open_palette_in_primary_hold(&mut wb);
    assert!(wb.key(&command('k', primary)));
    assert!(!wb.palette.open);
}
