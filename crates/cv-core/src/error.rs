//! Crate-wide error type. Lane crates wrap this or their own typed errors;
//! Boris stderr/exit codes always travel as data, never stringified away.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("engine: {0}")]
    Engine(String),
    #[error("store: {0}")]
    Store(String),
    #[error("contract drift: {0}")]
    ContractDrift(String),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;
