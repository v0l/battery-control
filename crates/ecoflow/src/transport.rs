//! Transport for EcoFlow BLE: write `0002`, notify `0003`. Local only.

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

/// Supported encrypted models keyed by serial prefix.
pub const SUPPORTED_PREFIXES: &[&str] = &["HD31", "Y711"];

/// Human model name for a serial prefix.
pub fn model_name(serial: &str) -> &'static str {
    if serial.starts_with("HD31") {
        "Smart Home Panel 2"
    } else if serial.starts_with("Y711") {
        "Delta Pro Ultra"
    } else {
        "EcoFlow"
    }
}
