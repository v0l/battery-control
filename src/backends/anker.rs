//! Adapter for [`anker_solix`] — Anker SOLIX portable power stations over BLE.

use crate::battery::Battery;
use crate::types::{BatteryStatus, Command, PortDirection, PortInfo, Sensor};
use crate::{Capabilities, DeviceInfo, Error, Result};
use anker_solix::{Brightness, Device, PortStatus as AnkerPort, Telemetry};
use async_trait::async_trait;
use std::time::Duration;

/// A SOLIX power station exposed through the unified [`Battery`] trait.
pub struct AnkerBattery {
    device: Device,
    info: DeviceInfo,
}

impl AnkerBattery {
    /// Discover and connect to a station by name substring or MAC.
    pub async fn connect(target: &str, scan_secs: u64) -> Result<Self> {
        let device = Device::find_and_connect(target, scan_secs)
            .await
            .map_err(|e| Error::Transport(e.to_string()))?;
        let info = DeviceInfo {
            backend: "anker".into(),
            model: Some(device.name().to_string()),
            ..Default::default()
        };
        Ok(Self { device, info })
    }

    /// Wrap an already-connected [`anker_solix::Device`].
    pub fn from_device(device: Device) -> Self {
        let info = DeviceInfo {
            backend: "anker".into(),
            model: Some(device.name().to_string()),
            ..Default::default()
        };
        Self { device, info }
    }
}

fn port(id: &str, label: &str, p: &anker_solix::Port) -> PortInfo {
    let (on, direction) = match p.status {
        AnkerPort::Output => (Some(true), Some(PortDirection::Out)),
        AnkerPort::Input => (Some(true), Some(PortDirection::In)),
        AnkerPort::Off => (Some(false), None),
        AnkerPort::Unknown => (None, None),
    };
    PortInfo {
        id: id.to_string(),
        label: Some(label.to_string()),
        direction,
        on,
        watts: p.watts.map(|w| w as f32),
    }
}

fn to_status(t: &Telemetry) -> BatteryStatus {
    let mut ports = vec![
        port("ac", "AC", &t.ac),
        port("dc", "DC (12V)", &t.dc),
        port("solar", "Solar", &t.solar),
        port("usb_c1", "USB-C 1", &t.usb_c1),
        port("usb_c2", "USB-C 2", &t.usb_c2),
        port("usb_c3", "USB-C 3", &t.usb_c3),
        port("usb_a1", "USB-A 1", &t.usb_a1),
    ];
    ports.retain(|p| p.on.is_some() || p.watts.is_some());

    BatteryStatus {
        soc: t.battery_percentage.map(|v| v as f32),
        soh: t.battery_health.map(|v| v as f32),
        temperatures: t
            .temperature_c
            .map(|v| Sensor {
                id: "battery".into(),
                label: Some("Battery".into()),
                celsius: v as f32,
            })
            .into_iter()
            .collect(),
        power_in: t.ac_power_in.map(|v| v as f32),
        power_out: t.power_out.map(|v| v as f32),
        soc_limit_max: t.max_battery_percentage.map(|v| v as f32),
        soc_limit_min: t.min_battery_percentage.map(|v| v as f32),
        time_remaining_h: t.time_remaining_hours.map(|v| v as f32),
        ports,
        ..Default::default()
    }
}

#[async_trait]
impl Battery for AnkerBattery {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::READ_BASIC
            | Capabilities::READ_PORTS
            | Capabilities::READ_TEMPERATURE
            | Capabilities::TOGGLE_PORTS
            | Capabilities::WRITE_SETTINGS
    }

    async fn status(&mut self) -> Result<BatteryStatus> {
        let t = self
            .device
            .next_telemetry(Duration::from_secs(12))
            .await
            .map_err(|e| Error::Transport(e.to_string()))?;
        // Enrich static info from the first snapshot.
        if self.info.serial.is_none() {
            self.info.serial = t.serial_number.clone();
        }
        Ok(to_status(&t))
    }

    async fn execute(&mut self, cmd: Command) -> Result<()> {
        match cmd {
            Command::Toggle { id, on } => {
                match id.as_str() {
                    "ac" => self.device.set_ac(on).await,
                    "dc" => self.device.set_dc(on).await,
                    // Light bar / display are gen-1 only; the device returns
                    // UnsupportedModel on gen-2 until the codes are known.
                    "display" => self.device.set_display(on).await,
                    "light" => {
                        self.device
                            .set_light(if on { Brightness::High } else { Brightness::Off })
                            .await
                    }
                    other => {
                        return Err(Error::InvalidArgument(format!(
                            "'{other}' is not controllable on this device"
                        )))
                    }
                }
                .map_err(|e| Error::Transport(e.to_string()))
            }
            Command::Set { id, value } if id == "light" => {
                let b = Brightness::parse(&value)
                    .ok_or_else(|| Error::InvalidArgument(format!("bad light mode: {value}")))?;
                self.device
                    .set_light(b)
                    .await
                    .map_err(|e| Error::Transport(e.to_string()))
            }
            Command::Set { .. } => Err(Error::Unsupported),
        }
    }
}
