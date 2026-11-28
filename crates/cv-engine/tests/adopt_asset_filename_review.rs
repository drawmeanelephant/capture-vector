//! Regression probes from the independent review of the journaled
//! in-place asset-filename adoption: transitive component blocking,
//! journaled-plan internal consistency, statically knowable destination
//! conflicts, resume reconciliation, journal tampering, and provenance
//! truthfulness. The harness lives in `common::asset_filename` and
//! mirrors adopt_asset_filename.rs exactly.

mod common;

use common::asset_filename::*;
use cv_engine::adopt::{self, ExpectedWorkspaceInstance, asset_filename};
use cv_store::imports as store;

// ---------------------------------------------------------------------------
// A page referencing both a collision-bound and a safe asset merges the
// components; the merged component is blocked, so the safe move never
// runs — and provenance must record it skipped, not applied.
// ---------------------------------------------------------------------------

#[test]
fn transitively_blocked_safe_move_records_skipped_collision() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    stage_workspace(root);
    // copier.md references BOTH a collision-bound dest (Foo-Bar.png) and a
    // safe dest (hello-world.png) → it merges those two components into one.
    write(
        &root.join("content/copier.md"),
        page(
            "copier",
            "Copier",
            "# Copier\n\n![a](case-collision.assets/Foo Bar.png)\n![b](spaces.assets/hello world.png)\n",
        ),
    );
    seed_run(root, "asset-run");
    // Candidate page + injected rewrite rows, before source() snapshots out/.
    write(
        &run_dir(root).join("out/content/copier.md"),
        page(
            "copier",
            "Copier",
            "# Copier\n\n![a](case-collision.assets/Foo-Bar.png)\n![b](spaces.assets/hello-world.png)\n",
        ),
    );
    patch_out(root, "rewrite_manifest.json", |v| {
        v["rewrites"].as_array_mut().unwrap().extend([
            serde_json::json!({
                "page_path": "copier.md",
                "original_dest": "case-collision.assets/Foo Bar.png",
                "rewritten_dest": "case-collision.assets/Foo-Bar.png",
                "reason": "image_or_link_dest"
            }),
            serde_json::json!({
                "page_path": "copier.md",
                "original_dest": "spaces.assets/hello world.png",
                "rewritten_dest": "spaces.assets/hello-world.png",
                "reason": "image_or_link_dest"
            }),
        ]);
    });
    patch_out(root, "report.json", |v| {
        v["counts"]["markdown_rewrites"] = serde_json::json!(9);
    });
    let token = cv_store::workspace_instance::ensure(root).unwrap();
    let expected = ExpectedWorkspaceInstance::new(
        token,
        cv_engine::file_identity::identity(root),
        cv_store::workspace_mutation::current_identity(root).unwrap(),
    );
    let src = adopt::source(root, "asset-run").unwrap();

    let report = transact(root, &src, &expected).unwrap();
    assert!(!pending_path(root).exists(), "journal cleared");

    // The merged component is blocked: the SAFE move must not have run.
    assert!(
        root.join("content/spaces.assets/hello world.png").exists(),
        "safe source must remain — transitively blocked component"
    );
    assert!(
        !root.join("content/spaces.assets/hello-world.png").exists(),
        "safe dest must not exist — transitively blocked component"
    );
    assert_eq!(report.asset_moves, 3, "nested/percent/unicode only");

    // The page that merged the components is honestly skipped...
    let copier = ops_for(root, store::AssetOperationKind::Markdown, "copier.md");
    assert_eq!(copier.len(), 1);
    assert_eq!(
        copier[0].status,
        store::AssetOperationStatus::SkippedCollision
    );
    // ...and the blocked asset is honestly skipped...
    let blocked = ops_for(
        root,
        store::AssetOperationKind::Asset,
        "case-collision.assets/Foo Bar.png",
    );
    assert_eq!(
        blocked[0].status,
        store::AssetOperationStatus::SkippedCollision
    );

    // The transitively blocked SAFE asset records skipped_collision like
    // its pages — `applied` would claim a rename that never ran.
    let safe = ops_for(
        root,
        store::AssetOperationKind::Asset,
        "spaces.assets/hello world.png",
    );
    assert_eq!(safe.len(), 1);
    assert_eq!(
        safe[0].status,
        store::AssetOperationStatus::SkippedCollision,
        "transitively blocked asset move recorded {:?} in durable provenance \
         while the file never moved",
        safe[0].status
    );
}

// ---------------------------------------------------------------------------
// An inventoried file at a journaled temp name makes the page's staged
// write impossible before anything is journaled: the whole component is
// collision-blocked, the litter is preserved, and the rest of the plan
// still applies. A deferred refusal would deadlock — the litter is
// inventory-bound whether present or removed.
// ---------------------------------------------------------------------------

#[test]
fn inventoried_file_at_journaled_temp_name_blocks_its_component() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    stage_workspace(root);
    write(
        &root
            .join("content")
            .join("spaces.md.cv-adopt-asset-run.tmp"),
        b"user litter\n",
    );
    seed_run(root, "asset-run");
    let token = cv_store::workspace_instance::ensure(root).unwrap();
    let expected = ExpectedWorkspaceInstance::new(
        token,
        cv_engine::file_identity::identity(root),
        cv_store::workspace_mutation::current_identity(root).unwrap(),
    );
    let src = adopt::source(root, "asset-run").unwrap();

    let report = transact(root, &src, &expected).unwrap();
    assert!(!pending_path(root).exists(), "journal cleared");

    // The litter is inventoried content and is never touched.
    assert_eq!(
        read(root, "content/spaces.md.cv-adopt-asset-run.tmp"),
        "user litter\n"
    );
    // Its component never ran: source and page stay pre-image.
    assert!(root.join("content/spaces.assets/hello world.png").exists());
    assert!(!root.join("content/spaces.assets/hello-world.png").exists());
    assert!(read(root, "content/spaces.md").contains("hello world.png"));
    assert_eq!(report.asset_moves, 3, "nested/percent/unicode only");

    let littered = ops_for(
        root,
        store::AssetOperationKind::Asset,
        "spaces.assets/hello world.png",
    );
    assert_eq!(
        littered[0].status,
        store::AssetOperationStatus::SkippedCollision,
    );
    let page_op = ops_for(root, store::AssetOperationKind::Markdown, "spaces.md");
    assert_eq!(
        page_op[0].status,
        store::AssetOperationStatus::SkippedCollision,
    );
}

// Same name, but for a page in an already-blocked component: no journaled
// temp exists for it either way, so the file is ordinary inventory and
// adoption proceeds past the rest of the plan.
#[test]
fn inventoried_file_at_blocked_page_temp_name_survives() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    stage_workspace(root);
    write(
        &root
            .join("content")
            .join("case-collision.md.cv-adopt-asset-run.tmp"),
        b"user litter\n",
    );
    seed_run(root, "asset-run");
    let token = cv_store::workspace_instance::ensure(root).unwrap();
    let expected = ExpectedWorkspaceInstance::new(
        token,
        cv_engine::file_identity::identity(root),
        cv_store::workspace_mutation::current_identity(root).unwrap(),
    );
    let src = adopt::source(root, "asset-run").unwrap();

    transact(root, &src, &expected).unwrap();
    assert_eq!(
        read(root, "content/case-collision.md.cv-adopt-asset-run.tmp"),
        "user litter\n"
    );
}

// ---------------------------------------------------------------------------
// A DIRECTORY at a would-be destination is visible to the plan through
// the inventory's directory entries: the component is collision-blocked
// before anything is journaled, never discovered mid-transaction.
// ---------------------------------------------------------------------------

#[test]
fn directory_at_a_destination_blocks_its_component_before_any_write() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    stage_workspace(root);
    // `spaces.assets/hello-world.png` as a REAL directory holding a file.
    write(
        &root
            .join("content/spaces.assets/hello-world.png")
            .join("inner.bin"),
        b"dir contents\n",
    );
    seed_run(root, "asset-run");
    let token = cv_store::workspace_instance::ensure(root).unwrap();
    let expected = ExpectedWorkspaceInstance::new(
        token,
        cv_engine::file_identity::identity(root),
        cv_store::workspace_mutation::current_identity(root).unwrap(),
    );
    let src = adopt::source(root, "asset-run").unwrap();

    let report = transact(root, &src, &expected).unwrap();
    assert!(!pending_path(root).exists(), "journal cleared");

    // The directory and its contents are preserved untouched.
    assert_eq!(
        std::fs::read(root.join("content/spaces.assets/hello-world.png/inner.bin")).unwrap(),
        b"dir contents\n"
    );
    // The spaces component never ran.
    assert!(root.join("content/spaces.assets/hello world.png").exists());
    assert!(read(root, "content/spaces.md").contains("hello world.png"));
    // The unblocked components applied fully — no mid-transaction abort.
    assert_eq!(report.asset_moves, 3, "nested/percent/unicode only");
    assert!(!backup_dir(root).exists());

    let ops = ops_for(
        root,
        store::AssetOperationKind::Asset,
        "spaces.assets/hello world.png",
    );
    assert_eq!(ops[0].status, store::AssetOperationStatus::SkippedCollision,);
    let page_op = ops_for(root, store::AssetOperationKind::Markdown, "spaces.md");
    assert_eq!(
        page_op[0].status,
        store::AssetOperationStatus::SkippedCollision,
    );
}

// A FILE where a destination's parent directory must be created is
// visible in the inventory too — the component is blocked before the
// journal, and the file is preserved.
#[test]
fn file_at_a_destination_parent_blocks_its_component_before_any_write() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    stage_workspace(root);
    // dest `nested.assets/deep/sub-dir/shot.png` needs `sub-dir/` to be a
    // directory; a file there is a statically knowable conflict.
    write(
        &root.join("content/nested.assets/deep/sub-dir"),
        b"a file, not a dir\n",
    );
    seed_run(root, "asset-run");
    let token = cv_store::workspace_instance::ensure(root).unwrap();
    let expected = ExpectedWorkspaceInstance::new(
        token,
        cv_engine::file_identity::identity(root),
        cv_store::workspace_mutation::current_identity(root).unwrap(),
    );
    let src = adopt::source(root, "asset-run").unwrap();

    let report = transact(root, &src, &expected).unwrap();
    assert!(!pending_path(root).exists(), "journal cleared");

    assert_eq!(
        read(root, "content/nested.assets/deep/sub-dir"),
        "a file, not a dir\n"
    );
    assert!(
        root.join("content/nested.assets/deep/sub dir/shot.png")
            .exists(),
        "blocked source must remain"
    );
    assert!(
        !root
            .join("content/nested.assets/deep/sub-dir/shot.png")
            .exists(),
        "unreachable dest must not be created"
    );
    assert_eq!(report.asset_moves, 3, "percent/spaces/unicode only");
    assert!(!backup_dir(root).exists());

    let ops = ops_for(
        root,
        store::AssetOperationKind::Asset,
        "nested.assets/deep/sub dir/shot.png",
    );
    assert_eq!(ops[0].status, store::AssetOperationStatus::SkippedCollision,);
    let page_op = ops_for(root, store::AssetOperationKind::Markdown, "nested.md");
    assert_eq!(
        page_op[0].status,
        store::AssetOperationStatus::SkippedCollision,
    );
}

// ---------------------------------------------------------------------------
// Forging the journaled phase forward cannot skip work — phases are
// advisory and the executor converges from live state.
// ---------------------------------------------------------------------------

#[test]
fn forged_forward_phase_cannot_skip_work() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let (_id, src, expected) = fixture(root);
    journal_only(root, &src, &expected);
    patch_journal(root, |v| {
        v["transformation"]["phase"] = serde_json::json!("verified");
    });

    let report = transact(root, &src, &expected).unwrap();
    assert_eq!(report.asset_moves, 4);
    assert_eq!(report.markdown_writes, 4);
    assert!(
        root.join("content/spaces.assets/hello-world.png").exists(),
        "forged phase must not skip the real work"
    );
    assert!(!pending_path(root).exists());
}

// ---------------------------------------------------------------------------
// A byte-identical COPY at a journaled destination (new inode) is
// foreign — identity pins distinguish our link from a restored copy.
// ---------------------------------------------------------------------------

#[test]
fn byte_identical_copy_at_journaled_dest_is_foreign() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let (_id, src, expected) = fixture(root);
    journal_only(root, &src, &expected);
    std::fs::copy(
        root.join("content/spaces.assets/hello world.png"),
        root.join("content/spaces.assets/hello-world.png"),
    )
    .unwrap();

    let err = transact(root, &src, &expected).unwrap_err();
    assert!(matches!(err, adopt::AdoptError::Foreign(_)), "{err:?}");
    assert_eq!(
        std::fs::read(root.join("content/spaces.assets/hello-world.png")).unwrap(),
        b"asset-stub-v1-spaces\n".to_vec(),
        "the foreign copy is preserved, never silently adopted"
    );
    assert!(pending_path(root).exists());
}

// A foreign HARD LINK to the journaled source at the dest path IS
// indistinguishable from this transformation's own link — and harmlessly
// converges as Resumed (the journaled end state is the same inode there).
#[test]
fn foreign_hardlink_at_journaled_dest_converges_as_resumed() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let (_id, src, expected) = fixture(root);
    journal_only(root, &src, &expected);
    std::fs::hard_link(
        root.join("content/spaces.assets/hello world.png"),
        root.join("content/spaces.assets/hello-world.png"),
    )
    .unwrap();

    let report = transact(root, &src, &expected).unwrap();
    assert!(report.resumed >= 1);
    assert!(!root.join("content/spaces.assets/hello world.png").exists());
    let ops = ops_for(
        root,
        store::AssetOperationKind::Asset,
        "spaces.assets/hello world.png",
    );
    assert_eq!(
        ops[0].status,
        store::AssetOperationStatus::Resumed,
        "foreign-but-identical link records as resumed, honestly"
    );
}

// ---------------------------------------------------------------------------
// Deleting a journaled page stops resume before any mutation.
// ---------------------------------------------------------------------------

#[test]
fn deleted_journaled_page_stops_resume_before_mutation() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let (_id, src, expected) = fixture(root);
    journal_only(root, &src, &expected);
    std::fs::remove_file(root.join("content/spaces.md")).unwrap();

    let err = transact(root, &src, &expected).unwrap_err();
    assert!(matches!(err, adopt::AdoptError::Foreign(_)), "{err:?}");
    assert_no_content_effect(root);
    assert!(pending_path(root).exists());
}

// ---------------------------------------------------------------------------
// A journal with no transformation snapshot is a Run error.
// ---------------------------------------------------------------------------

#[test]
fn journal_without_transformation_is_run_error() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let (_id, src, expected) = fixture(root);
    journal_only(root, &src, &expected);
    patch_journal(root, |v| {
        v.as_object_mut().unwrap().remove("transformation");
    });

    let err = transact(root, &src, &expected).unwrap_err();
    assert!(matches!(err, adopt::AdoptError::Run(_)), "{err:?}");
    assert_no_content_effect(root);
}

// Tampering with any journaled component field is caught by the normalized
// plan comparison — before mutation.
#[test]
fn tampered_component_field_invalidates_journal() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let (_id, src, expected) = fixture(root);
    journal_only(root, &src, &expected);
    patch_journal(root, |v| {
        v["transformation"]["components"][0]["asset_moves"][0]["sha256"] = serde_json::json!("00");
    });

    let err = transact(root, &src, &expected).unwrap_err();
    assert!(matches!(err, adopt::AdoptError::Run(_)), "{err:?}");
    assert_no_content_effect(root);
}

// ---------------------------------------------------------------------------
// Rewriting the durable journal after full adoption simulates a crash
// between Recorded and clear_pending: resume must converge idempotently,
// merge the event once, and clear the journal.
// ---------------------------------------------------------------------------

#[test]
fn restored_journal_after_full_adoption_resumes_without_double_event() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let (_id, src, expected) = fixture(root);
    journal_only(root, &src, &expected);
    let saved = std::fs::read(pending_path(root)).unwrap();

    transact(root, &src, &expected).unwrap();
    assert!(!pending_path(root).exists());

    // Simulate crash-after-Recorded: the journal file reappears.
    std::fs::write(pending_path(root), &saved).unwrap();

    let report = transact(root, &src, &expected).unwrap();
    assert!(!pending_path(root).exists());
    assert!(report.resumed > 0);
    let rec = store::read_adoption_record(root, "asset-run")
        .unwrap()
        .unwrap();
    assert_eq!(rec.events.len(), 1, "resume must merge, not duplicate");
    assert!(!rec.asset_operations.is_empty());
}

// ---------------------------------------------------------------------------
// Byte-editing the captured input inventory invalidates the journal.
// ---------------------------------------------------------------------------

#[test]
fn byte_edited_input_inventory_invalidates_journal() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let (_id, src, expected) = fixture(root);
    journal_only(root, &src, &expected);
    // Any byte change to the pinned inventory file breaks the binding.
    let inv = run_dir(root).join("input-inventory.json");
    let mut bytes = std::fs::read(&inv).unwrap();
    bytes.push(b' ');
    std::fs::write(&inv, bytes).unwrap();

    let err = transact(root, &src, &expected).unwrap_err();
    assert!(matches!(err, adopt::AdoptError::Run(_)), "{err:?}");
    assert_no_content_effect(root);
}

// ---------------------------------------------------------------------------
// A rewrite page whose candidate bytes equal the inventory runs no write:
// its provenance records `unchanged`, not `applied`.
// ---------------------------------------------------------------------------

#[test]
fn page_post_equal_to_pre_records_unchanged() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    stage_workspace(root);
    seed_run(root, "asset-run");
    // Candidate page bytes identical to the inventoried live page.
    let live = std::fs::read(root.join("content/spaces.md")).unwrap();
    write(&run_dir(root).join("out/content/spaces.md"), &live);
    let token = cv_store::workspace_instance::ensure(root).unwrap();
    let expected = ExpectedWorkspaceInstance::new(
        token,
        cv_engine::file_identity::identity(root),
        cv_store::workspace_mutation::current_identity(root).unwrap(),
    );
    let src = adopt::source(root, "asset-run").unwrap();

    transact(root, &src, &expected).unwrap();
    let ops = ops_for(root, store::AssetOperationKind::Markdown, "spaces.md");
    assert_eq!(ops.len(), 1);
    assert_eq!(
        ops[0].status,
        store::AssetOperationStatus::Unchanged,
        "a no-op page row recorded {:?} although nothing was written",
        ops[0].status
    );
}

// ---------------------------------------------------------------------------
// A symlinked run directory is refused through the locked ancestor checks.
// ---------------------------------------------------------------------------

#[test]
fn symlinked_run_directory_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let (_id, src, expected) = fixture(root);
    let dir = run_dir(root);
    let moved = root.join("moved-run");
    std::fs::rename(&dir, &moved).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&moved, &dir).unwrap();

    let err = transact(root, &src, &expected).unwrap_err();
    eprintln!("symlinked run dir error: {err}");
    assert_no_content_effect(root);
}

// ---------------------------------------------------------------------------
// Empty directories are entries now, not invisible traversal structure:
// one appearing after the journal lands is foreign and refuses recovery,
// and one appearing between capture and a fresh attempt is stale.
// ---------------------------------------------------------------------------

#[test]
fn empty_directory_added_after_journaling_is_foreign() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let (_id, src, expected) = fixture(root);
    journal_only(root, &src, &expected);
    std::fs::create_dir_all(root.join("content/foreign-empty")).unwrap();

    let err = transact(root, &src, &expected).unwrap_err();
    assert!(matches!(err, adopt::AdoptError::Foreign(_)), "{err:?}");
    assert!(
        root.join("content/foreign-empty").exists(),
        "foreign directories are never removed"
    );
    assert_no_content_effect(root);
    assert!(pending_path(root).exists());
}

#[test]
fn empty_directory_added_after_capture_is_stale() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    stage_workspace(root);
    seed_run(root, "asset-run");
    std::fs::create_dir_all(root.join("content/foreign-empty")).unwrap();
    let token = cv_store::workspace_instance::ensure(root).unwrap();
    let expected = ExpectedWorkspaceInstance::new(
        token,
        cv_engine::file_identity::identity(root),
        cv_store::workspace_mutation::current_identity(root).unwrap(),
    );
    let src = adopt::source(root, "asset-run").unwrap();

    let err = transact(root, &src, &expected).unwrap_err();
    assert!(matches!(err, adopt::AdoptError::Stale(_)), "{err:?}");
    assert_no_content_effect(root);
    assert!(!pending_path(root).exists(), "no journal may be written");
}

// ---------------------------------------------------------------------------
// A FIFO at a journaled move source is foreign (Other ≠ File), not a
// byte match — non-regular files cannot smuggle through identity checks.
// ---------------------------------------------------------------------------

#[cfg(unix)]
#[test]
fn fifo_at_journaled_source_is_foreign() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let (_id, src, expected) = fixture(root);
    journal_only(root, &src, &expected);
    std::fs::remove_file(root.join("content/spaces.assets/hello world.png")).unwrap();
    let fifo = std::ffi::CString::new(
        root.join("content/spaces.assets/hello world.png")
            .to_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o644) }, 0);

    let err = transact(root, &src, &expected).unwrap_err();
    assert!(matches!(err, adopt::AdoptError::Foreign(_)), "{err:?}");
    assert_no_content_effect(root);
}

// ---------------------------------------------------------------------------
// A foreign file at a journaled backup path (.cv is outside the live
// input scan) is still refused cleanly at the first exec phase — before
// any content mutation.
// ---------------------------------------------------------------------------

#[test]
fn foreign_file_at_journaled_backup_path_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let (_id, src, expected) = fixture(root);
    journal_only(root, &src, &expected);
    let first = all_moves(root)[0].backup.clone();
    write(&root.join(&first), b"foreign\n");

    let err = transact(root, &src, &expected).unwrap_err();
    assert!(matches!(err, adopt::AdoptError::Foreign(_)), "{err:?}");
    assert_eq!(
        std::fs::read(root.join(&first)).unwrap(),
        b"foreign\n".to_vec()
    );
    // The refusal happened at move 0 of phase_backups: no content dests.
    for rel in [
        "nested.assets/deep/sub-dir/shot.png",
        "percent.assets/diagram-copy.png",
        "spaces.assets/hello-world.png",
        "unicode.assets/caf.png",
    ] {
        assert!(!root.join("content").join(rel).exists(), "{rel} linked");
    }
    assert!(pending_path(root).exists());
}

// ---------------------------------------------------------------------------
// A journaled destination that collides with a journaled Markdown temp
// name (lab-derived dest embedding the run id) is internally
// inconsistent: validating the plan's operation paths before journaling
// refuses the evidence instead of writing a permanently deadlocked
// journal.
// ---------------------------------------------------------------------------

#[test]
fn journaled_dest_colliding_with_markdown_temp_is_refused_before_journaling() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    stage_workspace(root);
    write(&root.join("content/evil.png"), b"asset-stub-v1-evil\n");
    write(
        &root.join("content/copier.md"),
        page("copier", "Copier", "# Copier\n\n![e](evil.png)\n"),
    );
    seed_run(root, "asset-run");
    // Candidate: copier page + an accepted asset at the temp-colliding name.
    write(
        &run_dir(root).join("out/content/copier.md"),
        page(
            "copier",
            "Copier",
            "# Copier\n\n![e](copier.md.cv-adopt-asset-run.tmp)\n",
        ),
    );
    write(
        &run_dir(root).join("out/content/copier.md.cv-adopt-asset-run.tmp"),
        b"asset-stub-v1-evil\n",
    );
    let evil_sha = store::sha256_hex(b"asset-stub-v1-evil\n");
    patch_out(root, "asset_filename_manifest.json", |v| {
        v["assets"].as_array_mut().unwrap().push(serde_json::json!({
            "source_path": "evil.png",
            "within_tree_source": "evil.png",
            "page_stem": "copier",
            "within_tree_dest": "evil.png",
            "dest_path": "copier.md.cv-adopt-asset-run.tmp",
            "action": "rewritten",
            "reason": "name_injection",
            "sha256": evil_sha,
            "bytes": 19
        }));
    });
    patch_out(root, "rewrite_manifest.json", |v| {
        v["rewrites"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "page_path": "copier.md",
                "original_dest": "evil.png",
                "rewritten_dest": "copier.md.cv-adopt-asset-run.tmp",
                "reason": "image_or_link_dest"
            }));
    });
    patch_out(root, "report.json", |v| {
        v["counts"]["assets"] = serde_json::json!(13);
        v["counts"]["rewritten"] = serde_json::json!(7);
        v["counts"]["markdown_rewrites"] = serde_json::json!(8);
    });
    let token = cv_store::workspace_instance::ensure(root).unwrap();
    let expected = ExpectedWorkspaceInstance::new(
        token,
        cv_engine::file_identity::identity(root),
        cv_store::workspace_mutation::current_identity(root).unwrap(),
    );
    let src = adopt::source(root, "asset-run").unwrap();

    let err = journal_result(root, &src, &expected).unwrap_err();
    assert!(matches!(err, adopt::AdoptError::Report(_)), "{err:?}");
    assert!(err.to_string().contains("collides with journaled"), "{err}");
    assert!(!pending_path(root).exists(), "no journal may be written");
    assert!(
        root.join("content/evil.png").exists(),
        "the workspace is untouched by refused evidence"
    );
    assert!(read(root, "content/copier.md").contains("evil.png"));
}

// A journaled path can also conflict by prefix: a destination placed
// under a page's staged-write temporary can never be created — the temp
// is a file, not a directory.
#[test]
fn journaled_dest_under_a_journaled_temp_is_refused_before_journaling() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    stage_workspace(root);
    write(&root.join("content/evil.png"), b"asset-stub-v1-evil\n");
    write(
        &root.join("content/copier.md"),
        page("copier", "Copier", "# Copier\n\n![e](evil.png)\n"),
    );
    seed_run(root, "asset-run");
    write(
        &run_dir(root).join("out/content/copier.md"),
        page(
            "copier",
            "Copier",
            "# Copier\n\n![e](spaces.md.cv-adopt-asset-run.tmp/evil.png)\n",
        ),
    );
    write(
        &run_dir(root).join("out/content/spaces.md.cv-adopt-asset-run.tmp/evil.png"),
        b"asset-stub-v1-evil\n",
    );
    let evil_sha = store::sha256_hex(b"asset-stub-v1-evil\n");
    patch_out(root, "asset_filename_manifest.json", |v| {
        v["assets"].as_array_mut().unwrap().push(serde_json::json!({
            "source_path": "evil.png",
            "within_tree_source": "evil.png",
            "page_stem": "copier",
            "within_tree_dest": "evil.png",
            "dest_path": "spaces.md.cv-adopt-asset-run.tmp/evil.png",
            "action": "rewritten",
            "reason": "name_injection",
            "sha256": evil_sha,
            "bytes": 19
        }));
    });
    patch_out(root, "rewrite_manifest.json", |v| {
        v["rewrites"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "page_path": "copier.md",
                "original_dest": "evil.png",
                "rewritten_dest": "spaces.md.cv-adopt-asset-run.tmp/evil.png",
                "reason": "image_or_link_dest"
            }));
    });
    patch_out(root, "report.json", |v| {
        v["counts"]["assets"] = serde_json::json!(13);
        v["counts"]["rewritten"] = serde_json::json!(7);
        v["counts"]["markdown_rewrites"] = serde_json::json!(8);
    });
    let token = cv_store::workspace_instance::ensure(root).unwrap();
    let expected = ExpectedWorkspaceInstance::new(
        token,
        cv_engine::file_identity::identity(root),
        cv_store::workspace_mutation::current_identity(root).unwrap(),
    );
    let src = adopt::source(root, "asset-run").unwrap();

    let err = journal_result(root, &src, &expected).unwrap_err();
    assert!(matches!(err, adopt::AdoptError::Report(_)), "{err:?}");
    assert!(err.to_string().contains("unreachable"), "{err}");
    assert!(!pending_path(root).exists(), "no journal may be written");
}

// ---------------------------------------------------------------------------
// A journaled destination beneath a foreign-volume mount inside the input
// can never take the transformation's hard link — the first attempt fails
// EXDEV and every resume fails identically, a journaled deadlock. The
// pre-journal mount check walks each journaled destination/temp to its
// deepest existing ancestor and refuses while nothing is on disk.
// ---------------------------------------------------------------------------

#[test]
fn destination_beneath_a_foreign_volume_refuses_before_journaling() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let (_id, src, expected) = fixture(root);
    // `nested.assets/deep/sub-dir` does not exist yet: the mount at `deep`
    // is the deepest existing ancestor of the journaled destination.
    let mount = root.join("content/nested.assets/deep");
    let volume_of = foreign_volume_at(&mount);

    let err = asset_filename::transact_existing_with_identity(tx_with_volume(
        root, &src, &expected, &volume_of,
    ))
    .unwrap_err();
    assert!(matches!(err, adopt::AdoptError::Run(_)), "{err:?}");
    assert!(err.to_string().contains("different volume"), "{err}");
    assert!(
        err.to_string()
            .contains("nested.assets/deep/sub-dir/shot.png"),
        "{err}"
    );
    assert!(!pending_path(root).exists(), "no journal may be written");
    assert_no_content_effect(root);
}

// A mount beneath the run directory's asset-backup path is also a
// journaled creation boundary: backup links are created there before any
// destination link, so the same pre-journal volume proof must cover it.
#[test]
fn backup_beneath_a_foreign_volume_refuses_before_journaling() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let (_id, src, expected) = fixture(root);
    let mount = run_dir(root).join("asset-backup");
    std::fs::create_dir_all(&mount).unwrap();
    let volume_of = foreign_volume_at(&mount);

    let err = asset_filename::transact_existing_with_identity(tx_with_volume(
        root, &src, &expected, &volume_of,
    ))
    .unwrap_err();
    assert!(matches!(err, adopt::AdoptError::Run(_)), "{err:?}");
    assert!(err.to_string().contains("journaled backup"), "{err}");
    assert!(err.to_string().contains("different volume"), "{err}");
    assert!(!pending_path(root).exists(), "no journal may be written");
    assert!(
        std::fs::read_dir(&mount).unwrap().next().is_none(),
        "the pre-existing mount directory is untouched and no backup link was created"
    );
}

// A mount that appears between journaling and the resume attempt blocks
// recovery the same way — the journal stays for a retry after the mount is
// removed, and no phase ran.
#[test]
fn foreign_volume_appearing_after_journaling_blocks_the_resume() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let (_id, src, expected) = fixture(root);
    journal_only(root, &src, &expected);
    let mount = root.join("content/nested.assets/deep");
    let volume_of = foreign_volume_at(&mount);

    let err = asset_filename::transact_existing_with_identity(tx_with_volume(
        root, &src, &expected, &volume_of,
    ))
    .unwrap_err();
    assert!(matches!(err, adopt::AdoptError::Run(_)), "{err:?}");
    assert_no_content_effect(root);
    assert!(pending_path(root).exists(), "journal stays for the retry");
}

// A staged-write temporary beneath the mount is refused identically —
// temp and dest share the pre-journal check and every violation is named.
#[test]
fn staged_temp_beneath_a_foreign_volume_refuses_before_journaling() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let (_id, src, expected) = fixture(root);
    // The whole content input sits on the mount: every journaled path's
    // deepest existing ancestor reports the foreign volume.
    let mount = root.join("content");
    let volume_of = foreign_volume_at(&mount);

    let err = asset_filename::transact_existing_with_identity(tx_with_volume(
        root, &src, &expected, &volume_of,
    ))
    .unwrap_err();
    assert!(matches!(err, adopt::AdoptError::Run(_)), "{err:?}");
    assert!(err.to_string().contains("different volume"), "{err}");
    assert!(
        err.to_string().contains(".cv-adopt-asset-run.tmp"),
        "the staged-temp violation must be named: {err}"
    );
    assert!(!pending_path(root).exists(), "no journal may be written");
    assert_no_content_effect(root);
}

// ---------------------------------------------------------------------------
// A schema-1 input inventory records no directories: directories at
// journaled destinations or parents are invisible to it, resurrecting the
// post-journal deadlock the schema-2 entries were written to prevent. The
// transformation refuses and asks for a fresh capture instead of trusting
// a partial guard.
// ---------------------------------------------------------------------------

#[test]
fn schema_1_input_inventory_refuses_the_transformation() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let (_id, src, expected) = fixture(root);
    // Downgrade the captured inventory the way a schema-1 writer left it:
    // version stamp 1 and no directory entries.
    let inv = run_dir(root).join(store::INPUT_INVENTORY);
    let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(&inv).unwrap()).unwrap();
    v["schema_version"] = serde_json::json!(1);
    v["entries"]
        .as_array_mut()
        .unwrap()
        .retain(|e| e["type"] != "directory");
    std::fs::write(&inv, serde_json::to_vec_pretty(&v).unwrap()).unwrap();

    let err = transact(root, &src, &expected).unwrap_err();
    assert!(matches!(err, adopt::AdoptError::Run(_)), "{err:?}");
    assert!(err.to_string().contains("schema-1"), "{err}");
    assert!(
        err.to_string()
            .contains("run the asset-filename utility again"),
        "{err}"
    );
    assert!(!pending_path(root).exists(), "no journal may be written");
    assert_no_content_effect(root);
}

// ---------------------------------------------------------------------------
// Collision-bound components are disclosed as a count, not silently folded
// into the applied totals: the report's skipped_collisions equals exactly
// the skipped_collision rows the durable provenance recorded.
// ---------------------------------------------------------------------------

#[test]
fn skipped_collisions_are_counted_for_disclosure() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let (_id, src, expected) = fixture(root);
    let report = transact(root, &src, &expected).unwrap();
    let adoption = store::read_adoption_record(root, "asset-run")
        .unwrap()
        .unwrap();
    let recorded = adoption
        .asset_operations
        .iter()
        .filter(|o| o.status == store::AssetOperationStatus::SkippedCollision)
        .count();
    assert_eq!(recorded, 4, "both collision-bound components");
    assert_eq!(
        report.skipped_collisions, recorded,
        "the disclosed count must equal the recorded rows"
    );
}
