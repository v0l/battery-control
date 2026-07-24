//! Transport for the Pylontech RS485 console. Frames are `\r`-terminated ASCII
//! lines, so `read_line` reads until the terminator.

use crate::error::Result;
use async_trait::async_trait;

#[async_trait]
pub trait Transport: Send {
    async fn open(&mut self) -> Result<()>;
    async fn close(&mut self) -> Result<()>;
    async fn write(&mut self, data: &[u8]) -> Result<usize>;
    /// Read one `\r`-terminated line (terminator included). Empty on timeout.
    async fn read_line(&mut self) -> Result<Vec<u8>>;
}

#[cfg(feature = "serial")]
mod serial;
#[cfg(feature = "serial")]
pub use serial::SerialTransport;
