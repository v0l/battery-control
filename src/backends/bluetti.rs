//! Adapter for [`bluetti`] — Bluetti power stations over **local BLE** (no
//! cloud). Covers the plaintext Modbus models (AC200M/AC300/EB3A/EP500 family).

use crate::battery::{require, Battery};
use crate::types::{BatteryStatus, Command, PortDirection, PortInfo, Reading};
use crate::{Capabilities, DeviceInfo, Error, Result};
use async_trait::async_trait;
use bluetti::{Bluetti, BluettiData};

/// A Bluetti power station exposed through the unified [`Battery`] trait.
pub struct BluettiStation {
    dev: Bluetti,
    info: DeviceInfo,
}

impl BluettiStation {
    fn wrap(dev: Bluetti) -> Self {
        Self {
            dev,
            info: DeviceInfo {
                backend: "bluetti".into(),
                manufacturer: Some("Bluetti".into()),
                ..Default::default()
            },
        }
    }

    /// Connect over BLE by stable `Peripheral::id()` string.
    #[cfg(feature = "bluetti")]
    pub async fn connect_bluetooth(id: &str) -> Result<Self> {
        let dev = Bluetti::connect_ble(id)
            .await
            .map_err(|e| Error::Transport(e.to_string()))?;
        Ok(Self::wrap(dev))
    }

    fn refresh_info(&mut self, d: &BluettiData) {
        let id = self.dev.identity();
        if self.info.model.is_none() {
            self.info.model = d.device_type.clone().or_else(|| id.name.clone());
        }
        if self.info.serial.is_none() {
            self.info.serial = id.serial.clone();
        }
        if self.info.firmware.is_none() {
            self.info.firmware = id.firmware.clone();
        }
    }
}

fn port(id: &str, label: &str, watts: u16, on: Option<bool>, settable: bool) -> PortInfo {
    PortInfo {
        id: id.to_string(),
        label: Some(label.to_string()),
        direction: Some(if id.contains("input") {
            PortDirection::In
        } else {
            PortDirection::Out
        }),
        on,
        watts: Some(watts as f32),
        settable,
    }
}

fn to_status(d: &BluettiData) -> BatteryStatus {
    let mut s = BatteryStatus::default();
    s.set(Reading::Soc, Some(d.total_battery_percent as f64))
        .set(Reading::Voltage, (d.total_battery_voltage > 0.0).then_some(d.total_battery_voltage as f64))
        .set(Reading::PowerIn, Some(d.input_power() as f64))
        .set(Reading::PowerOut, Some(d.output_power() as f64));

    let mut ports = vec![
        port("ac", "AC output", d.ac_output_power, Some(d.ac_output_on), true),
        port("dc", "DC output", d.dc_output_power, Some(d.dc_output_on), true),
    ];
    if d.ac_input_power > 0 {
        ports.push(port("ac_input", "AC input", d.ac_input_power, None, false));
    }
    if d.dc_input_power > 0 {
        ports.push(port("solar", "Solar/DC input", d.dc_input_power, None, false));
    }
    s.ports = ports;

    s.cells = d
        .cells
        .iter()
        .enumerate()
        .map(|(i, v)| crate::types::CellInfo {
            index: i as u8,
            voltage: Some(*v),
            resistance: None,
            balancing: None,
        })
        .collect();
    s
}

#[async_trait]
impl Battery for BluettiStation {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::READ_BASIC | Capabilities::READ_PORTS | Capabilities::READ_CELLS
    }

    async fn status(&mut self) -> Result<BatteryStatus> {
        let data = self
            .dev
            .read()
            .await
            .map_err(|e| Error::Transport(e.to_string()))?
            .clone();
        self.refresh_info(&data);
        Ok(to_status(&data))
    }

    async fn execute(&mut self, cmd: Command) -> Result<()> {
        match cmd {
            Command::Toggle { id, on } => {
                require(self.capabilities(), Capabilities::READ_PORTS)?;
                match id.as_str() {
                    "ac" | "dc" => self
                        .dev
                        .set(&id, on)
                        .await
                        .map_err(|e| Error::Transport(e.to_string())),
                    other => Err(Error::InvalidArgument(format!(
                        "'{other}' is not controllable on this device"
                    ))),
                }
            }
            Command::Set { .. } => Err(Error::Unsupported),
        }
    }

    async fn disconnect(&mut self) -> Result<()> {
        self.dev
            .disconnect()
            .await
            .map_err(|e| Error::Transport(e.to_string()))
    }
}
