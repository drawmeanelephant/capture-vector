use super::scene::{SEEDED, corrupt, seed, seed_run};
use super::*;
use cv_fixtures::lab::lab_captures;
use std::path::{Path, PathBuf};

fn load(root: &Path) -> Imports {
    DiskPort::open(root, LabState::NoWorkspace).load()
}

/// A pre-lock legacy run on disk: valid record bytes and output, but no
/// `run.lock`. Test setup writes bytes directly because it is constructing
/// the foreign past the read-only projection must handle, not performing an
/// app write.
fn legacy_unfinished_run(root: &Path, id: &str) -> PathBuf {
    let dir = store::run_dir(root, id).unwrap();
    std::fs::create_dir_all(dir.join(store::OUT)).unwrap();
    let rec = RunRecord::started(
        id,
        "hugo",
        store::RecordedLab {
            tool_id: "lab".into(),
            source: "test".into(),
            path: "/lab".into(),
        },
        7,
    );
    let mut bytes = serde_json::to_vec_pretty(&rec).unwrap();
    bytes.push(b'\n');
    std::fs::write(dir.join(store::RECORD), bytes).unwrap();
    std::fs::write(dir.join(store::OUT).join("partial.txt"), b"partial").unwrap();
    dir
}

fn run_folder_bytes(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            out.push((
                path.file_name().unwrap().to_string_lossy().into_owned(),
                std::fs::read(&path).unwrap(),
            ));
        } else if path.is_dir() {
            for nested in std::fs::read_dir(&path).unwrap() {
                let nested = nested.unwrap().path();
                if nested.is_file() {
                    out.push((
                        format!(
                            "{}/{}",
                            path.file_name().unwrap().to_string_lossy(),
                            nested.file_name().unwrap().to_string_lossy()
                        ),
                        std::fs::read(&nested).unwrap(),
                    ));
                }
            }
        }
    }
    out.sort();
    out
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
fn run_status_follows_the_recorded_outcome() {
    let d = tempfile::tempdir().unwrap();
    let id = seed_run(d.path(), "jekyll", 1);
    let mut rec = match &store::scan(d.path()).unwrap()[0].record {
        Ok(r) => r.clone(),
        Err(e) => panic!("{e}"),
    };
    let lock = store::lock_run(d.path(), &id).unwrap();
    for (exit, want) in [
        ("ok", RunStatus::Ok),
        ("usage", RunStatus::Refused),
        ("io", RunStatus::Failed),
    ] {
        rec.finished.as_mut().unwrap().exit = exit.into();
        store::write(&lock, &rec).unwrap();
        assert_eq!(load(d.path()).runs[0].status, want, "{exit}");
    }
    rec.adopted = Some(store::Adopted {
        at_ms: 2,
        workspace: "/w".into(),
    });
    store::write(&lock, &rec).unwrap();
    assert_eq!(load(d.path()).runs[0].status, RunStatus::Adopted);
    rec.adopted = None;
    rec.finished = None;
    store::write(&lock, &rec).unwrap();
    drop(lock);
    let r = &load(d.path()).runs[0];
    assert_eq!(r.status, RunStatus::Unfinished);
    assert_eq!(r.id, id);
}

#[test]
fn an_unfinished_record_without_a_lock_projects_unfinished_without_rewriting() {
    let d = tempfile::tempdir().unwrap();
    let dir = legacy_unfinished_run(d.path(), "legacy");
    let before = run_folder_bytes(&dir);
    let r = &load(d.path()).runs[0];
    assert_eq!(r.status, RunStatus::Unfinished);
    assert_eq!(run_folder_bytes(&dir), before);
    assert!(
        !dir.join(store::RUN_LOCK).exists(),
        "read-only projection must not create a lock"
    );
}

#[test]
fn a_held_run_lock_projects_running_and_release_projects_unfinished() {
    let d = tempfile::tempdir().unwrap();
    let dir = legacy_unfinished_run(d.path(), "legacy");
    assert!(!dir.join(store::RUN_LOCK).exists());
    let lock = store::lock_run(d.path(), "legacy").unwrap();
    assert_eq!(load(d.path()).runs[0].status, RunStatus::Running);
    drop(lock);
    assert_eq!(load(d.path()).runs[0].status, RunStatus::Unfinished);
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
    assert_eq!(status(Path::new("/unused"), &rec), RunStatus::Cancelled);
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

#[test]
fn a_second_review_writer_is_read_only_and_refuses_decisions() {
    let d = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(d.path()).unwrap();
    let id = seed_run(&root, "jekyll", 1);
    let item = load(&root).runs[0].review.first().map(|i| i.id.clone());
    let Some(item) = item else {
        panic!("jekyll capture has review items")
    };

    let mut writer = DiskPort::open(&root, LabState::NoWorkspace);
    let mut loser = DiskPort::open(&root, LabState::NoWorkspace);
    writer.record(&id, &item, ReviewState::Accepted).unwrap();
    let err = loser.record(&id, &item, ReviewState::Deferred).unwrap_err();
    assert!(err.contains("read-only"), "{err}");
    drop(writer);
    // A handle that lost the race does not become a writer later in-place;
    // only a fresh open after the holder closes can win the lock.
    let err = loser.record(&id, &item, ReviewState::Deferred).unwrap_err();
    assert!(err.contains("read-only"), "{err}");
    drop(loser);
    let mut fresh = DiskPort::open(&root, LabState::NoWorkspace);
    fresh.record(&id, &item, ReviewState::Deferred).unwrap();
}

#[test]
fn wrong_role_or_wrong_family_primary_artifacts_never_supply_count_evidence() {
    let d = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(d.path()).unwrap();
    let hugo_emit = seed_run(&root, "hugo", 1);
    let hugo_inventory = seed_run(&root, "hugo-inventory", 2);
    let jekyll_emit = seed_run(&root, "jekyll", 3);
    let base = root.join(".cv").join("imports");
    let out = |id: &str| base.join(id).join("out");

    // Exact-family inventory in an emit run is the wrong role.
    std::fs::copy(
        out(&hugo_inventory).join("inventory.json"),
        out(&hugo_emit).join("inventory.json"),
    )
    .unwrap();
    std::fs::remove_file(out(&hugo_emit).join("report.json")).unwrap();

    // Exact-family report in an inventory run is the wrong role.
    let runs = load(&root).runs;
    let emit = runs.iter().find(|r| r.id == hugo_emit).unwrap();
    assert!(!emit.count_evidence);
    assert!(emit.counts.iter().all(|(_, n)| *n == 0));
    assert!(
        emit.problem.as_deref().unwrap_or("").contains("wrong-role"),
        "emit wrong role is visible: {:?}",
        emit.problem
    );

    // Wrong-family report in a Hugo emit run is not positive evidence.
    let wrong_emit = seed_run(&root, "hugo", 4);
    std::fs::copy(
        out(&jekyll_emit).join("report.json"),
        out(&wrong_emit).join("report.json"),
    )
    .unwrap();
    let wrong_inv = seed_run(&root, "hugo-inventory", 5);
    let hugo_good_emit = seed_run(&root, "hugo", 6);
    std::fs::copy(
        out(&hugo_good_emit).join("report.json"),
        out(&wrong_inv).join("report.json"),
    )
    .unwrap();
    std::fs::remove_file(out(&wrong_inv).join("inventory.json")).unwrap();

    let runs = load(&root).runs;
    for (id, needle) in [(wrong_emit, "wrong-family"), (wrong_inv, "wrong-role")] {
        let run = runs.iter().find(|r| r.id == id).unwrap();
        assert!(!run.count_evidence, "{id} evidence");
        assert!(run.counts.iter().all(|(_, n)| *n == 0), "{id} counts");
        assert!(
            run.problem.as_deref().unwrap_or("").contains(needle),
            "{id} problem: {:?}",
            run.problem
        );
    }

    // The untouched exact-family Hugo inventory remains valid-zero evidence.
    let valid = runs.iter().find(|r| r.id == hugo_inventory).unwrap();
    assert!(
        valid.count_evidence,
        "valid-zero Hugo inventory remains evidence"
    );
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
        let ids = choices.iter().map(|c| c.id.clone()).collect::<Vec<_>>();
        // Return the shared module to the family step for the next probe.
        m.answer(
            "migrate.new.mode",
            &cv_model::dialog::DialogAnswer {
                action: None,
                ..Default::default()
            },
        );
        m.run(NEW);
        ids
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
