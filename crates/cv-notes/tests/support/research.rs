// Manifest-verified access to fixtures/research + an engine that pairs
// StubEngine's replayed verbs with cv-engine's real staged writer.
#![allow(dead_code)]

use cv_core::engine::ExitClass;
use cv_core::engine::{
    Engine, EngineCaps, EngineInfo, EventSink, Fingerprint, PageDraft, Verb, WatchHandle,
    WriteOutcome,
};
use cv_core::rules::ArtifactSet;
use cv_fixtures::{Manifest, StubEngine};
use std::path::{Path, PathBuf};

pub fn root() -> PathBuf {
    cv_fixtures::fixtures_root().join("research")
}

pub fn manifest() -> Manifest {
    Manifest::load(&root().join("MANIFEST.toml"))
}

/// Verified bytes of `fixtures/research/<rel>`.
pub fn file(rel: &str) -> Vec<u8> {
    let bytes = std::fs::read(root().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"));
    manifest().verify(rel, &bytes).unwrap();
    bytes
}

pub fn file_str(rel: &str) -> String {
    String::from_utf8(file(rel)).unwrap()
}

/// Copy a verified corpus-src tree into `dst`.
pub fn copy_corpus(name: &str, dst: &Path) {
    let m = manifest();
    let prefix = format!("corpus-src/{name}/");
    for (rel, _) in m.pinned_files() {
        if let Some(sub) = rel.strip_prefix(&prefix) {
            let to = dst.join(sub);
            std::fs::create_dir_all(to.parent().unwrap()).unwrap();
            std::fs::write(to, file(rel)).unwrap();
        }
    }
}

pub struct StagedStub {
    pub stub: StubEngine,
    pub root: PathBuf,
}

impl Engine for StagedStub {
    fn locate(&self) -> cv_core::Result<EngineInfo> {
        self.stub.locate()
    }
    fn capabilities(&self) -> EngineCaps {
        self.stub.capabilities()
    }
    fn snapshot(&self) -> cv_core::Result<ArtifactSet> {
        self.stub.snapshot()
    }
    fn watch(&self, sink: EventSink) -> cv_core::Result<Box<dyn WatchHandle>> {
        self.stub.watch(sink)
    }
    fn run(&self, cmd: &Verb, sink: EventSink) -> cv_core::Result<ExitClass> {
        self.stub.run(cmd, sink)
    }
    fn write_content(
        &self,
        draft: &PageDraft,
        base: Option<Fingerprint>,
    ) -> cv_core::Result<WriteOutcome> {
        cv_engine::write::write_staged(&self.root, &draft.path, draft.body.as_bytes(), base)
            .map_err(|e| cv_core::Error::Engine(e.to_string()))
    }
}
