//! Adapter for [`jk_bms`] (v0.3, async) — JIKONG (JK) battery management systems.
//!
//! As of 0.3 the `jk_bms` library ships its own transports behind features, so
//! this adapter just wires up [`jk_bms::SerialTransport`] /
//! [`jk_bms::BluetoothTransport`] and drives the crate's async read/write flow.

use crate::battery::{require, Battery};
use crate::types::{BatteryStatus, CellInfo, Command};
use crate::{Capabilities, DeviceInfo, Error, Result};
use async_trait::async_trait;
use jk_bms::{
    build_setting_write_frame, error_bitmask_to_strings, jk_read, BluetoothTransport, JkSession,
    MybmmModule, MybmmPack, SerialTransport, Transport, MYBMM_BALANCE_CONTROL,
    MYBMM_CHARGE_CONTROL, MYBMM_DISCHARGE_CONTROL,
};

/// A JK BMS exposed through the unified [`Battery`] trait.
pub struct JkBattery {
    session: JkSession,
    pack: MybmmPack,
    info: DeviceInfo,
}

impl JkBattery {
    async fn open_with(
        transport_name: &str,
        target: &str,
        transport: Box<dyn Transport>,
    ) -> Result<Self> {
        let mut pack = MybmmPack::new("jk");
        pack.transport = transport_name.into();
        pack.target = target.into();
        let module = MybmmModule::new(
            "jk",
            MYBMM_CHARGE_CONTROL | MYBMM_DISCHARGE_CONTROL | MYBMM_BALANCE_CONTROL,
        );
        let mut session = JkSession {
            pp: pack.clone(),
            tp: module,
            tp_handle: Some(transport),
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

    /// Open a JK BMS over a serial port (e.g. `"/dev/ttyUSB0"`, `9600`).
    pub async fn open_serial(path: &str, baud: u32) -> Result<Self> {
        Self::open_with("serial", path, Box::new(SerialTransport::new(path, baud))).await
    }

    /// Connect to a JK BMS over BLE, addressed by its stable `Peripheral::id()`
    /// string (macOS-safe). Uses the default JK notify characteristic.
    pub async fn connect_bluetooth(id: &str) -> Result<Self> {
        Self::open_with("bt", id, Box::new(BluetoothTransport::from_target(id))).await
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
