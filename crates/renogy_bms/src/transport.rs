//! Transport for Renogy BLE (BT-1/BT-2): write characteristic `FFD1`, notify
//! characteristic `FFF1`. Each `read_frame` returns one notification; the
//! caller reassembles the Modbus response.

use crate::error::Result;
use async_trait::async_trait;

#[async_trait]
pub trait Transport: Send {
    async fn open(&mut self) -> Result<()>;
    async fn close(&mut self) -> Result<()>;
    async fn write(&mut self, data: &[u8]) -> Result<usize>;
    async fn read_frame(&mut self) -> Result<Vec<u8>>;
}

#[cfg(feature = "bluetooth")]
mod bluetooth;
#[cfg(feature = "bluetooth")]
pub use bluetooth::{scan, BluetoothTransport, BtDevice};
