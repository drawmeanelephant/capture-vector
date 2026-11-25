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
