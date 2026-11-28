use super::*;

/// Cancellation is the terminal product outcome even though cv-engine also
/// emits `VerbFailed` evidence for Activity before returning its error.
#[cfg(unix)]
#[test]
fn cancelling_a_verb_keeps_cancellation_as_the_terminal_status() {
    let root = tempfile::tempdir().unwrap();
    let bin = root.path().join("boris");
    install_executable(
        &bin,
        r#"#!/bin/sh
case "$1" in
build) sleep 30;;
*) exit 2;;
esac
"#,
    );

    struct DetachOnDrop;
    impl Drop for DetachOnDrop {
        fn drop(&mut self) {
            cv_model::modules::engine::detach();
        }
    }
    let _detach = DetachOnDrop;

    let mut wb = Workbench::new(Registry::with_all());
    let delivery = test_delivery(root.path(), WatchMode::Native, &wb);
    let sched = Arc::new(EngineScheduler::new(
        delivery.engine.clone(),
        SeatPolicy::StopWatch,
    ));
    let slot = EngineSlot::default();
    let _ = slot.sched.set(sched.clone());
    slot.install_verbs(slot.verb_route(delivery.clone(), None), slot.generation());
    cv_model::modules::engine::install(Box::new(SlotVerbs(slot.clone())));

    wb.run_command("engine.build");
    assert_eq!(wb.status, "Building the site…");
    assert!(wb.dialogs.is_open());
    wb.dialog_cancel();
    assert_eq!(wb.status, "Cancelling site build…");

    let deadline = std::time::Instant::now() + Duration::from_secs(4);
    while wb.status == "Cancelling site build…" {
        wb.drain();
        assert!(
            std::time::Instant::now() < deadline,
            "cancelled verb did not finish; status {:?}",
            wb.status
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    wb.drain();

    assert_eq!(wb.status, "Site build cancelled");
    let activity = wb
        .registry
        .get(
            wb.registry
                .index_of(cv_model::modules::activity::ID)
                .unwrap(),
        )
        .unwrap();
    let rows: Vec<_> = (0..activity.list_len(None))
        .map(|i| activity.row(None, i))
        .collect();
    assert!(
        rows.iter()
            .any(|r| r.title == "build failed" && r.detail.contains("cancelled")),
        "cancellation evidence remains inspectable in Activity: {rows:?}"
    );
}
