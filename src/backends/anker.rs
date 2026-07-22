//! Adapter for [`anker_solix`] — Anker SOLIX portable power stations over BLE.

use crate::battery::{require, Battery};
use crate::types::{BatteryStatus, Command, PortInfo, PortKind};
use crate::{Capabilities, DeviceInfo, Error, Result};
use anker_solix::{Device, PortStatus as AnkerPort, Telemetry};
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

fn port(kind: PortKind, p: &anker_solix::Port) -> PortInfo {
    PortInfo {
        kind,
        on: match p.status {
            AnkerPort::Output | AnkerPort::Input => Some(true),
            AnkerPort::Off => Some(false),
            AnkerPort::Unknown => None,
        },
        watts: p.watts.map(|w| w as f32),
    }
}

fn to_status(t: &Telemetry) -> BatteryStatus {
    let mut ports = vec![
        port(PortKind::Ac, &t.ac),
        port(PortKind::Dc, &t.dc),
        port(PortKind::Solar, &t.solar),
        port(PortKind::UsbC, &t.usb_c1),
        port(PortKind::UsbC, &t.usb_c2),
        port(PortKind::UsbA, &t.usb_a1),
    ];
    ports.retain(|p| p.on.is_some() || p.watts.is_some());

    BatteryStatus {
        soc: t.battery_percentage.map(|v| v as f32),
        soh: t.battery_health.map(|v| v as f32),
        temperature_c: t.temperature_c.map(|v| v as f32),
        power_in: t.ac_power_in.map(|v| v as f32),
        power_out: t.power_out.map(|v| v as f32),
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
            Command::SetPort { kind, on } => {
                require(self.capabilities(), Capabilities::TOGGLE_PORTS)?;
                match kind {
                    PortKind::Ac => self.device.set_ac(on).await,
                    PortKind::Dc => self.device.set_dc(on).await,
                    _ => return Err(Error::Unsupported),
                }
                .map_err(|e| Error::Transport(e.to_string()))
            }
            _ => Err(Error::Unsupported),
        }
    }
}
