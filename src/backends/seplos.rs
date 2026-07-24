//! Adapter for [`seplos_bms`] — Seplos V3 rack packs (Modbus RTU over RS485).
//! Read-only; polls (the unified [`Battery::updates`] fallback handles live).

use crate::battery::Battery;
use crate::types::{BatteryStatus, CellInfo, Reading, Unit};
use crate::{Capabilities, DeviceInfo, Error, Result};
use async_trait::async_trait;
use seplos_bms::{SeplosBms, SeplosData};

/// A Seplos V3 BMS exposed through the unified [`Battery`] trait.
pub struct SeplosBattery {
    bms: SeplosBms,
    info: DeviceInfo,
}

impl SeplosBattery {
    fn wrap(bms: SeplosBms) -> Self {
        let addr = bms.address();
        Self {
            bms,
            info: DeviceInfo {
                backend: "seplos".into(),
                manufacturer: Some("Seplos".into()),
                serial: Some(format!("addr{addr}")),
                ..Default::default()
            },
        }
    }

    /// Open a Seplos V3 pack over RS485 (`path`, `baud` 19200, Modbus `address`).
    #[cfg(feature = "seplos")]
    pub async fn open_serial(path: &str, baud: u32, address: u8) -> Result<Self> {
        let bms = SeplosBms::open_serial(path, baud, address)
            .await
            .map_err(|e| Error::Transport(e.to_string()))?;
        Ok(Self::wrap(bms))
    }
}

fn to_status(d: &SeplosData) -> BatteryStatus {
    let mut s = BatteryStatus::default();
    s.set(Reading::Soc, Some(d.soc as f64))
        .set(Reading::Soh, Some(d.soh as f64))
        .set(Reading::Voltage, Some(d.voltage as f64))
        .set(Reading::Current, Some(d.current as f64))
        .set(Reading::PowerIn, (d.power > 0.0).then_some(d.power as f64))
        .set(Reading::PowerOut, (d.power < 0.0).then(|| d.power.abs() as f64))
        .set(Reading::CapacityRemainingAh, Some(d.remaining_ah as f64))
        .set(Reading::CapacityFullAh, Some(d.total_ah as f64))
        .set(Reading::Cycles, Some(d.cycles as f64));

    for (i, t) in d.cell_temps.iter().enumerate() {
        s.set_labeled(&format!("temp.t{}", i + 1), &format!("T{}", i + 1), *t as f64, Unit::Celsius);
    }
    s.set_labeled("temp.environment", "Ambient", d.ambient_temp as f64, Unit::Celsius);
    s.set_labeled("temp.power", "Power", d.power_temp as f64, Unit::Celsius);

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
impl Battery for SeplosBattery {
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
            self.info.model = Some(format!("Seplos V3 {}S", status.cells.len()));
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
