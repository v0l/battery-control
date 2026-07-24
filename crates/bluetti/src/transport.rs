//! Transport for Bluetti BLE: notify `FF01`, write `FF02`. `read_frame`
//! returns one notification; the caller reassembles the Modbus response.

use crate::error::Result;
use async_trait::async_trait;

#[async_trait]
pub trait Transport: Send {
    async fn open(&mut self) -> Result<()>;
    async fn close(&mut self) -> Result<()>;
    async fn write(&mut self, data: &[u8]) -> Result<usize>;
    async fn read_frame(&mut self) -> Result<Vec<u8>>;
    fn identity(&self) -> ble_util::Identity {
        ble_util::Identity::default()
    }
}

#[cfg(feature = "bluetooth")]
mod bluetooth;
#[cfg(feature = "bluetooth")]
pub use bluetooth::{scan, BluetoothTransport, BtDevice};
