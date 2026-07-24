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
                ..Default::default()
            },
        }
    }

    fn refresh_info(&mut self, d: &SokData) {
        // Prefer the standard BLE Device Information Service (read at connect),
        // falling back to any identity the telemetry protocol carries.
        let id = self.bms.identity();
        if self.info.manufacturer.is_none() {
            self.info.manufacturer = id.manufacturer.clone();
        }
        if self.info.model.is_none() {
            // The pack's advertised name (SOK-AA52810) or protocol model is a
            // better label than the DIS model, which is often just the BLE
            // module (e.g. BK-BLE-1.0).
            self.info.model = d
                .model
                .clone()
                .or_else(|| id.name.clone())
                .or_else(|| id.model.clone())
                .or_else(|| Some("SOK".into()));
        }
        if self.info.serial.is_none() {
            self.info.serial = id.serial.clone().or_else(|| d.serial.clone());
        }
        if self.info.firmware.is_none() {
            self.info.firmware = id.firmware.clone();
        }
        if self.info.hardware.is_none() {
            self.info.hardware = id.hardware.clone();
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
        .set(Reading::CapacityRemainingAh, d.remaining.map(|v| v as f64))
        .set(Reading::CapacityFullAh, Some(d.capacity as f64))
        .set(Reading::Cycles, d.cycles.map(|v| v as f64));

    // ABC exposes up to four probes (cell1/cell2/MOSFET/environment); EE one.
    let labels = ["T1", "T2", "MOSFET", "Environment"];
    if d.temps.len() > 1 {
        for (i, t) in d.temps.iter().enumerate() {
            let id = format!("temp.t{}", i + 1);
            s.set_labeled(&id, labels.get(i).copied().unwrap_or("T"), *t as f64, Unit::Celsius);
        }
    } else {
        s.set_labeled("temp.battery", "Battery", d.temperature as f64, Unit::Celsius);
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
