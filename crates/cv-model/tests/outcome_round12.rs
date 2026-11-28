//! Round-12 causal-replay and retention boundary regressions.

use cv_core::engine::ExitClass;
use cv_core::events::engine::{EngineEvent, VerbReport};
use cv_core::events::{AppEvent, Cursor, Envelope, EventId, SessionId, SnapshotEpoch};
use cv_core::id::WorkspaceId;
use cv_model::module::Registry;
use cv_model::status::StatusNotice;
use cv_model::workbench::{Inbound, Workbench};
use cv_model::workspace_key::WorkspaceKey;

fn key(path: &str, file: u64) -> WorkspaceKey {
    WorkspaceKey::new(std::path::PathBuf::from(path), Some((7, file)))
}

fn install_key(wb: &mut Workbench, path: &str, file: u64) {
    wb.workspace_key = Some(key(path, file));
    wb.workspace = Some(std::path::PathBuf::from(path));
}

fn verb_outcome(id: &str, verb: &str, seq: u64, stderr_bytes: usize) -> Envelope {
    Envelope {
        workspace: WorkspaceId("ws".into()),
        snapshot_epoch: SnapshotEpoch(0),
        build_id: None,
        cursor: Cursor {
            session: SessionId("adoption-gate".into()),
            seq,
        },
        event_id: EventId(id.into()),
        cause: None,
        payload: AppEvent::Engine(EngineEvent::VerbReported(VerbReport {
            verb: verb.into(),
            argv: vec![verb.into()],
            exit: ExitClass::Ok,
            code: Some(0),
            duration_ms: 1,
            stdout: String::new(),
            stderr: "x".repeat(stderr_bytes),
        })),
    }
}

fn titles(wb: &Workbench) -> Vec<String> {
    let i = wb.registry.index_of("activity").expect("activity module");
    let m = wb.registry.get(i).expect("activity module");
    (0..m.list_len(None))
        .map(|r| m.row(None, r).title)
        .collect()
}

fn count_titles(wb: &Workbench, title: &str) -> usize {
    titles(wb).into_iter().filter(|t| t == title).count()
}

/// Mirror the real gate pattern: validate is terminally emitted and visibly
/// committed in generation 1; the generation retires; a causally later live
/// build commits in generation 2; only then the gate journal republishes the
/// validate with its original causal stamp. Correct newest-first Activity
/// keeps validate below (older than) the later build. Reusing the old stamp
/// cannot reorder truth that was already committed in the replacement.
#[test]
fn r12_replayed_pre_reset_outcome_causally_precedes_but_commits_after_live_truth() {
    let validate = verb_outcome("r12-replay-validate", "validate", 1, 0);
    let later_live_build = verb_outcome("r12-live-build-after-reset", "build", 2, 0);
    let mut wb = Workbench::new(Registry::with_all());
    install_key(&mut wb, "ws", 1);
    let outcomes = wb.outcome_sink();
    let validate_stamp = outcomes.send_for(key("ws", 1), validate.clone());
    wb.drain();
    assert_eq!(count_titles(&wb, "validate finished"), 1);

    wb.reset_project(Registry::with_all());
    install_key(&mut wb, "ws", 1);
    wb.inbox().send(Inbound::Envelope(later_live_build));
    wb.drain();
    assert_eq!(count_titles(&wb, "build finished"), 1);
    assert_eq!(count_titles(&wb, "validate finished"), 0);

    outcomes.send_stamped_for(validate_stamp, key("ws", 1), validate);
    wb.drain();
    let rows = titles(&wb);
    let iv = rows.iter().position(|t| t == "validate finished").unwrap();
    let ib = rows.iter().position(|t| t == "build finished").unwrap();
    println!(
        "R12-REPLAY-PRECESSION rows(newest-first)={rows:?} validate_index={iv} build_index={ib}; validate causally preceded build"
    );
    assert!(
        iv > ib,
        "journal replay committed the older validate after the newer live build: {rows:?}"
    );
}

/// Exactly 64 held foreign outcomes is inside the documented bound; 65 must
/// evict exactly one and disclose that count truthfully.
#[test]
fn r12_retention_exactly_64_keeps_all_and_65_evicts_one_with_disclosure() {
    for n in [64usize, 65] {
        let mut wb = Workbench::new(Registry::with_all());
        install_key(&mut wb, "foreign-current", 99);
        let sink = wb.outcome_sink();
        for i in 0..n {
            sink.send_for(
                key("owner", 1),
                verb_outcome(&format!("held-{i:02}"), "validate", i as u64 + 1, 0),
            );
        }
        wb.drain();
        println!("R12-RETENTION-COUNT n={n} status={:?}", wb.status);
        if n == 64 {
            assert_ne!(
                wb.status,
                "discarded 1 held outcome(s): retention is bounded to 64 outcomes or 1048576 bytes"
            );
        } else {
            assert_eq!(
                wb.status,
                "discarded 1 held outcome(s): retention is bounded to 64 outcomes or 1048576 bytes"
            );
        }
        install_key(&mut wb, "owner", 1);
        wb.drain();
        let got = count_titles(&wb, "validate finished");
        println!("R12-RETENTION-COUNT n={n} drained={got}");
        assert_eq!(got, 64, "n={n} must retain exactly the documented boundary");
    }
}

fn retention_bytes(key: &WorkspaceKey, env: &Envelope) -> usize {
    key.path().as_os_str().len() + serde_json::to_vec(env).map(|v| v.len()).unwrap_or(4096) + 64
}

/// The byte bound is documented as about 1 MiB and enforced at exactly
/// 1,048,576 accounted bytes: equality is retained, one more byte evicts.
#[test]
fn r12_retention_exactly_one_mib_is_kept_and_one_byte_over_evicts() {
    const LIMIT: usize = 1024 * 1024;
    let owner = key("owner", 1);
    let probe = verb_outcome("exact-mib", "validate", 1, 0);
    let base = retention_bytes(&owner, &probe);
    let exact_len = LIMIT - base;
    let exact = verb_outcome("exact-mib", "validate", 1, exact_len);
    assert_eq!(retention_bytes(&owner, &exact), LIMIT);

    let mut wb = Workbench::new(Registry::with_all());
    install_key(&mut wb, "foreign-current", 99);
    wb.outcome_sink().send_for(owner.clone(), exact.clone());
    wb.drain();
    println!("R12-RETENTION-BYTES exact_status={:?}", wb.status);
    assert!(
        wb.status.is_empty(),
        "exactly 1 MiB must not evict: {:?}",
        wb.status
    );
    install_key(&mut wb, "owner", 1);
    wb.drain();
    assert_eq!(count_titles(&wb, "validate finished"), 1);

    let over = verb_outcome("exact-mib-plus-tiny", "validate", 2, exact_len);
    let tiny = verb_outcome("tiny-after-exact", "build", 3, 0);
    let mut wb = Workbench::new(Registry::with_all());
    install_key(&mut wb, "foreign-current", 99);
    let sink = wb.outcome_sink();
    sink.send_for(owner.clone(), over);
    sink.send_for(owner.clone(), tiny);
    wb.drain();
    println!("R12-RETENTION-BYTES over_status={:?}", wb.status);
    assert_eq!(
        wb.status,
        "discarded 1 held outcome(s): retention is bounded to 64 outcomes or 1048576 bytes"
    );
    install_key(&mut wb, "owner", 1);
    wb.drain();
    let rows = titles(&wb);
    println!("R12-RETENTION-BYTES over_rows={rows:?}");
    assert_eq!(
        count_titles(&wb, "validate finished"),
        0,
        "oldest oversized outcome should be evicted"
    );
    assert_eq!(count_titles(&wb, "build finished"), 1);
}

/// Causal age is sequence order, not channel-arrival order. A newest stamped
/// outcome that happens to be first in the retention vector must not be the
/// victim merely because older stamped outcomes arrived behind it.
#[test]
fn r12_retention_eviction_uses_causal_age_not_channel_arrival() {
    let mut wb = Workbench::new(Registry::with_all());
    install_key(&mut wb, "foreign-current", 99);
    let sink = wb.outcome_sink();
    // This ordering is reachable when concurrent senders mint ordered stamps
    // but their channel publications interleave. Put the causally newest
    // envelope first in the channel, then 64 causally older envelopes.
    sink.send_stamped_for(
        1000,
        key("owner", 1),
        verb_outcome("newest-build", "build", 1000, 0),
    );
    for i in 0..64 {
        sink.send_stamped_for(
            i + 1,
            key("owner", 1),
            verb_outcome(&format!("older-{i:02}"), "validate", i + 1, 0),
        );
    }
    wb.drain();
    install_key(&mut wb, "owner", 1);
    wb.drain();
    let rows = titles(&wb);
    println!(
        "R12-RETENTION-CAUSAL-AGE rows(newest-first)={rows:?} validate={} build={}",
        count_titles(&wb, "validate finished"),
        count_titles(&wb, "build finished")
    );
    assert_eq!(
        count_titles(&wb, "build finished"),
        1,
        "causally newest outcome was evicted before older outcomes: {rows:?}"
    );
    assert_eq!(count_titles(&wb, "validate finished"), 63);
}

/// A retention disclosure produced while a batch is draining must still be
/// visible when drain returns. Ordinary status messages in the same batch
/// must not overwrite the fact before redraw.
#[test]
fn r12_retention_disclosure_survives_a_batched_boot_status() {
    let mut wb = Workbench::new(Registry::with_all());
    install_key(&mut wb, "foreign-current", 99);
    let sink = wb.outcome_sink();
    for i in 0..65u64 {
        sink.send_for(
            key("owner", 1),
            verb_outcome(&format!("held-{i:02}"), "validate", i + 1, 0),
        );
    }
    wb.inbox()
        .send(Inbound::Status(StatusNotice::engine_evidence(
            "engine ready",
        )));
    wb.drain();
    println!("R12-RETENTION-DISCLOSURE final_status={:?}", wb.status);
    assert!(
        wb.status.contains("discarded 1 held outcome(s)"),
        "retention disclosure was overwritten before redraw; final status: {:?}",
        wb.status
    );
}

/// Positive control for the rename attack: when the vacated spelling is not
/// reused, device+file-index identity should still recognize the moved owner
/// at its new spelling and drain the held outcome.
#[test]
fn r12_moved_owner_positive_control_drains_at_new_path() {
    let outcome = verb_outcome("moved-validate", "validate", 1, 0);
    let mut wb = Workbench::new(Registry::with_all());
    install_key(&mut wb, "foreign-current", 99);
    wb.outcome_sink().send_for(key("old-path", 1), outcome);
    wb.drain();
    assert_eq!(count_titles(&wb, "validate finished"), 0);
    install_key(&mut wb, "new-path", 1);
    wb.drain();
    let got = count_titles(&wb, "validate finished");
    println!("R12-MOVE-POSITIVE validate_rows_at_new_path={got}");
    assert_eq!(got, 1);
}

/// Stress the real concurrent producers: 65 threads mint causal stamps and
/// publish through the uninstrumented OutcomeSink. If publication order and
/// mint order diverge, eviction must still choose the lowest causal stamp as
/// the oldest victim even when publication order diverges.
#[test]
fn r12_concurrent_retention_eviction_victim_is_causally_oldest() {
    use std::sync::{Arc, Barrier, Mutex};
    for rep in 0..80 {
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
        let rows = titles(&wb);
        let sent = sent.lock().unwrap().clone();
        let missing: Vec<(u64, String)> = sent
            .iter()
            .filter(|(_, verb)| !rows.iter().any(|t| t == &format!("{verb} finished")))
            .cloned()
            .collect();
        let min = sent.iter().map(|(seq, _)| *seq).min().unwrap();
        println!(
            "R12-CONCURRENT-EVICTION rep={rep} missing={missing:?} min_seq={min} rows={} unique_stamps={}",
            rows.len(),
            {
                let mut v: Vec<u64> = sent.iter().map(|(seq, _)| *seq).collect();
                v.sort_unstable();
                v.dedup();
                v.len()
            }
        );
        assert_eq!(
            rows.len(),
            64,
            "rep {rep}: exactly one outcome should be evicted"
        );
        assert_eq!(missing.len(), 1, "rep {rep}: exactly one missing verb");
        assert_eq!(
            missing[0].0, min,
            "rep {rep}: eviction victim was not the causally oldest outcome: missing={missing:?} min_seq={min}"
        );
    }
}
