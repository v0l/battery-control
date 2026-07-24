//! Transport for PACE over RS485. `read_frame` returns whatever bytes are
//! currently available; the caller reassembles the Modbus response.

use crate::error::Result;
use async_trait::async_trait;

#[async_trait]
pub trait Transport: Send {
    async fn open(&mut self) -> Result<()>;
    async fn close(&mut self) -> Result<()>;
    async fn write(&mut self, data: &[u8]) -> Result<usize>;
    /// Read some bytes. Returns an empty vec on timeout.
    async fn read_frame(&mut self) -> Result<Vec<u8>>;
}

#[cfg(feature = "serial")]
mod serial;
#[cfg(feature = "serial")]
pub use serial::SerialTransport;
