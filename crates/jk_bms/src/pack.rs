pub const MYBMM_PACK_NAME_LEN: usize = 32;
pub const MAX_TEMPS: usize = 8;
pub const MAX_CELLS: usize = 32;

/// JK BMS protocol version, matching the ESPHome reference implementation.
/// - JK04: Old JK-BMS with IEEE 754 float cell voltages (4 bytes per cell)
/// - JK02_24S: JK-BMS BLE v2 with 24-cell max (2-byte integer mV cell voltages)
/// - JK02_32S: JK-BMS BLE v2 with 32-cell max (2-byte integer mV, with 16-byte offset shift)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProtocolVersion {
    #[default]
    Jk02_24S,
    Jk02_32S,
    Jk04,
}

impl ProtocolVersion {
    /// Determine protocol version from the model string in the info frame.
    /// Models starting with "JK_PB2", "JK-BD", "JK_HY" are JK02_32S.
    /// Models starting with "JK-B2" are JK02_24S.
    /// Models starting with "JK04" or similar old prefixes are JK04.
    pub fn from_model(model: &str) -> Self {
        if model.starts_with("JK_PB2") || model.starts_with("JK-BD") || model.starts_with("JK_HY") {
            ProtocolVersion::Jk02_32S
        } else {
            ProtocolVersion::Jk02_24S
        }
    }

    /// Whether this is a JK02 variant (24S or 32S)
    pub fn is_jk02(&self) -> bool {
        matches!(self, ProtocolVersion::Jk02_24S | ProtocolVersion::Jk02_32S)
    }

    /// Whether this is JK04 variant
    pub fn is_jk04(&self) -> bool {
        matches!(self, ProtocolVersion::Jk04)
    }

    /// Offset shift for 32S models (extra 16 bytes per cell group)
    pub fn cell_offset(&self) -> usize {
        match self {
            ProtocolVersion::Jk02_32S => 16,
            _ => 0,
        }
    }

    /// Number of max cell slots in the frame
    pub fn max_frame_cells(&self) -> usize {
        match self {
            ProtocolVersion::Jk02_24S => 24,
            ProtocolVersion::Jk02_32S => 32,
            ProtocolVersion::Jk04 => 24,
        }
    }
}

/// JK BMS settings (read/write configuration values).
/// Parsed from frame type 0x01 (settings frame).
/// Field names and offsets match the ESPHome `decode_jk02_settings_()` reference.
#[derive(Debug, Clone, Default)]
pub struct JkSettings {
    // Voltage settings (V)
    pub smart_sleep_voltage: f32,
    pub cell_uvp: f32,                    // Cell undervoltage protection
    pub cell_uvpr: f32,                   // Cell UVP recovery
    pub cell_ovp: f32,                    // Cell overvoltage protection
    pub cell_ovpr: f32,                   // Cell OVP recovery
    pub balance_trigger_voltage: f32,
    pub cell_soc100_voltage: f32,
    pub cell_soc0_voltage: f32,
    pub cell_request_charge_voltage: f32, // RCV
    pub cell_request_float_voltage: f32,  // RFV
    pub power_off_voltage: f32,
    pub balance_starting_voltage: f32,

    // Current settings (A)
    pub max_charge_current: f32,
    pub max_discharge_current: f32,
    pub max_balance_current: f32,

    // Timing settings (s)
    pub charge_ocp_delay: f32,            // Charge overcurrent protection delay
    pub charge_ocp_recovery: f32,         // Charge OCP recovery time
    pub discharge_ocp_delay: f32,
    pub discharge_ocp_recovery: f32,
    pub scp_recovery: f32,                // Short circuit protection recovery time
    pub scp_delay: f32,                   // Short circuit protection delay (JK02_24S: s, JK02_32S: μs)

    // Temperature settings (°C)
    pub charge_otp: f32,                  // Charge overtemperature protection
    pub charge_otp_recovery: f32,
    pub discharge_otp: f32,
    pub discharge_otp_recovery: f32,
    pub charge_utp: f32,                  // Charge undertemperature protection
    pub charge_utp_recovery: f32,
    pub power_tube_otp: f32,              // MOSFET overtemperature protection
    pub power_tube_otp_recovery: f32,
    // JK02_32S only:
    pub discharge_utp: f32,
    pub discharge_utp_recovery: f32,
    pub heating_start_temperature: f32,
    pub heating_stop_temperature: f32,

    // Cell configuration
    pub cell_count: u8,
    pub total_battery_capacity: f32,      // Ah

    // Switches
    pub charging_switch: bool,
    pub discharging_switch: bool,
    pub balancer_switch: bool,
    // JK02_32S only switches:
    pub heating_switch: bool,
    pub disable_temp_sensors: bool,
    pub display_always_on: bool,
    pub smart_sleep_switch: bool,
    pub disable_pcl_module: bool,
    pub timed_stored_data: bool,
    pub charging_float_mode: bool,

    // JK02_32S only
    pub discharge_precharge_time: u8,     // seconds
    pub cell_request_charge_voltage_time: f32, // hours (register 0xB3, factor 10, length 1)
    pub cell_request_float_voltage_time: f32,  // hours (register 0xB4, factor 10, length 1)
    pub re_bulk_soc: f32,                      // % (register 0xB7, factor 1, length 1)

    // Calibration
    pub voltage_calibration: f32,
    pub current_calibration: f32,

    // Connection wire resistances (Ohm)
    pub wire_resistance: [f32; 32],

    // Raw settings frame data (for debugging / unknown fields)
    pub raw_frame: Vec<u8>,
}

impl JkSettings {
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug, Clone)]
pub struct MybmmPack {
    pub name: String,
    pub uuid: String,
    pub r#type: String,
    pub transport: String,
    pub target: String,
    pub opts: String,
    pub state: u16,
    pub failed: i32,
    pub error: i32,
    pub errmsg: String,
    pub capacity: f32,
    pub voltage: f32,
    pub current: f32,
    pub power: f32,
    pub charging_power: f32,
    pub discharging_power: f32,
    pub status: i32,
    pub ntemps: i32,
    pub temps: [f32; MAX_TEMPS],
    pub power_tube_temp: f32,
    pub cells: i32,
    pub cellvolt: [f32; MAX_CELLS],
    pub cellres: [f32; MAX_CELLS],
    pub cell_min: f32,
    pub cell_max: f32,
    pub cell_diff: f32,
    pub cell_avg: f32,
    pub balancebits: u32,
    pub capabilities: u16,
    pub protocol_version: ProtocolVersion,
    // Device info from info frame
    pub model: String,
    pub hwvers: String,
    pub swvers: String,
    // Rich cell info fields from JK02 protocol
    pub error_bitmask: u16,
    pub balancing_current: f32,
    pub balancing: bool,
    pub soc: f32,
    pub capacity_remaining: f32,
    pub total_battery_capacity: f32,
    pub charging_cycles: u32,
    pub total_charging_cycle_capacity: f32,
    pub soh: f32,
    pub total_runtime: u32,
    pub charging: bool,
    pub discharging: bool,
    pub precharging: bool,
    pub heating: bool,
    pub enabled_cells_bitmask: u32,
    // Settings (populated from frame type 0x01)
    pub settings: Option<JkSettings>,
}

impl MybmmPack {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            uuid: String::new(),
            r#type: String::new(),
            transport: String::new(),
            target: String::new(),
            opts: String::new(),
            state: 0,
            failed: 0,
            error: 0,
            errmsg: String::new(),
            capacity: 0.0,
            voltage: 0.0,
            current: 0.0,
            power: 0.0,
            charging_power: 0.0,
            discharging_power: 0.0,
            status: 0,
            ntemps: 0,
            temps: [0.0; MAX_TEMPS],
            power_tube_temp: 0.0,
            cells: 0,
            cellvolt: [0.0; MAX_CELLS],
            cellres: [0.0; MAX_CELLS],
            cell_min: 0.0,
            cell_max: 0.0,
            cell_diff: 0.0,
            cell_avg: 0.0,
            balancebits: 0,
            capabilities: 0,
            protocol_version: ProtocolVersion::default(),
            model: String::new(),
            hwvers: String::new(),
            swvers: String::new(),
            error_bitmask: 0,
            balancing_current: 0.0,
            balancing: false,
            soc: 0.0,
            capacity_remaining: 0.0,
            total_battery_capacity: 0.0,
            charging_cycles: 0,
            total_charging_cycle_capacity: 0.0,
            soh: 0.0,
            total_runtime: 0,
            charging: false,
            discharging: false,
            precharging: false,
            heating: false,
            enabled_cells_bitmask: 0,
            settings: None,
        }
    }

    /// Backwards compatibility: returns true if this is a PB2-series (JK02_32S) BMS
    pub fn is_pb2(&self) -> bool {
        self.protocol_version == ProtocolVersion::Jk02_32S
    }
}
