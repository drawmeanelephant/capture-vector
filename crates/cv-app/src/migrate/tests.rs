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
    assert_eq!(load(d.path()).runs[0].status, RunStatus::Running);
    assert_eq!(load(d.path()).runs[0].id, id);
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
