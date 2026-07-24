//! Adapter for [`jackery`] — Jackery portable power stations over **local BLE**
//! (no cloud). The encryption key is derived from the advertisement at scan
//! time and threaded through the [`Locator`](crate::discovery::Locator).

use crate::battery::{require, Battery};
use crate::types::{BatteryStatus, Command, PortDirection, PortInfo, Reading, Unit};
use crate::{Capabilities, DeviceInfo, Error, Result};
use async_trait::async_trait;
use jackery::{Jackery, JackeryData};

/// A Jackery power station exposed through the unified [`Battery`] trait.
pub struct JackeryStation {
    dev: Jackery,
    info: DeviceInfo,
}

impl JackeryStation {
    fn wrap(dev: Jackery) -> Self {
        let info = DeviceInfo {
            backend: "jackery".into(),
            manufacturer: Some("Jackery".into()),
            model: Some(dev.model()),
            serial: Some(dev.serial().to_string()),
            ..Default::default()
        };
        Self { dev, info }
    }

    /// Connect over BLE with the advertisement-derived `key`, `model`, `serial`.
    #[cfg(feature = "jackery")]
    pub async fn connect_bluetooth(
        id: &str,
        key: Vec<u8>,
        model: u16,
        serial: String,
    ) -> Result<Self> {
        let dev = Jackery::connect_ble(id, key, model, serial)
            .await
            .map_err(|e| Error::Transport(e.to_string()))?;
        Ok(Self::wrap(dev))
    }
}

fn port(id: &str, label: &str, on: bool) -> PortInfo {
    PortInfo {
        id: id.to_string(),
        label: Some(label.to_string()),
        direction: Some(PortDirection::Out),
        on: Some(on),
        watts: None,
        settable: true,
    }
}

fn to_status(d: &JackeryData) -> BatteryStatus {
    let mut s = BatteryStatus::default();
    s.set(Reading::Soc, Some(d.soc()))
        .set(Reading::PowerIn, Some(d.input_power()))
        .set(Reading::PowerOut, Some(d.output_power()));
    if d.bt != 0 {
        s.set_labeled("temp.battery", "Battery", d.temperature_c(), Unit::Celsius);
    }

    let mut ports = vec![port("ac", "AC output", d.ac_on())];
    // Split-DC models expose USB + car; others a single DC output.
    if d.odcu != 0 || d.odcc != 0 {
        ports.push(port("usb", "USB output", d.usb_on()));
        ports.push(port("car", "Car output", d.car_on()));
    } else {
        ports.push(port("dc", "DC output", d.dc_on()));
    }
    s.ports = ports;
    s.alarms = d.alarms();
    s
}

#[async_trait]
impl Battery for JackeryStation {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::READ_BASIC | Capabilities::READ_PORTS
    }

    async fn status(&mut self) -> Result<BatteryStatus> {
        let data = self
            .dev
            .read()
            .await
            .map_err(|e| Error::Transport(e.to_string()))?;
        Ok(to_status(data))
    }

    async fn execute(&mut self, cmd: Command) -> Result<()> {
        match cmd {
            Command::Toggle { id, on } => {
                require(self.capabilities(), Capabilities::READ_PORTS)?;
                match id.as_str() {
                    "ac" | "dc" | "usb" | "car" => self
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
