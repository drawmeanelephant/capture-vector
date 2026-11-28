//! Shared harness for the journaled in-place asset-filename adoption
//! probes: stages the StubLab workspace, seeds the captured run, and
//! exposes the transaction and journal helpers the regression files use.
//! Mirrors the local harness in adopt_asset_filename.rs.

use cv_engine::adopt::{self, ExpectedWorkspaceInstance, asset_filename};
use cv_fixtures::lab::StubLab;
use cv_store::imports::{self as store, Finished, RecordedLab, RunRecord};
use std::path::Path;

pub fn write(path: &Path, bytes: impl AsRef<[u8]>) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

pub fn symlink_file(target: &Path, link: &Path) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(target, link).unwrap();
}

pub fn page(id: &str, title: &str, body: &str) -> String {
    format!("---\nid: {id}\ntitle: {title}\nstatus: published\n---\n\n{body}")
}

pub fn stage_workspace(root: &Path) {
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

pub fn seed_run(root: &Path, run_id: &str) {
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

pub fn fixture(root: &Path) -> (String, adopt::AdoptionSource, ExpectedWorkspaceInstance) {
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

pub fn tx<'a>(
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

/// A transaction whose volume lookup is injected — the test seam for a
/// cross-volume boundary inside the content input.
pub fn tx_with_volume<'a>(
    root: &'a Path,
    src: &'a adopt::AdoptionSource,
    expected: &'a ExpectedWorkspaceInstance,
    volume_of: &'a dyn Fn(&Path) -> Option<u64>,
) -> asset_filename::AssetFilenameTransaction<'a> {
    asset_filename::AssetFilenameTransaction {
        target_root: root,
        src,
        content_input: "content",
        at_ms: 2,
        expected_instance: expected,
        volume_of: Some(volume_of),
    }
}

/// Volume lookup reporting `foreign_dir` as a volume distinct from the
/// run directory's — every other path keeps its real identity.
pub fn foreign_volume_at(foreign_dir: &Path) -> impl Fn(&Path) -> Option<u64> + '_ {
    move |path| {
        if path == foreign_dir {
            Some(u64::MAX)
        } else {
            cv_engine::file_identity::identity(path).map(|i| i.0)
        }
    }
}

pub fn transact(
    root: &Path,
    src: &adopt::AdoptionSource,
    expected: &ExpectedWorkspaceInstance,
) -> Result<adopt::TxReport, adopt::AdoptError> {
    asset_filename::transact_existing_with_identity(tx(root, src, expected))
}

pub fn journal_only(
    root: &Path,
    src: &adopt::AdoptionSource,
    expected: &ExpectedWorkspaceInstance,
) {
    asset_filename::journal_only_for_tests(tx(root, src, expected)).unwrap();
}

pub fn journal_result(
    root: &Path,
    src: &adopt::AdoptionSource,
    expected: &ExpectedWorkspaceInstance,
) -> Result<store::PendingAdoption, adopt::AdoptError> {
    asset_filename::journal_only_for_tests(tx(root, src, expected))
}

pub fn read(root: &Path, rel: &str) -> String {
    std::fs::read_to_string(root.join(rel)).unwrap()
}

pub fn run_dir(root: &Path) -> std::path::PathBuf {
    root.join(".cv/imports/asset-run")
}

pub fn pending_path(root: &Path) -> std::path::PathBuf {
    run_dir(root).join(store::PENDING)
}

pub fn journal(root: &Path) -> store::PendingAdoption {
    store::read_pending(root, "asset-run").unwrap().unwrap()
}

pub fn all_moves(root: &Path) -> Vec<store::PlannedAssetMove> {
    journal(root)
        .transformation
        .as_ref()
        .unwrap()
        .components
        .iter()
        .flat_map(|c| c.asset_moves.clone())
        .collect()
}

pub fn patch_journal(root: &Path, patch: impl FnOnce(&mut serde_json::Value)) {
    let mut v: serde_json::Value =
        serde_json::from_slice(&std::fs::read(pending_path(root)).unwrap()).unwrap();
    patch(&mut v);
    std::fs::write(pending_path(root), serde_json::to_vec(&v).unwrap()).unwrap();
}

pub fn patch_out(root: &Path, rel: &str, patch: impl FnOnce(&mut serde_json::Value)) {
    let path = run_dir(root).join(store::OUT).join(rel);
    let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    patch(&mut v);
    std::fs::write(&path, serde_json::to_vec(&v).unwrap()).unwrap();
}

pub fn backup_dir(root: &Path) -> std::path::PathBuf {
    run_dir(root).join("asset-backup")
}

pub fn assert_no_content_effect(root: &Path) {
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
}

pub fn ops_for(
    root: &Path,
    kind: store::AssetOperationKind,
    source: &str,
) -> Vec<store::AssetOperationProvenance> {
    store::read_adoption_record(root, "asset-run")
        .unwrap()
        .unwrap()
        .asset_operations
        .into_iter()
        .filter(|o| o.kind == kind && o.source == source)
        .collect()
}
