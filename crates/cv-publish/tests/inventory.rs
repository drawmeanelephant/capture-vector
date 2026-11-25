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
            .any(|b| b.contains("is in a reserved namespace")),
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

/// Artifact paths differing only by case collide on the case-insensitive
/// filesystems copy-to-folder targets. They are rejected before sealing,
/// not left to exact-match deployment failure.
#[test]
fn case_colliding_artifact_paths_block_the_grant() {
    let dir = tempfile::tempdir().unwrap();
    let mut eng = CapturedEngine::new(dir.path());
    eng.case_collision_entry = true;
    let mut p = Publisher::open(dir.path(), &eng, &CLOCK, &NoSecrets).unwrap();
    let out = p
        .preflight(
            &PreflightRequest { config: cfg() },
            &Rules::ok(),
            &mut ids("run"),
        )
        .unwrap();
    let PreflightOutcome::Blocked { blocks, .. } = out else {
        panic!("case-colliding artifact paths must block: {out:?}")
    };
    assert!(
        blocks.iter().any(|b| b.contains("case-collides with")),
        "{blocks:?}"
    );
}

/// Direct completeness probe: a planted output file with no inventory
/// record blocks the grant rather than being silently dropped or shipped.
#[test]
fn a_planted_uninventoried_output_file_blocks_the_grant() {
    let dir = tempfile::tempdir().unwrap();
    let mut eng = CapturedEngine::new(dir.path());
    eng.plant_uninventoried = true;
    let mut p = Publisher::open(dir.path(), &eng, &CLOCK, &NoSecrets).unwrap();
    let out = p
        .preflight(
            &PreflightRequest { config: cfg() },
            &Rules::ok(),
            &mut ids("run"),
        )
        .unwrap();
    let PreflightOutcome::Blocked { blocks, .. } = out else {
        panic!("uninventoried output must block: {out:?}")
    };
    assert!(
        blocks.iter().any(|b| b.contains("not-in-the-inventory.txt")
            && b.contains("no committed inventory record")),
        "{blocks:?}"
    );
}

/// Direct completeness probe: removing the CSS record from the inventory
/// while the file remains in the output blocks the grant.
#[test]
fn an_omitted_css_inventory_record_blocks_the_grant() {
    let dir = tempfile::tempdir().unwrap();
    let mut eng = CapturedEngine::new(dir.path());
    eng.omit_css_record = true;
    let mut p = Publisher::open(dir.path(), &eng, &CLOCK, &NoSecrets).unwrap();
    let out = p
        .preflight(
            &PreflightRequest { config: cfg() },
            &Rules::ok(),
            &mut ids("run"),
        )
        .unwrap();
    let PreflightOutcome::Blocked { blocks, .. } = out else {
        panic!("omitted css record must block: {out:?}")
    };
    assert!(
        blocks
            .iter()
            .any(|b| b.contains("assets/css/boris.css")
                && b.contains("no committed inventory record")),
        "{blocks:?}"
    );
}

/// Compiler-cache content is a reserved namespace: an inventory record for
/// `.boris-cache/` is never payload.
#[test]
fn a_compiler_cache_record_blocks_the_grant() {
    let dir = tempfile::tempdir().unwrap();
    let mut eng = CapturedEngine::new(dir.path());
    eng.cache_record = true;
    let mut p = Publisher::open(dir.path(), &eng, &CLOCK, &NoSecrets).unwrap();
    let out = p
        .preflight(
            &PreflightRequest { config: cfg() },
            &Rules::ok(),
            &mut ids("run"),
        )
        .unwrap();
    let PreflightOutcome::Blocked { blocks, .. } = out else {
        panic!("cache record must block: {out:?}")
    };
    assert!(
        blocks
            .iter()
            .any(|b| b.contains(".boris-cache") && b.contains("reserved namespace")),
        "{blocks:?}"
    );
}

/// A payload file that is a hardlink alias of proof bytes ships evidence
/// with no lexical tell; file identity is the only sound detection.
#[cfg(unix)]
#[test]
fn a_hardlink_alias_of_proof_bytes_blocks_the_grant() {
    let dir = tempfile::tempdir().unwrap();
    let mut eng = CapturedEngine::new(dir.path());
    eng.hardlink_alias = true;
    let mut p = Publisher::open(dir.path(), &eng, &CLOCK, &NoSecrets).unwrap();
    let out = p
        .preflight(
            &PreflightRequest { config: cfg() },
            &Rules::ok(),
            &mut ids("run"),
        )
        .unwrap();
    let PreflightOutcome::Blocked { blocks, .. } = out else {
        panic!("hardlink alias must block: {out:?}")
    };
    assert!(
        blocks
            .iter()
            .any(|b| b.contains("hardlink alias") || b.contains("multiply linked")),
        "{blocks:?}"
    );
}

/// A payload file with links outside the sealed tree can mutate after
/// sealing without touching any sealed path.
#[cfg(unix)]
#[test]
fn a_multiply_linked_payload_file_blocks_the_grant() {
    let dir = tempfile::tempdir().unwrap();
    let mut eng = CapturedEngine::new(dir.path());
    eng.multiply_linked = true;
    let mut p = Publisher::open(dir.path(), &eng, &CLOCK, &NoSecrets).unwrap();
    let out = p
        .preflight(
            &PreflightRequest { config: cfg() },
            &Rules::ok(),
            &mut ids("run"),
        )
        .unwrap();
    let PreflightOutcome::Blocked { blocks, .. } = out else {
        panic!("multiply linked payload must block: {out:?}")
    };
    assert!(
        blocks.iter().any(|b| b.contains("multiply linked")),
        "{blocks:?}"
    );
}

/// Real Boris 0.9.0 commits `aux.html` and `guides/a:b.html` as `html-page`
/// artifacts (probe-verified). Host-valid output must survive inventory,
/// preflight, and copy-to-folder delivery on unix hosts.
#[test]
fn host_valid_boris_page_names_deploy_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let mut eng = FakeEngine::new(dir.path());
    eng.pages = vec![
        ("index.html", "<h1>home</h1>"),
        ("aux.html", "<h1>aux</h1>"),
        ("guides/a:b.html", "<h1>colon</h1>"),
    ];
    let mut p = Publisher::open(dir.path(), &eng, &CLOCK, &NoSecrets)
        .unwrap()
        .with_adapter(cv_publish::adapters::CopyToFolder::new(dir.path()));
    let out = p
        .preflight(
            &PreflightRequest { config: cfg() },
            &Rules::ok(),
            &mut ids("run"),
        )
        .unwrap();
    let PreflightOutcome::Granted { run, .. } = out else {
        panic!("host-valid real Boris names must grant: {out:?}")
    };
    p.approve(&run, "tester").unwrap();
    p.publish(&run, &cfg(), &Rules::ok()).unwrap();
    let dest = dir.path().join("deployed");
    assert!(dest.join("aux.html").exists());
    assert!(dest.join("guides/a:b.html").exists());
}

/// Boris's own deploy verdict is decisive: `proof verify` fails when a ran
/// check records status `failed` (probe-verified: broken fragment link →
/// rendered-html failed with HTML_FRAGMENT_MISSING, verify exits 1 while
/// build/validate/check exit 0). EngineDocumented deploy errors must block
/// the grant even though Audit projection is deferred.
#[test]
fn a_failed_deploy_check_blocks_the_grant_with_finding_detail() {
    let dir = tempfile::tempdir().unwrap();
    let mut eng = CapturedEngine::new(dir.path());
    eng.failed_check = true;
    let mut p = Publisher::open(dir.path(), &eng, &CLOCK, &NoSecrets).unwrap();
    let out = p
        .preflight(
            &PreflightRequest { config: cfg() },
            &Rules::ok(),
            &mut ids("run"),
        )
        .unwrap();
    let PreflightOutcome::Blocked { blocks, .. } = out else {
        panic!("a failed deploy check must block: {out:?}")
    };
    assert!(
        blocks.iter().any(|b| b.contains("rendered-html")
            && b.contains("HTML_FRAGMENT_MISSING")
            && b.contains("guides/publishing")
            && b.contains("guides/publishing.html")),
        "{blocks:?}"
    );
}

/// The digest inside checks.json binds the exact artifacts.json bytes.
/// Tampering with the inventory after the proof pack is written must block
/// rather than letting evidence and sealed inventory diverge.
#[test]
fn a_divergent_checks_inventory_digest_blocks_the_grant() {
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
        panic!("divergent digest must block: {out:?}")
    };
    assert!(
        blocks
            .iter()
            .any(|b| b.contains("artifact-inventory digest") && b.contains("does not match")),
        "{blocks:?}"
    );
}

/// An unknown key or malformed digest shape in the inventory is rejected at
/// the root parse/validation cause, not as a misleading missing-record
/// cascade.
#[test]
fn a_noncanonical_digest_shape_fails_at_the_root_cause() {
    let dir = tempfile::tempdir().unwrap();
    let mut eng = CapturedEngine::new(dir.path());
    eng.uppercase_sha = true;
    let mut p = Publisher::open(dir.path(), &eng, &CLOCK, &NoSecrets).unwrap();
    let out = p
        .preflight(
            &PreflightRequest { config: cfg() },
            &Rules::ok(),
            &mut ids("run"),
        )
        .unwrap();
    let PreflightOutcome::Blocked { blocks, .. } = out else {
        panic!("uppercase sha must block: {out:?}")
    };
    assert!(
        blocks
            .iter()
            .any(|b| b.contains("sha256 is not lowercase hex digest shape")),
        "{blocks:?}"
    );
    assert!(
        !blocks.iter().any(|b| b.contains("missing from the output")),
        "no misleading cascade: {blocks:?}"
    );
}

/// Real pinned capture: Boris builds the broken-fragment corpus green but
/// records a deploy-blocking rendered-html failure. This is the exact shape
/// the reviewer reproduced — preflight must refuse the grant and surface
/// Boris's finding detail.
#[test]
fn real_boris_broken_fragment_output_blocks_the_grant() {
    let dir = tempfile::tempdir().unwrap();
    let mut eng = CapturedEngine::new(dir.path());
    eng.fixture = "captures/happy-broken-fragment/dist/dist";
    let mut p = Publisher::open(dir.path(), &eng, &CLOCK, &NoSecrets).unwrap();
    let out = p
        .preflight(
            &PreflightRequest { config: cfg() },
            &Rules::ok(),
            &mut ids("run"),
        )
        .unwrap();
    let PreflightOutcome::Blocked { blocks, .. } = out else {
        panic!("real deploy-blocking proof evidence must block: {out:?}")
    };
    assert!(
        blocks.iter().any(|b| b.contains("rendered-html")
            && b.contains("HTML_FRAGMENT_MISSING")
            && b.contains("index")),
        "{blocks:?}"
    );
}

fn mutate_checks(eng: &mut CapturedEngine, f: impl Fn(&mut serde_json::Value) + Send + 'static) {
    eng.post_build = Some(Box::new(move |out: &std::path::Path| {
        let path = out.join("_boris/proof/checks.json");
        let mut idx: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        f(&mut idx);
        std::fs::write(&path, serde_json::to_vec_pretty(&idx).unwrap()).unwrap();
    }));
}

fn mutate_artifacts(eng: &mut CapturedEngine, f: impl Fn(&mut serde_json::Value) + Send + 'static) {
    eng.post_build = Some(Box::new(move |out: &std::path::Path| {
        let path = out.join("_boris/proof/artifacts.json");
        let mut idx: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        f(&mut idx);
        std::fs::write(&path, serde_json::to_vec_pretty(&idx).unwrap()).unwrap();
    }));
}

fn assert_blocked(eng: &CapturedEngine, dir: &std::path::Path, needle: &str) {
    let mut p = Publisher::open(dir, eng, &CLOCK, &NoSecrets).unwrap();
    let out = p
        .preflight(
            &PreflightRequest { config: cfg() },
            &Rules::ok(),
            &mut ids("run"),
        )
        .unwrap();
    let PreflightOutcome::Blocked { blocks, .. } = out else {
        panic!("{needle}: expected a durable preflight block, got {out:?}")
    };
    assert!(
        blocks.iter().any(|b| b.contains(needle)),
        "{needle}: not surfaced in {blocks:?}"
    );
}

#[test]
fn an_incomplete_check_status_blocks_the_grant() {
    let dir = tempfile::tempdir().unwrap();
    let mut eng = CapturedEngine::new(dir.path());
    mutate_checks(&mut eng, |idx| {
        for c in idx["checks"].as_array_mut().unwrap() {
            if c["id"].as_str() == Some("rendered-html") {
                c["status"] = serde_json::json!("incomplete");
            }
        }
    });
    assert_blocked(&eng, dir.path(), "rendered-html incomplete");
}

#[test]
fn an_eligible_check_that_did_not_run_blocks_the_grant() {
    let dir = tempfile::tempdir().unwrap();
    let mut eng = CapturedEngine::new(dir.path());
    mutate_checks(&mut eng, |idx| {
        for c in idx["checks"].as_array_mut().unwrap() {
            if c["id"].as_str() == Some("rendered-html") {
                c["ran"] = serde_json::json!(false);
            }
        }
    });
    assert_blocked(&eng, dir.path(), "eligible but did not run");
}

#[test]
fn a_missing_inventory_descriptor_in_checks_blocks_the_grant() {
    let dir = tempfile::tempdir().unwrap();
    let mut eng = CapturedEngine::new(dir.path());
    mutate_checks(&mut eng, |idx| {
        idx.as_object_mut().unwrap().remove("artifact_inventory");
    });
    assert_blocked(&eng, dir.path(), "artifact-inventory descriptor missing");
}

#[test]
fn a_wrong_scope_checks_target_blocks_the_grant() {
    let dir = tempfile::tempdir().unwrap();
    let mut eng = CapturedEngine::new(dir.path());
    mutate_checks(&mut eng, |idx| {
        idx["target"] = serde_json::json!("github-pages");
    });
    assert_blocked(&eng, dir.path(), "proof checks: target");
}

#[test]
fn a_missing_format_version_key_blocks_at_the_root_cause() {
    let dir = tempfile::tempdir().unwrap();
    let mut eng = CapturedEngine::new(dir.path());
    mutate_artifacts(&mut eng, |idx| {
        idx["artifacts"][0]
            .as_object_mut()
            .unwrap()
            .remove("format_version");
    });
    assert_blocked(&eng, dir.path(), "missing the required format_version key");
}

#[test]
fn an_unknown_record_key_is_rejected_at_the_root_cause() {
    let dir = tempfile::tempdir().unwrap();
    let mut eng = CapturedEngine::new(dir.path());
    mutate_artifacts(&mut eng, |idx| {
        idx["artifacts"][0]["surprise"] = serde_json::json!(true);
    });
    assert_blocked(&eng, dir.path(), "unknown field");
}

#[test]
fn malformed_dimensions_are_rejected_at_the_root_cause() {
    let dir = tempfile::tempdir().unwrap();
    let mut eng = CapturedEngine::new(dir.path());
    mutate_artifacts(&mut eng, |idx| {
        idx["artifacts"][0]["dimensions"] = serde_json::json!({"width": 0, "height": 100});
    });
    assert_blocked(&eng, dir.path(), "dimensions");
}

#[test]
fn an_empty_format_version_string_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let mut eng = CapturedEngine::new(dir.path());
    mutate_artifacts(&mut eng, |idx| {
        idx["artifacts"][0]["format_version"] = serde_json::json!("");
    });
    assert_blocked(&eng, dir.path(), "empty string");
}

#[test]
fn an_unknown_check_status_blocks_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let mut eng = CapturedEngine::new(dir.path());
    mutate_checks(&mut eng, |idx| {
        idx["checks"][1]["status"] = serde_json::json!("surprise-pass");
    });
    assert_blocked(&eng, dir.path(), "unknown status");
}

#[test]
fn a_wrong_checks_format_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let mut eng = CapturedEngine::new(dir.path());
    mutate_checks(&mut eng, |idx| {
        idx["format"] = serde_json::json!("not-boris-checks");
    });
    assert_blocked(&eng, dir.path(), "not boris-publication-checks");
}

#[test]
fn malformed_coverage_blocks_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let mut eng = CapturedEngine::new(dir.path());
    mutate_checks(&mut eng, |idx| {
        idx["checks"][0]["coverage"] = serde_json::json!("sort-of-complete");
    });
    assert_blocked(&eng, dir.path(), "unknown coverage");
}

#[test]
fn a_semantically_inconsistent_check_row_blocks() {
    let dir = tempfile::tempdir().unwrap();
    let mut eng = CapturedEngine::new(dir.path());
    mutate_checks(&mut eng, |idx| {
        idx["checks"][0]["eligible"] = serde_json::json!(false);
    });
    assert_blocked(&eng, dir.path(), "not eligible but ran");
}

#[test]
fn unknown_artifact_semantics_fail_at_the_root_cause() {
    let dir = tempfile::tempdir().unwrap();
    let mut eng = CapturedEngine::new(dir.path());
    mutate_artifacts(&mut eng, |idx| {
        idx["artifacts"][0]["semantics"] = serde_json::json!("bogus-semantics");
    });
    assert_blocked(&eng, dir.path(), "unknown semantics");
}
