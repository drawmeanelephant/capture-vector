//! One pin: cv-engine's `PINNED` is exactly the engine the fixtures were
//! captured with (`fixtures/MANIFEST.toml` `engine_version`), which is
//! also what `xtask fetch-boris` builds and `xtask release` bundles.

use cv_contracts::build_info::BuildInfo;
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
