//! Typed engine errors. Doctor consumes these directly; the `Engine` port
//! flattens them into `cv_core::Error::Engine` with the same text.

use crate::exit::ExitOutcome;
use cv_core::engine::EngineSource;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Spawn failures are free-form text in status lines and support bundles.
/// The typed error keeps the program path for structured handling, but the
/// text names only the binary: the full path is environment evidence, not a
/// safe default for a string that travels.
fn program_label(program: &Path) -> String {
    let raw = program.as_os_str().to_string_lossy();
    raw.rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or("engine")
        .to_string()
}

#[derive(Debug, Error)]
pub enum EngineError {
    /// The paths stay on the error for Doctor; the text only counts them,
    /// since it is shown in the status toast and logged.
    #[error("boris not found (searched {} locations)", .searched.len())]
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
    #[error("spawn {}: {source}", program_label(.program))]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_error_text_names_only_the_binary_file_name() {
        for (program, label) in [
            ("/home/redacted-user/tools/boris", "boris"),
            (r"C:\Users\redacted-user\tools\boris.exe", "boris.exe"),
        ] {
            let err = EngineError::Spawn {
                program: PathBuf::from(program),
                source: std::io::Error::new(std::io::ErrorKind::NotFound, "missing"),
            };
            let msg = err.to_string();
            assert!(msg.starts_with(&format!("spawn {label}:")), "{msg}");
            assert!(!msg.contains("redacted-user"), "{msg}");
        }
    }

    #[test]
    fn not_found_counts_searched_paths_without_listing_them() {
        let searched = (0..24)
            .map(|i| PathBuf::from(format!("/opt/tool-{i}/bin/boris")))
            .collect();
        let msg = EngineError::NotFound { searched }.to_string();
        assert_eq!(msg, "boris not found (searched 24 locations)");
    }
}
