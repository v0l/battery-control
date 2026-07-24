//! Adapter for [`sok_bms`] — SOK 12V Bluetooth LiFePO4 batteries.
//!
//! SOK exposes telemetry only (no control channel in the reference protocol),
//! so this is a read-only backend. It polls; the unified [`Battery::updates`]
//! fallback handles real-time.

use crate::battery::Battery;
use crate::types::{BatteryStatus, CellInfo, Reading, Unit};
use crate::{Capabilities, DeviceInfo, Error, Result};
use async_trait::async_trait;
use sok_bms::{SokBms, SokData};

/// A SOK battery exposed through the unified [`Battery`] trait.
pub struct SokBattery {
    bms: SokBms,
    info: DeviceInfo,
}

impl SokBattery {
    fn wrap(bms: SokBms) -> Self {
        Self {
            bms,
            info: DeviceInfo {
                backend: "sok".into(),
                model: Some("SOK".into()),
                ..Default::default()
            },
        }
    }

    /// Connect over BLE, addressed by its stable `Peripheral::id()` string.
    #[cfg(feature = "sok")]
    pub async fn connect_bluetooth(id: &str) -> Result<Self> {
        let bms = SokBms::connect_ble(id)
            .await
            .map_err(|e| Error::Transport(e.to_string()))?;
        Ok(Self::wrap(bms))
    }
}

fn to_status(d: &SokData) -> BatteryStatus {
    let mut s = BatteryStatus::default();
    s.set(Reading::Soc, Some(d.soc as f64))
        .set(Reading::Voltage, Some(d.voltage as f64))
        .set(Reading::Current, Some(d.current as f64))
        .set(Reading::PowerIn, (d.power > 0.0).then_some(d.power as f64))
        .set(Reading::PowerOut, (d.power < 0.0).then(|| d.power.abs() as f64))
        .set(Reading::CapacityFullAh, Some(d.capacity as f64))
        .set(Reading::Cycles, Some(d.cycles as f64));

    s.set_labeled("temp.battery", "Battery", d.temperature as f64, Unit::Celsius);

    s.cells = d
        .cells
        .iter()
        .enumerate()
        .map(|(i, v)| CellInfo {
            index: i as u8,
            voltage: Some(*v),
            resistance: None,
            balancing: None,
        })
        .collect();
    s
}

#[async_trait]
impl Battery for SokBattery {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::READ_BASIC | Capabilities::READ_CELLS | Capabilities::READ_TEMPERATURE
    }

    async fn status(&mut self) -> Result<BatteryStatus> {
        let data = self
            .bms
            .read()
            .await
            .map_err(|e| Error::Transport(e.to_string()))?;
        Ok(to_status(data))
    }

    async fn disconnect(&mut self) -> Result<()> {
        self.bms
            .disconnect()
            .await
            .map_err(|e| Error::Transport(e.to_string()))
    }
}
