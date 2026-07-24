//! Transports for the JBD protocol. `read`/`write` move raw bytes; framing is
//! the caller's job (see [`crate::protocol::FrameAssembler`]).

use crate::error::Result;
use async_trait::async_trait;

#[async_trait]
pub trait Transport: Send {
    async fn open(&mut self) -> Result<()>;
    async fn close(&mut self) -> Result<()>;
    async fn write(&mut self, data: &[u8]) -> Result<usize>;
    /// Read some bytes into `buf`. Returns 0 on timeout (not an error).
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize>;
    /// Static device identity (BLE DIS), read at `open`. Empty by default.
    fn identity(&self) -> ble_util::Identity {
        ble_util::Identity::default()
    }
}

#[cfg(feature = "bluetooth")]
mod bluetooth;
#[cfg(feature = "bluetooth")]
pub use bluetooth::{scan, BluetoothTransport, BtDevice, NOTIFY_UUID, SERVICE_UUID, WRITE_UUID};

#[cfg(feature = "serial")]
mod serial;
#[cfg(feature = "serial")]
pub use serial::SerialTransport;
