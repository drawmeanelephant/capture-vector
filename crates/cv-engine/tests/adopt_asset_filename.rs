//! Regression tests for snapshot/phase in-place asset-filename adoption.
//! The lab output is the pinned capture; the workspace content it was
//! captured from is reconstructed in scratch space along with the
//! Capture Vector-owned input inventory the lab run recorded.

use cv_engine::adopt::{self, ExpectedWorkspaceInstance, asset_filename};
use cv_fixtures::lab::StubLab;
use cv_store::imports::{self as store, Finished, RecordedLab, RunRecord};
use std::path::Path;

fn write(path: &Path, bytes: impl AsRef<[u8]>) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn symlink_file(target: &Path, link: &Path) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(target, link).unwrap();
}

fn page(id: &str, title: &str, body: &str) -> String {
    format!("---\nid: {id}\ntitle: {title}\nstatus: published\n---\n\n{body}")
}

fn stage_workspace(root: &Path) {
    let c = root.join("content");
    write(
        &c.join("case-collision.md"),
        page(
            "case-collision",
            "Case collision",
            "# Case\n\n![a](case-collision.assets/Foo Bar.png)\n![b](case-collision.assets/foo-bar.png)\n",
        ),
    );
    write(
        &c.join("nested.md"),
        page(
            "nested",
            "Nested",
            "# Nested\n\n![shot](nested.assets/deep/sub dir/shot.png)\n",
        ),
    );
    write(
        &c.join("percent.md"),
        page(
            "percent",
            "Percent names",
            "# Percent\n\n![diagram](percent.assets/diagram%20copy.png)\n",
        ),
    );
    write(
        &c.join("safe.md"),
        page(
            "safe",
            "Safe names",
            "# Safe\n\n![ok](safe.assets/already-ok.png)\n",
        ),
    );
    write(
        &c.join("sanitize-collision.md"),
        page(
            "sanitize-collision",
            "Sanitize collision",
            "# Sanitize collision\n\n![a](sanitize-collision.assets/foo bar.png)\n![b](sanitize-collision.assets/foo-bar.png)\n",
        ),
    );
    write(
        &c.join("spaces.md"),
        page(
            "spaces",
            "Spaces",
            "# Spaces\n\n![hero](spaces.assets/hello world.png)\n![encoded](spaces.assets/hello%20world.png)\n",
        ),
    );
    write(
        &c.join("symlink.md"),
        page(
            "symlink",
            "Symlink",
            "# Symlink\n\n![real](symlink.assets/real.png)\n![alias](symlink.assets/alias.png)\n",
        ),
    );
    write(
        &c.join("traversal.md"),
        page(
            "traversal",
            "Traversal",
            "# Traversal\n\n![ok](traversal.assets/nested/ok.png)\n![escape](traversal.assets/../escape.png)\n![dotdot](traversal.assets/nested/../../escape.png)\n",
        ),
    );
    write(
        &c.join("unicode.md"),
        page(
            "unicode",
            "Unicode",
            "# Unicode\n\n![cafe](unicode.assets/café.png)\n",
        ),
    );

    for (rel, bytes) in [
        (
            "case-collision.assets/Foo Bar.png",
            &b"asset-stub-v1-case-a\n"[..],
        ),
        (
            "case-collision.assets/foo-bar.png",
            &b"asset-stub-v1-case-b\n"[..],
        ),
        (
            "nested.assets/deep/sub dir/shot.png",
            &b"asset-stub-v1-nested\n"[..],
        ),
        (
            "percent.assets/diagram%20copy.png",
            &b"asset-stub-v1-percent\n"[..],
        ),
        ("safe.assets/already-ok.png", &b"asset-stub-v1-safe\n"[..]),
        (
            "sanitize-collision.assets/foo bar.png",
            &b"asset-stub-v1-sc-a\n"[..],
        ),
        (
            "sanitize-collision.assets/foo-bar.png",
            &b"asset-stub-v1-sc-b\n"[..],
        ),
        (
            "spaces.assets/hello world.png",
            &b"asset-stub-v1-spaces\n"[..],
        ),
        (
            "symlink.assets/real.png",
            &b"asset-stub-v1-symlink-real\n"[..],
        ),
        (
            "traversal.assets/nested/ok.png",
            &b"asset-stub-v1-trav-ok\n"[..],
        ),
        ("unicode.assets/café.png", &b"asset-stub-v1-unicode\n"[..]),
    ] {
        write(&c.join(rel), bytes);
    }
    std::fs::create_dir_all(c.join("symlink.assets")).unwrap();
    symlink_file(
        &c.join("symlink.assets/real.png"),
        &c.join("symlink.assets/alias.png"),
    );
}

/// Mirrors `run_lab_with_lock`: the stub lab writes `out/`, the run record is
/// stored, and the content-input inventory is captured under the run lock —
/// the freshness authority every later adoption attempt reconciles against.
fn seed_run(root: &Path, run_id: &str) {
    let dir = store::run_dir(root, run_id).unwrap();
    let transcript = StubLab::new()
        .run("asset-filename", &dir.join(store::OUT))
        .unwrap();
    let lock = store::lock_run(root, run_id).unwrap();
    let mut rec = RunRecord::started(
        run_id,
        "asset-filename",
        RecordedLab {
            tool_id: StubLab::new().tool_id(),
            source: "fixture".into(),
            path: "fixtures/captures/lab".into(),
        },
        1,
    );
    let input = root.join("content").canonicalize().unwrap();
    rec.inputs = vec![store::input("--root", &input.display().to_string())];
    rec.argv = vec![
        "--mode=asset-filename".into(),
        format!("--root={}", input.display()),
        format!("--out={}", dir.join(store::OUT).display()),
    ];
    assert!(transcript.argv.iter().any(|a| a == "--mode=asset-filename"));
    rec.finished = Some(Finished {
        exit_code: Some(0),
        exit: store::FinishedExit::Ok,
        outcome: "exited 0".into(),
        duration_ms: 0,
    });
    store::write(&lock, &rec).unwrap();
    store::capture_input_inventory(&lock, &input).unwrap();
}

fn fixture(root: &Path) -> (String, adopt::AdoptionSource, ExpectedWorkspaceInstance) {
    stage_workspace(root);
    let run_id = "asset-run";
    seed_run(root, run_id);
    let token = cv_store::workspace_instance::ensure(root).unwrap();
    let expected = ExpectedWorkspaceInstance::new(
        token,
        cv_engine::file_identity::identity(root),
        cv_store::workspace_mutation::current_identity(root).unwrap(),
    );
    let src = adopt::source(root, run_id).unwrap();
    (run_id.into(), src, expected)
}

fn tx<'a>(
    root: &'a Path,
    src: &'a adopt::AdoptionSource,
    expected: &'a ExpectedWorkspaceInstance,
) -> asset_filename::AssetFilenameTransaction<'a> {
    asset_filename::AssetFilenameTransaction {
        target_root: root,
        src,
        content_input: "content",
        at_ms: 2,
        expected_instance: expected,
        volume_of: None,
    }
}

fn transact(
    root: &Path,
    src: &adopt::AdoptionSource,
    expected: &ExpectedWorkspaceInstance,
) -> Result<adopt::TxReport, adopt::AdoptError> {
    asset_filename::transact_existing_with_identity(tx(root, src, expected))
}

/// Journal the plan and stop before any content mutation — the state a crash
/// immediately after preflight would leave on disk.
fn journal_only(root: &Path, src: &adopt::AdoptionSource, expected: &ExpectedWorkspaceInstance) {
    asset_filename::journal_only_for_tests(tx(root, src, expected)).unwrap();
}

fn read(root: &Path, rel: &str) -> String {
    std::fs::read_to_string(root.join(rel)).unwrap()
}

fn run_dir(root: &Path) -> std::path::PathBuf {
    root.join(".cv/imports/asset-run")
}

fn pending_path(root: &Path) -> std::path::PathBuf {
    run_dir(root).join(store::PENDING)
}

fn journal(root: &Path) -> store::PendingAdoption {
    store::read_pending(root, "asset-run").unwrap().unwrap()
}

fn all_moves(root: &Path) -> Vec<store::PlannedAssetMove> {
    journal(root)
        .transformation
        .as_ref()
        .unwrap()
        .components
        .iter()
        .flat_map(|c| c.asset_moves.clone())
        .collect()
}

/// Link a transaction-scoped backup for each move as if the BackupsLinked
/// phase had completed for it.
fn link_backups(root: &Path, take: impl Fn(&store::PlannedAssetMove) -> bool) {
    for mv in all_moves(root) {
        if !take(&mv) {
            continue;
        }
        let backup = root.join(&mv.backup);
        let src = root.join("content").join(&mv.source);
        std::fs::create_dir_all(backup.parent().unwrap()).unwrap();
        std::fs::hard_link(&src, &backup).unwrap();
    }
}

/// Link destinations from their backups as if DestinationsLinked had run.
fn link_dests(root: &Path, take: impl Fn(&store::PlannedAssetMove) -> bool) {
    for mv in all_moves(root) {
        if !take(&mv) {
            continue;
        }
        let backup = root.join(&mv.backup);
        let dest = root.join("content").join(&mv.dest);
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::hard_link(&backup, &dest).unwrap();
    }
}

/// Apply the candidate post-image to one page as if the MarkdownWritten
/// phase had reached it.
fn write_page_post(root: &Path, page_rel: &str) {
    let post = store::read_out_file(root, "asset-run", &format!("content/{page_rel}")).unwrap();
    write(&root.join("content").join(page_rel), post);
}

/// Rewrite one journaled field, simulating journal corruption between
/// attempts.
fn patch_journal(root: &Path, patch: impl FnOnce(&mut serde_json::Value)) {
    let mut v: serde_json::Value =
        serde_json::from_slice(&std::fs::read(pending_path(root)).unwrap()).unwrap();
    patch(&mut v);
    std::fs::write(pending_path(root), serde_json::to_vec(&v).unwrap()).unwrap();
}

/// Rewrite one evidence file inside `out/`, simulating tampering or a bad lab.
fn patch_out(root: &Path, rel: &str, patch: impl FnOnce(&mut serde_json::Value)) {
    let path = run_dir(root).join(store::OUT).join(rel);
    let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    patch(&mut v);
    std::fs::write(&path, serde_json::to_vec(&v).unwrap()).unwrap();
}

fn backup_dir(root: &Path) -> std::path::PathBuf {
    run_dir(root).join("asset-backup")
}

fn assert_no_content_effect(root: &Path) {
    assert!(!backup_dir(root).exists(), "backup links must not exist");
    for rel in [
        "nested.assets/deep/sub-dir/shot.png",
        "percent.assets/diagram-copy.png",
        "spaces.assets/hello-world.png",
        "unicode.assets/caf.png",
    ] {
        assert!(
            !root.join("content").join(rel).exists(),
            "{rel} must not have been linked"
        );
    }
    for rel in [
        "nested.assets/deep/sub dir/shot.png",
        "percent.assets/diagram%20copy.png",
        "spaces.assets/hello world.png",
        "unicode.assets/café.png",
    ] {
        assert!(
            root.join("content").join(rel).is_file(),
            "{rel} must be untouched"
        );
    }
}

#[test]
fn transformation_moves_safe_assets_rewrites_pages_and_leaves_collisions_untouched() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let (_id, src, expected) = fixture(&root);
    let report = transact(&root, &src, &expected).unwrap();
    assert_eq!((report.asset_moves, report.markdown_writes), (4, 4));
    assert!(report.written >= 8, "{report:?}");

    for (gone, present) in [
        (
            "spaces.assets/hello world.png",
            "spaces.assets/hello-world.png",
        ),
        ("unicode.assets/café.png", "unicode.assets/caf.png"),
        (
            "nested.assets/deep/sub dir/shot.png",
            "nested.assets/deep/sub-dir/shot.png",
        ),
        (
            "percent.assets/diagram%20copy.png",
            "percent.assets/diagram-copy.png",
        ),
    ] {
        assert!(!root.join("content").join(gone).exists(), "{gone} remains");
        assert!(
            root.join("content").join(present).is_file(),
            "{present} missing"
        );
    }
    let spaces = read(&root, "content/spaces.md");
    assert!(spaces.contains("spaces.assets/hello-world.png"));
    assert!(!spaces.contains("hello world.png"));
    assert_eq!(
        spaces.matches("id: spaces").count(),
        1,
        "in-place rewrite must not duplicate explicit ids"
    );

    for collision in [
        "sanitize-collision.assets/foo bar.png",
        "sanitize-collision.assets/foo-bar.png",
        "case-collision.assets/Foo Bar.png",
        "case-collision.assets/foo-bar.png",
    ] {
        assert!(
            root.join("content").join(collision).is_file(),
            "{collision} must be untouched"
        );
    }
    // The transitive block: collision-contaminated pages keep their original
    // references, so no reference dangles to a move that never happened.
    assert!(read(&root, "content/sanitize-collision.md").contains("foo bar.png"));
    assert!(read(&root, "content/case-collision.md").contains("Foo Bar.png"));
    assert!(
        root.join("content/symlink.assets/alias.png")
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(!pending_path(&root).exists());
    assert!(!backup_dir(&root).exists());
}

#[test]
fn provenance_covers_every_outcome_with_final_hashes_and_collision_reasons() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let (_id, src, expected) = fixture(&root);
    transact(&root, &src, &expected).unwrap();
    let adoption = store::read_adoption_record(&root, "asset-run")
        .unwrap()
        .unwrap();
    assert!(adoption.pages.is_empty(), "no page ids are invented");

    // Every manifest row lands in provenance exactly once with a status.
    let manifest: serde_json::Value = serde_json::from_slice(
        &store::read_out_file(&root, "asset-run", "asset_filename_manifest.json").unwrap(),
    )
    .unwrap();
    let row_count = manifest["assets"].as_array().unwrap().len();
    let asset_rows: Vec<_> = adoption
        .asset_operations
        .iter()
        .filter(|o| o.kind == store::AssetOperationKind::Asset)
        .collect();
    assert_eq!(asset_rows.len(), row_count);
    for op in &asset_rows {
        match op.status {
            store::AssetOperationStatus::SkippedCollision => {
                assert!(
                    op.reason.contains("collision"),
                    "skipped row must name the collision: {op:?}"
                );
            }
            store::AssetOperationStatus::Applied | store::AssetOperationStatus::Resumed => {
                assert!(op.sha256.is_some() && op.bytes > 0, "{op:?}");
            }
            store::AssetOperationStatus::Unchanged | store::AssetOperationStatus::Rejected => {}
        }
    }

    // Markdown rows: written pages carry the final post-image hash/bytes;
    // collision-blocked pages name the blocking reason and carry no hash.
    let markdown: Vec<_> = adoption
        .asset_operations
        .iter()
        .filter(|o| o.kind == store::AssetOperationKind::Markdown)
        .collect();
    let applied: Vec<_> = markdown
        .iter()
        .filter(|o| o.status == store::AssetOperationStatus::Applied)
        .collect();
    let blocked: Vec<_> = markdown
        .iter()
        .filter(|o| o.status == store::AssetOperationStatus::SkippedCollision)
        .collect();
    assert_eq!(applied.len(), 4, "{markdown:?}");
    assert_eq!(blocked.len(), 2, "{markdown:?}");
    for op in applied {
        let live = std::fs::read(root.join("content").join(&op.target)).unwrap();
        assert_eq!(
            op.sha256.as_deref(),
            Some(store::sha256_hex(&live).as_str())
        );
        assert_eq!(op.bytes, live.len() as u64);
        assert!(op.rewrites >= 1);
    }
    for op in blocked {
        assert!(op.reason.contains("collision"), "{op:?}");
        assert!(op.sha256.is_none(), "{op:?}");
    }
}

#[test]
fn stale_markdown_refuses_before_the_first_write() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let (_id, src, expected) = fixture(&root);
    std::fs::write(
        root.join("content/spaces.md"),
        "user edit after the lab ran\n",
    )
    .unwrap();
    let err = transact(&root, &src, &expected).unwrap_err();
    assert!(matches!(err, adopt::AdoptError::Stale(_)), "{err}");
    assert_no_content_effect(&root);
    assert!(store::read_pending(&root, "asset-run").unwrap().is_none());
}

#[test]
fn foreign_destination_is_never_overwritten() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let (_id, src, expected) = fixture(&root);
    // A file that appeared at a would-be destination after the lab ran is a
    // live entry outside the inventory: refuse, never overwrite.
    write(
        &root.join("content/spaces.assets/hello-world.png"),
        b"foreign",
    );
    let err = transact(&root, &src, &expected).unwrap_err();
    assert!(matches!(err, adopt::AdoptError::Stale(_)), "{err}");
    assert_eq!(
        std::fs::read(root.join("content/spaces.assets/hello-world.png")).unwrap(),
        b"foreign"
    );
    assert!(!backup_dir(&root).exists());
    assert!(root.join("content/spaces.assets/hello world.png").exists());
}

#[test]
fn completed_transformation_is_an_idempotent_noop() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let (_id, src, expected) = fixture(&root);
    let first = transact(&root, &src, &expected).unwrap();
    assert!(first.written > 0);
    let second = transact(&root, &src, &expected).unwrap();
    assert_eq!((second.planned, second.written, second.resumed), (0, 0, 0));
    assert!(root.join("content/spaces.assets/hello-world.png").exists());
}

#[test]
fn crash_during_destination_link_resumes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let (_id, src, expected) = fixture(&root);
    journal_only(&root, &src, &expected);
    // Crash after all backups but only the first destination link.
    link_backups(&root, |_| true);
    let first = all_moves(&root)[0].dest.clone();
    link_dests(&root, |m| m.dest == first);

    let report = transact(&root, &src, &expected).unwrap();
    assert!(report.written > 0 && report.resumed > 0, "{report:?}");
    for rel in [
        "nested.assets/deep/sub-dir/shot.png",
        "percent.assets/diagram-copy.png",
        "spaces.assets/hello-world.png",
        "unicode.assets/caf.png",
    ] {
        assert!(root.join("content").join(rel).is_file(), "{rel}");
    }
    assert!(!pending_path(&root).exists());
    assert!(!backup_dir(&root).exists());
}

#[test]
fn crash_after_first_markdown_write_resumes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let (_id, src, expected) = fixture(&root);
    journal_only(&root, &src, &expected);
    link_backups(&root, |_| true);
    link_dests(&root, |_| true);
    write_page_post(&root, "nested.md");

    let report = transact(&root, &src, &expected).unwrap();
    assert!(report.resumed > 0, "{report:?}");
    for page in ["nested.md", "percent.md", "spaces.md", "unicode.md"] {
        let live = std::fs::read(root.join("content").join(page)).unwrap();
        let post = store::read_out_file(&root, "asset-run", &format!("content/{page}")).unwrap();
        assert_eq!(live, post, "{page} must hold the post-image");
    }
    assert!(!pending_path(&root).exists());
    assert!(!backup_dir(&root).exists());
}

#[test]
fn fully_linked_backup_state_resumes_every_move_as_hard_links() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let (_id, src, expected) = fixture(&root);
    journal_only(&root, &src, &expected);
    link_backups(&root, |_| true);
    link_dests(&root, |_| true);

    let report = transact(&root, &src, &expected).unwrap();
    assert_eq!(report.asset_moves, 4);
    let adoption = store::read_adoption_record(&root, "asset-run")
        .unwrap()
        .unwrap();
    let resumed = adoption
        .asset_operations
        .iter()
        .filter(|o| {
            o.kind == store::AssetOperationKind::Asset
                && o.status == store::AssetOperationStatus::Resumed
        })
        .count();
    assert_eq!(resumed, 4, "every journaled move resumes via its links");
    assert!(!backup_dir(&root).exists());
}

#[test]
fn user_restored_source_survives_instead_of_being_taken_for_a_partial() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let (_id, src, expected) = fixture(&root);
    journal_only(&root, &src, &expected);
    // The user deletes the asset and restores a byte-identical copy: same
    // bytes, new identity. The resume must not unlink it.
    let rel = "spaces.assets/hello world.png";
    let bytes = std::fs::read(root.join("content").join(rel)).unwrap();
    std::fs::remove_file(root.join("content").join(rel)).unwrap();
    write(&root.join("content").join(rel), bytes);

    let err = transact(&root, &src, &expected).unwrap_err();
    assert!(matches!(err, adopt::AdoptError::Foreign(_)), "{err}");
    assert!(err.to_string().contains("journaled move source"), "{err}");
    assert_eq!(
        std::fs::read(root.join("content").join(rel)).unwrap(),
        b"asset-stub-v1-spaces\n"
    );
    assert!(pending_path(&root).is_file(), "journal stays for review");
}

#[test]
fn foreign_destination_replacement_after_journaling_refuses_and_is_preserved() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let (_id, src, expected) = fixture(&root);
    journal_only(&root, &src, &expected);
    write(
        &root.join("content/spaces.assets/hello-world.png"),
        b"foreign-bytes",
    );
    let err = transact(&root, &src, &expected).unwrap_err();
    assert!(matches!(err, adopt::AdoptError::Foreign(_)), "{err}");
    assert_eq!(
        std::fs::read(root.join("content/spaces.assets/hello-world.png")).unwrap(),
        b"foreign-bytes"
    );
    assert!(root.join("content/spaces.assets/hello world.png").exists());
    assert!(pending_path(&root).is_file());
}

#[test]
fn in_place_source_write_after_journaling_is_detected() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let (_id, src, expected) = fixture(&root);
    journal_only(&root, &src, &expected);
    // Rewrite the source in place: same path, same inode, new bytes.
    write(
        &root.join("content/spaces.assets/hello world.png"),
        b"edited-in-place",
    );
    let err = transact(&root, &src, &expected).unwrap_err();
    assert!(matches!(err, adopt::AdoptError::Foreign(_)), "{err}");
    assert!(err.to_string().contains("journaled move source"), "{err}");
    assert_eq!(
        std::fs::read(root.join("content/spaces.assets/hello world.png")).unwrap(),
        b"edited-in-place"
    );
}

#[test]
fn corrupted_markdown_journal_refuses_before_any_asset_write() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let (_id, src, expected) = fixture(&root);
    journal_only(&root, &src, &expected);
    patch_journal(&root, |v| {
        v["transformation"]["components"][0]["markdown"][0]["pre_sha256"] =
            serde_json::Value::String("0".repeat(64));
    });
    let err = transact(&root, &src, &expected).unwrap_err();
    assert!(matches!(err, adopt::AdoptError::Run(_)), "{err}");
    assert_no_content_effect(&root);
    assert!(pending_path(&root).is_file());
}

#[test]
fn foreign_addition_after_journaling_blocks_recovery() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let (_id, src, expected) = fixture(&root);
    journal_only(&root, &src, &expected);
    write(
        &root.join("content/late.md"),
        page("late", "Late", "# Late\n"),
    );
    let err = transact(&root, &src, &expected).unwrap_err();
    assert!(matches!(err, adopt::AdoptError::Foreign(_)), "{err}");
    assert!(err.to_string().contains("appeared after"), "{err}");
    assert_no_content_effect(&root);
    assert!(pending_path(&root).is_file());
}

#[test]
fn foreign_edit_of_an_untouched_file_after_journaling_blocks_recovery() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let (_id, src, expected) = fixture(&root);
    journal_only(&root, &src, &expected);
    write(&root.join("content/safe.md"), "foreign edit\n");
    let err = transact(&root, &src, &expected).unwrap_err();
    assert!(matches!(err, adopt::AdoptError::Foreign(_)), "{err}");
    assert!(err.to_string().contains("changed after"), "{err}");
    assert_no_content_effect(&root);
}

#[test]
fn rewrite_row_for_an_absent_candidate_page_is_malformed() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let (_id, src, expected) = fixture(&root);
    // Point one rewrite row at a page the candidate does not contain.
    patch_out(&root, "rewrite_manifest.json", |v| {
        v["rewrites"][0]["page_path"] = serde_json::Value::String("ghost.md".into());
    });
    let err = transact(&root, &src, &expected).unwrap_err();
    assert!(matches!(err, adopt::AdoptError::Report(_)), "{err}");
    assert!(
        err.to_string().contains("absent from the lab candidate"),
        "{err}"
    );
    assert_no_content_effect(&root);
}

#[test]
fn unchanged_row_whose_destination_differs_is_malformed() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let (_id, src, expected) = fixture(&root);
    patch_out(&root, "asset_filename_manifest.json", |v| {
        for row in v["assets"].as_array_mut().unwrap() {
            if row["action"] == "unchanged" {
                // An empty destination skips candidate acceptance but
                // violates the unchanged-row invariant itself.
                row["dest_path"] = serde_json::Value::String(String::new());
            }
        }
    });
    let err = transact(&root, &src, &expected).unwrap_err();
    assert!(matches!(err, adopt::AdoptError::Report(_)), "{err}");
    assert!(err.to_string().contains("unchanged"), "{err}");
    assert_no_content_effect(&root);
}

#[test]
fn ambiguous_crash_left_temp_is_foreign_and_never_replaced() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let (_id, src, expected) = fixture(&root);
    journal_only(&root, &src, &expected);
    // A temp file at the journaled staging path whose identity was never
    // recorded — ambiguous foreign data.
    let j = journal(&root);
    let temp_rel = j.transformation.as_ref().unwrap().components[0].markdown[0]
        .temp
        .clone();
    let temp_abs = root.join("content").join(&temp_rel);
    write(&temp_abs, b"ambiguous partial temp\n");

    let err = transact(&root, &src, &expected).unwrap_err();
    assert!(matches!(err, adopt::AdoptError::Foreign(_)), "{err}");
    assert!(err.to_string().contains("unexplained"), "{err}");
    assert_eq!(
        std::fs::read(&temp_abs).unwrap(),
        b"ambiguous partial temp\n"
    );
    assert!(pending_path(&root).is_file());
}

#[test]
fn journaled_move_identity_that_no_longer_matches_refuses_recovery() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let (_id, src, expected) = fixture(&root);
    journal_only(&root, &src, &expected);
    patch_journal(&root, |v| {
        v["transformation"]["components"][0]["asset_moves"][0]["identity"]["index"] =
            serde_json::json!(1);
    });
    let err = transact(&root, &src, &expected).unwrap_err();
    assert!(matches!(err, adopt::AdoptError::Run(_)), "{err}");
    assert!(err.to_string().contains("component plan"), "{err}");
    assert_no_content_effect(&root);
}

#[test]
fn evidence_mutated_after_journaling_invalidates_the_journal() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let (_id, src, expected) = fixture(&root);
    journal_only(&root, &src, &expected);
    patch_out(&root, "asset_filename_manifest.json", |v| {
        v["policy"] = serde_json::Value::String("tampered".into());
    });
    let err = transact(&root, &src, &expected).unwrap_err();
    assert!(matches!(err, adopt::AdoptError::Run(_)), "{err}");
    assert_no_content_effect(&root);
    assert!(pending_path(&root).is_file());
}

#[test]
fn run_without_input_inventory_is_a_run_again_error() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let (_id, src, expected) = fixture(&root);
    std::fs::remove_file(run_dir(&root).join(store::INPUT_INVENTORY)).unwrap();
    let err = transact(&root, &src, &expected).unwrap_err();
    assert!(matches!(err, adopt::AdoptError::Run(_)), "{err}");
    assert!(err.to_string().contains("input inventory"), "{err}");
    assert_no_content_effect(&root);
}

#[test]
fn journaled_plan_from_a_different_workspace_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let (_id, src, expected) = fixture(&root);
    journal_only(&root, &src, &expected);
    // The same journal file in a foreign workspace cannot bind this one.
    patch_journal(&root, |v| {
        v["transformation"]["workspace_token"] = serde_json::json!("deadbeef");
    });
    let err = transact(&root, &src, &expected).unwrap_err();
    assert!(matches!(err, adopt::AdoptError::Run(_)), "{err}");
    assert!(err.to_string().contains("different workspace"), "{err}");
    assert_no_content_effect(&root);
}

#[test]
fn workspace_additions_after_the_lab_invalidate_fresh_evidence() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let (_id, src, expected) = fixture(&root);
    write(
        &root.join("content/late.md"),
        page("late", "Late page", "# Late\n"),
    );
    let err = transact(&root, &src, &expected).unwrap_err();
    assert!(matches!(err, adopt::AdoptError::Stale(_)), "{err}");
    assert_no_content_effect(&root);
    assert!(store::read_pending(&root, "asset-run").unwrap().is_none());
}

#[test]
fn journaled_identity_bound_asset_is_unlinked_only_while_original() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let (_id, src, expected) = fixture(&root);
    journal_only(&root, &src, &expected);
    link_backups(&root, |_| true);
    link_dests(&root, |_| true);
    for p in ["nested.md", "percent.md", "spaces.md", "unicode.md"] {
        write_page_post(&root, p);
    }

    // Between MarkdownWritten and SourcesUnlinked the user replaced a source
    // with a byte-identical file: different identity, must survive.
    let rel = "unicode.assets/café.png";
    let bytes = std::fs::read(root.join("content").join(rel)).unwrap();
    std::fs::remove_file(root.join("content").join(rel)).unwrap();
    write(&root.join("content").join(rel), bytes);

    let err = transact(&root, &src, &expected).unwrap_err();
    assert!(matches!(err, adopt::AdoptError::Foreign(_)), "{err}");
    assert_eq!(
        std::fs::read(root.join("content").join(rel)).unwrap(),
        b"asset-stub-v1-unicode\n",
        "restored source must survive — no unlink of foreign identity"
    );
    // Every other source is still present: the whole attempt aborted before
    // the unlink phase.
    assert!(root.join("content/spaces.assets/hello world.png").exists());
}
