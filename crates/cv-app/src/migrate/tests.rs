use super::scene::{SEEDED, corrupt, seed, seed_run};
use super::*;
use cv_fixtures::lab::lab_captures;
use std::path::{Path, PathBuf};

fn load(root: &Path) -> Imports {
    DiskPort::open(root, LabState::NoWorkspace).load()
}

#[test]
fn every_captured_mode_projects_a_readable_run() {
    let d = tempfile::tempdir().unwrap();
    for (i, mode) in lab_captures().iter().enumerate() {
        seed_run(d.path(), mode, i as i64);
    }
    let i = load(d.path());
    assert_eq!(i.runs.len(), lab_captures().len());
    for r in &i.runs {
        assert_eq!(r.problem, None, "{}", r.mode);
        assert_eq!(r.status, RunStatus::Ok, "{}", r.mode);
        assert!(r.format.is_some(), "{} names its format", r.mode);
        assert!(r.report.is_some(), "{} has a Markdown report", r.mode);
        assert!(r.family.is_some(), "{} has a family", r.mode);
        assert!(r.out.starts_with(".cv/imports/"), "{}", r.out);
    }
}

#[test]
fn hugo_counts_are_the_labs_by_class_summary() {
    let d = tempfile::tempdir().unwrap();
    seed_run(d.path(), "hugo", 1);
    let r = &load(d.path()).runs[0];
    assert_eq!(r.format, Some(("boris-hugo-migration-lab".to_string(), 1)));
    assert_eq!(
        r.counts,
        vec![
            ("exact".to_string(), 0),
            ("transformed".to_string(), 5),
            ("unsupported".to_string(), 3),
            ("human_review".to_string(), 4),
        ]
    );
    // Every unsupported / human_review page becomes one review item.
    assert_eq!(r.review.len(), 7);
    assert!(
        r.review
            .iter()
            .all(|i| i.id.len() == 16 && !i.id.contains('/'))
    );
    assert_eq!(r.source, "mini-hugo");
    assert_eq!(
        r.provenance.as_deref().map(|p| p.ends_with("/provenance/")),
        Some(true)
    );
}

#[test]
fn substack_review_items_come_from_its_findings() {
    let d = tempfile::tempdir().unwrap();
    seed_run(d.path(), "substack", 1);
    let r = &load(d.path()).runs[0];
    assert!(!r.review.is_empty());
    assert!(
        r.review
            .iter()
            .all(|i| matches!(i.class.as_str(), "human_review" | "unsupported"))
    );
}

#[test]
fn triage_survives_reopening_the_workspace() {
    let d = tempfile::tempdir().unwrap();
    seed(d.path());
    let mut port = DiskPort::open(d.path(), LabState::NoWorkspace);
    let i = port.load();
    let run = i.runs.iter().find(|r| !r.review.is_empty()).unwrap();
    let (rid, iid) = (run.id.clone(), run.review[0].id.clone());
    port.record(&rid, &iid, ReviewState::Deferred).unwrap();
    drop(port);
    let i = load(d.path());
    let run = i.runs.iter().find(|r| r.id == rid).unwrap();
    assert_eq!(run.review[0].state, ReviewState::Deferred);
    assert_eq!(i.store, ".cv/imports/review.jsonl");
}

#[test]
fn an_unmirrored_report_schema_is_disclosed_not_guessed() {
    let d = tempfile::tempdir().unwrap();
    corrupt(d.path());
    let r = &load(d.path()).runs[0];
    let p = r.problem.as_deref().unwrap();
    assert!(p.contains("schema_version 9"), "{p}");
    assert!(r.counts.is_empty() && r.review.is_empty());
    assert!(r.report.is_some(), "the lab's Markdown still shows");
}

#[test]
fn run_status_follows_the_record() {
    let d = tempfile::tempdir().unwrap();
    let id = seed_run(d.path(), "jekyll", 1);
    let mut rec = match &store::scan(d.path()).unwrap()[0].record {
        Ok(r) => r.clone(),
        Err(e) => panic!("{e}"),
    };
    for (exit, want) in [
        ("ok", RunStatus::Ok),
        ("usage", RunStatus::Refused),
        ("io", RunStatus::Failed),
    ] {
        rec.finished.as_mut().unwrap().exit = exit.into();
        store::write(d.path(), &rec).unwrap();
        assert_eq!(load(d.path()).runs[0].status, want, "{exit}");
    }
    rec.adopted = Some(store::Adopted {
        at_ms: 2,
        workspace: "/w".into(),
    });
    store::write(d.path(), &rec).unwrap();
    assert_eq!(load(d.path()).runs[0].status, RunStatus::Adopted);
    rec.adopted = None;
    rec.finished = None;
    store::write(d.path(), &rec).unwrap();
    // An unfinished record can no longer be observed as `running` through a
    // fresh open: workers die with the app, so the open-time sweep closes
    // it as interrupted. In-session runs still display Running because the
    // sweep only fires once, at open, before any worker exists.
    let r = &load(d.path()).runs[0];
    assert_eq!(r.status, RunStatus::Failed);
    assert_eq!(r.id, id);
}

#[test]
fn a_run_left_unfinished_by_a_quit_is_closed_as_interrupted_on_open() {
    let d = tempfile::tempdir().unwrap();
    let id = seed_run(d.path(), "jekyll", 1);
    let mut rec = store::scan(d.path()).unwrap()[0].record.clone().unwrap();
    rec.finished = None;
    store::write(d.path(), &rec).unwrap();

    // First open after the "quit": the orphaned record is closed honestly.
    let r = &load(d.path()).runs[0];
    assert_eq!(r.status, RunStatus::Failed);
    let rec = store::scan(d.path()).unwrap()[0].record.clone().unwrap();
    let f = rec.finished.expect("sweep wrote a finish");
    assert_eq!(f.exit, "interrupted");
    assert!(
        f.outcome.contains("quitting is not cancellation"),
        "{}",
        f.outcome
    );
    let _ = id;
}

#[test]
fn lab_refusal_names_are_detected_regardless_of_exit_code() {
    // The pinned lab reports safety refusals at exit 2 or 3 depending on
    // when they fire; the stderr error name is the stable signal.
    for (stderr, want) in [
        (
            "error: migration-lab (wordpress) failed: InputSymlink",
            "symlink appears in an input",
        ),
        (
            "error: migration-lab (jekyll) failed: OutputSymlink",
            "symlink appears in that output",
        ),
        (
            "error: migration-lab (hugo) failed: InputOutputOverlap",
            "overlap",
        ),
        (
            "error: migration-lab (hugo) failed: OutputNotOwned",
            "not owned",
        ),
    ] {
        assert!(lab_refusal_name(stderr).is_some(), "{stderr} is a refusal");
        let plain = plain_lab_refusal(stderr, Path::new("/out"));
        assert!(plain.contains(want), "{plain} should contain {want:?}");
    }
    assert!(lab_refusal_name("error: migration-lab (hugo) failed: InvalidHugoRoot").is_none());
}

#[test]
fn a_refused_run_is_recorded_without_spawning() {
    let d = tempfile::tempdir().unwrap();
    let lab = LabIdentity {
        source: EngineSource::Path,
        path: d.path().join("no-such-lab"),
        tool_id: engine_lab::PINNED.tool_id.into(),
    };
    let rec = run_lab(
        d.path(),
        "r1",
        &lab,
        engine_lab::mode("hugo").unwrap(),
        vec![("--root".into(), d.path().join("absent"))],
        Vec::new(),
        &CancelToken::new(),
    )
    .unwrap();
    assert!(rec.refused.as_deref().unwrap().contains("--root"));
    assert!(rec.finished.is_none());
    let r = &load(d.path()).runs[0];
    assert_eq!(r.status, RunStatus::Refused);
    assert_eq!(r.boris_pin.as_deref(), Some("boris/0.9.0 @ 66efcfb"));
}

#[test]
fn seeded_scene_modes_are_captured() {
    for m in SEEDED {
        assert!(lab_captures().iter().any(|c| c == m), "{m}");
    }
}

#[test]
fn lab_refusals_are_plain_language_and_name_the_output_path() {
    let out = Path::new("/site/.cv/imports/run/out");
    let symlink = plain_lab_refusal("error: migration-lab (hugo) failed: OutputSymlink\n", out);
    assert!(symlink.contains("symlink"), "{symlink}");
    assert!(symlink.contains(out.to_str().unwrap()), "{symlink}");
    let overlap = plain_lab_refusal(
        "error: migration-lab (hugo) failed: InputOutputOverlap\n",
        out,
    );
    assert!(overlap.contains("overlaps an input path"), "{overlap}");
    assert!(overlap.contains(out.to_str().unwrap()), "{overlap}");
    let unowned = plain_lab_refusal("error: migration-lab (hugo) failed: OutputNotOwned\n", out);
    assert!(unowned.contains("not empty"), "{unowned}");
    assert!(unowned.contains("left unchanged"), "{unowned}");
    assert!(unowned.contains(out.to_str().unwrap()), "{unowned}");
}

#[test]
fn adapter_overlap_refusal_is_plain_language_with_both_paths() {
    let err = LabError::Overlap {
        flag: "--root",
        input: PathBuf::from("/input"),
        out: PathBuf::from("/input/out"),
    };
    let text = plain_prepare_refusal(&err);
    assert!(text.contains("--root"), "{text}");
    assert!(text.contains("/input/out"), "{text}");
    assert!(text.contains("/input"), "{text}");
    assert!(text.contains("overlaps"), "{text}");
}

#[test]
fn cancelled_exit_projects_as_cancelled_instead_of_a_lab_failure() {
    let mut rec = RunRecord::started(
        "run",
        "hugo",
        store::RecordedLab {
            tool_id: "lab".into(),
            source: "test".into(),
            path: "/lab".into(),
        },
        0,
    );
    rec.finished = Some(store::Finished {
        exit_code: None,
        exit: "cancelled".into(),
        outcome: "cancelled; child process group killed".into(),
        duration_ms: 3,
    });
    assert_eq!(status(&rec), RunStatus::Cancelled);
}

#[test]
fn a_successful_run_with_no_decodable_output_document_is_a_problem() {
    let d = tempfile::tempdir().unwrap();
    let id = seed_run(d.path(), "jekyll", 1);
    // Remove every lab document but leave the record reporting success.
    let out = d.path().join(".cv").join("imports").join(&id).join("out");
    for f in std::fs::read_dir(&out).unwrap() {
        let f = f.unwrap().path();
        if f.is_file() {
            std::fs::remove_file(&f).unwrap();
        }
    }
    let r = &load(d.path()).runs[0];
    assert_eq!(r.status, RunStatus::Ok);
    assert!(
        r.problem
            .as_deref()
            .unwrap_or("")
            .contains("no report.json"),
        "{:?}",
        r.problem
    );
}

#[test]
fn a_valid_zero_count_inventory_is_not_a_problem() {
    let d = tempfile::tempdir().unwrap();
    seed_run(d.path(), "hugo", 1);
    let r = &load(d.path()).runs[0];
    assert_eq!(r.problem, None);
    assert!(!r.counts.is_empty(), "decoded counts are positive evidence");
}

/// Mutate the captured jekyll-inventory document the way the reviewer's
/// tampered-shape probes did, then project.
fn project_mutated_inventory(mutate: impl Fn(&mut serde_json::Value)) -> ImportRun {
    let d = tempfile::tempdir().unwrap();
    let id = seed_run(d.path(), "jekyll-inventory", 1);
    let doc = d
        .path()
        .join(".cv")
        .join("imports")
        .join(&id)
        .join("out")
        .join("inventory.json");
    let mut json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&doc).unwrap()).unwrap();
    mutate(&mut json);
    std::fs::write(&doc, serde_json::to_vec(&json).unwrap()).unwrap();
    load(d.path()).runs.into_iter().next().unwrap()
}

#[test]
fn a_row_less_inventory_is_not_count_evidence() {
    let r = project_mutated_inventory(|json| {
        json["constructs"] = serde_json::json!([]);
    });
    assert_eq!(r.status, RunStatus::Ok);
    assert_eq!(r.problem, None, "the document still decodes");
    assert!(
        !r.count_evidence,
        "empty constructs carry no count evidence even though the aggregate is pre-seeded"
    );
}

#[test]
fn unknown_class_only_rows_are_not_count_evidence() {
    let r = project_mutated_inventory(|json| {
        for row in json["constructs"].as_array_mut().unwrap() {
            row["class"] = serde_json::json!("bogus-class");
        }
    });
    assert_eq!(r.problem, None, "unknown classes still decode");
    assert!(
        !r.count_evidence,
        "wrong-class rows are a tampered shape, not evidence"
    );
}

#[test]
fn a_valid_inventory_carries_count_evidence() {
    let r = project_mutated_inventory(|_| {});
    assert!(
        r.count_evidence,
        "the untouched capture is positive evidence"
    );
}

/// The sweep-gating half of the stale-write race coverage: a foreign-owned
/// workspace is never swept and discloses itself; an owned workspace is
/// swept. The cross-process exclusion half — a live holder really does
/// exclude other processes until it dies — is proven in cv-engine's wlock
/// tests, because processes spawn only inside cv-engine.
#[test]
fn a_foreign_owned_workspace_is_never_swept() {
    let d = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(d.path()).unwrap();
    seed_run(&root, "jekyll", 1);
    let mut rec = store::scan(&root).unwrap()[0].record.clone().unwrap();
    rec.finished = None;
    store::write(&root, &rec).unwrap();

    let port = DiskPort::open_with_ownership(&root, LabState::NoWorkspace, Ownership::Foreign);
    let i = migrate_port_load(port);
    assert!(
        i.error
            .as_deref()
            .unwrap_or("")
            .contains("another Capture Vector process"),
        "{:?}",
        i.error
    );
    let rec = store::scan(&root).unwrap()[0].record.clone().unwrap();
    assert!(
        rec.finished.is_none(),
        "a foreign-owned record is never rewritten: {rec:?}"
    );

    // Ownership acquired (the other process exited, releasing the flock):
    // the sweep applies and the orphaned record closes as interrupted.
    let port = DiskPort::open_with_ownership(&root, LabState::NoWorkspace, Ownership::Owned);
    let _ = migrate_port_load(port);
    let rec = store::scan(&root).unwrap()[0].record.clone().unwrap();
    assert_eq!(
        rec.finished.as_ref().map(|f| f.exit.as_str()),
        Some("interrupted"),
        "the orphaned record is closed once owned: {rec:?}"
    );
}

fn migrate_port_load(mut port: DiskPort) -> Imports {
    port.load()
}

#[test]
fn foreign_review_writes_are_refused_and_owned_writes_work() {
    let d = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(d.path()).unwrap();
    let id = seed_run(&root, "jekyll", 1);
    let item = load(&root).runs[0].review.first().map(|i| i.id.clone());

    let mut foreign =
        DiskPort::open_with_ownership(&root, LabState::NoWorkspace, Ownership::Foreign);
    if let Some(item) = &item {
        let err = foreign
            .record(&id, item, ReviewState::Accepted)
            .unwrap_err();
        assert!(err.contains("not owned"), "{err}");
    }
    let mut unsafe_port =
        DiskPort::open_with_ownership(&root, LabState::NoWorkspace, Ownership::Unsafe);
    if let Some(item) = &item {
        assert!(
            unsafe_port
                .record(&id, item, ReviewState::Accepted)
                .is_err()
        );
    }
    let mut owned = DiskPort::open_with_ownership(&root, LabState::NoWorkspace, Ownership::Owned);
    if let Some(item) = &item {
        owned.record(&id, item, ReviewState::Accepted).unwrap();
    }
}

#[test]
fn the_wizard_offers_exactly_the_issue_172_scope() {
    use cv_model::command::Intent;
    use cv_model::module::Module;
    use cv_model::modules::migrate::{Imports, MigrateModule, MigratePort};
    struct RealPort(Imports);
    impl MigratePort for RealPort {
        fn load(&mut self) -> Imports {
            self.0.clone()
        }
        fn record(&mut self, _: &str, _: &str, _: ReviewState) -> Result<(), String> {
            Ok(())
        }
    }
    let imports = Imports {
        lab: LabState::Located {
            tool_id: "lab".into(),
            source: "test".into(),
            path: "/lab".into(),
        },
        families: families(),
        modes: modes(),
        runs: Vec::new(),
        error: None,
        store: "test".into(),
    };
    let mut m = MigrateModule::new(Box::new(RealPort(imports)));
    use cv_model::modules::migrate::NEW;
    let intents = m.run(NEW);
    let Some(Intent::Dialog(family_dialog)) =
        intents.iter().find(|i| matches!(i, Intent::Dialog(_)))
    else {
        panic!("expected family dialog, got {intents:?}");
    };
    let cv_model::dialog::DialogKind::Choose(families_offered) = &family_dialog.kind else {
        panic!("family step is a choice dialog");
    };
    let mut family_ids: Vec<_> = families_offered.iter().map(|c| c.id.as_str()).collect();
    family_ids.sort();
    // Dead-end families are hidden: exactly the two families of the #172
    // scope survive, and no other.
    assert_eq!(
        family_ids,
        ["blog-cms", "static-site"],
        "family choices must be exactly the #172 scope"
    );

    let mode_choices = |m: &mut MigrateModule, family: &str| {
        let out = m.answer(
            "migrate.new.family",
            &cv_model::dialog::DialogAnswer {
                action: Some("choose".into()),
                values: [(
                    cv_model::dialog::CHOICE.into(),
                    cv_model::dialog::Value::Text(family.into()),
                )]
                .into_iter()
                .collect(),
                ..Default::default()
            },
        );
        let Some(Intent::Dialog(d)) = out.iter().find(|i| matches!(i, Intent::Dialog(_))) else {
            panic!("expected mode dialog, got {out:?}");
        };
        let cv_model::dialog::DialogKind::Choose(choices) = &d.kind else {
            panic!("mode step is a choice dialog");
        };
        choices.iter().map(|c| c.id.clone()).collect::<Vec<_>>()
    };

    assert_eq!(
        mode_choices(&mut m, "blog-cms"),
        ["wordpress"],
        "blog-cms offers exactly wordpress"
    );
    assert_eq!(
        mode_choices(&mut m, "static-site"),
        ["hugo"],
        "static-site offers exactly hugo"
    );
}

#[test]
fn a_wrong_family_inventory_supplies_no_count_evidence() {
    // A decodable Jekyll inventory inside a hugo-inventory run is a
    // misplaced or tampered document: no counts, no evidence, visible problem.
    let d = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(d.path()).unwrap();
    let hugo = seed_run(&root, "hugo-inventory", 1);
    let jekyll = seed_run(&root, "jekyll-inventory", 2);
    let base = root.join(".cv").join("imports");
    std::fs::copy(
        base.join(&jekyll).join("out").join("inventory.json"),
        base.join(&hugo).join("out").join("inventory.json"),
    )
    .unwrap();
    let runs = load(&root).runs;
    let run = runs.iter().find(|r| r.id == hugo).expect("run projected");
    assert!(
        !run.count_evidence,
        "wrong-family inventory is not evidence"
    );
    assert!(
        run.counts.iter().all(|(_, n)| *n == 0),
        "wrong-family inventory supplies no counts: {:?}",
        run.counts
    );
    let problem = run.problem.as_deref().unwrap_or("");
    assert!(
        problem.contains("wrong-family"),
        "visible problem names the mismatch: {problem}"
    );
}
