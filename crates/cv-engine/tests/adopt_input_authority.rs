//! Adoption input authority refuses pathological spellings before mutation
//! and preserves legitimate relative inputs byte-exactly.

use cv_engine::adopt;
use cv_fixtures::lab::StubLab;
use cv_store::imports::{self as store, Finished, RecordedLab, RunRecord};
use std::collections::BTreeMap;
use std::path::Path;

fn hugo_source() -> (tempfile::TempDir, String) {
    let source = tempfile::tempdir().unwrap();
    let run_id = "input-authority-hugo";
    let source_root = source.path().canonicalize().unwrap();
    let lock = store::claim_run(&source_root, run_id).unwrap();
    let transcript = StubLab::new()
        .run("hugo", &lock.dir().join(store::OUT))
        .unwrap();
    let mut rec = RunRecord::started(
        run_id,
        "hugo",
        RecordedLab {
            tool_id: StubLab::new().tool_id(),
            source: "fixture".into(),
            path: "fixtures/captures/lab".into(),
        },
        1,
    );
    rec.argv = transcript.argv.clone();
    rec.finished = Some(Finished {
        exit_code: Some(0),
        exit: store::FinishedExit::Ok,
        outcome: "exited 0".into(),
        duration_ms: 0,
    });
    store::write(&lock, &rec).unwrap();
    (source, run_id.into())
}

#[test]
fn subfolder_errors_name_the_content_input_not_a_hardcoded_content_tree() {
    let absolute = adopt::subfolder("/abs").unwrap_err().to_string();
    assert!(absolute.contains("content input"), "{absolute}");
    assert!(!absolute.contains("content/"), "{absolute}");
    let empty = adopt::subfolder("").unwrap_err().to_string();
    assert!(empty.contains("content input"), "{empty}");
    assert!(!empty.contains("content/"), "{empty}");
}

#[test]
fn pathological_input_authority_is_refused_before_the_lock_carrier_or_content() {
    let (source, run_id) = hugo_source();
    let src = adopt::source(&source.path().canonicalize().unwrap(), &run_id).unwrap();
    let absolute = Path::new("/private/input").to_string_lossy().into_owned();
    for input in ["", ".", " ", " content", "content ", "content/", &absolute] {
        let target = tempfile::tempdir().unwrap();
        let target_root = target.path().canonicalize().unwrap();
        let err = adopt::transact(&target_root, &src, input, None, "new", &BTreeMap::new(), 1)
            .unwrap_err();
        assert!(
            err.to_string().contains("content input"),
            "{input:?}: {err}"
        );
        assert!(
            !target_root.join(".cv").exists(),
            "{input:?}: validation happened after mutation setup"
        );
    }
}

#[test]
fn relative_unicode_and_spaced_input_authority_is_preserved_byte_exactly() {
    let (source, run_id) = hugo_source();
    let src = adopt::source(&source.path().canonicalize().unwrap(), &run_id).unwrap();
    let input = "contént with spaces";
    let planned = adopt::plan(&src, input, Some("imported")).unwrap();
    assert!(
        planned
            .iter()
            .all(|p| p.target.starts_with("contént with spaces/imported/")),
        "input authority must not be normalized into a different tree"
    );
}

#[test]
fn metadata_and_output_lane_inputs_are_refused_before_the_carrier() {
    let (source, run_id) = hugo_source();
    let src = adopt::source(&source.path().canonicalize().unwrap(), &run_id).unwrap();
    for input in [
        ".cv",
        ".cv/publish",
        ".cv/imports",
        ".CV/publish",
        ".boris",
        ".boris/cache",
    ] {
        let target = tempfile::tempdir().unwrap();
        let target_root = target.path().canonicalize().unwrap();
        let err = adopt::transact(&target_root, &src, input, None, "new", &BTreeMap::new(), 1)
            .unwrap_err();
        assert!(
            err.to_string().contains("metadata") || err.to_string().contains("output lane"),
            "{input:?}: {err}"
        );
        assert!(
            !target_root.join(".cv").exists(),
            "{input:?}: validation happened after carrier creation"
        );
    }
}

#[cfg(unix)]
#[test]
fn a_symlinked_input_prefix_resolving_into_metadata_is_refused_before_the_carrier() {
    let (source, run_id) = hugo_source();
    let src = adopt::source(&source.path().canonicalize().unwrap(), &run_id).unwrap();
    let target = tempfile::tempdir().unwrap();
    let target_root = target.path().canonicalize().unwrap();
    // `.cv` does not exist yet: resolution must still understand the broken
    // link's future target rather than treating the link as a harmless
    // missing directory.
    std::os::unix::fs::symlink(".cv", target_root.join("linked-input")).unwrap();
    let err = adopt::transact(
        &target_root,
        &src,
        "linked-input",
        None,
        "new",
        &BTreeMap::new(),
        1,
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("resolves into workspace metadata"),
        "{err}"
    );
    assert!(!target_root.join(".cv").exists());
}

#[test]
fn existing_workspace_input_inside_import_evidence_is_refused_before_mutation() {
    let (source, run_id) = hugo_source();
    let src = adopt::source(&source.path().canonicalize().unwrap(), &run_id).unwrap();
    let target = tempfile::tempdir().unwrap();
    let target_root = target.path().canonicalize().unwrap();
    let token = cv_store::workspace_instance::ensure(&target_root).unwrap();
    let expected = adopt::ExpectedWorkspaceInstance::new(
        token,
        cv_engine::file_identity::identity(&target_root),
        cv_store::workspace_mutation::current_identity(&target_root).unwrap(),
    );
    let err = adopt::transact_existing_with_identity(adopt::ExistingWorkspaceTransaction {
        target_root: &target_root,
        src: &src,
        content_input: ".cv/imports",
        merge_subfolder: Some("imported"),
        scan_existing_ids: Box::new(BTreeMap::new),
        at_ms: 1,
        expected_instance: &expected,
    })
    .unwrap_err();
    assert!(
        err.to_string().contains("metadata") || err.to_string().contains("output lane"),
        "{err}"
    );
    assert!(
        !target_root
            .join(".cv/imports")
            .join(&run_id)
            .join(store::PENDING)
            .exists(),
        "metadata input must not reach journal creation"
    );
}

#[cfg(unix)]
#[test]
fn parent_dir_symlink_input_resolving_into_metadata_is_refused_pre_carrier() {
    let (source, run_id) = hugo_source();
    let src = adopt::source(&source.path().canonicalize().unwrap(), &run_id).unwrap();
    let target = tempfile::tempdir().unwrap();
    let target_root = target.path().canonicalize().unwrap();
    // `future/../.cv` is dangling until the transaction would create `.cv`.
    // Lexical parent arithmetic must still prove that link/sub denotes
    // `.cv/sub`, before the carrier can bootstrap the link into existence.
    std::os::unix::fs::symlink("future/../.cv", target_root.join("link")).unwrap();
    let err = adopt::transact(
        &target_root,
        &src,
        "link/sub",
        None,
        "new",
        &BTreeMap::new(),
        1,
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("resolves into workspace metadata"),
        "{err}"
    );
    assert!(!target_root.join(".cv").exists(), "carrier must not exist");
    assert!(
        !target_root.join(".cv/workspace-mutation.lock").exists(),
        "mutation lock must not exist"
    );
}
