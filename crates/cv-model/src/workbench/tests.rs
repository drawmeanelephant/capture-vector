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
fn bookkeeping_never_overwrites_an_operation_outcome() {
    migrate::install(Box::new(MemoryPort {
        imports: Imports::default(),
        records: Vec::new(),
    }));
    let mut wb = Workbench::new(Registry::with_all());
    wb.outcome_status("adopted 2 pages and 1 asset from run-1 into content/imported/site");

    wb.run_command(migrate::REFRESH);

    assert_eq!(
        wb.status,
        "adopted 2 pages and 1 asset from run-1 into content/imported/site"
    );
    assert_eq!(wb.status_kind(), Some(crate::status::StatusKind::Outcome));
}

#[test]
fn progress_for_a_new_operation_replaces_an_old_outcome_and_holds_against_ambient_notices() {
    let mut wb = Workbench::new(Registry::with_all());
    wb.outcome_status("Import finished; the run is in Runs");

    assert!(wb.progress_status("Building the site…"));
    assert_eq!(wb.status, "Building the site…");
    assert_eq!(wb.status_kind(), Some(crate::status::StatusKind::Progress));

    assert!(!wb.bookkeeping_status("3 import run(s) loaded"));
    assert!(!wb.evidence_status("boris build finished (exit 0, 12 ms)"));
    assert_eq!(wb.status, "Building the site…");

    assert!(wb.outcome_status("Site build finished"));
    assert_eq!(wb.status, "Site build finished");
    assert_eq!(wb.status_kind(), Some(crate::status::StatusKind::Outcome));
}

#[test]
fn engine_evidence_for_a_new_operation_can_replace_an_old_outcome() {
    let mut wb = Workbench::new(Registry::with_all());
    wb.outcome_status("Import finished; the run is in Runs");

    let operation = wb.begin_status_operation();
    assert!(wb.set_status_notice(StatusNotice::operation_evidence(
        operation,
        "Boris watch stopped · running one build for diagnostics…"
    )));
    assert_eq!(
        wb.status,
        "Boris watch stopped · running one build for diagnostics…"
    );
    assert_eq!(
        wb.status_kind(),
        Some(crate::status::StatusKind::EngineEvidence)
    );
}

#[test]
fn terminal_operation_evidence_completes_opening_progress_while_ambient_evidence_stays_out() {
    let mut wb = Workbench::new(Registry::with_all());
    wb.outcome_status("Import finished; the run is in Runs");
    let operation = wb.begin_status_operation();

    assert!(wb.progress_status("Opening workspace /site…"));
    assert!(!wb.evidence_status("boris build finished (exit 0, 12 ms)"));
    assert_eq!(wb.status, "Opening workspace /site…");

    assert!(
        wb.set_status_notice(StatusNotice::terminal_operation_evidence(
            operation,
            "/site · boris/0.9.0 (Path) · watch"
        ))
    );
    assert_eq!(wb.status, "/site · boris/0.9.0 (Path) · watch");
    assert_eq!(
        wb.status_kind(),
        Some(crate::status::StatusKind::EngineEvidence)
    );

    assert!(
        !wb.set_status_notice(StatusNotice::terminal_operation_evidence(
            operation,
            "stale boot evidence"
        ))
    );
    assert!(wb.evidence_status("Boris one-shot build succeeded · reopen workspace"));
    assert_eq!(
        wb.status, "Boris one-shot build succeeded · reopen workspace",
        "terminal evidence must close the opening operation so later engine truth can land"
    );
}

#[test]
fn intermediate_operation_evidence_does_not_close_the_operation_boundary() {
    let mut wb = Workbench::new(Registry::with_all());
    wb.outcome_status("Import finished; the run is in Runs");
    let operation = wb.begin_status_operation();

    assert!(wb.set_status_notice(StatusNotice::operation_evidence(
        operation,
        "research notes not persisted: locked"
    )));
    assert_eq!(wb.status, "research notes not persisted: locked");

    assert!(
        wb.set_status_notice(StatusNotice::terminal_operation_evidence(
            operation,
            "research notes not persisted: locked · /site: boris not found"
        ))
    );
    assert_eq!(
        wb.status,
        "research notes not persisted: locked · /site: boris not found"
    );
}

#[test]
fn ambient_engine_evidence_does_not_overwrite_a_fresh_outcome() {
    let mut wb = Workbench::new(Registry::with_all());
    wb.outcome_status("Import finished; the run is in Runs");

    assert!(!wb.evidence_status("boris build finished (exit 0, 12 ms)"));
    assert_eq!(wb.status, "Import finished; the run is in Runs");
}

#[test]
fn ambient_notices_replace_one_another_when_no_foreground_status_is_live() {
    let mut wb = Workbench::new(Registry::with_all());

    assert!(wb.bookkeeping_status("3 import run(s) loaded"));
    assert!(wb.evidence_status("boris build finished (exit 0, 12 ms)"));
    assert_eq!(wb.status, "boris build finished (exit 0, 12 ms)");
    assert!(wb.bookkeeping_status("4 import run(s) loaded"));
    assert_eq!(wb.status, "4 import run(s) loaded");
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
