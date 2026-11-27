//! Regression tests for operation-based in-place asset-filename adoption.
//! The lab output is the pinned capture; only the workspace content that
//! capture was captured from is reconstructed in scratch space.

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

fn symlink_dir(target: &Path, link: &Path) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(target, link).unwrap();
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

fn transact(
    root: &Path,
    src: &adopt::AdoptionSource,
    expected: &ExpectedWorkspaceInstance,
) -> Result<adopt::TxReport, adopt::AdoptError> {
    asset_filename::transact_existing_with_identity(asset_filename::AssetFilenameTransaction {
        target_root: root,
        src,
        content_input: "content",
        at_ms: 2,
        expected_instance: expected,
    })
}

fn read(root: &Path, rel: &str) -> String {
    std::fs::read_to_string(root.join(rel)).unwrap()
}

fn replace_once(haystack: &[u8], needle: &[u8], replacement: &[u8]) -> Vec<u8> {
    let at = haystack
        .windows(needle.len())
        .position(|w| w == needle)
        .unwrap();
    let mut out = Vec::new();
    out.extend_from_slice(&haystack[..at]);
    out.extend_from_slice(replacement);
    out.extend_from_slice(&haystack[at + needle.len()..]);
    out
}

fn asset_op(root: &Path, source: &str, target: &str) -> store::PendingAssetOperation {
    let source_ws = format!("content/{source}");
    let bytes = std::fs::read(root.join(&source_ws)).unwrap();
    let hash = store::sha256_hex(&bytes);
    store::PendingAssetOperation {
        kind: store::AssetOperationKind::Asset,
        source: source_ws,
        target: format!("content/{target}"),
        source_sha256: hash.clone(),
        source_bytes: bytes.len() as u64,
        target_sha256: hash,
        target_bytes: bytes.len() as u64,
    }
}

fn markdown_op(root: &Path, page: &str, rows: &[(&str, &str)]) -> store::PendingAssetOperation {
    let ws = format!("content/{page}");
    let original = std::fs::read(root.join(&ws)).unwrap();
    let mut final_bytes = original.clone();
    for (old, new) in rows {
        final_bytes = replace_once(&final_bytes, old.as_bytes(), new.as_bytes());
    }
    store::PendingAssetOperation {
        kind: store::AssetOperationKind::Markdown,
        source: ws.clone(),
        target: ws,
        source_sha256: store::sha256_hex(&original),
        source_bytes: original.len() as u64,
        target_sha256: store::sha256_hex(&final_bytes),
        target_bytes: final_bytes.len() as u64,
    }
}

/// The complete pre-write operation journal for the pinned fixture, in the
/// deterministic order the planner uses: authorized asset moves in manifest
/// order, then authorized Markdown rewrites in candidate order.
fn complete_operations(root: &Path) -> Vec<store::PendingAssetOperation> {
    vec![
        asset_op(
            root,
            "nested.assets/deep/sub dir/shot.png",
            "nested.assets/deep/sub-dir/shot.png",
        ),
        asset_op(
            root,
            "percent.assets/diagram%20copy.png",
            "percent.assets/diagram-copy.png",
        ),
        asset_op(
            root,
            "spaces.assets/hello world.png",
            "spaces.assets/hello-world.png",
        ),
        asset_op(root, "unicode.assets/café.png", "unicode.assets/caf.png"),
        markdown_op(
            root,
            "nested.md",
            &[(
                "nested.assets/deep/sub dir/shot.png",
                "nested.assets/deep/sub-dir/shot.png",
            )],
        ),
        markdown_op(
            root,
            "percent.md",
            &[(
                "percent.assets/diagram%20copy.png",
                "percent.assets/diagram-copy.png",
            )],
        ),
        markdown_op(
            root,
            "spaces.md",
            &[
                (
                    "spaces.assets/hello world.png",
                    "spaces.assets/hello-world.png",
                ),
                (
                    "spaces.assets/hello%20world.png",
                    "spaces.assets/hello-world.png",
                ),
            ],
        ),
        markdown_op(
            root,
            "unicode.md",
            &[("unicode.assets/café.png", "unicode.assets/caf.png")],
        ),
    ]
}

fn write_operations_journal(root: &Path, operations: Vec<store::PendingAssetOperation>) {
    let mut journal = store::PendingAdoption::new("asset-run", "existing", "content");
    journal.asset_operations = Some(operations);
    let lock = store::lock_run(root, "asset-run").unwrap();
    store::write_pending(&lock, &journal).unwrap();
}

fn pending_path(root: &Path) -> std::path::PathBuf {
    root.join(".cv/imports/asset-run").join(store::PENDING)
}

#[test]
fn transformation_moves_safe_assets_rewrites_pages_and_leaves_collisions_untouched() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let (_id, src, expected) = fixture(&root);
    let tx = transact(&root, &src, &expected).unwrap();
    assert!(tx.written >= 8, "{tx:?}");

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
    assert!(read(&root, "content/sanitize-collision.md").contains("foo bar.png"));
    assert!(
        root.join("content/symlink.assets/alias.png")
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(
        !root
            .join(".cv/imports/asset-run")
            .join(store::PENDING)
            .exists()
    );
    let adoption = store::read_adoption_record(&root, "asset-run")
        .unwrap()
        .unwrap();
    assert!(adoption.pages.is_empty(), "no page ids are invented");
    assert!(
        adoption
            .asset_operations
            .iter()
            .any(|o| o.status == store::AssetOperationStatus::SkippedCollision)
    );
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
    assert!(root.join("content/spaces.assets/hello world.png").exists());
    assert!(!root.join("content/spaces.assets/hello-world.png").exists());
    assert!(store::read_pending(&root, "asset-run").unwrap().is_none());
}

#[test]
fn foreign_destination_is_never_overwritten() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let (_id, src, expected) = fixture(&root);
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
fn journaled_interruption_resumes_without_treating_partial_output_as_foreign() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let (_id, src, expected) = fixture(&root);
    let operations = complete_operations(&root);
    // A crash after the first journaled asset rename: the journal still
    // contains every operation, and disk contains only that partial effect.
    std::fs::create_dir_all(root.join("content/nested.assets/deep/sub-dir")).unwrap();
    std::fs::rename(
        root.join("content/nested.assets/deep/sub dir/shot.png"),
        root.join("content/nested.assets/deep/sub-dir/shot.png"),
    )
    .unwrap();
    write_operations_journal(&root, operations);

    let tx = transact(&root, &src, &expected).unwrap();
    assert_eq!((tx.planned, tx.written, tx.resumed), (8, 7, 1));
    assert_eq!(tx.asset_moves, 4);
    assert_eq!(tx.markdown_writes, 4);
    for rel in [
        "nested.assets/deep/sub-dir/shot.png",
        "percent.assets/diagram-copy.png",
        "spaces.assets/hello-world.png",
        "unicode.assets/caf.png",
    ] {
        assert!(root.join("content").join(rel).is_file(), "{rel}");
    }
    assert!(store::read_pending(&root, "asset-run").unwrap().is_none());
}

#[test]
fn workspace_additions_after_the_lab_invalidate_fresh_evidence() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let (_id, src, expected) = fixture(&root);
    write(
        &root.join("content/late.md"),
        page(
            "late",
            "Late page",
            "# Late\n\n![hero](spaces.assets/hello world.png)\n",
        ),
    );

    let err = transact(&root, &src, &expected).unwrap_err();
    assert!(matches!(err, adopt::AdoptError::Stale(_)), "{err}");
    assert!(root.join("content/spaces.assets/hello world.png").exists());
    assert!(!root.join("content/spaces.assets/hello-world.png").exists());
    assert!(store::read_pending(&root, "asset-run").unwrap().is_none());
}

#[test]
fn partial_journals_are_refused_without_completing_or_clearing_them() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let (_id, src, expected) = fixture(&root);
    let operations = complete_operations(&root);
    write_operations_journal(&root, operations.into_iter().take(2).collect());
    let before = std::fs::read(pending_path(&root)).unwrap();

    let err = transact(&root, &src, &expected).unwrap_err();
    assert!(matches!(err, adopt::AdoptError::Run(_)), "{err}");
    assert!(err.to_string().contains("requires exactly 8"), "{err}");
    assert_eq!(std::fs::read(pending_path(&root)).unwrap(), before);
    assert!(
        store::read_adoption_record(&root, "asset-run")
            .unwrap()
            .is_none()
    );
    assert!(
        root.join("content/nested.assets/deep/sub dir/shot.png")
            .exists()
    );
    assert!(
        !root
            .join("content/nested.assets/deep/sub-dir/shot.png")
            .exists()
    );
}

#[test]
fn forged_journals_cannot_move_files_outside_the_content_input() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let (_id, src, expected) = fixture(&root);
    write(&root.join("boris.json"), br#"{"input":"content"}"#);
    let bytes = std::fs::read(root.join("boris.json")).unwrap();
    let hash = store::sha256_hex(&bytes);
    write_operations_journal(
        &root,
        vec![store::PendingAssetOperation {
            kind: store::AssetOperationKind::Asset,
            source: "boris.json".into(),
            target: "moved-boris.json".into(),
            source_sha256: hash.clone(),
            source_bytes: bytes.len() as u64,
            target_sha256: hash,
            target_bytes: bytes.len() as u64,
        }],
    );

    let err = transact(&root, &src, &expected).unwrap_err();
    assert!(matches!(err, adopt::AdoptError::Run(_)), "{err}");
    assert!(root.join("boris.json").is_file());
    assert!(!root.join("moved-boris.json").exists());
    assert!(pending_path(&root).is_file());
    assert!(
        store::read_adoption_record(&root, "asset-run")
            .unwrap()
            .is_none()
    );
}

#[test]
fn aliased_journal_operations_are_refused_without_deleting_the_only_copy() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let (_id, src, expected) = fixture(&root);
    let rel = "content/safe.assets/already-ok.png";
    let bytes = std::fs::read(root.join(rel)).unwrap();
    let hash = store::sha256_hex(&bytes);
    write_operations_journal(
        &root,
        vec![store::PendingAssetOperation {
            kind: store::AssetOperationKind::Asset,
            source: rel.into(),
            target: rel.into(),
            source_sha256: hash.clone(),
            source_bytes: bytes.len() as u64,
            target_sha256: hash,
            target_bytes: bytes.len() as u64,
        }],
    );

    let err = transact(&root, &src, &expected).unwrap_err();
    assert!(matches!(err, adopt::AdoptError::Run(_)), "{err}");
    assert_eq!(std::fs::read(root.join(rel)).unwrap(), bytes);
    assert!(pending_path(&root).is_file());
    assert!(
        store::read_adoption_record(&root, "asset-run")
            .unwrap()
            .is_none()
    );
}

#[test]
fn corrupted_journals_are_refused_and_retained() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let (_id, src, expected) = fixture(&root);
    let pending = pending_path(&root);
    write(&pending, b"{not valid adoption json");

    let err = transact(&root, &src, &expected).unwrap_err();
    assert!(matches!(err, adopt::AdoptError::Report(_)), "{err}");
    assert_eq!(
        std::fs::read(&pending).unwrap(),
        b"{not valid adoption json"
    );
    assert!(
        store::read_adoption_record(&root, "asset-run")
            .unwrap()
            .is_none()
    );
    assert!(root.join("content/spaces.assets/hello world.png").exists());
}

#[test]
fn symlinked_machine_evidence_is_refused_without_writes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let (_id, src, expected) = fixture(&root);
    let manifest = root.join(".cv/imports/asset-run/out/asset_filename_manifest.json");
    let copy = root.join("manifest-copy.json");
    std::fs::write(&copy, std::fs::read(&manifest).unwrap()).unwrap();
    std::fs::remove_file(&manifest).unwrap();
    symlink_file(&copy, &manifest);

    let err = transact(&root, &src, &expected).unwrap_err();
    assert!(matches!(err, adopt::AdoptError::Report(_)), "{err}");
    assert!(
        std::fs::symlink_metadata(&manifest)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(!root.join("content/spaces.assets/hello-world.png").exists());
    assert!(store::read_pending(&root, "asset-run").unwrap().is_none());
}

#[test]
fn nonordinary_machine_evidence_is_refused_without_writes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let (_id, src, expected) = fixture(&root);
    let manifest = root.join(".cv/imports/asset-run/out/rewrite_manifest.json");
    std::fs::remove_file(&manifest).unwrap();
    std::fs::create_dir(&manifest).unwrap();

    let err = transact(&root, &src, &expected).unwrap_err();
    assert!(matches!(err, adopt::AdoptError::Report(_)), "{err}");
    assert!(!root.join("content/spaces.assets/hello-world.png").exists());
    assert!(store::read_pending(&root, "asset-run").unwrap().is_none());
}

#[test]
fn the_initial_report_load_does_not_follow_a_symlink() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let (_id, _src, _expected) = fixture(&root);
    let report = root.join(".cv/imports/asset-run/out/report.json");
    let copy = root.join("report-copy.json");
    std::fs::write(&copy, std::fs::read(&report).unwrap()).unwrap();
    std::fs::remove_file(&report).unwrap();
    symlink_file(&copy, &report);

    assert!(adopt::source(&root, "asset-run").is_err());
    assert!(
        std::fs::symlink_metadata(&report)
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[test]
fn a_symlinked_asset_ancestor_is_never_followed() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let (_id, src, expected) = fixture(&root);
    let outside = tempfile::tempdir().unwrap();
    let outside_assets = outside.path().join("spaces.assets");
    std::fs::create_dir(&outside_assets).unwrap();
    std::fs::copy(
        root.join("content/spaces.assets/hello world.png"),
        outside_assets.join("hello world.png"),
    )
    .unwrap();
    std::fs::remove_dir_all(root.join("content/spaces.assets")).unwrap();
    symlink_dir(&outside_assets, &root.join("content/spaces.assets"));

    assert!(transact(&root, &src, &expected).is_err());
    assert!(outside_assets.join("hello world.png").is_file());
    assert!(store::read_pending(&root, "asset-run").unwrap().is_none());
}
