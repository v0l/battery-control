//! Adapter for [`jk_bms`] (v0.3.2+, async) — JIKONG (JK) battery management
//! systems, built on the crate's high-level [`JkBms`] handle.

use crate::battery::{require, Battery};
use crate::types::{
    BatteryStatus, CellInfo, Command, Reading, Setting, SettingKind, SettingValue, SwitchId, Unit,
};
use crate::{Capabilities, DeviceInfo, Error, Result};
use async_trait::async_trait;
use jk_bms::{error_bitmask_to_strings, JkBms, JkSettings, MybmmPack};
use std::time::Duration;

/// A JK BMS exposed through the unified [`Battery`] trait.
pub struct JkBattery {
    bms: JkBms,
    info: DeviceInfo,
    /// Cached device settings, fetched with the first status read and
    /// invalidated by setting writes. Attached to every [`BatteryStatus`].
    settings: Vec<Setting>,
}

impl JkBattery {
    fn wrap(bms: JkBms) -> Self {
        let info = DeviceInfo {
            backend: "jk".into(),
            ..Default::default()
        };
        Self {
            bms,
            info,
            settings: Vec::new(),
        }
    }

    /// Open a JK BMS over a serial port (e.g. `"/dev/ttyUSB0"`, `9600`).
    #[cfg(feature = "jk-serial")]
    pub async fn open_serial(path: &str, baud: u32) -> Result<Self> {
        let bms = JkBms::connect_serial(&format!("{path},{baud}"))
            .await
            .map_err(|e| Error::Transport(e.to_string()))?;
        Ok(Self::wrap(bms))
    }

    /// Connect to a JK BMS over BLE, addressed by its stable `Peripheral::id()`
    /// string (macOS-safe). Uses the default JK notify characteristic.
    #[cfg(feature = "jk-ble")]
    pub async fn connect_bluetooth(id: &str) -> Result<Self> {
        let bms = JkBms::connect_ble(id)
            .await
            .map_err(|e| Error::Transport(e.to_string()))?;
        Ok(Self::wrap(bms))
    }

    /// Record the model once the first read has revealed it.
    fn refresh_info(&mut self) {
        if self.info.model.is_none() && !self.bms.model().is_empty() {
            self.info.model = Some(self.bms.model().to_string());
        }
    }
}

/// Map the decoded JK settings frame to normalized [`Setting`]s.
///
/// Ids match the [`jk_bms::SETTINGS`] register names so every entry can be
/// written back with `Command::Set { id, value }` / `Command::Toggle` as-is.
fn map_settings(s: &JkSettings) -> Vec<Setting> {
    fn num(id: &str, label: &str, value: f32, unit: Option<Unit>) -> Setting {
        Setting {
            id: id.into(),
            label: Some(label.into()),
            value: SettingValue::Number(value as f64),
            kind: SettingKind::Number {
                min: None,
                max: None,
                step: None,
                unit,
            },
            writable: true,
        }
    }
    fn flag(id: &str, label: &str, value: bool) -> Setting {
        Setting {
            id: id.into(),
            label: Some(label.into()),
            value: SettingValue::Bool(value),
            kind: SettingKind::Bool,
            writable: true,
        }
    }

    use Unit as U;
    vec![
        // Voltage protections / thresholds
        num("cell_ovp", "Cell OVP", s.cell_ovp, Some(U::Volt)),
        num("cell_ovpr", "Cell OVP recovery", s.cell_ovpr, Some(U::Volt)),
        num("cell_uvp", "Cell UVP", s.cell_uvp, Some(U::Volt)),
        num("cell_uvpr", "Cell UVP recovery", s.cell_uvpr, Some(U::Volt)),
        num("balance_trigger_voltage", "Balance trigger", s.balance_trigger_voltage, Some(U::Volt)),
        num("balance_starting_voltage", "Balance start", s.balance_starting_voltage, Some(U::Volt)),
        num("cell_soc100_voltage", "Cell 100% voltage", s.cell_soc100_voltage, Some(U::Volt)),
        num("cell_soc0_voltage", "Cell 0% voltage", s.cell_soc0_voltage, Some(U::Volt)),
        num("cell_request_charge_voltage", "Request charge voltage", s.cell_request_charge_voltage, Some(U::Volt)),
        num("cell_request_float_voltage", "Request float voltage", s.cell_request_float_voltage, Some(U::Volt)),
        num("power_off_voltage", "Power-off voltage", s.power_off_voltage, Some(U::Volt)),
        num("smart_sleep_voltage", "Smart sleep voltage", s.smart_sleep_voltage, Some(U::Volt)),
        // Current limits
        num("max_charge_current", "Max charge current", s.max_charge_current, Some(U::Amp)),
        num("max_discharge_current", "Max discharge current", s.max_discharge_current, Some(U::Amp)),
        num("max_balance_current", "Max balance current", s.max_balance_current, Some(U::Amp)),
        // Protection timing
        num("charge_ocp_delay", "Charge OCP delay", s.charge_ocp_delay, Some(U::Second)),
        num("charge_ocp_recovery", "Charge OCP recovery", s.charge_ocp_recovery, Some(U::Second)),
        num("discharge_ocp_delay", "Discharge OCP delay", s.discharge_ocp_delay, Some(U::Second)),
        num("discharge_ocp_recovery", "Discharge OCP recovery", s.discharge_ocp_recovery, Some(U::Second)),
        num("scp_recovery", "Short-circuit recovery", s.scp_recovery, Some(U::Second)),
        // Temperature protections
        num("charge_otp", "Charge OTP", s.charge_otp, Some(U::Celsius)),
        num("charge_otp_recovery", "Charge OTP recovery", s.charge_otp_recovery, Some(U::Celsius)),
        num("discharge_otp", "Discharge OTP", s.discharge_otp, Some(U::Celsius)),
        num("discharge_otp_recovery", "Discharge OTP recovery", s.discharge_otp_recovery, Some(U::Celsius)),
        num("charge_utp", "Charge UTP", s.charge_utp, Some(U::Celsius)),
        num("charge_utp_recovery", "Charge UTP recovery", s.charge_utp_recovery, Some(U::Celsius)),
        num("power_tube_otp", "MOSFET OTP", s.power_tube_otp, Some(U::Celsius)),
        num("power_tube_otp_recovery", "MOSFET OTP recovery", s.power_tube_otp_recovery, Some(U::Celsius)),
        // Pack configuration
        num("cell_count", "Cell count", s.cell_count as f32, Some(U::Count)),
        num("total_battery_capacity", "Battery capacity", s.total_battery_capacity, Some(U::AmpHour)),
        // Auxiliary toggles (the main charge/discharge/balance/heat switches
        // surface as `switches`, not settings). Ids match SETTINGS names.
        flag("disable_temperature_sensors", "Disable temp sensors", s.disable_temp_sensors),
        flag("display_always_on", "Display always on", s.display_always_on),
        flag("smart_sleep", "Smart sleep", s.smart_sleep_switch),
        flag("disable_pcl_module", "Disable PCL module", s.disable_pcl_module),
        flag("timed_stored_data", "Timed stored data", s.timed_stored_data),
        flag("charging_float_mode", "Float charging mode", s.charging_float_mode),
    ]
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

    let mut s = BatteryStatus::default();
    s.set(Reading::Soc, Some(p.soc as f64))
        .set(Reading::Soh, Some(p.soh as f64))
        .set(Reading::Voltage, Some(p.voltage as f64))
        .set(Reading::Current, Some(p.current as f64))
        .set(Reading::PowerIn, (p.power > 0.0).then_some(p.power as f64))
        .set(Reading::PowerOut, (p.power < 0.0).then(|| p.power.abs() as f64))
        .set(Reading::CapacityRemainingAh, Some(p.capacity_remaining as f64))
        .set(Reading::CapacityFullAh, Some(p.total_battery_capacity as f64))
        .set(Reading::Cycles, Some(p.charging_cycles as f64));

    // Each probe becomes a temp.* sensor; MOSFET temp its own.
    let n = (p.ntemps.max(0) as usize).min(p.temps.len());
    for i in 0..n {
        s.set_labeled(&format!("temp.t{}", i + 1), &format!("T{}", i + 1), p.temps[i] as f64, Unit::Celsius);
    }
    if p.power_tube_temp != 0.0 {
        s.set_labeled("temp.mosfet", "MOSFET", p.power_tube_temp as f64, Unit::Celsius);
    }

    s.set_switch(SwitchId::Charging, Some(p.charging))
        .set_switch(SwitchId::Discharging, Some(p.discharging))
        .set_switch(SwitchId::Balancer, Some(p.balancing))
        .set_switch(SwitchId::Heater, Some(p.heating))
        .set_switch(SwitchId::Precharge, Some(p.precharging));

    if p.error_bitmask != 0 {
        s.alarms = error_bitmask_to_strings(p.error_bitmask)
            .into_iter()
            .map(|s| s.to_string())
            .collect();
    }
    s.cells = cells;
    s
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
        // First read (or after a setting write): also wait for the settings
        // frame and cache the normalized settings.
        if self.settings.is_empty() {
            match self.bms.read_settings().await {
                Ok(s) => self.settings = map_settings(&s),
                // Settings are additive; live data still flows without them.
                Err(e) => log::warn!("jk: settings read failed: {e}"),
            }
        } else {
            self.bms
                .read()
                .await
                .map_err(|e| Error::Transport(e.to_string()))?;
        }
        let mut status = to_status(self.bms.pack());
        status.settings = self.settings.clone();
        self.refresh_info();
        Ok(status)
    }

    fn has_stream(&self) -> bool {
        true
    }

    /// Real-time updates: the BMS pushes cell-info frames (~1/s over BLE)
    /// after a single request, so this decodes the push stream instead of
    /// polling.
    fn stream(&mut self) -> Option<crate::battery::StatusStream<'_>> {
        use std::collections::VecDeque;
        type State<'a> = (
            &'a mut JkBattery,
            Option<BatteryStatus>,
            VecDeque<crate::StatusUpdate>,
            bool,
        );
        let init: State = (self, None, VecDeque::new(), false);
        let stream = futures_util::stream::unfold(
            init,
            |(this, mut prev, mut queue, ended): State| async move {
                loop {
                    // Drain buffered updates from the last frame first.
                    if let Some(u) = queue.pop_front() {
                        return Some((Ok(u), (this, prev, queue, ended)));
                    }
                    if ended {
                        return None;
                    }
                    // A setting write cleared the cache: refresh it in-stream
                    // so subscribers see the device-confirmed values.
                    if this.settings.is_empty() {
                        if let Ok(s) = this.bms.read_settings().await {
                            this.settings = map_settings(&s);
                        }
                    }
                    match this.bms.next_update(Duration::from_secs(30)).await {
                        Ok(pack) => {
                            let mut status = to_status(pack);
                            status.settings = this.settings.clone();
                            this.refresh_info();
                            queue.extend(status.diff(prev.as_ref()));
                            prev = Some(status);
                            // Loop to emit the first queued update (if any).
                        }
                        // Surface the error once, then end the stream so callers
                        // can reconnect rather than spin on a dead channel.
                        Err(e) => {
                            return Some((
                                Err(Error::Transport(e.to_string())),
                                (this, prev, queue, true),
                            ));
                        }
                    }
                }
            },
        );
        Some(Box::pin(stream))
    }

    async fn disconnect(&mut self) -> Result<()> {
        self.bms
            .disconnect()
            .await
            .map_err(|e| Error::Transport(e.to_string()))
    }

    async fn execute(&mut self, cmd: Command) -> Result<()> {
        match cmd {
            Command::Toggle { id, on } => {
                let cap = match id.as_str() {
                    "charging" => Capabilities::TOGGLE_CHARGE,
                    "discharging" => Capabilities::TOGGLE_DISCHARGE,
                    "balancer" => Capabilities::TOGGLE_BALANCER,
                    // Any other id is attempted as a named JK setting switch.
                    _ => Capabilities::WRITE_SETTINGS,
                };
                require(self.capabilities(), cap)?;
                let value = if on { "on" } else { "off" };
                self.bms
                    .set(&id, value)
                    .await
                    .map_err(|e| Error::Transport(e.to_string()))?;
                // Toggles of auxiliary setting switches live in the settings
                // cache; refresh it. The three main switches come from live
                // frames, no re-read needed.
                if !matches!(id.as_str(), "charging" | "discharging" | "balancer") {
                    self.settings.clear();
                }
                Ok(())
            }
            Command::Set { id, value } => {
                require(self.capabilities(), Capabilities::WRITE_SETTINGS)?;
                self.bms
                    .set(&id, &value)
                    .await
                    .map_err(|e| Error::Transport(e.to_string()))?;
                // Re-read settings on the next status so the cache reflects
                // what the device actually accepted.
                self.settings.clear();
                Ok(())
            }
        }
    }
}
