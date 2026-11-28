//! Round-14 workspace-copy and evidence-corruption ownership probes.

use cv_app::slots;
use cv_core::engine::ExitClass;
use cv_core::events::engine::{EngineEvent, VerbReport};
use cv_core::events::shell::DegradedReason;
use cv_core::events::{AppEvent, Cursor, Envelope, EventId, SessionId, SnapshotEpoch};
use cv_core::id::WorkspaceId;
use cv_model::module::Registry;
use cv_model::status::StatusNotice;
use cv_model::workbench::Workbench;
use cv_model::workspace_key::WorkspaceKey;
use cv_store::workspace_instance::FILE;
use std::path::Path;

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        let ty = entry.file_type().unwrap();
        if ty.is_dir() {
            copy_tree(&entry.path(), &target);
        } else if ty.is_file() {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn validate_outcome(id: &str) -> Envelope {
    Envelope {
        workspace: WorkspaceId("ws".into()),
        snapshot_epoch: SnapshotEpoch(0),
        build_id: None,
        cursor: Cursor {
            session: SessionId("round14".into()),
            seq: 1,
        },
        event_id: EventId(id.into()),
        cause: None,
        payload: AppEvent::Engine(EngineEvent::VerbReported(VerbReport {
            verb: "validate".into(),
            argv: vec!["validate".into()],
            exit: ExitClass::Ok,
            code: Some(0),
            duration_ms: 1,
            stdout: String::new(),
            stderr: String::new(),
        })),
    }
}

fn validate_rows(wb: &Workbench) -> usize {
    let i = wb.registry.index_of("activity").expect("activity module");
    let m = wb.registry.get(i).expect("activity module");
    (0..m.list_len(None))
        .filter(|r| m.row(None, *r).title == "validate finished")
        .count()
}

fn install_key(wb: &mut Workbench, root: &Path, key: WorkspaceKey) {
    wb.workspace = Some(root.to_path_buf());
    wb.workspace_key = Some(key);
}

#[test]
fn r14_byte_copy_of_workspace_must_not_share_lifetime_or_outcomes() {
    let base = tempfile::tempdir().unwrap();
    let original = base.path().join("original");
    let copy = base.path().join("copy");
    std::fs::create_dir_all(&original).unwrap();
    let owner_key = slots::workspace_key(&original);
    let owner_token = std::fs::read_to_string(original.join(FILE)).unwrap();

    // Finder/`cp -r` style duplication copies the hidden `.cv` sidecar too.
    copy_tree(&original, &copy);
    let copy_token = std::fs::read_to_string(copy.join(FILE)).unwrap();
    let copy_key = slots::workspace_key(&copy);
    let same = owner_key.same_instance(&copy_key);

    let _original_slot = slots::slot_or_register(&original);
    let copy_sees_original_slot = slots::slot_for(&copy).is_some();

    let mut wb = Workbench::new(Registry::with_all());
    install_key(&mut wb, &copy, copy_key.clone());
    wb.outcome_sink()
        .send_for(owner_key.clone(), validate_outcome("copy-owned-validate"));
    wb.drain();
    let copied_rows = validate_rows(&wb);
    println!(
        "R14-COPY-OWNERSHIP same_instance={same} copy_sees_original_slot={copy_sees_original_slot} outcome_rows_in_copy={copied_rows} owner_token={} copy_token={} owner_file={:?} copy_file={:?}",
        owner_token.trim(),
        copy_token.trim(),
        cv_engine::file_identity::identity(&original),
        cv_engine::file_identity::identity(&copy)
    );
    if same || copy_sees_original_slot || copied_rows != 0 {
        panic!(
            "workspace copy falsely shares lifetime: same_instance={same}, slot_leak={copy_sees_original_slot}, foreign_outcomes_committed={copied_rows}"
        );
    }
}

#[test]
fn r14_workspace_move_positive_control_still_shares_lifetime() {
    let base = tempfile::tempdir().unwrap();
    let original = base.path().join("original-move");
    let moved = base.path().join("moved");
    std::fs::create_dir_all(&original).unwrap();
    let owner_key = slots::workspace_key(&original);
    std::fs::rename(&original, &moved).unwrap();
    let moved_key = slots::workspace_key(&moved);
    let same = owner_key.same_instance(&moved_key);
    println!("R14-MOVE-CONTROL same_instance_after_rename={same}");
    assert!(same, "rename must preserve workspace lifetime");
}

#[test]
fn r14_deleted_instance_remint_strands_old_outcomes_conservatively() {
    let base = tempfile::tempdir().unwrap();
    let owner = base.path().join("owner");
    let foreign = base.path().join("foreign");
    std::fs::create_dir_all(&owner).unwrap();
    std::fs::create_dir_all(&foreign).unwrap();
    let owner_key = slots::workspace_key(&owner);
    let first_token = std::fs::read_to_string(owner.join(FILE)).unwrap();
    let foreign_key = slots::workspace_key(&foreign);

    let mut wb = Workbench::new(Registry::with_all());
    install_key(&mut wb, &foreign, foreign_key);
    wb.outcome_sink()
        .send_for(owner_key, validate_outcome("deleted-instance-validate"));
    wb.drain();
    assert_eq!(validate_rows(&wb), 0, "setup: outcome must start held");

    std::fs::remove_file(owner.join(FILE)).unwrap();
    let reminted_key = slots::workspace_key(&owner);
    let reminted_token = std::fs::read_to_string(owner.join(FILE)).unwrap();
    install_key(&mut wb, &owner, reminted_key);
    wb.drain();
    let rows = validate_rows(&wb);
    println!(
        "R14-DELETE-REMINT first_token={} reminted_token={} rows_at_reminted_owner={rows} status={:?}",
        first_token.trim(),
        reminted_token.trim(),
        wb.status
    );
    assert_ne!(
        first_token, reminted_token,
        "remint must create a new instance"
    );
    assert_eq!(rows, 0, "unproven owner must not inherit old outcomes");
}

#[test]
fn r14_identity_lookup_must_not_resurrect_a_deleted_or_moved_workspace_root() {
    let base = tempfile::tempdir().unwrap();
    let moved = base.path().join("moved-away");
    let vacated = base.path().join("vacated");
    std::fs::create_dir_all(&moved).unwrap();
    let _key_while_present = slots::workspace_key(&moved);
    std::fs::rename(&moved, &vacated).unwrap();
    std::fs::remove_dir_all(&vacated).unwrap();

    assert!(!moved.exists(), "setup: moved-away path must be absent");
    let lookup = slots::slot_for(&moved).is_some();
    let resurrected = moved.exists();
    let minted = moved.join(FILE).exists();
    println!(
        "R14-MISSING-ROOT-RESURRECTION path={:?} lookup_found_slot={lookup} resurrected={resurrected} minted_instance={minted}",
        moved
    );
    if resurrected {
        panic!(
            "identity lookup recreated a missing workspace root (instance file present={minted})"
        );
    }
}

#[test]
fn r14_identity_lookup_must_not_mint_evidence_for_an_unopened_workspace() {
    let root = tempfile::tempdir().unwrap();
    let lookup = slots::slot_for(root.path()).is_some();
    let created_cv = root.path().join(".cv").exists();
    let minted = root.path().join(FILE).exists();
    println!(
        "R14-LOOKUP-MINT lookup_found_slot={lookup} created_cv={created_cv} minted_instance={minted}"
    );
    assert!(!lookup, "setup: unopened workspace has no slot");
    assert!(
        !created_cv && !minted,
        "read-only identity lookup must not create or mint `.cv` evidence"
    );
}

#[test]
fn r14_malformed_instance_identity_remains_deterministic_across_rename() {
    let base = tempfile::tempdir().unwrap();
    let original = base.path().join("malformed-original");
    let moved = base.path().join("malformed-moved");
    std::fs::create_dir_all(&original).unwrap();
    let _valid = slots::workspace_key(&original);
    std::fs::write(original.join(FILE), "foreign-corruption").unwrap();
    let before = slots::workspace_key(&original);
    std::fs::rename(&original, &moved).unwrap();
    let after = slots::workspace_key_lookup(&moved);
    let same = before.same_instance(&after);
    println!("R14-MALFORMED-RENAME same_degraded_identity={same}");
    assert!(
        same,
        "degraded identity must be path-independent and deterministic across rename"
    );
}

#[test]
fn r14_malformed_instance_must_not_silently_downgrade_identity_or_strand_outcomes() {
    let base = tempfile::tempdir().unwrap();
    let owner = base.path().join("owner-malformed");
    let foreign = base.path().join("foreign-malformed");
    std::fs::create_dir_all(&owner).unwrap();
    std::fs::create_dir_all(&foreign).unwrap();
    let owner_key = slots::workspace_key(&owner);
    let foreign_key = slots::workspace_key(&foreign);

    let mut wb = Workbench::new(Registry::with_all());
    install_key(&mut wb, &foreign, foreign_key);
    wb.outcome_sink()
        .send_for(owner_key, validate_outcome("malformed-instance-validate"));
    wb.drain();
    assert_eq!(validate_rows(&wb), 0, "setup: outcome must start held");

    std::fs::write(owner.join(FILE), "foreign-corruption").unwrap();
    let downgraded_key = slots::workspace_key(&owner);
    let legacy_key = WorkspaceKey::new(owner.clone(), cv_engine::file_identity::identity(&owner));
    let fell_back_to_inode = downgraded_key.same_instance(&legacy_key);
    install_key(&mut wb, &owner, downgraded_key.clone());
    wb.drain();
    let rows = validate_rows(&wb);
    println!(
        "R14-MALFORMED-DOWNGRADE fell_back_to_inode={fell_back_to_inode} rows_at_owner={rows} status={:?}",
        wb.status
    );
    if fell_back_to_inode || rows != 0 || wb.status.is_empty() {
        panic!(
            "malformed instance evidence silently downgraded identity: fallback={fell_back_to_inode}, rows={rows}, status={:?}",
            wb.status
        );
    }
    assert_eq!(
        wb.app.degraded,
        Some(DegradedReason::StoreCorrupt),
        "malformed workspace-instance evidence must use the persistent degraded-state channel"
    );
    wb.inbox().send(cv_model::workbench::Inbound::Status(
        StatusNotice::engine_evidence("engine ready"),
    ));
    wb.drain();
    assert_eq!(
        wb.app.degraded,
        Some(DegradedReason::StoreCorrupt),
        "ordinary later status must not clear the persistent degraded identity state"
    );

    wb.reset_project(Registry::with_all());
    install_key(&mut wb, &owner, downgraded_key);
    wb.drain();
    assert_eq!(
        wb.app.degraded,
        Some(DegradedReason::StoreCorrupt),
        "a reopened generation must represent the same malformed identity evidence again"
    );
}
