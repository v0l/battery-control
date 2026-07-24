//! Transport for the SOK protocols. Each `read_frame` returns one BLE
//! notification; the caller reassembles (ABC/Modbus) or header-matches (EE).

use crate::data::Variant;
use crate::error::Result;
use async_trait::async_trait;

#[async_trait]
pub trait Transport: Send {
    async fn open(&mut self) -> Result<()>;
    async fn close(&mut self) -> Result<()>;
    async fn write(&mut self, data: &[u8]) -> Result<usize>;
    /// Read one notification. Returns an empty vec on timeout.
    async fn read_frame(&mut self) -> Result<Vec<u8>>;
    /// Which protocol this device speaks (known after `open`).
    fn variant(&self) -> Variant;
}

#[cfg(feature = "bluetooth")]
mod bluetooth;
#[cfg(feature = "bluetooth")]
pub use bluetooth::{scan, BluetoothTransport, BtDevice};
