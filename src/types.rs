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

/// A controllable/observable output port (power-station-class devices).
#[derive(Debug, Clone, Serialize)]
pub struct PortInfo {
    pub kind: PortKind,
    pub on: Option<bool>,
    pub watts: Option<f32>,
}

/// Kinds of ports / outputs across device classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PortKind {
    Ac,
    Dc,
    Solar,
    UsbC,
    UsbA,
    Other,
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
    /// Representative temperature, °C.
    pub temperature_c: Option<f32>,
    /// Estimated time to full/empty, hours.
    pub time_remaining_h: Option<f32>,
    /// Remaining capacity, amp-hours.
    pub capacity_remaining_ah: Option<f32>,
    /// Full-pack capacity, amp-hours.
    pub capacity_full_ah: Option<f32>,
    /// Charge cycle count.
    pub cycles: Option<u32>,

    /// Charge/discharge MOSFET states (BMS-class), if reported.
    pub charging: Option<bool>,
    pub discharging: Option<bool>,

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

/// A control command. Backends advertise which they support via
/// [`crate::Capabilities`]; unsupported commands return [`crate::Error::Unsupported`].
#[derive(Debug, Clone)]
pub enum Command {
    /// Turn a named output port on/off (power stations).
    SetPort { kind: PortKind, on: bool },
    /// Enable/disable the charge MOSFET (BMS).
    SetCharging(bool),
    /// Enable/disable the discharge MOSFET (BMS).
    SetDischarging(bool),
    /// Enable/disable cell balancing (BMS).
    SetBalancer(bool),
    /// Set the charge ceiling, % (0–100).
    SetChargeLimit(u8),
    /// Write a named backend-specific setting.
    SetSetting { name: String, value: String },
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
