//! cv-fixtures — captured corpora + `StubEngine` + `StubStore`, and
//! captured migration-lab runs + `StubLab` ([`lab`]).
//!
//! Every fixture is a **real capture** pinned by sha256 in
//! `fixtures/MANIFEST.toml`. Lanes assert by corpus + file name; bytes are
//! verified against the manifest on read so stale or hand-authored
//! fixtures fail loudly.

pub mod lab;
pub mod manifest;
pub mod stub_engine;
pub mod stub_store;

pub use lab::StubLab;
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

    /// Every pinned capture under `prefix` (`docs/tree/`), relative to
    /// the corpus, sorted.
    pub fn pinned_under(&self, prefix: &str) -> Vec<String> {
        let root = format!("captures/{}/", self.name);
        self.manifest
            .pinned_files()
            .filter_map(|(k, _)| k.strip_prefix(&root))
            .filter(|rel| rel.starts_with(prefix))
            .map(str::to_string)
            .collect()
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
    assert_ne!(
        name,
        lab::LAB_DIR,
        "lab captures open with lab::lab_capture"
    );
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

/// The live session's IR projection (`build --out .cv/ir`) captured over
/// a corpus by `scripts/capture-ir-fixtures.sh`, pinned by
/// `fixtures/ir/MANIFEST.toml`: `stdout.txt`, `stderr.txt`,
/// `exit-code.txt`, and `graph.json` when Boris wrote one.
pub fn ir_capture(name: &str) -> Corpus {
    check_rel(name);
    let root = fixtures_root().join("ir");
    let dir = root.join("captures");
    assert!(dir.join(name).is_dir(), "no IR capture for corpus '{name}'");
    Corpus {
        name: name.into(),
        dir,
        manifest: Manifest::load(&root.join("MANIFEST.toml")),
    }
}

/// The `boris init` captures: `help/`, one `<archetype>/` per captured
/// starter (its report plus the `tree/` it wrote), and `refuse/` (a
/// target that is not empty). Opened with [`corpus`], but not a corpus:
/// it has no sources, builds or watch session.
pub const INIT: &str = "init";

/// The palette-verb captures: `<corpus>/<key>/` with `argv.txt` (one
/// argument per line), `stdout.txt`, `stderr.txt` and `exit-code.txt`
/// of one run from the corpus root. Not a corpus either.
pub const VERBS: &str = "verbs";

/// Every Boris corpus name present on disk (neither the lab captures nor
/// the init and verbs pseudo-captures).
pub fn corpora() -> Vec<String> {
    let mut v: Vec<_> = std::fs::read_dir(fixtures_root().join("captures"))
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir() && e.file_name() != lab::LAB_DIR)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n != INIT && n != VERBS)
        .collect();
    v.sort();
    v
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}
