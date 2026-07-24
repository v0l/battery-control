//! Adapter for [`renogy_bms`] — Renogy smart batteries over BLE (BT-1/BT-2).
//!
//! Telemetry only (no control channel), so this is a read-only backend that
//! polls; the unified [`Battery::updates`] fallback handles real-time.

use crate::battery::Battery;
use crate::types::{BatteryStatus, CellInfo, Reading, Unit};
use crate::{Capabilities, DeviceInfo, Error, Result};
use async_trait::async_trait;
use renogy_bms::{RenogyBms, RenogyData};

/// A Renogy smart battery exposed through the unified [`Battery`] trait.
pub struct RenogyBattery {
    bms: RenogyBms,
    info: DeviceInfo,
}

impl RenogyBattery {
    fn wrap(bms: RenogyBms) -> Self {
        Self {
            bms,
            info: DeviceInfo {
                backend: "renogy".into(),
                ..Default::default()
            },
        }
    }

    /// Connect over BLE by stable `Peripheral::id()` (stand-alone unit 0xFF).
    #[cfg(feature = "renogy")]
    pub async fn connect_bluetooth(id: &str) -> Result<Self> {
        let bms = RenogyBms::connect_ble(id)
            .await
            .map_err(|e| Error::Transport(e.to_string()))?;
        Ok(Self::wrap(bms))
    }

    fn refresh_info(&mut self, d: &RenogyData) {
        if self.info.model.is_none() {
            self.info.model = Some(d.model.clone().unwrap_or_else(|| "Renogy".into()));
        }
    }
}

fn to_status(d: &RenogyData) -> BatteryStatus {
    let mut s = BatteryStatus::default();
    s.set(Reading::Soc, Some(d.soc as f64))
        .set(Reading::Voltage, Some(d.voltage as f64))
        .set(Reading::Current, Some(d.current as f64))
        .set(Reading::PowerIn, (d.power > 0.0).then_some(d.power as f64))
        .set(Reading::PowerOut, (d.power < 0.0).then(|| d.power.abs() as f64))
        .set(Reading::CapacityRemainingAh, Some(d.remaining_ah as f64))
        .set(Reading::CapacityFullAh, Some(d.capacity_ah as f64));

    for (i, t) in d.temps.iter().enumerate() {
        s.set_labeled(&format!("temp.t{}", i + 1), &format!("T{}", i + 1), *t as f64, Unit::Celsius);
    }

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
impl Battery for RenogyBattery {
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
            .map_err(|e| Error::Transport(e.to_string()))?
            .clone();
        self.refresh_info(&data);
        Ok(to_status(&data))
    }

    async fn disconnect(&mut self) -> Result<()> {
        self.bms
            .disconnect()
            .await
            .map_err(|e| Error::Transport(e.to_string()))
    }
}
