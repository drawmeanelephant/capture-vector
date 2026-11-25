//! One pin: cv-engine's `PINNED` is exactly the engine the fixtures were
//! captured with (`fixtures/MANIFEST.toml` `engine_version`), which is
//! also what `xtask fetch-boris` builds and `xtask release` bundles.

use cv_contracts::build_info::BuildInfo;
use cv_engine::lab;
use cv_engine::locate::PINNED;
use cv_fixtures::manifest::Manifest;

#[test]
fn pinned_engine_matches_fixture_manifest() {
    let m = Manifest::load(&cv_fixtures::fixtures_root().join("MANIFEST.toml"));
    let bi: BuildInfo = serde_json::from_str(&m.engine_version).unwrap();
    assert!(bi.is_boris());
    assert_eq!(bi.version, PINNED.version);
    assert_eq!(bi.vcs_revision, PINNED.vcs_revision);
}

/// The lab pin `scripts/fetch-migration-lab.sh` builds is the one the
/// adapter gates on.
#[test]
fn pinned_lab_matches_fixture_manifest() {
    let m = Manifest::load(&cv_fixtures::fixtures_root().join("MANIFEST.toml"));
    let pin = m.migration_lab.expect("[migration_lab] in MANIFEST.toml");
    assert_eq!(pin.tool_id, lab::PINNED.tool_id);
    assert_eq!(pin.commit, lab::PINNED.commit);
}
