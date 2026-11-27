//! What the composition root learned about this install, as plain data:
//! the engine it located (or why it could not), the capability probe,
//! the `.cv/` sidecar state and the app build. Filled in `cv-app`; the
//! Doctor module only reads it.

use cv_core::engine::{EngineCaps, EngineSource};
use std::sync::{Arc, Mutex};

use crate::modules::migrate::LabState;

#[derive(Debug, Clone, Default)]
pub struct Facts {
    pub engine: EngineState,
    /// The engine pin every gate checks against (`fixtures/MANIFEST.toml`).
    pub pin: Pin,
    /// Every path the locator tries, in order, with the source it stands
    /// for. Empty until a workspace boot searches.
    pub candidates: Vec<(EngineSource, String)>,
    pub lab: Lab,
    pub probe: Option<Probe>,
    pub store: Store,
    pub app: AppBuild,
    /// `(prefix, replacement)` pairs applied to every string in the
    /// support bundle (the workspace root, the home directory).
    pub redact: Vec<(String, String)>,
}

/// Migration-lab locate facts, mirroring the engine rows with the
/// lab-specific contract (`version.txt` plus the pinned tool id).
#[derive(Debug, Clone, Default)]
pub struct Lab {
    pub state: LabState,
    /// The migration-lab pin every gate checks against.
    pub pin: Pin,
    /// Every path the lab locator tries, in order, with the source it
    /// stands for. Empty until a workspace boot searches.
    pub candidates: Vec<(EngineSource, String)>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pin {
    pub version: String,
    pub vcs_revision: String,
}

/// The engine lookup, from "nothing searched yet" to a gated binary.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum EngineState {
    /// No workspace is open, so nothing has looked for Boris.
    #[default]
    NotStarted,
    /// The boot thread is locating, gating and probing.
    Locating,
    /// The fixture demo replays a captured session; no binary runs.
    Fixture {
        corpus: String,
        version: String,
        vcs_revision: Option<String>,
    },
    Located(Located),
    /// No candidate exists; [`Facts::candidates`] lists what was searched.
    NotFound,
    /// `CV_BORIS_BIN` is set but does not name an executable file.
    OverrideMissing {
        path: String,
    },
    VersionMismatch {
        source: EngineSource,
        path: String,
        found: String,
        found_vcs: Option<String>,
    },
    /// `--version` matched but `--build-info` is not a Boris record.
    BuildInfoInvalid {
        path: String,
        raw: String,
    },
    /// Any other gate failure (spawn error, abnormal exit), verbatim.
    Failed {
        error: String,
    },
}

impl EngineState {
    /// The binary the gate ran, when it got that far.
    pub fn path(&self) -> Option<&str> {
        match self {
            Self::Located(l) => Some(&l.path),
            Self::OverrideMissing { path }
            | Self::VersionMismatch { path, .. }
            | Self::BuildInfoInvalid { path, .. } => Some(path),
            _ => None,
        }
    }

    pub fn source(&self) -> Option<&EngineSource> {
        match self {
            Self::Located(l) => Some(&l.source),
            Self::OverrideMissing { .. } => Some(&EngineSource::EnvOverride),
            Self::VersionMismatch { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// A binary that passed the `--version` / `--build-info` gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Located {
    pub source: EngineSource,
    pub path: String,
    pub version: String,
    pub vcs_revision: String,
    /// `boris --build-info` stdout, verbatim.
    pub build_info_raw: String,
}

/// What the capability probe observed on this OS.
#[derive(Debug, Clone)]
pub struct Probe {
    pub os: String,
    pub caps: EngineCaps,
    /// `boris watch --watch-json` handshook; otherwise one-shot rebuilds
    /// under a file watcher.
    pub native_watch: bool,
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Store {
    /// The open workspace; `None` shows the fixture demo or nothing.
    pub root: Option<String>,
    pub namespaces: Vec<Namespace>,
    /// Read-only health scan of `.cv/imports/`; `None` before a workspace
    /// boot has inspected the store.
    pub imports: Option<ImportsHealth>,
}

/// Sidecar evidence collected without opening run records for writing.
/// Counts distinguish ordinary empty state (a fresh workspace) from
/// degraded state (unreadable evidence), which the namespace stamp alone
/// cannot express.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportsHealth {
    /// Number of run directories found; `None` when the scan failed.
    pub runs: Option<u64>,
    /// Run directories whose `run.json` is absent or does not decode.
    pub unreadable_runs: u64,
    /// Non-blank lines in `review.jsonl`, including malformed ones.
    pub review_lines: u64,
    pub undecodable_review_lines: u64,
    pub unsupported_review_lines: u64,
    /// Bytes in a torn final line, kept for the next append.
    pub torn_review_bytes: u64,
    /// Why the run scan failed, verbatim (for example a symlinked `.cv`).
    pub scan_error: Option<String>,
    /// Why the review log could not be opened or read, verbatim.
    pub review_error: Option<String>,
}

/// One `.cv/<ns>/` sidecar namespace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Namespace {
    pub name: String,
    /// Relative to the workspace root, e.g. `.cv/research`.
    pub dir: String,
    /// The schema version this build reads and writes.
    pub supported: u32,
    pub stamp: Stamp,
    pub lock: Option<Lock>,
    /// The rebuildable index in OS app-data, when the namespace has one.
    pub index: Option<String>,
    /// Why this window could not open the namespace, verbatim.
    pub error: Option<String>,
}

/// `schema.json` as found on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stamp {
    /// No directory, or an empty one.
    Absent,
    /// Files written before stamping existed (version 0).
    Unstamped,
    Version(u32),
    Unreadable(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lock {
    /// Relative to the workspace root.
    pub path: String,
    pub holder: LockHolder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockHolder {
    ThisWindow,
    /// Another Capture Vector (or a stuck one) has the workspace open.
    OtherProcess,
    /// The lock file exists but no writer holds it.
    NotHeld,
    /// The lock exists but its holder could not be determined.
    Unknown,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppBuild {
    pub version: String,
    /// Empty for a plain `cargo build`; `xtask release` stamps it.
    pub vcs_revision: String,
    pub target: String,
    pub profile: String,
    pub toolchain: String,
    pub logging: String,
}

/// Shared between the composition root, which updates it from the
/// engine boot thread, and the Doctor module, which reads it while
/// rendering. A writer follows an update with an `Inbox` send, so the
/// UI wakes and redraws with the new facts.
#[derive(Debug, Clone, Default)]
pub struct Feed(Arc<Mutex<Facts>>);

impl Feed {
    pub fn get(&self) -> Facts {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn update(&self, f: impl FnOnce(&mut Facts)) {
        f(&mut self.0.lock().unwrap_or_else(|e| e.into_inner()));
    }
}
