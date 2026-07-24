//! Adapter for [`pylontech_rs485`] — Pylontech batteries over the RS485
//! **console** port (US2000/US3000 family). Read-only; polls. Complements the
//! CAN decoder ([`super::pylontech`]) with per-cell/per-module detail.

use crate::battery::Battery;
use crate::types::{BatteryStatus, CellInfo, Reading, Unit};
use crate::{Capabilities, DeviceInfo, Error, Result};
use async_trait::async_trait;
use pylontech_rs485::{PylontechData, PylontechRs485};

/// A Pylontech RS485 console chain exposed through the unified [`Battery`] trait.
pub struct PylontechConsole {
    bms: PylontechRs485,
    info: DeviceInfo,
}

impl PylontechConsole {
    fn wrap(bms: PylontechRs485) -> Self {
        Self {
            bms,
            info: DeviceInfo {
                backend: "pylontech-rs485".into(),
                manufacturer: Some("Pylontech".into()),
                ..Default::default()
            },
        }
    }

    /// Open over the RS485 console port (`path`, `baud` 115200, `address`).
    #[cfg(feature = "pylontech-rs485")]
    pub async fn open_serial(path: &str, baud: u32, address: u8) -> Result<Self> {
        let bms = PylontechRs485::open_serial(path, baud, address)
            .await
            .map_err(|e| Error::Transport(e.to_string()))?;
        Ok(Self::wrap(bms))
    }
}

fn to_status(d: &PylontechData) -> BatteryStatus {
    let mut s = BatteryStatus::default();
    let power = d.power();
    s.set(Reading::Soc, Some(d.soc() as f64))
        .set(Reading::Voltage, Some(d.voltage() as f64))
        .set(Reading::Current, Some(d.current() as f64))
        .set(Reading::PowerIn, (power > 0.0).then_some(power as f64))
        .set(Reading::PowerOut, (power < 0.0).then(|| power.abs() as f64))
        .set(Reading::CapacityRemainingAh, Some(d.remaining_ah() as f64))
        .set(Reading::CapacityFullAh, Some(d.total_ah() as f64));

    // Cells and temps across the whole chain, prefixed by module index.
    let mut cells = Vec::new();
    for (mi, m) in d.modules.iter().enumerate() {
        for (ci, t) in m.temps.iter().enumerate() {
            let label = if ci == 0 {
                format!("M{} BMS", mi + 1)
            } else {
                format!("M{} T{}", mi + 1, ci)
            };
            s.set_labeled(&format!("temp.m{}_{}", mi + 1, ci), &label, *t as f64, Unit::Celsius);
        }
        for v in &m.cells {
            cells.push(CellInfo {
                index: cells.len() as u8,
                voltage: Some(*v),
                resistance: None,
                balancing: None,
            });
        }
    }
    s.cells = cells;
    s
}

#[async_trait]
impl Battery for PylontechConsole {
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
        let status = to_status(data);
        if self.info.model.is_none() {
            self.info.model = Some(format!("Pylontech ({} modules)", data.modules.len()));
        }
        Ok(status)
    }

    async fn disconnect(&mut self) -> Result<()> {
        self.bms
            .disconnect()
            .await
            .map_err(|e| Error::Transport(e.to_string()))
    }
}
