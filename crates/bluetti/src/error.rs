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
}

pub type Result<T> = std::result::Result<T, Error>;
