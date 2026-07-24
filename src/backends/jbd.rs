//! Adapter for [`jbd_bms`] — JBD / Xiaoxiang / Overkill Solar / LLT battery
//! management systems, built on the crate's high-level [`JbdBms`] handle.
//!
//! JBD is poll-only: it answers a request with one frame, so this backend
//! implements [`Battery::status`] and lets the unified [`Battery::updates`]
//! poll-and-diff fallback handle real-time updates.

use crate::battery::{require, Battery};
use crate::types::{BatteryStatus, CellInfo, Command, Reading, SwitchId, Unit};
use crate::{Capabilities, DeviceInfo, Error, Result};
use async_trait::async_trait;
use jbd_bms::{JbdBms, JbdData};

/// A JBD BMS exposed through the unified [`Battery`] trait.
pub struct JbdBattery {
    bms: JbdBms,
    info: DeviceInfo,
}

impl JbdBattery {
    fn wrap(bms: JbdBms) -> Self {
        Self {
            bms,
            info: DeviceInfo {
                backend: "jbd".into(),
                ..Default::default()
            },
        }
    }

    /// Connect over BLE, addressed by its stable `Peripheral::id()` string.
    #[cfg(feature = "jbd-ble")]
    pub async fn connect_bluetooth(id: &str) -> Result<Self> {
        let bms = JbdBms::connect_ble(id)
            .await
            .map_err(|e| Error::Transport(e.to_string()))?;
        Ok(Self::wrap(bms))
    }

    /// Open a JBD BMS over a serial port (e.g. `"/dev/ttyUSB0"`, `9600`).
    #[cfg(feature = "jbd-serial")]
    pub async fn open_serial(path: &str, baud: u32) -> Result<Self> {
        let bms = JbdBms::connect_serial(&format!("{path},{baud}"))
            .await
            .map_err(|e| Error::Transport(e.to_string()))?;
        Ok(Self::wrap(bms))
    }

    fn refresh_info(&mut self) {
        let id = self.bms.identity();
        if self.info.manufacturer.is_none() {
            self.info.manufacturer = id.manufacturer.clone();
        }
        if self.info.serial.is_none() {
            self.info.serial = id.serial.clone();
        }
        if self.info.firmware.is_none() {
            self.info.firmware = id.firmware.clone();
        }
        if self.info.hardware.is_none() {
            self.info.hardware = id.hardware.clone();
        }
        if self.info.model.is_none() {
            // Prefer the advertised name, then the protocol cell-count model.
            let model = self.bms.model();
            self.info.model = id
                .name
                .clone()
                .or_else(|| (model != "JBD").then_some(model));
        }
    }
}

fn to_status(d: &JbdData) -> BatteryStatus {
    let b = &d.basic;
    let mut s = BatteryStatus::default();
    s.set(Reading::Soc, Some(b.soc as f64))
        .set(Reading::Voltage, Some(b.voltage as f64))
        .set(Reading::Current, Some(b.current as f64))
        .set(Reading::PowerIn, (b.power > 0.0).then_some(b.power as f64))
        .set(Reading::PowerOut, (b.power < 0.0).then(|| b.power.abs() as f64))
        .set(Reading::CapacityRemainingAh, Some(b.remaining_ah as f64))
        .set(Reading::CapacityFullAh, Some(b.full_ah as f64))
        .set(Reading::Cycles, Some(b.cycles as f64));

    for (i, t) in b.temps.iter().enumerate() {
        s.set_labeled(&format!("temp.t{}", i + 1), &format!("T{}", i + 1), *t as f64, Unit::Celsius);
    }

    s.set_switch(SwitchId::Charging, Some(b.charging))
        .set_switch(SwitchId::Discharging, Some(b.discharging));

    s.cells = d
        .cells
        .iter()
        .enumerate()
        .map(|(i, v)| CellInfo {
            index: i as u8,
            voltage: Some(*v),
            resistance: None,
            balancing: Some(b.balancing & (1 << i) != 0),
        })
        .collect();

    s.alarms = b.alarms();
    s
}

#[async_trait]
impl Battery for JbdBattery {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::READ_BASIC
            | Capabilities::READ_CELLS
            | Capabilities::READ_TEMPERATURE
            | Capabilities::READ_ALARMS
            | Capabilities::TOGGLE_CHARGE
            | Capabilities::TOGGLE_DISCHARGE
    }

    async fn status(&mut self) -> Result<BatteryStatus> {
        let data = self
            .bms
            .read()
            .await
            .map_err(|e| Error::Transport(e.to_string()))?;
        let status = to_status(data);
        self.refresh_info();
        Ok(status)
    }

    async fn execute(&mut self, cmd: Command) -> Result<()> {
        match cmd {
            Command::Toggle { id, on } => {
                let cap = match id.as_str() {
                    "charging" => Capabilities::TOGGLE_CHARGE,
                    "discharging" => Capabilities::TOGGLE_DISCHARGE,
                    _ => return Err(Error::Unsupported),
                };
                require(self.capabilities(), cap)?;
                self.bms
                    .set(&id, on)
                    .await
                    .map_err(|e| Error::Transport(e.to_string()))
            }
            Command::Set { .. } => Err(Error::Unsupported),
        }
    }

    async fn disconnect(&mut self) -> Result<()> {
        self.bms
            .disconnect()
            .await
            .map_err(|e| Error::Transport(e.to_string()))
    }
}
