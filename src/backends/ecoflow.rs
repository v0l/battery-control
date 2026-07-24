//! Adapter for [`ecoflow`] — EcoFlow encrypted power stations over **local BLE**
//! (no cloud data path). Only the encrypted high-end models are supported
//! (Smart Home Panel 2 `HD31`, Delta Pro Ultra `Y711`).
//!
//! Auth needs the account `user_id` (a one-time value extracted from the app);
//! it is read from the `ECOFLOW_USER_ID` environment variable at connect time.
//! Everything after that is fully local.

use crate::battery::Battery;
use crate::types::{BatteryStatus, Command, Reading};
use crate::{Capabilities, DeviceInfo, Error, Result};
use async_trait::async_trait;
use ecoflow::Ecoflow;

/// An EcoFlow device exposed through the unified [`Battery`] trait (experimental
/// — telemetry field mapping wants hardware validation).
pub struct EcoflowStation {
    dev: Ecoflow,
    info: DeviceInfo,
}

impl EcoflowStation {
    /// Connect over BLE and authenticate. `serial` comes from discovery; the
    /// account `user_id` is taken from `ECOFLOW_USER_ID`.
    #[cfg(feature = "ecoflow")]
    pub async fn connect_bluetooth(id: &str, serial: &str) -> Result<Self> {
        let user_id = std::env::var("ECOFLOW_USER_ID").map_err(|_| {
            Error::InvalidArgument(
                "EcoFlow needs the account user_id — set ECOFLOW_USER_ID (see docs)".into(),
            )
        })?;
        let dev = Ecoflow::connect_ble(id, serial, &user_id)
            .await
            .map_err(|e| Error::Transport(e.to_string()))?;
        let info = DeviceInfo {
            backend: "ecoflow".into(),
            manufacturer: Some("EcoFlow".into()),
            model: Some(dev.model().to_string()),
            serial: Some(dev.serial().to_string()),
            ..Default::default()
        };
        Ok(Self { dev, info })
    }
}

#[async_trait]
impl Battery for EcoflowStation {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::READ_BASIC
    }

    async fn status(&mut self) -> Result<BatteryStatus> {
        let t = self
            .dev
            .read()
            .await
            .map_err(|e| Error::Transport(e.to_string()))?;
        let mut s = BatteryStatus::default();
        s.set(Reading::Soc, t.soc);
        Ok(s)
    }

    async fn execute(&mut self, _cmd: Command) -> Result<()> {
        Err(Error::Unsupported)
    }

    async fn disconnect(&mut self) -> Result<()> {
        self.dev
            .disconnect()
            .await
            .map_err(|e| Error::Transport(e.to_string()))
    }
}
