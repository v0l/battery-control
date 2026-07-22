//! Native decoder for the **Pylontech CAN** protocol (500 kbps) — the de-facto
//! standard spoken by EG4, SOK and a long tail of "Pylontech-compatible" rack
//! batteries when talking to an inverter.
//!
//! This is battery→inverter telemetry, so it is **read-only**: state of charge,
//! voltage/current/temperature and the BMS's recommended charge/discharge
//! limits and alarm flags.
//!
//! The [`PylontechState`] accumulator is pure and transport-agnostic: feed it
//! raw 8-byte CAN frames keyed by their 11-bit id and read a normalized
//! [`BatteryStatus`]. The actual CAN socket (Linux `socketcan`) lives behind the
//! `can-socket` feature.

use crate::types::BatteryStatus;

fn u16le(d: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([d[i], d[i + 1]])
}
fn i16le(d: &[u8], i: usize) -> i16 {
    i16::from_le_bytes([d[i], d[i + 1]])
}

/// Accumulates the periodic Pylontech CAN frames into a coherent snapshot.
#[derive(Debug, Clone, Default)]
pub struct PylontechState {
    pub soc: Option<f32>,
    pub soh: Option<f32>,
    pub voltage: Option<f32>,
    pub current: Option<f32>,
    pub temperature_c: Option<f32>,
    pub charge_voltage_limit: Option<f32>,
    pub discharge_voltage_limit: Option<f32>,
    pub charge_current_limit: Option<f32>,
    pub discharge_current_limit: Option<f32>,
    pub charge_enabled: Option<bool>,
    pub discharge_enabled: Option<bool>,
    pub alarms: Vec<String>,
}

impl PylontechState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one CAN frame. Unknown ids and short frames are ignored.
    pub fn feed(&mut self, id: u32, data: &[u8]) {
        match id {
            // 0x351: charge/discharge voltage & current limits.
            0x351 if data.len() >= 8 => {
                self.charge_voltage_limit = Some(u16le(data, 0) as f32 * 0.1);
                self.charge_current_limit = Some(i16le(data, 2) as f32 * 0.1);
                self.discharge_current_limit = Some(i16le(data, 4) as f32 * 0.1);
                self.discharge_voltage_limit = Some(u16le(data, 6) as f32 * 0.1);
            }
            // 0x355: SOC / SOH (whole %).
            0x355 if data.len() >= 4 => {
                self.soc = Some(u16le(data, 0) as f32);
                self.soh = Some(u16le(data, 2) as f32);
            }
            // 0x356: measured voltage / current / temperature.
            0x356 if data.len() >= 6 => {
                self.voltage = Some(i16le(data, 0) as f32 * 0.01);
                self.current = Some(i16le(data, 2) as f32 * 0.1);
                self.temperature_c = Some(i16le(data, 4) as f32 * 0.1);
            }
            // 0x359: protection & warning flags.
            0x359 if data.len() >= 4 => {
                self.alarms = decode_alarms(data);
            }
            // 0x35C: charge/discharge enable request (bit flags in byte 0).
            0x35C if !data.is_empty() => {
                self.charge_enabled = Some(data[0] & 0x80 != 0);
                self.discharge_enabled = Some(data[0] & 0x40 != 0);
            }
            _ => {}
        }
    }

    /// True once we've seen enough frames to report meaningful telemetry.
    pub fn is_ready(&self) -> bool {
        self.soc.is_some() || self.voltage.is_some()
    }

    /// Convert the accumulated frames into a normalized status.
    pub fn to_status(&self) -> BatteryStatus {
        let power = match (self.voltage, self.current) {
            (Some(v), Some(i)) => Some(v * i),
            _ => None,
        };
        BatteryStatus {
            soc: self.soc,
            soh: self.soh,
            voltage: self.voltage,
            current: self.current,
            // Pylontech current is + charging / - discharging.
            power_in: power.filter(|p| *p > 0.0),
            power_out: power.filter(|p| *p < 0.0).map(f32::abs),
            temperature_c: self.temperature_c,
            charge_current_limit_a: self.charge_current_limit,
            discharge_current_limit_a: self.discharge_current_limit,
            charging: self.charge_enabled,
            discharging: self.discharge_enabled,
            alarms: self.alarms.clone(),
            ..Default::default()
        }
    }
}

/// Decode the common 0x359 protection/warning bits into human strings.
fn decode_alarms(d: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    // byte 0: protection flags
    let p = d[0];
    for (bit, name) in [
        (1, "cell over-voltage"),
        (2, "cell under-voltage"),
        (3, "pack over-voltage"),
        (4, "pack under-voltage"),
        (5, "charge over-current"),
        (6, "discharge over-current"),
        (7, "over-temperature"),
    ] {
        if p & (1 << bit) != 0 {
            out.push(format!("protection: {name}"));
        }
    }
    // byte 1: warnings
    if d.len() >= 2 {
        let w = d[1];
        for (bit, name) in [
            (1, "cell high-voltage"),
            (2, "cell low-voltage"),
            (5, "high current"),
            (7, "high temperature"),
        ] {
            if w & (1 << bit) != 0 {
                out.push(format!("warning: {name}"));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_core_frames() {
        let mut s = PylontechState::new();
        // 0x355: SOC=87, SOH=100
        s.feed(0x355, &[87, 0, 100, 0, 0, 0, 0, 0]);
        // 0x356: V=53.12 (5312*0.01), I=-15.0 (-150*0.1), T=21.5 (215*0.1)
        let i = (-150i16).to_le_bytes();
        s.feed(0x356, &[0xC0, 0x14, i[0], i[1], 0xD7, 0x00, 0, 0]);
        // 0x351: chg V limit 55.2 (552), chg I 100.0 (1000), dis I 100.0, dis V 45.0 (450)
        s.feed(0x351, &[0x28, 0x02, 0xE8, 0x03, 0xE8, 0x03, 0xC2, 0x01]);

        assert!(s.is_ready());
        let st = s.to_status();
        assert_eq!(st.soc, Some(87.0));
        assert_eq!(st.soh, Some(100.0));
        assert!((st.voltage.unwrap() - 53.12).abs() < 1e-3);
        assert!((st.current.unwrap() - (-15.0)).abs() < 1e-3);
        assert!((st.temperature_c.unwrap() - 21.5).abs() < 1e-3);
        // discharging => power_out positive, power_in none
        assert!(st.power_in.is_none());
        assert!((st.power_out.unwrap() - 53.12 * 15.0).abs() < 0.1);
        assert!((st.charge_current_limit_a.unwrap() - 100.0).abs() < 1e-3);
    }

    #[test]
    fn decodes_alarms() {
        let mut s = PylontechState::new();
        // protection byte: bit2 (under-voltage) + bit5 (charge over-current)
        s.feed(0x359, &[(1 << 2) | (1 << 5), 0, 0, 0]);
        let st = s.to_status();
        assert!(st.alarms.iter().any(|a| a.contains("under-voltage")));
        assert!(st.alarms.iter().any(|a| a.contains("charge over-current")));
    }

    #[test]
    fn ignores_short_and_unknown() {
        let mut s = PylontechState::new();
        s.feed(0x355, &[1]); // too short
        s.feed(0x999, &[0; 8]); // unknown id
        assert!(!s.is_ready());
    }
}

// --- Linux CAN socket transport (feature = "can-socket") --------------------

#[cfg(feature = "can-socket")]
pub use socket_impl::PylontechCan;

#[cfg(feature = "can-socket")]
mod socket_impl {
    use super::PylontechState;
    use crate::battery::Battery;
    use crate::{BatteryStatus, Capabilities, DeviceInfo, Error, Result};
    use async_trait::async_trait;
    use socketcan::{CanFrame, EmbeddedFrame, Frame, Socket};
    use std::time::{Duration, Instant};

    /// A read-only Pylontech-CAN battery on a Linux SocketCAN interface.
    pub struct PylontechCan {
        socket: socketcan::CanSocket,
        info: DeviceInfo,
    }

    impl PylontechCan {
        /// Open a SocketCAN interface (e.g. `"can0"`).
        pub fn open(interface: &str) -> Result<Self> {
            let socket = socketcan::CanSocket::open(interface)
                .map_err(|e| Error::Transport(format!("open {interface}: {e}")))?;
            socket
                .set_read_timeout(Duration::from_millis(1500))
                .map_err(|e| Error::Transport(e.to_string()))?;
            Ok(Self {
                socket,
                info: DeviceInfo {
                    backend: "pylontech-can".into(),
                    model: Some(format!("Pylontech-CAN ({interface})")),
                    ..Default::default()
                },
            })
        }

        /// Collect frames until a coherent snapshot is available or `timeout`.
        fn collect(&self, timeout: Duration) -> Result<PylontechState> {
            let mut state = PylontechState::new();
            let start = Instant::now();
            while start.elapsed() < timeout {
                match self.socket.read_frame() {
                    Ok(CanFrame::Data(f)) => {
                        state.feed(f.raw_id() & 0x7FF, f.data());
                        // One full round of the periodic set is ~1s; return once
                        // we have both SOC and measured voltage.
                        if state.soc.is_some() && state.voltage.is_some() {
                            return Ok(state);
                        }
                    }
                    Ok(_) => {}
                    Err(_) => continue,
                }
            }
            if state.is_ready() {
                Ok(state)
            } else {
                Err(Error::Timeout)
            }
        }
    }

    #[async_trait]
    impl Battery for PylontechCan {
        fn info(&self) -> &DeviceInfo {
            &self.info
        }

        fn capabilities(&self) -> Capabilities {
            Capabilities::READ_BASIC
                | Capabilities::READ_TEMPERATURE
                | Capabilities::READ_LIMITS
                | Capabilities::READ_ALARMS
        }

        async fn status(&mut self) -> Result<BatteryStatus> {
            // socketcan reads are blocking; run them off the async worker.
            let state = tokio::task::block_in_place(|| self.collect(Duration::from_secs(3)))?;
            Ok(state.to_status())
        }
    }
}
