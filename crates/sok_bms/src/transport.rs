//! Transport for the SOK protocol. Each `read` returns one BLE notification
//! (one response frame) — SOK frames are tagged by header, not length, so the
//! caller must not coalesce them.

use crate::error::Result;
use async_trait::async_trait;

#[async_trait]
pub trait Transport: Send {
    async fn open(&mut self) -> Result<()>;
    async fn close(&mut self) -> Result<()>;
    async fn write(&mut self, data: &[u8]) -> Result<usize>;
    /// Read one notification frame. Returns an empty vec on timeout.
    async fn read_frame(&mut self) -> Result<Vec<u8>>;
}

#[cfg(feature = "bluetooth")]
mod bluetooth;
#[cfg(feature = "bluetooth")]
pub use bluetooth::{scan, BluetoothTransport, BtDevice, NOTIFY_UUID, SERVICE_UUID, WRITE_UUID};
