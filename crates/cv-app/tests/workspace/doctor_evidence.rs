use super::*;

#[cfg(unix)]
#[test]
fn a_located_lab_is_disclosed_with_its_source_and_matching_pin() {
    let ws = workspace("happy");
    let cfg = tempfile::tempdir().unwrap();
    let lab = FakeLab::new(cv_engine::lab::PINNED.tool_id);
    let mut env = absent_env(cfg.path());
    env.lab_locator = Some(lab.locator());
    let mut wb = init();
    open(&mut wb, ws.path(), &env);

    let status = doctor_row(&wb, "lab", "lab.status");
    assert_eq!(status.title, cv_engine::lab::PINNED.tool_id);
    assert_eq!(status.detail, "CV_MIGRATION_LAB_BIN");
    assert_eq!(status.badge.as_ref().map(|c| c.text.as_str()), Some("OK"));
    assert_eq!(
        doctor_row(&wb, "lab", "lab.path").title,
        lab.bin.display().to_string()
    );
    assert_eq!(
        doctor_row(&wb, "lab", "lab.pin")
            .badge
            .as_ref()
            .map(|c| c.text.as_str()),
        Some("match")
    );
}

#[cfg(unix)]
#[test]
fn a_lab_pin_mismatch_is_disclosed_without_falling_through() {
    let ws = workspace("happy");
    let cfg = tempfile::tempdir().unwrap();
    let lab = FakeLab::new("boris-migration-lab/0.8.0");
    let mut env = absent_env(cfg.path());
    env.lab_locator = Some(lab.locator());
    let mut wb = init();
    open(&mut wb, ws.path(), &env);

    let status = doctor_row(&wb, "lab", "lab.status");
    assert_eq!(status.title, "boris-migration-lab/0.8.0");
    assert_eq!(
        status.detail,
        format!("pinned {}", cv_engine::lab::PINNED.tool_id)
    );
    assert_eq!(
        doctor_row(&wb, "lab", "lab.pin")
            .badge
            .as_ref()
            .map(|c| c.text.as_str()),
        Some("differs")
    );
}

#[test]
fn an_absent_lab_is_disclosed_with_the_injected_search_count() {
    let ws = workspace("happy");
    let cfg = tempfile::tempdir().unwrap();
    let empty = tempfile::tempdir().unwrap();
    let mut env = absent_env(cfg.path());
    env.lab_locator = Some(Locator {
        path_var: Some(std::env::join_paths([empty.path()]).unwrap()),
        ..Locator::default()
    });
    let mut wb = init();
    open(&mut wb, ws.path(), &env);

    let status = doctor_row(&wb, "lab", "lab.status");
    assert_eq!(status.title, "boris-migration-lab not found");
    assert_eq!(status.detail, "1 searched");
}

#[test]
fn a_fresh_imports_namespace_is_disclosed_without_creating_it() {
    let ws = workspace("happy");
    let cfg = tempfile::tempdir().unwrap();
    let env = absent_env(cfg.path());
    let mut wb = init();
    open(&mut wb, ws.path(), &env);

    let ns = doctor_row(&wb, "store", "store.ns.imports");
    assert_eq!(ns.title, ".cv/imports");
    assert_eq!(ns.detail, "not stamped");
    assert_eq!(
        doctor_row(&wb, "store", "store.imports.runs").detail,
        "none"
    );
    assert_eq!(
        doctor_row(&wb, "store", "store.imports.review").detail,
        "no decisions"
    );
    assert!(
        !ws.path().join(".cv/imports").exists(),
        "Doctor/store inspection must not create the imports namespace"
    );
}

#[test]
fn degraded_imports_evidence_is_counted_without_rewriting_it() {
    let ws = workspace("happy");
    let imports = ws.path().join(".cv/imports");
    std::fs::create_dir_all(imports.join("run-1")).unwrap();
    std::fs::write(
        imports.join("schema.json"),
        r#"{"format":"cv-sidecar","namespace":"imports","schema_version":1}"#,
    )
    .unwrap();
    std::fs::write(imports.join("run-1/run.json"), b"not json").unwrap();
    std::fs::write(imports.join("review.jsonl"), b"not json\n").unwrap();
    let before_run = std::fs::read(imports.join("run-1/run.json")).unwrap();
    let before_review = std::fs::read(imports.join("review.jsonl")).unwrap();

    let cfg = tempfile::tempdir().unwrap();
    let env = absent_env(cfg.path());
    let mut wb = init();
    open(&mut wb, ws.path(), &env);

    let ns = doctor_row(&wb, "store", "store.ns.imports");
    assert_eq!(ns.detail, "schema v1");
    let runs = doctor_row(&wb, "store", "store.imports.runs");
    assert_eq!(runs.detail, "1 run");
    assert_eq!(
        runs.badge.as_ref().map(|c| c.text.as_str()),
        Some("degraded")
    );
    let review = doctor_row(&wb, "store", "store.imports.review");
    assert_eq!(review.detail, "1 line");
    assert_eq!(
        review.badge.as_ref().map(|c| c.text.as_str()),
        Some("degraded")
    );
    assert_eq!(
        std::fs::read(imports.join("run-1/run.json")).unwrap(),
        before_run
    );
    assert_eq!(
        std::fs::read(imports.join("review.jsonl")).unwrap(),
        before_review
    );
}
