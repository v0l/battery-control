//! Normalized data model shared by every backend.
//!
//! Battery devices span three loose classes — cell-level BMS, power stations,
//! and battery monitors — so the model is deliberately **free-form and
//! id-addressed**: a snapshot is a handful of keyed collections rather than a
//! wide struct of hardcoded fields. A backend fills in whatever it can.
//!
//! * [`Sensor`]  — a read-only scalar reading (SOC, voltage, a temp probe, …).
//! * [`Switch`]  — a writable boolean (MOSFETs, port-like toggles).
//! * [`PortInfo`] — a power port (on/off + watts).
//! * [`CellInfo`] — per-cell detail.
//! * [`Setting`] — a readable/writable configuration value (BMS thresholds, …).
//! * `alarms`    — active warning strings.
//!
//! Everything is targeted by a stable string id and controlled through
//! [`Command`]; unsupported ids/commands return [`crate::Error::Unsupported`].

use core::fmt;
use core::str::FromStr;
use serde::Serialize;

/// Namespace prefix for temperature probe ids, e.g. `"temp.mosfet"`. Probes are
/// device-specific so they stay free-form rather than being part of [`Reading`].
pub const TEMP_PREFIX: &str = "temp.";

/// Static identity of a device (does not change between reads).
#[derive(Debug, Clone, Default, Serialize)]
pub struct DeviceInfo {
    /// Backend/protocol name, e.g. `"anker"`, `"jk"`, `"pylontech-can"`.
    pub backend: String,
    /// Manufacturer / brand, e.g. `"SOK"`, `"Renogy"` (BLE DIS `0x2A29`).
    pub manufacturer: Option<String>,
    /// Human model/product string if known, e.g. `"SOLIX C1000 Gen 2"`
    /// (BLE DIS model number `0x2A24`).
    pub model: Option<String>,
    /// Serial number (BLE DIS `0x2A25`).
    pub serial: Option<String>,
    /// Firmware revision (BLE DIS `0x2A26`).
    pub firmware: Option<String>,
    /// Hardware revision (BLE DIS `0x2A27`).
    pub hardware: Option<String>,
}

/// The **standard**, cross-backend reading ids as a typed enum, so code passes
/// a `Reading` instead of a bare string (and gets the [`unit`](Reading::unit)
/// and [`label`](Reading::label) for free). Device-specific readings still use
/// free-form [`Sensor::id`] strings; this covers the super-majority.
///
/// `to_string()` / `parse()` round-trip through the canonical id
/// (`Reading::Soc <-> "soc"`). Temperature probes are device-specific and use
/// the [`TEMP_PREFIX`] namespace instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reading {
    Soc,
    Soh,
    Voltage,
    Current,
    PowerIn,
    PowerOut,
    TimeRemainingH,
    CapacityRemainingAh,
    CapacityFullAh,
    Cycles,
    ChargeCurrentLimitA,
    DischargeCurrentLimitA,
    SocLimitMax,
    SocLimitMin,
}

impl Reading {
    /// Every standard reading.
    pub const ALL: [Reading; 14] = [
        Reading::Soc,
        Reading::Soh,
        Reading::Voltage,
        Reading::Current,
        Reading::PowerIn,
        Reading::PowerOut,
        Reading::TimeRemainingH,
        Reading::CapacityRemainingAh,
        Reading::CapacityFullAh,
        Reading::Cycles,
        Reading::ChargeCurrentLimitA,
        Reading::DischargeCurrentLimitA,
        Reading::SocLimitMax,
        Reading::SocLimitMin,
    ];

    /// Canonical id string, e.g. `"soc"`.
    pub const fn id(self) -> &'static str {
        match self {
            Reading::Soc => "soc",
            Reading::Soh => "soh",
            Reading::Voltage => "voltage",
            Reading::Current => "current",
            Reading::PowerIn => "power_in",
            Reading::PowerOut => "power_out",
            Reading::TimeRemainingH => "time_remaining_h",
            Reading::CapacityRemainingAh => "capacity_remaining_ah",
            Reading::CapacityFullAh => "capacity_full_ah",
            Reading::Cycles => "cycles",
            Reading::ChargeCurrentLimitA => "charge_current_limit_a",
            Reading::DischargeCurrentLimitA => "discharge_current_limit_a",
            Reading::SocLimitMax => "soc_limit_max",
            Reading::SocLimitMin => "soc_limit_min",
        }
    }

    /// The unit this reading is conventionally expressed in.
    pub const fn unit(self) -> Unit {
        match self {
            Reading::Soc | Reading::Soh | Reading::SocLimitMax | Reading::SocLimitMin => {
                Unit::Percent
            }
            Reading::Voltage => Unit::Volt,
            Reading::Current
            | Reading::ChargeCurrentLimitA
            | Reading::DischargeCurrentLimitA => Unit::Amp,
            Reading::PowerIn | Reading::PowerOut => Unit::Watt,
            Reading::TimeRemainingH => Unit::Hour,
            Reading::CapacityRemainingAh | Reading::CapacityFullAh => Unit::AmpHour,
            Reading::Cycles => Unit::Count,
        }
    }

    /// A human display label, e.g. `"SOC"`, `"Power in"`.
    pub const fn label(self) -> &'static str {
        match self {
            Reading::Soc => "SOC",
            Reading::Soh => "SOH",
            Reading::Voltage => "Voltage",
            Reading::Current => "Current",
            Reading::PowerIn => "Power in",
            Reading::PowerOut => "Power out",
            Reading::TimeRemainingH => "Time remaining",
            Reading::CapacityRemainingAh => "Capacity remaining",
            Reading::CapacityFullAh => "Capacity full",
            Reading::Cycles => "Cycles",
            Reading::ChargeCurrentLimitA => "Charge current limit",
            Reading::DischargeCurrentLimitA => "Discharge current limit",
            Reading::SocLimitMax => "Charge limit",
            Reading::SocLimitMin => "Discharge floor",
        }
    }
}

impl fmt::Display for Reading {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

impl FromStr for Reading {
    type Err = UnknownId;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Reading::ALL
            .into_iter()
            .find(|r| r.id() == s)
            .ok_or(UnknownId)
    }
}

/// The standard writable-boolean ([`Switch`]) ids as a typed enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SwitchId {
    Charging,
    Discharging,
    Balancer,
    Heater,
    Precharge,
}

impl SwitchId {
    pub const ALL: [SwitchId; 5] = [
        SwitchId::Charging,
        SwitchId::Discharging,
        SwitchId::Balancer,
        SwitchId::Heater,
        SwitchId::Precharge,
    ];

    /// Canonical id string, e.g. `"charging"`.
    pub const fn id(self) -> &'static str {
        match self {
            SwitchId::Charging => "charging",
            SwitchId::Discharging => "discharging",
            SwitchId::Balancer => "balancer",
            SwitchId::Heater => "heater",
            SwitchId::Precharge => "precharge",
        }
    }

    /// A human display label, e.g. `"Charging"`.
    pub const fn label(self) -> &'static str {
        match self {
            SwitchId::Charging => "Charging",
            SwitchId::Discharging => "Discharging",
            SwitchId::Balancer => "Balancer",
            SwitchId::Heater => "Heater",
            SwitchId::Precharge => "Precharge",
        }
    }
}

impl fmt::Display for SwitchId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

impl FromStr for SwitchId {
    type Err = UnknownId;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        SwitchId::ALL
            .into_iter()
            .find(|w| w.id() == s)
            .ok_or(UnknownId)
    }
}

/// Error returned when parsing an unknown standard id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnknownId;

impl fmt::Display for UnknownId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("unknown standard id")
    }
}

impl std::error::Error for UnknownId {}

/// The physical unit of a [`Sensor`] reading (also used by numeric settings).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    /// Percent, `%` (SOC, SOH, charge limits).
    Percent,
    /// Volts, `V`.
    Volt,
    /// Amps, `A` (+charge / -discharge).
    Amp,
    /// Watts, `W`.
    Watt,
    /// Degrees Celsius, `°C`.
    Celsius,
    /// Amp-hours, `Ah` (capacity).
    AmpHour,
    /// Hours, `h` (time remaining).
    Hour,
    /// Seconds, `s` (protection delays/recovery times).
    Second,
    /// Dimensionless count (cycles).
    Count,
}

impl Unit {
    /// A short display symbol, e.g. `"V"`, `"°C"`.
    pub fn symbol(self) -> &'static str {
        match self {
            Unit::Percent => "%",
            Unit::Volt => "V",
            Unit::Amp => "A",
            Unit::Watt => "W",
            Unit::Celsius => "°C",
            Unit::AmpHour => "Ah",
            Unit::Hour => "h",
            Unit::Second => "s",
            Unit::Count => "",
        }
    }
}

/// A single **read-only** scalar reading, keyed by id. Covers everything from
/// SOC and pack voltage to individual temperature probes — a device exposes as
/// many as it reports. Use the typed accessors on [`BatteryStatus`] for the
/// common ones, or [`BatteryStatus::reading`] for any id.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Sensor {
    /// Stable id, e.g. `"soc"`, `"voltage"`, `"current"`, `"power_in"`,
    /// `"temp.mosfet"`, `"cycles"`.
    pub id: String,
    /// Optional human name, e.g. `"MOSFET"`.
    pub label: Option<String>,
    pub value: f64,
    pub unit: Unit,
}

/// Per-cell measurement (BMS-class devices only).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct CellInfo {
    pub index: u8,
    pub voltage: Option<f32>,
    /// Internal resistance in ohms, if reported.
    pub resistance: Option<f32>,
    pub balancing: Option<bool>,
}

/// Direction of power flow through a port.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PortDirection {
    /// Power flows into the battery (e.g. solar / AC charging input).
    In,
    /// Power flows out of the battery (e.g. AC/DC/USB output).
    Out,
    /// Port can source or sink (e.g. USB-C PD).
    Bidir,
}

/// A controllable/observable power port. Free-form: devices expose wildly
/// different connectors (AC, 12 V, USB-C, car socket, Anderson, XT60, …). A
/// port is targeted by its [`id`](PortInfo::id) via [`Command::Toggle`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PortInfo {
    /// Stable, unique id, e.g. `"ac"`, `"usb_c1"`.
    pub id: String,
    /// Optional human-friendly name, e.g. `"USB-C 1"`.
    pub label: Option<String>,
    /// Direction of flow, if known.
    pub direction: Option<PortDirection>,
    pub on: Option<bool>,
    pub watts: Option<f32>,
    /// Whether this specific port accepts on/off control (e.g. Anker AC/DC are
    /// settable, but solar/USB monitor-only ports are not). Port controllability
    /// is entirely per-port — there is no device-wide "toggle ports" capability.
    pub settable: bool,
}

/// A named **writable boolean** on a device: charge/discharge MOSFETs
/// (ids `"charging"`/`"discharging"`), heater, precharge, balancer, … Toggled
/// via [`Command::Toggle`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Switch {
    /// Stable id, e.g. `"charging"`, `"heater"`, `"balancer"`.
    pub id: String,
    pub label: Option<String>,
    pub on: bool,
}

/// The current value of a [`Setting`].
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "type", content = "value")]
pub enum SettingValue {
    Bool(bool),
    Number(f64),
    Text(String),
}

/// The type and constraints of a [`Setting`], for UI rendering and validation.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum SettingKind {
    /// A boolean toggle (write with [`Command::Toggle`]).
    Bool,
    /// A numeric value with optional bounds/step/unit (write with
    /// [`Command::Set`], value formatted as a number).
    Number {
        min: Option<f64>,
        max: Option<f64>,
        step: Option<f64>,
        unit: Option<Unit>,
    },
    /// One of a fixed set of string options (write with [`Command::Set`]).
    Enum { options: Vec<String> },
    /// Free-form text (write with [`Command::Set`]).
    Text,
}

/// A **readable/writable** device configuration value — BMS thresholds (cell
/// OVP/UVP, balance start voltage), charge limits, sleep timers, display
/// brightness, … Free-form and id-addressed like the rest of the model; write
/// by sending the matching [`Command`] for its id ([`Command::Toggle`] for a
/// [`SettingValue::Bool`], [`Command::Set`] otherwise).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Setting {
    /// Stable id, e.g. `"charge_limit"`, `"cell_ovp"`, `"sleep_voltage"`.
    pub id: String,
    pub label: Option<String>,
    /// Current value.
    pub value: SettingValue,
    /// Type/constraints for the UI and validation.
    pub kind: SettingKind,
    /// `false` if the device reports the value but rejects writes.
    pub writable: bool,
}

/// A normalized snapshot of a battery's state: a set of free-form, id-addressed
/// collections. Empty collections mean the device doesn't report that class.
#[derive(Debug, Clone, Default, Serialize)]
pub struct BatteryStatus {
    /// Read-only scalar readings, keyed by id (SOC, V, I, power, temps, …).
    pub sensors: Vec<Sensor>,
    /// Writable booleans (charge/discharge MOSFETs, heater, balancer, …).
    pub switches: Vec<Switch>,
    /// Power ports (empty for non-station devices).
    pub ports: Vec<PortInfo>,
    /// Per-cell detail (empty for non-BMS devices).
    pub cells: Vec<CellInfo>,
    /// Readable/writable configuration values (BMS settings, limits, …).
    pub settings: Vec<Setting>,
    /// Active alarm/warning strings, if any.
    pub alarms: Vec<String>,
}

impl BatteryStatus {
    // --- builder helpers (keep backends terse) ------------------------------

    /// Push a **standard** reading (skips `None`). Id, unit and label come from
    /// the [`Reading`] — no loose strings or repeated units at call sites.
    pub fn set(&mut self, reading: Reading, value: Option<f64>) -> &mut Self {
        if let Some(value) = value {
            self.sensors.push(Sensor {
                id: reading.id().into(),
                label: Some(reading.label().into()),
                value,
                unit: reading.unit(),
            });
        }
        self
    }

    /// Push a device-specific read-only reading with an explicit id/unit
    /// (skips `None`). Use [`set`](Self::set) for the standard [`Reading`]s.
    pub fn set_custom(&mut self, id: &str, value: Option<f64>, unit: Unit) -> &mut Self {
        if let Some(value) = value {
            self.sensors.push(Sensor { id: id.into(), label: None, value, unit });
        }
        self
    }

    /// Push a labeled read-only reading (e.g. a named temperature probe).
    pub fn set_labeled(&mut self, id: &str, label: &str, value: f64, unit: Unit) -> &mut Self {
        self.sensors.push(Sensor {
            id: id.into(),
            label: Some(label.into()),
            value,
            unit,
        });
        self
    }

    /// Push a **standard** writable boolean switch (skips `None`). Id and label
    /// come from the [`SwitchId`].
    pub fn set_switch(&mut self, switch: SwitchId, on: Option<bool>) -> &mut Self {
        if let Some(on) = on {
            self.switches.push(Switch {
                id: switch.id().into(),
                label: Some(switch.label().into()),
                on,
            });
        }
        self
    }

    /// Push a device-specific switch with an explicit id/label (skips `None`).
    pub fn set_custom_switch(
        &mut self,
        id: &str,
        label: Option<&str>,
        on: Option<bool>,
    ) -> &mut Self {
        if let Some(on) = on {
            self.switches.push(Switch {
                id: id.into(),
                label: label.map(Into::into),
                on,
            });
        }
        self
    }

    /// Push a configuration setting.
    pub fn add_setting(&mut self, setting: Setting) -> &mut Self {
        self.settings.push(setting);
        self
    }

    // --- accessors ----------------------------------------------------------

    /// A sensor by id.
    pub fn sensor(&self, id: &str) -> Option<&Sensor> {
        self.sensors.iter().find(|s| s.id == id)
    }

    /// A reading's numeric value by id.
    pub fn reading(&self, id: &str) -> Option<f64> {
        self.sensor(id).map(|s| s.value)
    }

    /// A standard reading's value.
    pub fn get(&self, reading: Reading) -> Option<f64> {
        self.reading(reading.id())
    }
    /// State of charge, %.
    pub fn soc(&self) -> Option<f64> {
        self.get(Reading::Soc)
    }
    /// State of health, %.
    pub fn soh(&self) -> Option<f64> {
        self.get(Reading::Soh)
    }
    /// Pack voltage, V.
    pub fn voltage(&self) -> Option<f64> {
        self.get(Reading::Voltage)
    }
    /// Pack current, A (+charge / -discharge).
    pub fn current(&self) -> Option<f64> {
        self.get(Reading::Current)
    }
    /// Power into the battery, W.
    pub fn power_in(&self) -> Option<f64> {
        self.get(Reading::PowerIn)
    }
    /// Power out of the battery, W.
    pub fn power_out(&self) -> Option<f64> {
        self.get(Reading::PowerOut)
    }

    /// The highest temperature among all `°C` sensors, if any.
    pub fn temperature_c(&self) -> Option<f64> {
        self.sensors
            .iter()
            .filter(|s| s.unit == Unit::Celsius)
            .map(|s| s.value)
            .fold(None, |m, c| Some(m.map_or(c, |m: f64| m.max(c))))
    }

    /// A specific temperature sensor's value by id.
    pub fn temperature(&self, id: &str) -> Option<f64> {
        self.sensor(id).filter(|s| s.unit == Unit::Celsius).map(|s| s.value)
    }

    /// A switch state by id (e.g. `"charging"`, `"discharging"`, `"heater"`).
    pub fn switch(&self, id: &str) -> Option<bool> {
        self.switches.iter().find(|s| s.id == id).map(|s| s.on)
    }

    /// A setting by id.
    pub fn setting(&self, id: &str) -> Option<&Setting> {
        self.settings.iter().find(|s| s.id == id)
    }

    /// Highest cell voltage, if any cells are present.
    pub fn cell_max(&self) -> Option<f32> {
        self.cells
            .iter()
            .filter_map(|c| c.voltage)
            .fold(None, |m, v| Some(m.map_or(v, |m: f32| m.max(v))))
    }

    /// Lowest cell voltage, if any cells are present.
    pub fn cell_min(&self) -> Option<f32> {
        self.cells
            .iter()
            .filter_map(|c| c.voltage)
            .fold(None, |m, v| Some(m.map_or(v, |m: f32| m.min(v))))
    }

    /// Cell voltage spread (max - min), if cells are present.
    pub fn cell_delta(&self) -> Option<f32> {
        Some(self.cell_max()? - self.cell_min()?)
    }

    /// Compute the [`StatusUpdate`]s that turn `prev` into `self`. With
    /// `prev = None` (first snapshot) every populated element is emitted.
    pub fn diff(&self, prev: Option<&BatteryStatus>) -> Vec<StatusUpdate> {
        use StatusUpdate as U;
        let mut out = Vec::new();

        for s in &self.sensors {
            let unchanged = prev
                .and_then(|p| p.sensors.iter().find(|x| x.id == s.id))
                .is_some_and(|old| old == s);
            if !unchanged {
                out.push(U::Sensor(s.clone()));
            }
        }
        for w in &self.switches {
            let unchanged = prev
                .and_then(|p| p.switches.iter().find(|x| x.id == w.id))
                .is_some_and(|old| old == w);
            if !unchanged {
                out.push(U::Switch(w.clone()));
            }
        }
        for p in &self.ports {
            let unchanged = prev
                .and_then(|pp| pp.ports.iter().find(|x| x.id == p.id))
                .is_some_and(|old| old == p);
            if !unchanged {
                out.push(U::Port(p.clone()));
            }
        }
        for c in &self.cells {
            let unchanged = prev
                .and_then(|p| p.cells.iter().find(|x| x.index == c.index))
                .is_some_and(|old| old == c);
            if !unchanged {
                out.push(U::Cell(*c));
            }
        }
        for st in &self.settings {
            let unchanged = prev
                .and_then(|p| p.settings.iter().find(|x| x.id == st.id))
                .is_some_and(|old| old == st);
            if !unchanged {
                out.push(U::Setting(st.clone()));
            }
        }

        let alarms_changed = match prev {
            Some(p) => p.alarms != self.alarms,
            None => !self.alarms.is_empty(),
        };
        if alarms_changed {
            out.push(U::Alarms(self.alarms.clone()));
        }

        out
    }

    /// Fold one incremental [`StatusUpdate`] into this snapshot — the inverse
    /// of [`diff`](Self::diff). Apply a stream of updates to a starting
    /// snapshot (or `BatteryStatus::default()`) to maintain full live state.
    pub fn apply(&mut self, u: &StatusUpdate) {
        fn upsert<T: Clone>(v: &mut Vec<T>, item: &T, same: impl Fn(&T) -> bool) {
            match v.iter_mut().find(|x| same(x)) {
                Some(slot) => *slot = item.clone(),
                None => v.push(item.clone()),
            }
        }
        use StatusUpdate as U;
        match u {
            U::Sensor(x) => upsert(&mut self.sensors, x, |e| e.id == x.id),
            U::Switch(x) => upsert(&mut self.switches, x, |e| e.id == x.id),
            U::Port(x) => upsert(&mut self.ports, x, |e| e.id == x.id),
            U::Cell(x) => upsert(&mut self.cells, x, |e| e.index == x.index),
            U::Setting(x) => upsert(&mut self.settings, x, |e| e.id == x.id),
            U::Alarms(a) => self.alarms = a.clone(),
        }
    }
}

/// A single real-time change to a device's state, emitted by
/// [`Battery::stream`](crate::Battery::stream). Backends that push full frames
/// (e.g. Anker BLE) diff consecutive snapshots via [`BatteryStatus::diff`] and
/// emit only what changed — one update per changed element, keyed by its id.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum StatusUpdate {
    /// A read-only reading changed (keyed by [`Sensor::id`]).
    Sensor(Sensor),
    /// A switch changed (keyed by [`Switch::id`]).
    Switch(Switch),
    /// A port changed (keyed by [`PortInfo::id`]).
    Port(PortInfo),
    /// A cell changed (keyed by [`CellInfo::index`]).
    Cell(CellInfo),
    /// A setting changed (keyed by [`Setting::id`]).
    Setting(Setting),
    /// The active alarm set changed; carries the full new list.
    Alarms(Vec<String>),
}

/// A control command, addressed by a free-form **id** rather than a fixed
/// taxonomy — the same philosophy as sensors/ports/switches/settings. Backends
/// advertise coarse abilities via [`crate::Capabilities`] and validate the
/// specific id; anything unsupported returns [`crate::Error::Unsupported`].
#[derive(Debug, Clone)]
pub enum Command {
    /// Turn a port or switch on/off by id, e.g. `"ac"`, `"usb_c1"`,
    /// `"charging"`, `"discharging"`, `"balancer"`, `"heater"`, `"display"`.
    Toggle { id: String, on: bool },
    /// Set a named non-boolean value, e.g. `"charge_limit"` = `"80"`,
    /// `"light"` = `"high"`, `"display_timeout"` = `"30"`.
    Set { id: String, value: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_stats() {
        let s = BatteryStatus {
            cells: vec![
                CellInfo { index: 0, voltage: Some(3.30), ..Default::default() },
                CellInfo { index: 1, voltage: Some(3.35), ..Default::default() },
                CellInfo { index: 2, voltage: Some(3.28), ..Default::default() },
            ],
            ..Default::default()
        };
        assert_eq!(s.cell_min(), Some(3.28));
        assert_eq!(s.cell_max(), Some(3.35));
        assert!((s.cell_delta().unwrap() - 0.07).abs() < 1e-5);
    }

    #[test]
    fn empty_cells_no_stats() {
        let s = BatteryStatus::default();
        assert_eq!(s.cell_min(), None);
        assert_eq!(s.cell_delta(), None);
    }

    #[test]
    fn readings_and_accessors() {
        let mut s = BatteryStatus::default();
        s.set(Reading::Soc, Some(87.0))
            .set(Reading::Voltage, Some(13.2))
            .set_labeled("temp.mosfet", "MOSFET", 41.0, Unit::Celsius);
        assert_eq!(s.soc(), Some(87.0));
        assert_eq!(s.voltage(), Some(13.2));
        assert_eq!(s.reading("current"), None);
        assert_eq!(s.temperature_c(), Some(41.0));
        assert_eq!(s.temperature("temp.mosfet"), Some(41.0));
    }

    #[test]
    fn reading_id_roundtrip() {
        assert_eq!(Reading::PowerIn.to_string(), "power_in");
        assert_eq!("soc".parse::<Reading>(), Ok(Reading::Soc));
        assert!("nope".parse::<Reading>().is_err());
        assert_eq!(Reading::Voltage.unit(), Unit::Volt);
    }

    #[test]
    fn diff_emits_only_changes() {
        let mut a = BatteryStatus::default();
        a.set(Reading::Soc, Some(50.0)).set(Reading::Voltage, Some(13.0));
        let mut b = BatteryStatus::default();
        b.set(Reading::Soc, Some(51.0)).set(Reading::Voltage, Some(13.0));
        let updates = b.diff(Some(&a));
        assert_eq!(updates.len(), 1);
        assert!(matches!(&updates[0], StatusUpdate::Sensor(s) if s.id == "soc"));
    }
}
