//! Typed engine errors. Doctor consumes these directly; the `Engine` port
//! flattens them into `cv_core::Error::Engine` with the same text.

use crate::exit::ExitOutcome;
use cv_core::engine::EngineSource;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("boris not found (searched: {searched:?})")]
    NotFound { searched: Vec<PathBuf> },
    #[error("CV_BORIS_BIN points at {0:?}, which is not an executable file")]
    OverrideMissing(PathBuf),
    #[error(
        "boris version mismatch at {path:?} ({source_kind:?}): found {found} (vcs {found_vcs:?}), pinned {expected} (vcs {expected_vcs})"
    )]
    VersionMismatch {
        path: PathBuf,
        source_kind: EngineSource,
        found: String,
        found_vcs: Option<String>,
        expected: &'static str,
        expected_vcs: &'static str,
    },
    #[error("boris --build-info at {path:?} is not a boris build-info record: {raw:?}")]
    BuildInfoInvalid { path: PathBuf, raw: String },
    #[error("spawn {program:?}: {source}")]
    Spawn {
        program: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("watch handshake failed: {0}")]
    Handshake(String),
    #[error("watch is not supported by this engine on this OS: {0}")]
    WatchUnsupported(String),
    #[error("a watch session is already live for this workspace")]
    WatchAlreadyLive,
    #[error("cancelled; child process group killed")]
    Cancelled,
    #[error("child {stream} exceeded the {limit}-byte capture limit")]
    OutputLimitExceeded { stream: &'static str, limit: usize },
    #[error("verb not dispatchable through cv-engine: {0}")]
    UnsupportedVerb(String),
    #[error("refused: {0}")]
    Refused(String),
    #[error("path {0:?} escapes the workspace root")]
    PathEscape(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("child ended abnormally: {0:?}")]
    Abnormal(ExitOutcome),
}

impl From<EngineError> for cv_core::Error {
    fn from(e: EngineError) -> Self {
        cv_core::Error::Engine(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, EngineError>;
