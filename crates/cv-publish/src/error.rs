use cv_core::id::StableId;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PublishError {
    #[error("ledger: {0}")]
    Ledger(#[from] cv_ledger::LedgerError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("engine: {0}")]
    Engine(#[from] cv_core::Error),
    #[error("unknown run {0:?}")]
    UnknownRun(StableId),
    #[error("refused: {0}")]
    Refused(String),
    #[error("seal broken: {}", .0.join(", "))]
    SealBroken(Vec<&'static str>),
    #[error("unknown target '{0}'")]
    UnknownTarget(String),
    #[error("secret: {0}")]
    Secret(String),
}

pub type Result<T> = std::result::Result<T, PublishError>;
