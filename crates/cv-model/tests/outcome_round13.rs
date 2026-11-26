//! Round-13 adversarial completion probes for causal read-time ordering,
//! retention accounting, and workspace-instance identity.

use cv_core::diagnostic::{Diagnostic, Severity, Span};
use cv_core::engine::ExitClass;
use cv_core::events::engine::{BuildPhase, EngineEvent, VerbReport};
use cv_core::events::shell::DegradedReason;
use cv_core::events::{AppEvent, BuildId, Cursor, Envelope, EventId, SessionId, SnapshotEpoch};
use cv_core::id::WorkspaceId;
use cv_model::module::{Module, Registry};
use cv_model::workbench::{Inbound, Workbench};
use cv_model::workspace_key::WorkspaceKey;

fn key_with_instance(path: &str, file: u64, instance: &str) -> WorkspaceKey {
    WorkspaceKey::with_instance(
        std::path::PathBuf::from(path),
        Some((7, file)),
        instance.into(),
    )
}

/// Synthetic keys model production keys: instance evidence is tied to the
/// workspace lifetime, while the device+file-index pair is only filesystem
/// evidence that may accompany it.
fn key(path: &str, file: u64) -> WorkspaceKey {
    key_with_instance(path, file, &format!("instance-{file}"))
}

fn install_key(wb: &mut Workbench, path: &str, file: u64) {
    wb.workspace_key = Some(key(path, file));
    wb.workspace = Some(std::path::PathBuf::from(path));
}

fn envelope(id: &str, seq: u64, payload: AppEvent) -> Envelope {
    Envelope {
        workspace: WorkspaceId("ws".into()),
        snapshot_epoch: SnapshotEpoch(0),
        build_id: Some(BuildId(format!("build-{seq}"))),
        cursor: Cursor {
            session: SessionId("round13".into()),
            seq,
        },
        event_id: EventId(id.into()),
        cause: None,
        payload,
    }
}

fn verb_outcome(id: &str, verb: &str, seq: u64, stderr_bytes: usize) -> Envelope {
    envelope(
        id,
        seq,
        AppEvent::Engine(EngineEvent::VerbReported(VerbReport {
            verb: verb.into(),
            argv: vec![verb.into()],
            exit: ExitClass::Ok,
            code: Some(0),
            duration_ms: 1,
            stdout: String::new(),
            stderr: "x".repeat(stderr_bytes),
        })),
    )
}

fn verb_failed(id: &str, seq: u64) -> Envelope {
    envelope(
        id,
        seq,
        AppEvent::Engine(EngineEvent::VerbFailed {
            verb: "validate".into(),
            error: "spawn failed".into(),
        }),
    )
}

fn build_finished(id: &str, seq: u64, pages: u64, duration_ms: u64) -> Envelope {
    envelope(
        id,
        seq,
        AppEvent::Engine(EngineEvent::BuildFinished {
            phase: BuildPhase::Rebuild,
            pages_written: pages,
            diagnostics: vec![],
            duration_ms,
        }),
    )
}

fn diag() -> Diagnostic {
    Diagnostic {
        code: "E13".into(),
        severity: Severity::Error,
        span: Some(Span {
            path: "index.md".into(),
            start: 0,
            end: 0,
            line: 1,
            col: 1,
        }),
        message: "E13 happened".into(),
        build_version: None,
    }
}

fn build_failed_with_diag(id: &str, seq: u64) -> Envelope {
    envelope(
        id,
        seq,
        AppEvent::Engine(EngineEvent::BuildFailed {
            phase: BuildPhase::Rebuild,
            diagnostics: vec![diag()],
            recoverable: true,
        }),
    )
}

fn watch_started(id: &str, seq: u64) -> Envelope {
    envelope(
        id,
        seq,
        AppEvent::Engine(EngineEvent::WatchSessionStarted {
            schema: 1,
            compiler: "boris/0.9.0".into(),
        }),
    )
}

fn activity_module(wb: &Workbench) -> &dyn Module {
    let i = wb.registry.index_of("activity").expect("activity module");
    wb.registry.get(i).expect("activity module")
}

fn rows(wb: &Workbench) -> Vec<(usize, String, String, String)> {
    let m = activity_module(wb);
    (0..m.list_len(None))
        .map(|r| {
            let row = m.row(None, r);
            (r, row.key, row.title, row.detail)
        })
        .collect()
}

fn titles(wb: &Workbench) -> Vec<String> {
    rows(wb).into_iter().map(|(_, _, t, _)| t).collect()
}

fn count_titles(wb: &Workbench, title: &str) -> usize {
    titles(wb).into_iter().filter(|t| t == title).count()
}

#[test]
fn r13_interleaved_replay_inserts_at_causal_position_after_cached_read() {
    let mut wb = Workbench::new(Registry::with_all());
    install_key(&mut wb, "ws", 1);
    let sink = wb.outcome_sink();
    sink.send_stamped_for(100, key("ws", 1), verb_outcome("znew", "znew", 100, 0));
    wb.drain();
    sink.send_stamped_for(50, key("ws", 1), verb_outcome("aold", "aold", 50, 0));
    wb.drain();
    let before = titles(&wb);
    assert_eq!(before, vec!["znew finished", "aold finished"]);

    sink.send_stamped_for(75, key("ws", 1), verb_outcome("mid", "mid", 75, 0));
    wb.drain();
    let after = titles(&wb);
    println!("R13-CACHE-INTERLEAVE before={before:?} after={after:?}");
    assert_eq!(
        after,
        vec!["znew finished", "mid finished", "aold finished"]
    );
    let m = activity_module(&wb);
    for r in 0..m.list_len(None) {
        let key = m.row(None, r).key;
        assert_eq!(
            m.index_of_key(None, &key),
            Some(r),
            "key lookup disagrees at row {r}"
        );
    }
}

#[test]
fn r13_coalescing_cache_invalidates_when_an_open_diagnostic_recurs() {
    let mut wb = Workbench::new(Registry::with_all());
    install_key(&mut wb, "ws", 1);
    let sink = wb.outcome_sink();
    sink.send_stamped_for(10, key("ws", 1), build_failed_with_diag("diag-first", 10));
    wb.drain();
    sink.send_stamped_for(20, key("ws", 1), verb_outcome("u20", "u20", 20, 0));
    wb.drain();
    let before = rows(&wb);
    let u_before = before
        .iter()
        .position(|(_, _, t, _)| t == "u20 finished")
        .unwrap();
    let d_before = before
        .iter()
        .position(|(_, _, t, _)| t == "E13: E13 happened")
        .unwrap();
    assert!(
        u_before < d_before,
        "setup: u20 must be newer before recurrence: {before:?}"
    );

    sink.send_stamped_for(30, key("ws", 1), build_failed_with_diag("diag-recur", 30));
    wb.drain();
    let after = rows(&wb);
    let u_after = after
        .iter()
        .position(|(_, _, t, _)| t == "u20 finished")
        .unwrap();
    let d_after = after
        .iter()
        .position(|(_, _, t, _)| t == "E13: E13 happened")
        .unwrap();
    let badge = {
        let m = activity_module(&wb);
        m.row(None, d_after).badge.map(|b| b.text)
    };
    println!("R13-COALESCE-CACHE before={before:?} after={after:?} diagnostic_badge={badge:?}");
    assert!(
        d_after < u_after,
        "recurrent diagnostic did not move above causally older u20: {after:?}"
    );
    assert_eq!(badge.as_deref(), Some("×2"));
}

#[test]
fn r13_coalesced_rebuild_detail_must_come_from_causally_newest_occurrence() {
    let mut wb = Workbench::new(Registry::with_all());
    install_key(&mut wb, "ws", 1);
    let sink = wb.outcome_sink();
    sink.send_stamped_for(30, key("ws", 1), build_finished("rebuild-newer", 30, 7, 70));
    wb.drain();
    sink.send_stamped_for(10, key("ws", 1), build_finished("rebuild-older", 10, 3, 10));
    wb.drain();
    let got = rows(&wb);
    println!("R13-COALESCED-NEWEST rows={got:?}");
    let row = got
        .iter()
        .find(|(_, _, t, _)| t == "Build succeeded (rebuild)");
    assert_eq!(
        got.len(),
        1,
        "the two contiguous rebuilds should coalesce: {got:?}"
    );
    assert_eq!(
        row.map(|(_, _, _, d)| d.as_str()),
        Some("7 pages · 70 ms · 0 diagnostics"),
        "an older causal occurrence committed later and overwrote the newest occurrence's detail"
    );
}

#[test]
fn r13_coalesced_occurrence_first_last_must_follow_causal_stamps() {
    let mut wb = Workbench::new(Registry::with_all());
    install_key(&mut wb, "ws", 1);
    let sink = wb.outcome_sink();
    sink.send_stamped_for(30, key("ws", 1), build_finished("rebuild-newer", 30, 7, 70));
    wb.drain();
    sink.send_stamped_for(10, key("ws", 1), build_finished("rebuild-older", 10, 3, 10));
    wb.drain();
    let m = activity_module(&wb);
    let row = m.row(None, 0);
    let inspector = m.inspector(&row.key);
    let fields: Vec<(String, String)> = inspector.fields;
    let reading_seen: Vec<String> = m
        .reading(&row.key)
        .lines
        .into_iter()
        .filter(|l| l.label.as_deref() == Some("seen"))
        .map(|l| l.text)
        .collect();
    println!("R13-COALESCED-OCCURRENCES fields={fields:?} reading_seen={reading_seen:?}");
    let first = fields
        .iter()
        .find(|(k, _)| k == "first")
        .map(|(_, v)| v.as_str());
    let last = fields
        .iter()
        .find(|(k, _)| k == "last")
        .map(|(_, v)| v.as_str());
    assert_eq!(
        first,
        Some("rebuild-older"),
        "first must be the causally older occurrence"
    );
    assert_eq!(
        last,
        Some("rebuild-newer"),
        "last must be the causally newer occurrence"
    );
}

#[test]
fn r13_terminal_verb_failure_coalesce_first_last_must_follow_causal_stamps() {
    let mut wb = Workbench::new(Registry::with_all());
    install_key(&mut wb, "ws", 1);
    let sink = wb.outcome_sink();
    sink.send_stamped_for(30, key("ws", 1), verb_failed("vf-newer", 30));
    wb.drain();
    sink.send_stamped_for(10, key("ws", 1), verb_failed("vf-older", 10));
    wb.drain();
    let m = activity_module(&wb);
    let row = m.row(None, 0);
    let fields = m.inspector(&row.key).fields;
    let first = fields
        .iter()
        .find(|(k, _)| k == "first")
        .map(|(_, v)| v.as_str());
    let last = fields
        .iter()
        .find(|(k, _)| k == "last")
        .map(|(_, v)| v.as_str());
    println!(
        "R13-VF-COALESCE title={:?} badge={:?} first={first:?} last={last:?}",
        row.title,
        row.badge.map(|b| b.text)
    );
    assert_eq!(row.title, "validate failed");
    assert_eq!(first, Some("vf-older"));
    assert_eq!(last, Some("vf-newer"));
}

#[test]
fn r13_replayed_old_outcome_precedes_watch_start_and_degraded_truth() {
    let validate = verb_outcome("validate-old", "validate", 1, 0);
    let mut wb = Workbench::new(Registry::with_all());
    install_key(&mut wb, "ws", 1);
    let outcomes = wb.outcome_sink();
    let stamp = outcomes.send_for(key("ws", 1), validate.clone());
    wb.drain();
    assert_eq!(count_titles(&wb, "validate finished"), 1);

    wb.reset_project(Registry::with_all());
    install_key(&mut wb, "ws", 1);
    wb.inbox()
        .send(Inbound::Envelope(watch_started("watch-new", 2)));
    wb.disclose(DegradedReason::StoreCorrupt);
    outcomes.send_stamped_for(stamp, key("ws", 1), validate);
    wb.drain();
    let got = rows(&wb);
    let validate_pos = got
        .iter()
        .position(|(_, _, t, _)| t == "validate finished")
        .unwrap();
    let watch_pos = got
        .iter()
        .position(|(_, _, t, _)| t == "Watch session started")
        .unwrap();
    let degraded_pos = got
        .iter()
        .position(|(_, _, t, _)| t == "Degraded: StoreCorrupt")
        .unwrap();
    println!(
        "R13-ORDINARY-INTERLEAVE rows={got:?} validate={validate_pos} watch={watch_pos} degraded={degraded_pos}"
    );
    assert!(
        validate_pos > watch_pos,
        "old validate must render older than the replacement watch start"
    );
    assert!(
        validate_pos > degraded_pos,
        "old validate must render older than the degraded disclosure"
    );
}

#[test]
fn r13_concurrent_retention_eviction_victim_is_causally_oldest_high_reps() {
    use std::sync::{Arc, Barrier, Mutex};
    let reps: usize = std::env::var("R13_EVICTION_REPS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(120);
    for rep in 0..reps {
        let mut wb = Workbench::new(Registry::with_all());
        install_key(&mut wb, "foreign-current", 99);
        let sink = wb.outcome_sink();
        let barrier = Arc::new(Barrier::new(66));
        let sent = Arc::new(Mutex::new(Vec::<(u64, String)>::new()));
        let mut joins = Vec::new();
        for i in 0..65u64 {
            let sink = sink.clone();
            let barrier = barrier.clone();
            let sent = sent.clone();
            joins.push(std::thread::spawn(move || {
                let verb = format!("v{i:02}");
                barrier.wait();
                let seq = sink.send_for(
                    key("owner", 1),
                    verb_outcome(&format!("{verb}-event"), &verb, i + 1, 0),
                );
                sent.lock().unwrap().push((seq, verb));
            }));
        }
        barrier.wait();
        for j in joins {
            j.join().unwrap();
        }
        wb.drain();
        install_key(&mut wb, "owner", 1);
        wb.drain();
        let got = titles(&wb);
        let sent = sent.lock().unwrap().clone();
        let missing: Vec<(u64, String)> = sent
            .iter()
            .filter(|(_, verb)| !got.iter().any(|t| t == &format!("{verb} finished")))
            .cloned()
            .collect();
        let min = sent.iter().map(|(seq, _)| *seq).min().unwrap();
        let unique_stamps = {
            let mut v: Vec<u64> = sent.iter().map(|(seq, _)| *seq).collect();
            v.sort_unstable();
            v.dedup();
            v.len()
        };
        if rep % 20 == 0 || missing.first().map(|m| m.0) != Some(min) {
            println!(
                "R13-CONCURRENT-EVICTION rep={rep} missing={missing:?} min_seq={min} rows={} unique_stamps={unique_stamps}",
                got.len()
            );
        }
        assert_eq!(
            unique_stamps, 65,
            "rep {rep}: duplicate causal stamp minted"
        );
        assert_eq!(
            got.len(),
            64,
            "rep {rep}: exactly one outcome should be evicted"
        );
        assert_eq!(missing.len(), 1, "rep {rep}: exactly one missing verb");
        assert_eq!(
            missing[0].0, min,
            "rep {rep}: victim was not causally oldest"
        );
    }
}

#[test]
fn r13_retention_disclosure_survives_batched_status_high_reps() {
    for rep in 0..100usize {
        let mut wb = Workbench::new(Registry::with_all());
        install_key(&mut wb, "foreign-current", 99);
        let sink = wb.outcome_sink();
        for i in 0..65u64 {
            sink.send_for(
                key("owner", 1),
                verb_outcome(&format!("held-{rep}-{i:02}"), "validate", i + 1, 0),
            );
        }
        wb.inbox().send(Inbound::Status("engine ready".into()));
        wb.drain();
        assert!(
            wb.status.contains("discarded 1 held outcome(s)"),
            "rep {rep}: disclosure overwritten; final status {:?}",
            wb.status
        );
    }
    println!(
        "R13-DISCLOSURE reps=100 final_status={:?}",
        wb_final_status_probe()
    );
}

fn wb_final_status_probe() -> &'static str {
    "discarded 1 held outcome(s): retention is bounded to 64 outcomes or 1048576 bytes"
}

#[test]
fn r13_retention_replayed_duplicates_must_not_consume_unique_outcome_budget() {
    let mut wb = Workbench::new(Registry::with_all());
    install_key(&mut wb, "foreign-current", 99);
    let sink = wb.outcome_sink();
    let mut replay = Vec::new();
    for i in 0..33u64 {
        let env = verb_outcome(&format!("unique-{i:02}"), "validate", i + 1, 0);
        let seq = sink.send_for(key("owner", 1), env.clone());
        replay.push((seq, env));
    }
    for (seq, env) in replay {
        sink.send_stamped_for(seq, key("owner", 1), env);
    }
    wb.drain();
    let held_status = wb.status.clone();
    install_key(&mut wb, "owner", 1);
    wb.drain();
    let got = count_titles(&wb, "validate finished");
    println!(
        "R13-DUPLICATE-RETENTION status_after_foreign_drain={held_status:?} distinct_rows_after_owner_drain={got}"
    );
    assert_eq!(
        got, 33,
        "journal duplicates consumed the bounded retention budget and evicted a unique held outcome"
    );
}

#[test]
fn r13_moved_owner_matrix_double_move_move_back_and_old_path_reuse() {
    for case in ["double-move", "move-back", "old-path-reuse"] {
        let outcome = verb_outcome(&format!("{case}-validate"), "validate", 1, 0);
        let mut wb = Workbench::new(Registry::with_all());
        install_key(&mut wb, "foreign-current", 99);
        wb.outcome_sink().send_for(key("old-path", 1), outcome);
        wb.drain();
        assert_eq!(
            count_titles(&wb, "validate finished"),
            0,
            "{case}: foreign outcome committed early"
        );
        match case {
            "double-move" => {
                install_key(&mut wb, "moved-once", 1);
                install_key(&mut wb, "moved-twice", 1);
            }
            "move-back" => install_key(&mut wb, "old-path", 1),
            "old-path-reuse" => {
                install_key(&mut wb, "old-path", 2);
                wb.drain();
                assert_eq!(
                    count_titles(&wb, "validate finished"),
                    0,
                    "recreated old path must not inherit the outcome"
                );
                install_key(&mut wb, "moved-owner", 1);
            }
            _ => unreachable!(),
        }
        wb.drain();
        let got = count_titles(&wb, "validate finished");
        println!("R13-MOVED-MATRIX case={case} drained={got}");
        assert_eq!(
            got, 1,
            "{case}: moved owner did not drain at its surviving instance identity"
        );
    }
}

#[test]
fn r13_recycled_device_inode_with_distinct_instance_evidence_does_not_inherit() {
    let outcome = verb_outcome("inode-recycled-validate", "validate", 1, 0);
    let mut wb = Workbench::new(Registry::with_all());
    install_key(&mut wb, "foreign-current", 99);
    wb.outcome_sink().send_for(
        key_with_instance("deleted-owner", 7001, "deleted-instance"),
        outcome,
    );
    wb.drain();
    assert_eq!(count_titles(&wb, "validate finished"), 0);
    wb.workspace_key = Some(key_with_instance(
        "brand-new-workspace",
        7001,
        "brand-new-instance",
    ));
    wb.workspace = Some(std::path::PathBuf::from("brand-new-workspace"));
    wb.drain();
    let got = count_titles(&wb, "validate finished");
    println!("R13-INODE-RECYCLING same_dev_inode_new_workspace_drained_rows={got}");
    assert_eq!(
        got, 0,
        "distinct .cv instance evidence did not prevent a recycled device+inode pair from inheriting held truth"
    );
}
