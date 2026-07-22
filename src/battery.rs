use crate::{BatteryStatus, Capabilities, Command, DeviceInfo, Error, Result};
use async_trait::async_trait;

/// A uniform, async interface to any supported battery / BMS / power station.
///
/// Implementors are thin adapters over a device-specific crate or protocol.
/// Read support is expressed by populating [`BatteryStatus`]; control support is
/// gated by [`Capabilities`] and dispatched through [`Command`].
#[async_trait]
pub trait Battery: Send {
    /// Static device identity.
    fn info(&self) -> &DeviceInfo;

    /// What this device can read and control.
    fn capabilities(&self) -> Capabilities;

    /// Fetch a fresh, normalized status snapshot.
    async fn status(&mut self) -> Result<BatteryStatus>;

    /// Execute a control command.
    ///
    /// The default implementation rejects everything with [`Error::Unsupported`];
    /// controllable backends override this. Implementations should return
    /// [`Error::Unsupported`] for any individual command they don't handle.
    async fn execute(&mut self, cmd: Command) -> Result<()> {
        let _ = cmd;
        Err(Error::Unsupported)
    }
}

/// Helper for adapters: assert a capability is present before acting on a command.
pub(crate) fn require(caps: Capabilities, needed: Capabilities) -> Result<()> {
    if caps.contains(needed) {
        Ok(())
    } else {
        Err(Error::Unsupported)
    }
}
