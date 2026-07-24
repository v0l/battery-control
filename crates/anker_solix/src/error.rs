use thiserror::Error;

#[derive(Error, Debug)]
pub enum AnkerError {
    #[error("no bluetooth adapter found")]
    NoAdapter,

    #[error("device not found: {0}")]
    DeviceNotFound(String),

    #[error("required GATT characteristic {0} not found")]
    CharacteristicNotFound(&'static str),

    #[error("not connected / session not negotiated")]
    NotNegotiated,

    #[error("encryption negotiation timed out")]
    NegotiationTimeout,

    #[error("timed out waiting for response")]
    ResponseTimeout,

    #[error("malformed packet: {0}")]
    BadPacket(String),

    #[error("crypto error: {0}")]
    Crypto(String),

    #[error("bluetooth error: {0}")]
    Bluetooth(#[from] btleplug::Error),

    #[error("unsupported model: {0}")]
    UnsupportedModel(String),
}

pub type Result<T> = std::result::Result<T, AnkerError>;
