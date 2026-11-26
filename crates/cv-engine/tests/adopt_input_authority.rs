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
