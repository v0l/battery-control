use thiserror::Error;

#[derive(Error, Debug)]
pub enum JkError {
    #[error("Transport handle not initialized")]
    TransportNotInitialized,

    #[error("Transport open failed")]
    TransportOpenFailed,

    #[error("Transport close failed")]
    TransportCloseFailed,

    #[error("Write failed: {0}")]
    WriteFailed(i32),

    #[error("Read failed: {0}")]
    ReadFailed(i32),

    #[error("Invalid signature")]
    InvalidSignature,

    #[error("No voltage data found")]
    NoVoltageData,

    #[error("Timed out waiting for BMS update")]
    Timeout,

    #[error("Protocol error: {0}")]
    ProtocolError(String),

    #[error("Transport error: {0}")]
    TransportError(String),
}

pub type Result<T> = std::result::Result<T, JkError>;
