//! Live smoke against a real `boris-migration-lab` (ignored by default):
//!
//! ```sh
//! CV_MIGRATION_LAB_BIN=<lab> CV_MIGRATION_LAB_FIXTURES=<lab checkout>/fixtures \
//!   cargo test -p cv-app --test migrate_live -- --ignored
//! ```
//!
//! `CV_MIGRATE_LIVE_ROOT=<dir>` keeps the workspace so the app can be
//! launched against it (`--project <dir>`).

use cv_app::migrate::{DiskPort, locate, run_lab};
use cv_engine::lab as engine_lab;
use cv_engine::proc::CancelToken;
use cv_model::modules::migrate::{LabState, MigratePort, RunStatus};
use std::path::PathBuf;

const MODES: &[(&str, &str)] = &[
    ("hugo", "mini-hugo"),
    ("jekyll", "mini-jekyll"),
    ("substack", "mini-substack"),
];

#[test]
#[ignore = "needs a real boris-migration-lab"]
fn real_lab_runs_project_like_the_captures() {
    let fixtures = PathBuf::from(std::env::var_os("CV_MIGRATION_LAB_FIXTURES").expect("fixtures"));
    let keep = std::env::var_os("CV_MIGRATE_LIVE_ROOT").map(PathBuf::from);
    let tmp = tempfile::tempdir().unwrap();
    let root = keep.unwrap_or_else(|| tmp.path().to_path_buf());
    std::fs::create_dir_all(&root).unwrap();
    let root = root.canonicalize().unwrap();
    let lab = engine_lab::locate(
        &engine_lab::locator_from_env(&root),
        &root,
        engine_lab::PINNED,
    )
    .expect("pinned lab");
    for (i, (mode, input)) in MODES.iter().enumerate() {
        let rec = run_lab(
            &root,
            &format!("{}-{mode}", cv_app::migrate::now_ms() + i as i64),
            &lab,
            engine_lab::mode(mode).unwrap(),
            vec![("--root".into(), fixtures.join(input))],
            Vec::new(),
            &CancelToken::new(),
        )
        .unwrap();
        assert_eq!(rec.refused, None, "{mode}");
        assert_eq!(rec.finished.as_ref().unwrap().exit, "ok", "{mode}");
    }
    let state = locate(&root);
    assert!(matches!(state, LabState::Located { .. }), "{state:?}");
    let imports = DiskPort::open(&root, state).load();
    assert_eq!(imports.runs.len(), MODES.len());
    for r in &imports.runs {
        assert_eq!(r.status, RunStatus::Ok, "{}", r.id);
        assert_eq!(r.problem, None, "{}", r.id);
        assert!(r.format.is_some(), "{}", r.id);
        assert!(!r.counts.is_empty(), "{}", r.id);
    }
}

/// Every inventory-first mode the wizard exposes must really decode: the
/// regression for the reviewer's ghost/mkdocs/mastodon/atproto/tinderbox
/// dead ends, which are no longer offered.
#[test]
#[ignore = "needs a real boris-migration-lab"]
fn real_inventory_modes_decode_with_count_evidence() {
    let fixtures = PathBuf::from(std::env::var_os("CV_MIGRATION_LAB_FIXTURES").expect("fixtures"));
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let lab = engine_lab::locate(
        &engine_lab::locator_from_env(&root),
        &root,
        engine_lab::PINNED,
    )
    .expect("pinned lab");

    // The exposed inventory-first families and their fixture inputs.
    let cases: &[(&str, &str, &str)] = &[
        ("hugo-inventory", "--root", "mini-hugo"),
        ("jekyll-inventory", "--root", "mini-jekyll"),
        ("substack-inventory", "--root", "mini-substack"),
        ("enex-inventory", "--root", "mini-enex"),
    ];
    for (i, (mode, flag, input)) in cases.iter().enumerate() {
        let spec = engine_lab::mode(mode).unwrap();
        // Sanity: this mode is actually exposed through the wizard projection.
        assert!(
            cv_app::migrate::modes().iter().any(|m| m.name == *mode),
            "{mode} must remain projected"
        );
        let input = fixtures.join(input);
        let rec = run_lab(
            &root,
            &format!("{}-{mode}", cv_app::migrate::now_ms() + i as i64),
            &lab,
            spec,
            vec![(flag.to_string(), input)],
            Vec::new(),
            &CancelToken::new(),
        )
        .unwrap();
        assert_eq!(rec.refused, None, "{mode}");
        assert_eq!(rec.finished.as_ref().unwrap().exit, "ok", "{mode}");
    }
    let imports = DiskPort::open(&root, locate(&root)).load();
    assert_eq!(imports.runs.len(), cases.len());
    for r in &imports.runs {
        assert_eq!(r.status, RunStatus::Ok, "{}", r.id);
        assert_eq!(r.problem, None, "{} must decode", r.id);
        assert!(
            r.count_evidence,
            "{} must carry positive count evidence",
            r.id
        );
    }
}

/// The dead-end families the wizard no longer offers: their inventory
/// formats are not mirrored in cv-contracts, so they stay unprojected
/// until contracts exist.
#[test]
fn unsupported_inventory_first_modes_are_not_projected() {
    let projected: Vec<_> = cv_app::migrate::modes()
        .into_iter()
        .map(|m| m.name)
        .collect();
    for mode in ["ghost", "mkdocs", "mastodon", "atproto", "tinderbox"] {
        assert!(
            !projected.iter().any(|m| m == mode),
            "{mode} must not be offered until its contract exists"
        );
    }
    for mode in ["hugo", "jekyll", "substack", "enex", "wordpress"] {
        assert!(projected.iter().any(|m| m == mode), "{mode} stays offered");
    }
}
