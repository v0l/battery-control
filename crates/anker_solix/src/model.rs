//! Model detection, telemetry decoding and control commands for SOLIX portable
//! power stations.
//!
//! Two telemetry layouts are supported:
//!
//! * **Gen 1** (C300/C800/C1000 gen-1, F2000, F3800): one TLV parameter per
//!   field (`c1` = battery %, `bb` = AC status, ...). Streams telemetry as soon
//!   as the session is negotiated on commands `c402`/`c405`.
//! * **Gen 2** (C1000 Gen 2 / A1763): several fields packed into each parameter
//!   (`a5` = temp+SOC+health, `a7` = AC status+watts, ...). Streams nothing
//!   until it receives a `4100` subscribe command; telemetry arrives on
//!   `c421`/`c900`; AC/DC are controlled with `4101`/`4102`.

use crate::protocol::{param_int, param_int_range, param_string_range, Params};
use serde::Serialize;

/// A supported (or generically-handled) portable power station model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Model {
    C1000Gen2,
    Gen1,
}

impl Model {
    /// Best-effort detection from the advertised BLE name.
    pub fn detect(name: &str) -> Model {
        let n = name.to_ascii_uppercase();
        // Gen 2 advertises names like "A1763" or "C1000 Gen 2" / "...G2".
        if n.contains("A1763") || n.contains("GEN 2") || n.contains("GEN2") || n.contains("G2") {
            Model::C1000Gen2
        } else {
            Model::Gen1
        }
    }

    /// Command codes on which this model streams encrypted telemetry.
    pub fn telemetry_commands(self) -> &'static [[u8; 2]] {
        match self {
            Model::C1000Gen2 => &[[0xc4, 0x21], [0xc9, 0x00]],
            Model::Gen1 => &[[0xc4, 0x02], [0x43, 0x00], [0xc4, 0x05]],
        }
    }

    /// Command that must be sent after negotiation to start telemetry, if any.
    pub fn subscribe_command(self) -> Option<([u8; 2], &'static [u8])> {
        match self {
            Model::C1000Gen2 => Some(([0x41, 0x00], &[0xa1, 0x01, 0x21])),
            Model::Gen1 => None,
        }
    }

    pub fn cmd_ac_output(self) -> [u8; 2] {
        match self {
            Model::C1000Gen2 => [0x41, 0x01],
            Model::Gen1 => [0x40, 0x4a],
        }
    }

    pub fn cmd_dc_output(self) -> [u8; 2] {
        match self {
            Model::C1000Gen2 => [0x41, 0x02],
            Model::Gen1 => [0x40, 0x4b],
        }
    }

    /// Command code to set the LED light-bar mode. Documented for gen-1; the
    /// gen-2 code is not yet known, so returns `None`.
    pub fn cmd_light_mode(self) -> Option<[u8; 2]> {
        match self {
            Model::Gen1 => Some([0x40, 0x4f]),
            Model::C1000Gen2 => None,
        }
    }

    /// Command code to turn the LCD display on/off.
    ///
    /// Gen-1 uses `4052`; gen-2 uses `4103` (alongside `4100`=subscribe,
    /// `4101`=AC, `4102`=DC).
    pub fn cmd_display_on_off(self) -> Option<[u8; 2]> {
        match self {
            Model::Gen1 => Some([0x40, 0x52]),
            Model::C1000Gen2 => Some([0x41, 0x03]),
        }
    }
}

/// Brightness / mode for the LED light bar (and LCD display brightness).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Brightness {
    Off = 0,
    Low = 1,
    Medium = 2,
    High = 3,
    /// SOS strobe (light bar only; not valid for the display).
    Sos = 4,
}

impl Brightness {
    /// Parse a human string (`off`/`low`/`medium`/`high`/`sos`).
    pub fn parse(s: &str) -> Option<Brightness> {
        match s.to_ascii_lowercase().as_str() {
            "off" | "0" => Some(Brightness::Off),
            "low" | "1" => Some(Brightness::Low),
            "medium" | "med" | "2" => Some(Brightness::Medium),
            "high" | "3" => Some(Brightness::High),
            "sos" | "4" => Some(Brightness::Sos),
            _ => None,
        }
    }
}

/// Payload prefix for light-bar brightness; append the [`Brightness`] byte.
pub const PAYLOAD_LIGHT_PREFIX: [u8; 6] = [0xa1, 0x01, 0x21, 0xa2, 0x02, 0x01];

/// Build the light-bar brightness payload.
pub fn light_mode_payload(b: Brightness) -> Vec<u8> {
    let mut v = PAYLOAD_LIGHT_PREFIX.to_vec();
    v.push(b as u8);
    v
}

/// Status of a physical port / output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PortStatus {
    #[default]
    Unknown,
    Off,
    Output,
    Input,
}

impl PortStatus {
    fn from_int(v: Option<i64>) -> PortStatus {
        match v {
            Some(0) => PortStatus::Off,
            Some(1) => PortStatus::Output,
            Some(2) => PortStatus::Input,
            _ => PortStatus::Unknown,
        }
    }

    /// For input-only ports, an "output" (1) reading actually means input.
    fn from_input_only(v: Option<i64>) -> PortStatus {
        match v {
            Some(0) => PortStatus::Off,
            Some(1) | Some(2) => PortStatus::Input,
            _ => PortStatus::Unknown,
        }
    }
}

/// A named port with its on/off status and instantaneous power.
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct Port {
    pub status: PortStatus,
    pub watts: Option<i64>,
}

/// Decoded snapshot of a power station's state. Fields are `Option` because not
/// every model / packet populates every value.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Telemetry {
    pub battery_percentage: Option<i64>,
    pub battery_percentage_expansion: Option<i64>,
    pub battery_health: Option<i64>,
    pub num_expansion: Option<i64>,
    pub max_battery_percentage: Option<i64>,
    pub min_battery_percentage: Option<i64>,

    pub power_in: Option<i64>,
    pub power_out: Option<i64>,

    pub ac: Port,
    pub dc: Port,
    pub solar: Port,
    pub ac_power_in: Option<i64>,

    pub usb_c1: Port,
    pub usb_c2: Port,
    pub usb_c3: Port,
    pub usb_a1: Port,
    pub usb_a2: Port,

    pub time_remaining_hours: Option<f64>,
    pub temperature_c: Option<i64>,
    pub software_version: Option<String>,
    pub serial_number: Option<String>,
    pub part_number: Option<String>,
}

impl Telemetry {
    pub fn from_params(model: Model, p: &Params) -> Telemetry {
        match model {
            Model::C1000Gen2 => Self::from_params_gen2(p),
            Model::Gen1 => Self::from_params_gen1(p),
        }
    }

    /// Gen 2 (C1000 Gen 2 / A1763) packed layout.
    fn from_params_gen2(p: &Params) -> Telemetry {
        let range = |k: &str, b: usize, e: usize, s: bool| param_int_range(p, k, b, Some(e), s);
        let tail = |k: &str, b: usize| param_int(p, k, b, false);
        Telemetry {
            temperature_c: range("a5", 1, 2, true),
            battery_percentage: range("a5", 3, 4, false),
            battery_health: range("a5", 4, 5, false),
            max_battery_percentage: range("d9", 4, 5, false),
            min_battery_percentage: range("d9", 5, 6, false),

            power_out: range("a6", 1, 3, false),
            ac_power_in: range("a6", 3, 5, false),

            ac: Port {
                status: PortStatus::from_int(range("a7", 1, 2, false)),
                watts: range("a7", 2, 4, false),
            },
            solar: Port {
                status: PortStatus::from_input_only(range("a8", 1, 2, false)),
                watts: tail("a8", 2),
            },
            dc: Port {
                status: PortStatus::from_int(range("b2", 1, 2, false)),
                watts: tail("b2", 2),
            },
            usb_c1: Port {
                status: PortStatus::from_int(range("aa", 1, 2, false)),
                watts: tail("aa", 2),
            },
            usb_c2: Port {
                status: PortStatus::from_int(range("ab", 1, 2, false)),
                watts: tail("ab", 2),
            },
            usb_c3: Port {
                status: PortStatus::from_int(range("ac", 1, 2, false)),
                watts: tail("ac", 2),
            },
            usb_a1: Port {
                status: PortStatus::from_int(range("ae", 1, 2, false)),
                watts: tail("ae", 2),
            },

            serial_number: param_string_range(p, "a2", 3, Some(20)),
            part_number: param_string_range(p, "a2", 22, Some(27)),
            ..Default::default()
        }
    }

    /// Gen 1 one-field-per-parameter layout.
    fn from_params_gen1(p: &Params) -> Telemetry {
        let i = |k: &str| param_int(p, k, 1, false);
        Telemetry {
            battery_percentage: i("c1"),
            battery_percentage_expansion: i("c2"),
            battery_health: i("c3"),
            num_expansion: i("c5"),

            power_in: i("af"),
            power_out: i("b0"),
            ac_power_in: i("a5"),

            ac: Port {
                status: PortStatus::from_int(i("bb")),
                watts: i("a6"),
            },
            dc: Port {
                status: PortStatus::from_int(param_int_range(p, "b2", 1, Some(2), false)),
                watts: param_int(p, "b2", 2, false),
            },
            solar: Port {
                status: PortStatus::Unknown,
                watts: i("ae"),
            },
            usb_c1: Port { status: PortStatus::Unknown, watts: i("a7") },
            usb_c2: Port { status: PortStatus::Unknown, watts: i("a8") },
            usb_a1: Port { status: PortStatus::Unknown, watts: i("a9") },
            usb_a2: Port { status: PortStatus::Unknown, watts: i("aa") },

            time_remaining_hours: i("a4").map(|v| v as f64 / 10.0),
            temperature_c: param_int(p, "bd", 1, true),
            software_version: param_int(p, "b3", 1, false).map(format_dotted_version),
            serial_number: param_string_range(p, "d0", 1, None),
            ..Default::default()
        }
    }
}

fn format_dotted_version(v: i64) -> String {
    v.to_string()
        .chars()
        .map(|c| c.to_string())
        .collect::<Vec<_>>()
        .join(".")
}

// --- Control payloads ---------------------------------------------------------

/// Request an on-demand telemetry snapshot (gen-1).
pub const CMD_STATUS_REQUEST: [u8; 2] = [0x40, 0x40];
pub const PAYLOAD_STATUS_REQUEST: [u8; 3] = [0xa1, 0x01, 0x21];

pub const PAYLOAD_ON: [u8; 7] = [0xa1, 0x01, 0x21, 0xa2, 0x02, 0x01, 0x01];
pub const PAYLOAD_OFF: [u8; 7] = [0xa1, 0x01, 0x21, 0xa2, 0x02, 0x01, 0x00];

/// The on/off payload for a boolean control.
pub fn on_off_payload(on: bool) -> &'static [u8] {
    if on {
        &PAYLOAD_ON
    } else {
        &PAYLOAD_OFF
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::parse_params;

    #[test]
    fn model_detection() {
        assert_eq!(Model::detect("A1763"), Model::C1000Gen2);
        assert_eq!(Model::detect("Anker C1000 Gen 2"), Model::C1000Gen2);
        assert_eq!(Model::detect("Anker C1000"), Model::Gen1);
        assert_eq!(Model::detect("F3800"), Model::Gen1);
    }

    #[test]
    fn gen2_telemetry_cmds_and_control() {
        let m = Model::C1000Gen2;
        assert_eq!(m.telemetry_commands(), &[[0xc4, 0x21], [0xc9, 0x00]]);
        assert_eq!(m.cmd_ac_output(), [0x41, 0x01]);
        assert_eq!(m.cmd_dc_output(), [0x41, 0x02]);
        assert!(m.subscribe_command().is_some());
    }

    #[test]
    fn decode_gen2_packed_params() {
        // a5: type, temp=25(signed), <skip>, soc=80, health=100
        //   bytes: [00, 19, 00, 50, 64]
        // a7: type, ac_status=01, watts LE = 0x0064 = 100 -> [00,01,64,00]
        let payload = [
            0xa5, 0x05, 0x00, 0x19, 0x00, 0x50, 0x64, //
            0xa7, 0x04, 0x00, 0x01, 0x64, 0x00, //
        ];
        let p = parse_params(&payload);
        let t = Telemetry::from_params(Model::C1000Gen2, &p);
        assert_eq!(t.temperature_c, Some(25));
        assert_eq!(t.battery_percentage, Some(80));
        assert_eq!(t.battery_health, Some(100));
        assert_eq!(t.ac.status, PortStatus::Output);
        assert_eq!(t.ac.watts, Some(100));
    }

    #[test]
    fn gen1_still_decodes() {
        let payload = [0xc1, 0x02, 0x00, 0x55, 0xbb, 0x02, 0x00, 0x01];
        let p = parse_params(&payload);
        let t = Telemetry::from_params(Model::Gen1, &p);
        assert_eq!(t.battery_percentage, Some(85));
        assert_eq!(t.ac.status, PortStatus::Output);
    }
}
