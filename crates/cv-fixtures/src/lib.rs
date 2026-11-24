//! cv-fixtures — captured corpora + `StubEngine` + `StubStore`.
//!
//! Every fixture is a **real capture** pinned by sha256 in
//! `fixtures/MANIFEST.toml`. Lanes assert by corpus + file name; bytes are
//! verified against the manifest on read so stale or hand-authored
//! fixtures fail loudly.

pub mod manifest;
pub mod stub_engine;
pub mod stub_store;

pub use manifest::Manifest;
pub use stub_engine::StubEngine;
pub use stub_store::StubStore;

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// The checked-in fixture root (relative to the workspace).
pub fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

/// A loaded, hash-verified fixture corpus.
pub struct Corpus {
    pub name: String,
    dir: PathBuf,
    manifest: Manifest,
}

impl Corpus {
    /// Verified bytes of one captured file, e.g. `watch/session.ndjson`
    /// or `build/stderr.txt`.
    pub fn file(&self, rel: &str) -> Vec<u8> {
        check_rel(rel);
        let path = self.dir.join(&self.name).join(rel);
        let key = format!("captures/{}/{rel}", self.name);
        let bytes = std::fs::read(&path)
            .unwrap_or_else(|e| panic!("fixture {key} unreadable at {path:?}: {e}"));
        self.manifest
            .verify(&key, &bytes)
            .unwrap_or_else(|e| panic!("fixture {key} failed manifest verification: {e}"));
        bytes
    }

    pub fn file_str(&self, rel: &str) -> String {
        String::from_utf8(self.file(rel)).expect("fixture not utf-8")
    }

    /// The corpus source tree (authoring inputs that produced captures).
    pub fn corpus_src(&self, rel: &str) -> PathBuf {
        check_rel(rel);
        let p = fixtures_root()
            .join("corpus-src")
            .join(&self.name)
            .join(rel);
        self.manifest
            .verify(
                &format!("corpus-src/{}/{rel}", self.name),
                &std::fs::read(&p).expect("corpus-src missing"),
            )
            .expect("corpus-src failed manifest verification");
        p
    }
}

/// A fixture-relative path can't leave the fixture tree: no `..`,
/// absolute, or rooted components. The manifest verifies bytes, not
/// paths — this is what keeps a stray caller from walking out.
fn check_rel(rel: &str) {
    assert!(
        !rel.is_empty()
            && std::path::Path::new(rel)
                .components()
                .all(|c| matches!(c, std::path::Component::Normal(_))),
        "fixture path {rel:?} must be a plain relative path"
    );
}

/// Open a corpus by name (`happy`, `broken-frontmatter`, …).
pub fn corpus(name: &str) -> Corpus {
    check_rel(name);
    let manifest = Manifest::load(&fixtures_root().join("MANIFEST.toml"));
    let dir = fixtures_root().join("captures");
    assert!(
        dir.join(name).is_dir(),
        "unknown corpus '{name}' — assert by manifest name only"
    );
    Corpus {
        name: name.into(),
        dir,
        manifest,
    }
}

/// Every corpus name present on disk.
pub fn corpora() -> Vec<String> {
    let mut v: Vec<_> = std::fs::read_dir(fixtures_root().join("captures"))
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}
