//! Engine port — capability-honest, synchronous. `cv-engine` is the only
//! `std::process::Command` owner; consumers see this trait and fixtures
//! (`StubEngine`) only.

use crate::events::Envelope;
use crate::sys::{Hash, Timestamp};
use serde::{Deserialize, Serialize};

/// Event sink — the channel-sink port shape. Implementations deliver
/// envelopes; the rune side wakes and drains, never blocks.
pub type EventSink = Box<dyn FnMut(Envelope) + Send>;

/// What this engine can do **on this OS** — probed at startup, surfaced
/// in Doctor. Platform gaps are honest, never hidden.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct EngineCaps {
    pub watch: bool,
    pub serve: bool,
    pub authoring: bool,
    pub writes: bool,
}

/// Exit code classes: Boris exit `0/1/2/3` maps verbatim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExitClass {
    Ok,
    ContentFailure,
    UsageError,
    IoFailure,
}

impl ExitClass {
    pub fn from_code(code: i32) -> Self {
        match code {
            0 => Self::Ok,
            1 => Self::ContentFailure,
            2 => Self::UsageError,
            _ => Self::IoFailure,
        }
    }
}

/// Engine identity as recorded in the ledger: `boris --version` gate +
/// `--build-info` output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineInfo {
    pub version: String,
    pub vcs_revision: Option<String>,
    pub caps: EngineCaps,
}

/// A Boris verb. Mutating verbs serialize per workspace; reads
/// parallelize (see `EngineScheduler`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Verb {
    Build,
    Validate,
    Check,
    Plan,
    Graph,
    Export { kind: ExportKind },
    Publish { target: String },
    Custom { name: String, args: Vec<String> },
}

impl Verb {
    /// The verb's name as `EngineEvent::VerbFinished` reports it — one
    /// spelling for every `Engine` implementation.
    pub fn name(&self) -> String {
        match self {
            Self::Build => "build".into(),
            Self::Validate => "validate".into(),
            Self::Check => "check".into(),
            Self::Plan => "plan".into(),
            Self::Graph => "graph".into(),
            Self::Export { kind } => format!("export:{kind:?}").to_lowercase(),
            Self::Publish { target } => format!("publish:{target}"),
            Self::Custom { name, .. } => name.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExportKind {
    Out,
    Rag,
    Context,
    Llms,
    Rss,
    Sitemap,
}

/// Fingerprint stolen from boris-editor: SHA-256 + mtime, staged
/// `*.tmp` + atomic rename (the watcher treats a rename as one clean
/// change; Windows retries on sharing violation).
///
/// Windows retry pin: on `ERROR_SHARING_VIOLATION`/`ERROR_ACCESS_DENIED`
/// from the rename, retry up to 8 times on a linear 25 ms backoff
/// (`cv-engine`'s `SHARING_RETRIES`/`SHARING_BACKOFF`; AV filters and
/// reader handles release quickly). Exhaustion fails the
/// write as an honest diagnostic — saves never silently drop, and the
/// retry loop never runs on the UI thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fingerprint {
    pub sha256: Hash,
    pub mtime: Timestamp,
}

/// Draft content handed to `write_content`. The writer does **not**
/// validate frontmatter — `boris validate` judges.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageDraft {
    /// Repo-relative destination (a path is a write target, not an id).
    pub path: String,
    pub body: String,
    /// Expected fingerprint of the file being replaced — saves are
    /// fingerprinted, so a file changed under us fails honestly.
    pub base: Option<Fingerprint>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WriteOutcome {
    Written {
        fingerprint: Fingerprint,
    },
    /// Base fingerprint didn't match disk — the file moved under us.
    StaleBase {
        found: Fingerprint,
    },
}

/// Handle to a live watch session. `stop()` kills child + process group.
pub trait WatchHandle: Send {
    fn session(&self) -> &crate::events::SessionId;
    fn stop(&mut self);
}

/// The engine port. Synchronous signatures by design — `cv-engine`
/// adapts subprocess IO onto threads and feeds `EventSink`; callers stay
/// sync.
pub trait Engine: Send {
    fn locate(&self) -> crate::Result<EngineInfo>;
    fn capabilities(&self) -> EngineCaps;
    /// Disk-truth resync — re-read `.boris/` + last reports. Called on
    /// watch session change or cursor desync.
    fn snapshot(&self) -> crate::Result<crate::rules::ArtifactSet>;
    fn watch(&self, sink: EventSink) -> crate::Result<Box<dyn WatchHandle>>;
    fn run(&self, cmd: &Verb, sink: EventSink) -> crate::Result<ExitClass>;
    fn write_content(
        &self,
        draft: &PageDraft,
        base: Option<Fingerprint>,
    ) -> crate::Result<WriteOutcome>;
}

/// Where the engine binary came from (Doctor disclosure).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EngineSource {
    /// `CV_BORIS_BIN` env override.
    EnvOverride,
    /// Bundled with the app — the pinned one in `MANIFEST.toml`.
    Bundled,
    Path,
    SiblingCheckout,
}

pub use crate::events::Cursor as EngineCursor;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_map_verbatim() {
        assert_eq!(ExitClass::from_code(0), ExitClass::Ok);
        assert_eq!(ExitClass::from_code(1), ExitClass::ContentFailure);
        assert_eq!(ExitClass::from_code(2), ExitClass::UsageError);
        assert_eq!(ExitClass::from_code(3), ExitClass::IoFailure);
        assert_eq!(ExitClass::from_code(137), ExitClass::IoFailure);
    }
}
