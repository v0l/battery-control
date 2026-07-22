use thiserror::Error;

/// Errors surfaced by any battery backend.
#[derive(Error, Debug)]
pub enum Error {
    /// The device does not support the requested command.
    #[error("operation not supported by this device")]
    Unsupported,

    /// Discovery found no matching device.
    #[error("device not found: {0}")]
    NotFound(String),

    /// A transport / connection failure (BLE, serial, CAN, ...).
    #[error("transport error: {0}")]
    Transport(String),

    /// The device returned data we couldn't decode.
    #[error("decode error: {0}")]
    Decode(String),

    /// A command argument was invalid for this device.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    /// Timed out waiting for the device.
    #[error("timed out")]
    Timeout,

    /// A backend-specific error passed through verbatim.
    #[error("backend error: {0}")]
    Backend(String),
}

pub type Result<T> = std::result::Result<T, Error>;
