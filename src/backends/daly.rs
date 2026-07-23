//! Adapter for [`dalybms_lib`] — Daly BMS over serial (async client).
//!
//! Note: Daly reports current as **negative = charging, positive = discharging**,
//! the opposite of this crate's convention, so the sign is flipped on the way in.
//! Per-cell voltages are not exposed here because the upstream `CellVoltages`
//! type has no public accessor; SOC/voltage/current/MOSFET/capacity are.

use crate::battery::{require, Battery};
use crate::types::{BatteryStatus, Command, Reading, SwitchId};
use crate::{Capabilities, DeviceInfo, Error, Result};
use async_trait::async_trait;
use dalybms_lib::tokio_serial_async::DalyBMS;

/// A Daly BMS exposed through the unified [`Battery`] trait.
pub struct DalyBattery {
    bms: DalyBMS,
    info: DeviceInfo,
}

impl DalyBattery {
    /// Open a Daly BMS on a serial port (e.g. `"/dev/ttyUSB0"`).
    pub fn open_serial(path: &str) -> Result<Self> {
        let bms = DalyBMS::new(path).map_err(|e| Error::Transport(format!("{e:?}")))?;
        Ok(Self {
            bms,
            info: DeviceInfo {
                backend: "daly".into(),
                model: Some(format!("Daly BMS ({path})")),
                ..Default::default()
            },
        })
    }
}

#[async_trait]
impl Battery for DalyBattery {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::READ_BASIC
            | Capabilities::TOGGLE_CHARGE
            | Capabilities::TOGGLE_DISCHARGE
            | Capabilities::SET_CHARGE_LIMIT
    }

    async fn status(&mut self) -> Result<BatteryStatus> {
        let soc = self
            .bms
            .get_soc()
            .await
            .map_err(|e| Error::Transport(format!("{e:?}")))?;
        // Flip Daly's sign so positive = charging (our convention).
        let current = -soc.current;
        let power = soc.total_voltage * current;

        let mut status = BatteryStatus::default();
        status
            .set(Reading::Soc, Some(soc.soc_percent as f64))
            .set(Reading::Voltage, Some(soc.total_voltage as f64))
            .set(Reading::Current, Some(current as f64))
            .set(Reading::PowerIn, (power > 0.0).then_some(power as f64))
            .set(Reading::PowerOut, (power < 0.0).then(|| power.abs() as f64));

        // MOSFET status is optional enrichment; ignore failures.
        if let Ok(m) = self.bms.get_mosfet_status().await {
            status
                .set_switch(SwitchId::Charging, Some(m.charging_mosfet))
                .set_switch(SwitchId::Discharging, Some(m.discharging_mosfet))
                .set(Reading::Cycles, Some(m.bms_cycles as f64))
                .set(Reading::CapacityRemainingAh, Some(m.capacity_ah as f64));
        }

        Ok(status)
    }

    async fn execute(&mut self, cmd: Command) -> Result<()> {
        match cmd {
            Command::Toggle { id, on } => match id.as_str() {
                "charging" => {
                    require(self.capabilities(), Capabilities::TOGGLE_CHARGE)?;
                    self.bms
                        .set_charge_mosfet(on)
                        .await
                        .map_err(|e| Error::Transport(format!("{e:?}")))
                }
                "discharging" => {
                    require(self.capabilities(), Capabilities::TOGGLE_DISCHARGE)?;
                    self.bms
                        .set_discharge_mosfet(on)
                        .await
                        .map_err(|e| Error::Transport(format!("{e:?}")))
                }
                other => Err(Error::InvalidArgument(format!(
                    "'{other}' is not controllable on this device"
                ))),
            },
            Command::Set { id, value } if id == "charge_limit" || id == "soc" => {
                require(self.capabilities(), Capabilities::SET_CHARGE_LIMIT)?;
                let pct: f32 = value
                    .parse()
                    .map_err(|_| Error::InvalidArgument(format!("bad %: {value}")))?;
                if !(0.0..=100.0).contains(&pct) {
                    return Err(Error::InvalidArgument("charge limit out of 0..100".into()));
                }
                // Daly exposes SOC calibration rather than a true charge ceiling.
                self.bms
                    .set_soc(pct)
                    .await
                    .map_err(|e| Error::Transport(format!("{e:?}")))
            }
            Command::Set { .. } => Err(Error::Unsupported),
        }
    }
}
