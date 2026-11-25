mod support;

use cv_ledger::journal::Entry;
use cv_publish::plan::SealedPlan;
use cv_publish::secrets::NoSecrets;
use cv_publish::{PreflightOutcome, PreflightRequest, Publisher};
use support::*;

#[test]
fn deployable_plan_excludes_proof_evidence_and_uninventoried_output() {
    let dir = tempfile::tempdir().unwrap();
    let eng = CapturedEngine::new(dir.path());
    let mut p = Publisher::open(dir.path(), &eng, &CLOCK, &NoSecrets).unwrap();
    let run = granted(
        p.preflight(
            &PreflightRequest { config: cfg() },
            &Rules::ok(),
            &mut ids("run"),
        )
        .unwrap(),
    );
    let plan: SealedPlan = serde_json::from_value(p.run(&run).unwrap().plan.unwrap()).unwrap();
    assert!(!plan.files.is_empty());
    assert!(
        plan.files
            .iter()
            .all(|f| !f.path.starts_with("_boris/proof/"))
    );
    assert!(plan.files.iter().all(|f| f.path != "robots.txt"));
}

#[test]
fn missing_or_malformed_artifact_inventory_is_a_durable_preflight_block() {
    for malformed in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let mut eng = CapturedEngine::new(dir.path());
        eng.omit_proof = !malformed;
        eng.malformed_proof = malformed;
        let mut p = Publisher::open(dir.path(), &eng, &CLOCK, &NoSecrets).unwrap();
        let out = p
            .preflight(
                &PreflightRequest { config: cfg() },
                &Rules::ok(),
                &mut ids("run"),
            )
            .unwrap();
        let PreflightOutcome::Blocked { run, blocks } = out else {
            panic!("proof inventory defect must block: {out:?}")
        };
        assert!(
            blocks.iter().any(|b| b.starts_with("proof artifacts:")),
            "{blocks:?}"
        );
        assert!(
            p.journal().records().iter().any(|r| matches!(
                &r.entry,
                Entry::PreflightBlocked { run: rrun, .. } if *rrun == run
            )),
            "the block must be durable"
        );
    }
}

#[test]
fn source_input_mutation_during_preflight_blocks_the_grant() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("content")).unwrap();
    std::fs::write(dir.path().join("content/index.md"), "# home\n").unwrap();
    let mut eng = FakeEngine::new(dir.path());
    eng.mutate_source_on_ir = true;
    let mut p = Publisher::open(dir.path(), &eng, &CLOCK, &NoSecrets).unwrap();
    let out = p
        .preflight(
            &PreflightRequest { config: cfg() },
            &Rules::ok(),
            &mut ids("run"),
        )
        .unwrap();
    let PreflightOutcome::Blocked { blocks, .. } = out else {
        panic!("source mutation must block: {out:?}")
    };
    assert!(
        blocks
            .iter()
            .any(|b| b == "source inputs changed during preflight"),
        "{blocks:?}"
    );
    assert!(
        blocks
            .iter()
            .any(|b| b == "graph: injected.html has no sealed HTML artifact"),
        "{blocks:?}"
    );
}

#[test]
fn proof_target_mismatch_blocks_preflight_before_a_grant_is_issued() {
    let dir = tempfile::tempdir().unwrap();
    let mut eng = FakeEngine::new(dir.path());
    eng.proof_target = Some("public".into());
    let mut p = Publisher::open(dir.path(), &eng, &CLOCK, &NoSecrets).unwrap();

    let out = p
        .preflight(
            &PreflightRequest { config: cfg() },
            &Rules::ok(),
            &mut ids("run"),
        )
        .unwrap();

    let PreflightOutcome::Blocked { blocks, .. } = out else {
        panic!("mismatched proof target must block preflight: {out:?}")
    };
    assert!(
        blocks
            .iter()
            .any(|b| b == "proof artifacts: target public is not default"),
        "{blocks:?}"
    );
    assert!(
        !p.journal()
            .records()
            .iter()
            .any(|r| matches!(r.entry, Entry::GrantIssued { .. })),
        "a grant must never seal artifacts from another proof target"
    );
}

/// A symlinked directory cycle inside the source tree must not hang the
/// frozen-input digest; preflight completes normally.
#[cfg(unix)]
#[test]
fn a_symlink_cycle_in_source_inputs_does_not_hang_preflight() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("content")).unwrap();
    std::fs::write(dir.path().join("content/index.md"), "# home\n").unwrap();
    std::os::unix::fs::symlink(dir.path().join("content"), dir.path().join("content/loop"))
        .unwrap();
    let eng = FakeEngine::new(dir.path());
    let mut p = Publisher::open(dir.path(), &eng, &CLOCK, &NoSecrets).unwrap();
    let out = p.preflight(
        &PreflightRequest { config: cfg() },
        &Rules::ok(),
        &mut ids("run"),
    );
    assert!(
        matches!(out, Ok(PreflightOutcome::Granted { .. })),
        "symlink cycles must not hang or block preflight: {out:?}"
    );
}

/// Boris's packaging contract excludes `_boris/proof` from deployable
/// output. A well-formed inventory that nevertheless lists proof evidence
/// as a committed artifact blocks the grant rather than shipping evidence.
#[test]
fn a_proof_evidence_entry_in_the_inventory_is_never_payload() {
    let dir = tempfile::tempdir().unwrap();
    let mut eng = CapturedEngine::new(dir.path());
    eng.proof_payload_entry = true;
    let mut p = Publisher::open(dir.path(), &eng, &CLOCK, &NoSecrets).unwrap();
    let out = p
        .preflight(
            &PreflightRequest { config: cfg() },
            &Rules::ok(),
            &mut ids("run"),
        )
        .unwrap();
    let PreflightOutcome::Blocked { blocks, .. } = out else {
        panic!("proof evidence in the inventory must block: {out:?}")
    };
    assert!(
        blocks
            .iter()
            .any(|b| b.contains("is evidence, never payload")),
        "{blocks:?}"
    );
}

/// Real Boris commits `.html` content assets as `kind: content-asset` —
/// opaque payload, not pages. The pinned happy-assets capture contains
/// `guides/getting-started.assets/manual.html`; preflight must grant with
/// the asset in the deployable plan and no false graph-membership block.
#[test]
fn an_html_content_asset_is_payload_not_a_page() {
    let dir = tempfile::tempdir().unwrap();
    let eng = CapturedEngine::with_html_content_asset(dir.path());
    let mut p = Publisher::open(dir.path(), &eng, &CLOCK, &NoSecrets).unwrap();
    let out = p.preflight(
        &PreflightRequest { config: cfg() },
        &Rules::ok(),
        &mut ids("run"),
    );
    let Ok(PreflightOutcome::Granted { run, .. }) = out else {
        panic!("html content asset must not block preflight: {out:?}")
    };
    let plan: SealedPlan = serde_json::from_value(p.run(&run).unwrap().plan.unwrap()).unwrap();
    assert_eq!(plan.files.len(), 6, "{:?}", plan.files);
    assert!(
        plan.files
            .iter()
            .any(|f| f.path == "guides/getting-started.assets/manual.html"),
        "the content asset is committed payload: {:?}",
        plan.files
    );
}

/// Duplicate artifact paths in a well-formed inventory are rejected before
/// sealing rather than repeated in the plan.
#[test]
fn duplicate_artifact_paths_block_the_grant() {
    let dir = tempfile::tempdir().unwrap();
    let mut eng = CapturedEngine::new(dir.path());
    eng.duplicate_entry = true;
    let mut p = Publisher::open(dir.path(), &eng, &CLOCK, &NoSecrets).unwrap();
    let out = p
        .preflight(
            &PreflightRequest { config: cfg() },
            &Rules::ok(),
            &mut ids("run"),
        )
        .unwrap();
    let PreflightOutcome::Blocked { blocks, .. } = out else {
        panic!("duplicate artifact paths must block: {out:?}")
    };
    assert!(
        blocks
            .iter()
            .any(|b| b.starts_with("proof artifacts: duplicate artifact path")),
        "{blocks:?}"
    );
}

/// A symlink in the output tree makes the output inventory unreadable.
/// That failure is a durable PreflightBlocked, not a hard error.
#[cfg(unix)]
#[test]
fn an_unreadable_output_inventory_is_a_durable_preflight_block() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("content")).unwrap();
    std::fs::write(dir.path().join("content/index.md"), "# home\n").unwrap();
    let mut eng = FakeEngine::new(dir.path());
    eng.symlink_output = true;
    let mut p = Publisher::open(dir.path(), &eng, &CLOCK, &NoSecrets).unwrap();
    let out = p
        .preflight(
            &PreflightRequest { config: cfg() },
            &Rules::ok(),
            &mut ids("run"),
        )
        .unwrap();
    let PreflightOutcome::Blocked { run, blocks } = out else {
        panic!("output inventory failure must block, not error: {out:?}")
    };
    assert!(
        blocks.iter().any(|b| b.starts_with("output inventory:")),
        "{blocks:?}"
    );
    assert!(
        p.journal().records().iter().any(|r| matches!(
            &r.entry,
            Entry::PreflightBlocked { run: rrun, .. } if *rrun == run
        )),
        "the block must be durable"
    );
}
