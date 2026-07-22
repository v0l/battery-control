//! Normalized data model shared by every backend.
//!
//! Battery devices span three loose classes — cell-level BMS, power stations,
//! and battery monitors — so every field is optional and the collections are
//! empty when a device doesn't report them. A backend fills in whatever it can.

use serde::Serialize;

/// Static identity of a device (does not change between reads).
#[derive(Debug, Clone, Default, Serialize)]
pub struct DeviceInfo {
    /// Backend/protocol name, e.g. `"anker"`, `"jk"`, `"pylontech-can"`.
    pub backend: String,
    /// Human model string if known, e.g. `"SOLIX C1000 Gen 2"`.
    pub model: Option<String>,
    pub serial: Option<String>,
    pub firmware: Option<String>,
}

/// Per-cell measurement (BMS-class devices only).
#[derive(Debug, Clone, Copy, Default, Serialize)]
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

/// A controllable/observable port on a device.
///
/// Ports are intentionally free-form: there is no fixed set of port types
/// because devices expose wildly different connectors (AC, 12 V, USB-C, car
/// socket, Anderson, XT60, wireless pad, …). A port is identified by its
/// [`id`](PortInfo::id) and targeted by that id via [`Command::SetPort`].
#[derive(Debug, Clone, Serialize)]
pub struct PortInfo {
    /// Stable, unique identifier used to target the port, e.g. `"ac"`,
    /// `"usb_c1"`. Distinguishes otherwise-identical ports.
    pub id: String,
    /// Optional human-friendly name, e.g. `"USB-C 1"`.
    pub label: Option<String>,
    /// Direction of flow, if known (may reflect the *current* flow on
    /// bidirectional ports).
    pub direction: Option<PortDirection>,
    pub on: Option<bool>,
    pub watts: Option<f32>,
}

/// A named temperature sensor. Devices expose different probes (cell groups,
/// MOSFET, ambient, balancer, ...), so temperatures are free-form.
#[derive(Debug, Clone, Serialize)]
pub struct Sensor {
    /// Stable id, e.g. `"t1"`, `"mosfet"`, `"ambient"`.
    pub id: String,
    /// Optional human name, e.g. `"MOSFET"`.
    pub label: Option<String>,
    pub celsius: f32,
}

/// A named on/off switch on a device (heater, precharge, balancer, ...).
#[derive(Debug, Clone, Serialize)]
pub struct Switch {
    /// Stable id, e.g. `"heater"`, `"precharge"`, `"balancer"`.
    pub id: String,
    pub label: Option<String>,
    pub on: bool,
}

/// A normalized snapshot of a battery's state.
///
/// `current` is signed: **positive = charging**, **negative = discharging**.
#[derive(Debug, Clone, Default, Serialize)]
pub struct BatteryStatus {
    /// State of charge, %.
    pub soc: Option<f32>,
    /// State of health, %.
    pub soh: Option<f32>,
    /// Pack voltage, volts.
    pub voltage: Option<f32>,
    /// Pack current, amps (+charge / -discharge).
    pub current: Option<f32>,
    /// Instantaneous power into the battery, watts.
    pub power_in: Option<f32>,
    /// Instantaneous power out of the battery, watts.
    pub power_out: Option<f32>,
    /// Temperature sensors (free-form; devices expose different probes).
    pub temperatures: Vec<Sensor>,
    /// Estimated time to full/empty, hours.
    pub time_remaining_h: Option<f32>,
    /// Remaining capacity, amp-hours.
    pub capacity_remaining_ah: Option<f32>,
    /// Full-pack capacity, amp-hours.
    pub capacity_full_ah: Option<f32>,
    /// Charge cycle count.
    pub cycles: Option<u32>,

    /// Charge/discharge MOSFET states (BMS-class), if reported. These are
    /// convenience accessors for the two near-universal toggles; any other
    /// device switches (heater, precharge, balancer, ...) live in [`switches`].
    pub charging: Option<bool>,
    pub discharging: Option<bool>,
    /// Free-form device switches beyond charge/discharge (heater, precharge,
    /// balancer, ...).
    pub switches: Vec<Switch>,

    /// BMS-recommended limits (Pylontech-class), if reported.
    pub charge_current_limit_a: Option<f32>,
    pub discharge_current_limit_a: Option<f32>,

    /// Per-cell detail (empty for non-BMS devices).
    pub cells: Vec<CellInfo>,
    /// Output ports (empty for non-station devices).
    pub ports: Vec<PortInfo>,
    /// Active alarm/warning strings, if any.
    pub alarms: Vec<String>,
}

impl BatteryStatus {
    /// The highest reported temperature (a representative value), if any.
    pub fn temperature_c(&self) -> Option<f32> {
        self.temperatures
            .iter()
            .map(|s| s.celsius)
            .fold(None, |m, c| Some(m.map_or(c, |m: f32| m.max(c))))
    }

    /// A specific sensor's temperature by id.
    pub fn temperature(&self, id: &str) -> Option<f32> {
        self.temperatures.iter().find(|s| s.id == id).map(|s| s.celsius)
    }

    /// A switch state by id (also covers `"charging"`/`"discharging"`).
    pub fn switch(&self, id: &str) -> Option<bool> {
        match id {
            "charging" => self.charging,
            "discharging" => self.discharging,
            _ => self.switches.iter().find(|s| s.id == id).map(|s| s.on),
        }
    }

    /// Highest cell voltage, if any cells are present.
    pub fn cell_max(&self) -> Option<f32> {
        self.cells.iter().filter_map(|c| c.voltage).fold(None, |m, v| {
            Some(m.map_or(v, |m: f32| m.max(v)))
        })
    }

    /// Lowest cell voltage, if any cells are present.
    pub fn cell_min(&self) -> Option<f32> {
        self.cells.iter().filter_map(|c| c.voltage).fold(None, |m, v| {
            Some(m.map_or(v, |m: f32| m.min(v)))
        })
    }

    /// Cell voltage spread (max - min), if cells are present.
    pub fn cell_delta(&self) -> Option<f32> {
        Some(self.cell_max()? - self.cell_min()?)
    }
}

/// A control command, addressed by a free-form **id** rather than a fixed
/// taxonomy — the same philosophy as ports/sensors/switches. Backends advertise
/// coarse abilities via [`crate::Capabilities`] and validate the specific id;
/// anything unsupported returns [`crate::Error::Unsupported`].
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
}
