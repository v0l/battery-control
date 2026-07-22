//! Adapter for [`jk_bms`] (v0.2, async) — JIKONG (JK) battery management systems.
//!
//! The `jk_bms` *library* ships no transport (its serial/BLE/CAN transports live
//! in the `jktool` binary), so this adapter provides a small async
//! [`tokio_serial`]-based [`jk_bms::Transport`] and drives the crate's async
//! read/write flow directly.

use crate::battery::{require, Battery};
use crate::types::{BatteryStatus, CellInfo, Command};
use crate::{Capabilities, DeviceInfo, Error, Result};
use async_trait::async_trait;
use jk_bms::{
    build_setting_write_frame, error_bitmask_to_strings, jk_read, JkSession, MybmmModule,
    MybmmPack, Transport, MYBMM_BALANCE_CONTROL, MYBMM_CHARGE_CONTROL, MYBMM_DISCHARGE_CONTROL,
};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_serial::{SerialPortBuilderExt, SerialStream};

/// Async serial transport implementing `jk_bms::Transport`.
struct SerialTransport {
    path: String,
    baud: u32,
    port: Option<SerialStream>,
}

impl SerialTransport {
    fn new(path: impl Into<String>, baud: u32) -> Self {
        Self {
            path: path.into(),
            baud,
            port: None,
        }
    }
}

#[async_trait]
impl Transport for SerialTransport {
    async fn open(&mut self) -> jk_bms::Result<()> {
        let port = tokio_serial::new(&self.path, self.baud)
            .open_native_async()
            .map_err(|e| jk_bms::JkError::TransportError(e.to_string()))?;
        self.port = Some(port);
        Ok(())
    }

    async fn close(&mut self) -> jk_bms::Result<()> {
        self.port = None;
        Ok(())
    }

    async fn write(&mut self, data: &[u8]) -> jk_bms::Result<usize> {
        let port = self
            .port
            .as_mut()
            .ok_or(jk_bms::JkError::TransportNotInitialized)?;
        port.write(data)
            .await
            .map_err(|e| jk_bms::JkError::WriteFailed(e.raw_os_error().unwrap_or(-1)))
    }

    async fn read(&mut self, buf: &mut [u8]) -> jk_bms::Result<usize> {
        let port = self
            .port
            .as_mut()
            .ok_or(jk_bms::JkError::TransportNotInitialized)?;
        // Bound each read; a lull just means "no data this round".
        match tokio::time::timeout(Duration::from_millis(1000), port.read(buf)).await {
            Ok(Ok(n)) => Ok(n),
            Ok(Err(e)) => Err(jk_bms::JkError::ReadFailed(e.raw_os_error().unwrap_or(-1))),
            Err(_) => Ok(0),
        }
    }
}

/// A JK BMS exposed through the unified [`Battery`] trait.
pub struct JkBattery {
    session: JkSession,
    pack: MybmmPack,
    info: DeviceInfo,
}

impl JkBattery {
    /// Open a JK BMS over a serial port (e.g. `"/dev/ttyUSB0"`, `9600`).
    pub async fn open_serial(path: &str, baud: u32) -> Result<Self> {
        let mut pack = MybmmPack::new("jk");
        pack.transport = "serial".into();
        pack.target = path.into();
        let module = MybmmModule::new(
            "jk",
            MYBMM_CHARGE_CONTROL | MYBMM_DISCHARGE_CONTROL | MYBMM_BALANCE_CONTROL,
        );
        let mut session = JkSession {
            pp: pack.clone(),
            tp: module,
            tp_handle: Some(Box::new(SerialTransport::new(path, baud))),
        };
        session
            .open()
            .await
            .map_err(|e| Error::Transport(e.to_string()))?;

        let info = DeviceInfo {
            backend: "jk".into(),
            model: (!pack.model.is_empty()).then(|| pack.model.clone()),
            ..Default::default()
        };
        Ok(Self {
            session,
            pack,
            info,
        })
    }

    async fn write_frame(&mut self, frame: &[u8]) -> Result<()> {
        let handle = self
            .session
            .tp_handle
            .as_mut()
            .ok_or(Error::Transport("transport not open".into()))?;
        handle
            .write(frame)
            .await
            .map(|_| ())
            .map_err(|e| Error::Transport(e.to_string()))
    }

    async fn set_switch(&mut self, name: &str, on: bool, needed: Capabilities) -> Result<()> {
        require(self.capabilities(), needed)?;
        let value = if on { "on" } else { "off" };
        let frame = build_setting_write_frame(name, value, self.pack.protocol_version)
            .ok_or(Error::Unsupported)?;
        self.write_frame(&frame).await
    }
}

fn to_status(p: &MybmmPack) -> BatteryStatus {
    let cells = (0..p.cells.max(0) as usize)
        .map(|i| CellInfo {
            index: i as u8,
            voltage: p.cellvolt.get(i).copied(),
            resistance: p.cellres.get(i).copied(),
            balancing: None,
        })
        .collect();

    let temps = &p.temps[..(p.ntemps.max(0) as usize).min(p.temps.len())];
    let temperature_c = temps
        .iter()
        .cloned()
        .fold(None, |m: Option<f32>, t| Some(m.map_or(t, |m| m.max(t))));

    let alarms = if p.error_bitmask != 0 {
        error_bitmask_to_strings(p.error_bitmask)
            .into_iter()
            .map(|s| s.to_string())
            .collect()
    } else {
        Vec::new()
    };

    BatteryStatus {
        soc: Some(p.soc),
        soh: Some(p.soh),
        voltage: Some(p.voltage),
        current: Some(p.current),
        power_in: (p.power > 0.0).then_some(p.power),
        power_out: (p.power < 0.0).then(|| p.power.abs()),
        temperature_c,
        capacity_remaining_ah: Some(p.capacity_remaining),
        capacity_full_ah: Some(p.total_battery_capacity),
        cycles: Some(p.charging_cycles),
        charging: Some(p.charging),
        discharging: Some(p.discharging),
        cells,
        alarms,
        ..Default::default()
    }
}

#[async_trait]
impl Battery for JkBattery {
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
            | Capabilities::TOGGLE_BALANCER
            | Capabilities::WRITE_SETTINGS
    }

    async fn status(&mut self) -> Result<BatteryStatus> {
        jk_read(&mut self.session, &mut self.pack)
            .await
            .map_err(|e| Error::Transport(e.to_string()))?;
        Ok(to_status(&self.pack))
    }

    async fn execute(&mut self, cmd: Command) -> Result<()> {
        match cmd {
            Command::SetCharging(on) => {
                self.set_switch("charging", on, Capabilities::TOGGLE_CHARGE).await
            }
            Command::SetDischarging(on) => {
                self.set_switch("discharging", on, Capabilities::TOGGLE_DISCHARGE)
                    .await
            }
            Command::SetBalancer(on) => {
                self.set_switch("balancer", on, Capabilities::TOGGLE_BALANCER).await
            }
            Command::SetSetting { name, value } => {
                require(self.capabilities(), Capabilities::WRITE_SETTINGS)?;
                let frame = build_setting_write_frame(&name, &value, self.pack.protocol_version)
                    .ok_or(Error::Unsupported)?;
                self.write_frame(&frame).await
            }
            _ => Err(Error::Unsupported),
        }
    }
}
