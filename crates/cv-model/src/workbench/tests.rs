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
