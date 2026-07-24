//! Adapter for [`pace_bms`] — PACE-BMS packs (`PACE_MODBUS`) over RS485.
//! Read-only; polls (the unified [`Battery::updates`] fallback handles live).

use crate::battery::Battery;
use crate::types::{BatteryStatus, CellInfo, Reading, SwitchId, Unit};
use crate::{Capabilities, DeviceInfo, Error, Result};
use async_trait::async_trait;
use pace_bms::{PaceBms, PaceData};

/// A PACE BMS exposed through the unified [`Battery`] trait.
pub struct PaceBattery {
    bms: PaceBms,
    info: DeviceInfo,
}

impl PaceBattery {
    fn wrap(bms: PaceBms) -> Self {
        let addr = bms.address();
        Self {
            bms,
            info: DeviceInfo {
                backend: "pace".into(),
                serial: Some(format!("addr{addr}")),
                ..Default::default()
            },
        }
    }

    /// Open a PACE pack over RS485 (`path`, `baud`, bus `address`).
    #[cfg(feature = "pace")]
    pub async fn open_serial(path: &str, baud: u32, address: u8) -> Result<Self> {
        let bms = PaceBms::open_serial(path, baud, address)
            .await
            .map_err(|e| Error::Transport(e.to_string()))?;
        Ok(Self::wrap(bms))
    }
}

fn to_status(d: &PaceData) -> BatteryStatus {
    let mut s = BatteryStatus::default();
    s.set(Reading::Soc, Some(d.soc as f64))
        .set(Reading::Soh, Some(d.soh as f64))
        .set(Reading::Voltage, Some(d.voltage as f64))
        .set(Reading::Current, Some(d.current as f64))
        .set(Reading::PowerIn, (d.power > 0.0).then_some(d.power as f64))
        .set(Reading::PowerOut, (d.power < 0.0).then(|| d.power.abs() as f64))
        .set(Reading::CapacityRemainingAh, Some(d.remaining_ah as f64))
        .set(Reading::CapacityFullAh, Some(d.full_ah as f64))
        .set(Reading::Cycles, Some(d.cycles as f64));

    for (i, t) in d.temps.iter().enumerate() {
        s.set_labeled(&format!("temp.t{}", i + 1), &format!("T{}", i + 1), *t as f64, Unit::Celsius);
    }
    if let Some(t) = d.mosfet_temp {
        s.set_labeled("temp.mosfet", "MOSFET", t as f64, Unit::Celsius);
    }
    if let Some(t) = d.environment_temp {
        s.set_labeled("temp.environment", "Environment", t as f64, Unit::Celsius);
    }

    s.set_switch(SwitchId::Charging, Some(d.charging))
        .set_switch(SwitchId::Discharging, Some(d.discharging))
        .set_switch(SwitchId::Balancer, Some(d.balancing));

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

    s.alarms = d.alarms();
    s
}

#[async_trait]
impl Battery for PaceBattery {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::READ_BASIC
            | Capabilities::READ_CELLS
            | Capabilities::READ_TEMPERATURE
            | Capabilities::READ_ALARMS
    }

    async fn status(&mut self) -> Result<BatteryStatus> {
        let data = self
            .bms
            .read()
            .await
            .map_err(|e| Error::Transport(e.to_string()))?;
        let status = to_status(data);
        if self.info.model.is_none() {
            self.info.model = Some(format!("PACE {}S", status.cells.len()));
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
