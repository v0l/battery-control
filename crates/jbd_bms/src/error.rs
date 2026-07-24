use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("transport: {0}")]
    Transport(String),
    #[error("protocol: {0}")]
    Protocol(String),
    #[error("device not found")]
    NotFound,
    #[error("timed out")]
    Timeout,
    #[error("unsupported: {0}")]
    Unsupported(String),
}

pub type Result<T> = std::result::Result<T, Error>;
