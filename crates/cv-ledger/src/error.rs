use thiserror::Error;

#[derive(Debug, Error)]
pub enum LedgerError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("journal line {line} is corrupt: {msg}")]
    Corrupt { line: usize, msg: String },
    #[error("index: {0}")]
    Index(#[from] rusqlite::Error),
    #[error("ledger is locked by another writer: {0}")]
    Locked(String),
    #[error("schema: {0}")]
    Schema(#[from] cv_store::sidecar::SidecarError),
    #[error("proof pack: {0}")]
    Proof(String),
}

pub type Result<T> = std::result::Result<T, LedgerError>;
