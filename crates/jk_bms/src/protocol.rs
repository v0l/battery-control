use crate::pack::{JkSettings, MybmmPack, ProtocolVersion};

const SIG_BYTES: [u8; 4] = [0x55, 0xAA, 0xEB, 0x90];
// Minimum frame size per the ESPHome reference (all frames are 300 bytes with CRC at byte 299)
const JK02_FRAME_SIZE: usize = 300;
// Maximum response size (frame can be up to ~400 bytes, but meaningful data is first 300)
const MAX_RESPONSE_SIZE: usize = 400;
// Minimum data needed after frame start for PB2 voltage/current/temp fields (for early-exit scanner)
const PB2_FRAME_SIZE: usize = 168;

pub fn get_short(data: &[u8], offset: usize) -> u16 {
    if offset + 2 <= data.len() {
        u16::from_le_bytes([data[offset], data[offset + 1]])
    } else {
        0
    }
}

pub fn get_signed_short(data: &[u8], offset: usize) -> i16 {
    if offset + 2 <= data.len() {
        i16::from_le_bytes([data[offset], data[offset + 1]])
    } else {
        0
    }
}

pub fn get_16bit(data: &[u8], offset: usize) -> u16 {
    if offset + 2 <= data.len() {
        (u16::from(data[offset + 1]) << 8) | u16::from(data[offset])
    } else {
        0
    }
}

pub fn get_32bit(data: &[u8], offset: usize) -> u32 {
    if offset + 4 <= data.len() {
        (u32::from(get_16bit(data, offset + 2)) << 16) | u32::from(get_16bit(data, offset))
    } else {
        0
    }
}

/// Convert a u32 to IEEE 754 f32 (used by JK04 protocol for cell voltages/resistances)
pub fn ieee_float(raw: u32) -> f32 {
    f32::from_bits(raw)
}

/// CRC check: sum of all bytes (mod 256). Matches the ESPHome reference `crc()` function.
pub fn crc(data: &[u8], len: usize) -> u8 {
    let mut crc: u8 = 0;
    for &b in &data[..len.min(data.len())] {
        crc = crc.wrapping_add(b);
    }
    crc
}

/// Error flag descriptions, matching ESPHome reference
pub const ERROR_DESCRIPTIONS: [&str; 16] = [
    "Charge Overtemperature",               // bit 0
    "Charge Undertemperature",              // bit 1
    "Coprocessor communication error",      // bit 2
    "Cell Undervoltage",                    // bit 3
    "Battery pack undervoltage",            // bit 4
    "Discharge overcurrent",                // bit 5
    "Discharge short circuit",              // bit 6
    "Discharge overtemperature",            // bit 7
    "Wire resistance",                      // bit 8
    "Mosfet overtemperature",               // bit 9
    "Cell count is not equal to settings",  // bit 10
    "Current sensor anomaly",               // bit 11
    "Cell Overvoltage",                     // bit 12
    "Battery pack overvoltage",             // bit 13
    "Charge overcurrent protection",        // bit 14
    "Charge short circuit",                 // bit 15
];

/// Convert error bitmask to a list of active error descriptions
pub fn error_bitmask_to_strings(bitmask: u16) -> Vec<&'static str> {
    let mut errors = vec![];
    for (i, desc) in ERROR_DESCRIPTIONS.iter().enumerate() {
        if bitmask & (1 << i) != 0 {
            errors.push(*desc);
        }
    }
    errors
}

/// Parse cell voltages for JK02 protocol (2-byte LE unsigned, * 0.001 V)
fn parse_jk02_cell_voltages(pp: &mut MybmmPack, data: &[u8], offset: usize) {
    let cells = pp.protocol_version.max_frame_cells();
    let mut cells_enabled = 0usize;
    let mut min_cell_voltage = 100.0f32;
    let mut max_cell_voltage = -100.0f32;
    let mut average_cell_voltage = 0.0f32;
    let mut _min_voltage_cell = 0u8;
    let mut _max_voltage_cell = 0u8;

    for i in 0..cells {
        let cell_voltage = get_16bit(data, i * 2 + 6) as f32 * 0.001;
        let cell_resistance = get_16bit(data, i * 2 + 64 + offset) as f32 * 0.001;

        if cell_voltage > 0.0 {
            average_cell_voltage += cell_voltage;
            cells_enabled += 1;
        }
        if cell_voltage > 0.0 && cell_voltage < min_cell_voltage {
            min_cell_voltage = cell_voltage;
            _min_voltage_cell = (i + 1) as u8;
        }
        if cell_voltage > max_cell_voltage {
            max_cell_voltage = cell_voltage;
            _max_voltage_cell = (i + 1) as u8;
        }
        pp.cellvolt[i] = cell_voltage;
        pp.cellres[i] = cell_resistance;
    }

    pp.cells = cells_enabled as i32;
    if cells_enabled > 0 {
        pp.cell_avg = average_cell_voltage / cells_enabled as f32;
    }
    pp.cell_min = min_cell_voltage;
    pp.cell_max = max_cell_voltage;
    pp.cell_diff = max_cell_voltage - min_cell_voltage;
}

/// Parse JK02 24S cell info frame, matching ESPHome `decode_jk02_cell_info_()`
fn parse_jk02_cell_info(pp: &mut MybmmPack, data: &[u8]) {
    let offset = pp.protocol_version.cell_offset();

    parse_jk02_cell_voltages(pp, data, offset);

    // Enabled cells bitmask at offset 54 + offset (4 bytes)
    pp.enabled_cells_bitmask = get_32bit(data, 54 + offset);

    // After cell voltages/resistances, double the offset for remaining fields
    // (because cell resistance section adds another `offset` worth of bytes)
    let offset2 = offset * 2;

    // 112: Power tube temperature (32S) or Unknown (24S)
    if pp.protocol_version == ProtocolVersion::Jk02_32S {
        pp.power_tube_temp = get_16bit(data, 112 + offset2) as i16 as f32 * 0.1;
    }

    // 118: Total battery voltage (4 bytes, * 0.001 V)
    let total_voltage = get_32bit(data, 118 + offset2) as f32 * 0.001;
    pp.voltage = total_voltage;

    // 126: Charge current (4 bytes, signed, * 0.001 A)
    let current = get_32bit(data, 126 + offset2) as i32 as f32 * 0.001;
    pp.current = current;

    // Power = voltage * current (don't use offset 122 which is unsigned)
    pp.power = total_voltage * current;
    pp.charging_power = pp.power.max(0.0);
    pp.discharging_power = pp.power.min(0.0).abs();

    // 130: Temperature Sensor 1 (2 bytes, signed, * 0.1 °C)
    pp.temps[0] = get_16bit(data, 130 + offset2) as i16 as f32 * 0.1;
    // 132: Temperature Sensor 2
    pp.temps[1] = get_16bit(data, 132 + offset2) as i16 as f32 * 0.1;
    pp.ntemps = 2;

    // 134: MOS Temperature (24S) or Error bitmask (32S)
    if pp.protocol_version == ProtocolVersion::Jk02_32S {
        pp.error_bitmask = get_16bit(data, 134 + offset2);
    } else {
        pp.power_tube_temp = get_16bit(data, 134 + offset2) as i16 as f32 * 0.1;
    }

    // 136: System alarms / error bitmask (24S only; 32S already read at 134)
    if pp.protocol_version != ProtocolVersion::Jk02_32S {
        pp.error_bitmask = get_16bit(data, 136 + offset2);
    }

    // 138: Balance current (2 bytes, signed, * 0.001 A)
    pp.balancing_current = get_16bit(data, 138 + offset2) as i16 as f32 * 0.001;

    // 140: Balancing action (0=off, 1=charging balancer, 2=discharging balancer)
    pp.balancing = data.get(140 + offset2).is_some_and(|&b| b != 0x00);

    // 141: State of charge (%)
    pp.soc = *data.get(141 + offset2).unwrap_or(&0) as f32;

    // 142: Capacity remaining (4 bytes, * 0.001 Ah)
    pp.capacity_remaining = get_32bit(data, 142 + offset2) as f32 * 0.001;

    // 146: Nominal/total battery capacity (4 bytes, * 0.001 Ah)
    pp.total_battery_capacity = get_32bit(data, 146 + offset2) as f32 * 0.001;

    // 150: Charging cycles (4 bytes, raw count)
    pp.charging_cycles = get_32bit(data, 150 + offset2);

    // 154: Total charging cycle capacity (4 bytes, * 0.001 Ah)
    pp.total_charging_cycle_capacity = get_32bit(data, 154 + offset2) as f32 * 0.001;

    // 158: SOH - State of Health (%)
    pp.soh = *data.get(158 + offset2).unwrap_or(&0) as f32;

    // 162: Total runtime (4 bytes, seconds)
    pp.total_runtime = get_32bit(data, 162 + offset2);

    // 166: Charging mosfet enabled (0x00=off, 0x01=on)
    pp.charging = data.get(166 + offset2).is_some_and(|&b| b != 0x00);

    // 167: Discharging mosfet enabled
    pp.discharging = data.get(167 + offset2).is_some_and(|&b| b != 0x00);

    // 168: Precharging
    pp.precharging = data.get(168 + offset2).is_some_and(|&b| b != 0x00);

    // 183: Heating
    pp.heating = data.get(183 + offset2).is_some_and(|&b| b != 0x00);

    // 32S additional temp sensors
    if pp.protocol_version == ProtocolVersion::Jk02_32S {
        // Temp sensor 5 at 222+offset2
        if data.len() > 223 + offset2 {
            pp.temps[2] = get_16bit(data, 226 + offset2) as i16 as f32 * 0.1;
            pp.temps[3] = get_16bit(data, 224 + offset2) as i16 as f32 * 0.1;
            pp.temps[4] = get_16bit(data, 222 + offset2) as i16 as f32 * 0.1;
            pp.ntemps = 5;
        }
    }
}

/// Parse JK04 cell info frame, matching ESPHome `decode_jk04_cell_info_()`
/// JK04 uses IEEE 754 floats for cell voltages (4 bytes per cell) and resistances
fn parse_jk04_cell_info(pp: &mut MybmmPack, data: &[u8]) {
    let cells = 24;
    let mut cells_enabled = 0usize;
    let mut min_cell_voltage = 100.0f32;
    let mut max_cell_voltage = -100.0f32;
    let mut average_cell_voltage = 0.0f32;
    let mut total_voltage = 0.0f32;
    let mut _min_voltage_cell = 0u8;
    let mut _max_voltage_cell = 0u8;

    for i in 0..cells {
        // Cell voltages: IEEE 754 float at offset 6 + i*4
        let cell_voltage = ieee_float(get_32bit(data, i * 4 + 6));
        // Cell resistances: IEEE 754 float at offset 102 + i*4
        let cell_resistance = ieee_float(get_32bit(data, i * 4 + 102));

        total_voltage += cell_voltage;
        if cell_voltage > 0.0 {
            average_cell_voltage += cell_voltage;
            cells_enabled += 1;
        }
        if cell_voltage > 0.0 && cell_voltage < min_cell_voltage {
            min_cell_voltage = cell_voltage;
            _min_voltage_cell = (i + 1) as u8;
        }
        if cell_voltage > max_cell_voltage {
            max_cell_voltage = cell_voltage;
            _max_voltage_cell = (i + 1) as u8;
        }
        pp.cellvolt[i] = cell_voltage;
        pp.cellres[i] = cell_resistance;
    }

    pp.cells = cells_enabled as i32;
    if cells_enabled > 0 {
        pp.cell_avg = average_cell_voltage / cells_enabled as f32;
    }
    pp.cell_min = min_cell_voltage;
    pp.cell_max = max_cell_voltage;
    pp.cell_diff = max_cell_voltage - min_cell_voltage;
    pp.voltage = total_voltage;

    // 222: Balancing current (IEEE 754 float)
    pp.balancing_current = ieee_float(get_32bit(data, 222));

    // 220: Balancing action (0=off, 1=charging, 2=discharging)
    pp.balancing = data.get(220).is_some_and(|&b| b != 0x00);

    // 286: Total runtime (3 bytes + padding?)
    pp.total_runtime = get_32bit(data, 286);

    // JK04 doesn't have the same rich set of fields at fixed offsets
    // for SOC/SOH/etc; those are less well-documented for this protocol
}

/// Parse cell info based on the protocol version
fn parse_cell_info(pp: &mut MybmmPack, data: &[u8]) {
    match pp.protocol_version {
        ProtocolVersion::Jk04 => parse_jk04_cell_info(pp, data),
        ProtocolVersion::Jk02_24S | ProtocolVersion::Jk02_32S => parse_jk02_cell_info(pp, data),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseFlags {
    pub got_res: bool,
    pub got_volts: bool,
    pub got_info: bool,
}

impl Default for ParseFlags {
    fn default() -> Self {
        Self::new()
    }
}

impl ParseFlags {
    pub fn new() -> Self {
        Self {
            got_res: false,
            got_volts: false,
            got_info: false,
        }
    }
}

/// Verify CRC of a 300-byte frame. CRC is sum of first 299 bytes, compared to byte at index 299.
fn verify_crc(data: &[u8]) -> bool {
    if data.len() < 300 {
        return false;
    }
    let computed = crc(data, 299);
    let expected = data[299];
    computed == expected
}

pub fn getdata(pp: &mut MybmmPack, data: &[u8]) -> ParseFlags {
    let mut flags = ParseFlags::new();
    let mut j = 0;
    let mut start = 0;
    let mut i = 0;

    while i < data.len() {
        let byte = data[i];
        if j < SIG_BYTES.len() && byte == SIG_BYTES[j] {
            if j == 0 {
                start = i;
            }
            j += 1;

            if j >= SIG_BYTES.len() {
                // Found full signature 55 AA EB 90 at data[start..start+4]
                // Subtype byte follows at data[i+1]
                if i + 1 < data.len() {
                    let subtype = data[i + 1];
                    // Determine required frame size based on subtype
                    let required_size = match subtype {
                        2 => JK02_FRAME_SIZE,
                        3 => JK02_FRAME_SIZE,
                        1 => JK02_FRAME_SIZE,
                        _ => 6,
                    };

                    if start + required_size <= data.len() {
                        let frame_data = &data[start..];

                        // Verify CRC before parsing
                        if !verify_crc(frame_data) {
                            log::warn!("CRC check failed on frame at offset {}", start);
                            i = start + PB2_FRAME_SIZE.min(data.len() - start);
                            j = 0;
                            continue;
                        }

                        match subtype {
                            3 => {
                                // Parse info to detect BMS version and extract device strings
                                if frame_data.len() > 10 {
                                    let model_end = frame_data[6..].iter().position(|&b| b == 0).unwrap_or(0);
                                    if model_end > 0 {
                                        let model = String::from_utf8_lossy(&frame_data[6..6 + model_end]);
                                        pp.protocol_version = ProtocolVersion::from_model(&model);
                                        pp.model = model.to_string();
                                    }
                                    // Parse hw/sw version strings (follow model after null)
                                    let mut pos = 6 + model_end + 1;
                                    // Skip null padding
                                    while pos < frame_data.len() && frame_data[pos] == 0 {
                                        pos += 1;
                                    }
                                    // Hardware version
                                    if pos < frame_data.len() {
                                        let end = frame_data[pos..].iter().position(|&b| b == 0).unwrap_or(0);
                                        if end > 0 {
                                            pp.hwvers = String::from_utf8_lossy(&frame_data[pos..pos + end]).to_string();
                                            pos += end + 1;
                                        }
                                    }
                                    // Skip null padding
                                    while pos < frame_data.len() && frame_data[pos] == 0 {
                                        pos += 1;
                                    }
                                    // Software version
                                    if pos < frame_data.len() {
                                        let end = frame_data[pos..].iter().position(|&b| b == 0).unwrap_or(0);
                                        if end > 0 {
                                            pp.swvers = String::from_utf8_lossy(&frame_data[pos..pos + end]).to_string();
                                        }
                                    }
                                }
                                flags.got_info = true;
                            }
                            1 => {
                                // Settings frame — parse based on protocol version
                                let settings = if pp.protocol_version.is_jk04() {
                                    parse_jk04_settings(frame_data)
                                } else {
                                    parse_jk02_settings(pp.protocol_version, frame_data)
                                };
                                pp.settings = Some(settings);
                                flags.got_res = true;
                            }
                            2 => {
                                parse_cell_info(pp, frame_data);
                                flags.got_volts = true;
                            }
                            _ => {}
                        }
                    }
                }

                // Skip past this frame's data to avoid re-matching within it
                i = start + PB2_FRAME_SIZE.min(data.len() - start);
                j = 0;
                continue;
            }
        } else {
            // Reset signature matcher if byte doesn't match
            // But check if this byte could be the start of a new signature
            if byte == SIG_BYTES[0] {
                j = 1;
                start = i;
            } else {
                j = 0;
            }
        }

        if flags.got_volts {
            break;
        }
        i += 1;
    }

    flags
}

pub fn get_info_command() -> [u8; 20] {
    [0xaa, 0x55, 0x90, 0xeb, 0x97, 0x00, 0x00, 0x00, 0x00, 0x00,
     0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x11]
}

pub fn get_cell_info_command() -> [u8; 20] {
    [0xaa, 0x55, 0x90, 0xeb, 0x96, 0x00, 0x00, 0x00, 0x00, 0x00,
     0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10]
}

/// Frame assembler for BLE MTU fragmentation.
///
/// JK BMS frames are 300 bytes, but BLE notifications are typically 20-200 bytes.
/// This accumulator mirrors the ESPHome `assemble()` function:
/// - Accumulate incoming notification fragments
/// - Flush buffer when a new preamble (55 AA EB 90) is seen
/// - Once buffer reaches 300+ bytes, verify CRC and decode
///
/// Usage:
/// ```ignore
/// let mut assembler = FrameAssembler::new();
/// // Feed each BLE notification:
/// assembler.feed(&notification_bytes);
/// // Try to decode:
/// if let Some(frame) = assembler.decode() {
///     let flags = getdata(&mut pack, &frame);
/// }
/// ```
#[derive(Debug, Clone)]
pub struct FrameAssembler {
    buffer: Vec<u8>,
}

impl FrameAssembler {
    pub fn new() -> Self {
        Self {
            buffer: Vec::with_capacity(MAX_RESPONSE_SIZE),
        }
    }

    /// Feed incoming BLE notification data into the assembler.
    /// If the data starts with the frame preamble, flush the buffer first
    /// (new frame starting, discard any partial previous frame).
    pub fn feed(&mut self, data: &[u8]) {
        // Drop buffer if it's grown too large (corrupted/stale)
        if self.buffer.len() > MAX_RESPONSE_SIZE {
            log::warn!("Frame assembler buffer overflow, clearing");
            self.buffer.clear();
        }

        // Flush buffer on every preamble (new frame starting)
        if data.len() >= 4 && data[0] == 0x55 && data[1] == 0xAA && data[2] == 0xEB && data[3] == 0x90 {
            self.buffer.clear();
        }

        self.buffer.extend_from_slice(data);
    }

    /// Try to extract and decode a complete frame.
    /// Returns the frame data if a complete, CRC-verified frame is available.
    /// The returned slice is the first 300 bytes of the assembled frame.
    pub fn try_decode(&mut self) -> Option<Vec<u8>> {
        if self.buffer.len() < JK02_FRAME_SIZE {
            return None;
        }

        // Verify CRC over the first 300 bytes
        if !verify_crc(&self.buffer) {
            log::warn!("Frame assembler CRC check failed, clearing buffer");
            self.buffer.clear();
            return None;
        }

        // Extract the frame and clear the buffer
        let frame: Vec<u8> = self.buffer.drain(..JK02_FRAME_SIZE).collect();
        Some(frame)
    }

    /// Convenience: feed data and try to decode in one call.
    /// Returns parsed flags if a frame was successfully decoded.
    pub fn feed_and_decode(&mut self, pp: &mut MybmmPack, data: &[u8]) -> Option<ParseFlags> {
        self.feed(data);
        self.try_decode().map(|frame| getdata(pp, &frame))
    }

    /// Clear the assembler buffer
    pub fn clear(&mut self) {
        self.buffer.clear();
    }

    /// Current buffer length (useful for debugging)
    pub fn buffer_len(&self) -> usize {
        self.buffer.len()
    }
}

impl Default for FrameAssembler {
    fn default() -> Self {
        Self::new()
    }
}

// ===========================================================================
// Settings support: register map, parsing, and write frame builder
// ===========================================================================

/// A single setting register definition.
/// Maps a human-readable name to protocol-specific register addresses,
/// with a scaling factor and byte length for encoding/decoding.
#[derive(Debug, Clone)]
pub struct SettingDef {
    pub name: &'static str,
    pub unit: &'static str,
    /// (JK04 register, JK02_24S register, JK02_32S register). 0 = not supported.
    pub registers: [u8; 3],
    /// Multiply value by this to get the raw integer for the wire.
    /// Divide raw integer by this to get the human-readable value.
    pub factor: f32,
    /// Number of value bytes in the write frame (1 or 4).
    pub length: u8,
    /// Whether this setting is a switch (bool) rather than a numeric value.
    pub is_switch: bool,
}

/// Complete register map for JK BMS settings.
/// Derived from ESPHome `NUMBERS` and `SWITCHES` tables.
pub const SETTINGS: &[SettingDef] = &[
    // Voltage settings
    SettingDef { name: "smart_sleep_voltage",            unit: "V",  registers: [0x00, 0x01, 0x01], factor: 1000.0, length: 4, is_switch: false },
    SettingDef { name: "cell_uvp",                       unit: "V",  registers: [0x00, 0x02, 0x02], factor: 1000.0, length: 4, is_switch: false },
    SettingDef { name: "cell_uvpr",                      unit: "V",  registers: [0x00, 0x03, 0x03], factor: 1000.0, length: 4, is_switch: false },
    SettingDef { name: "cell_ovp",                       unit: "V",  registers: [0x00, 0x04, 0x04], factor: 1000.0, length: 4, is_switch: false },
    SettingDef { name: "cell_ovpr",                      unit: "V",  registers: [0x00, 0x05, 0x05], factor: 1000.0, length: 4, is_switch: false },
    SettingDef { name: "balance_trigger_voltage",        unit: "V",  registers: [0x00, 0x06, 0x06], factor: 1000.0, length: 4, is_switch: false },
    SettingDef { name: "cell_soc100_voltage",            unit: "V",  registers: [0x00, 0x07, 0x07], factor: 1000.0, length: 4, is_switch: false },
    SettingDef { name: "cell_soc0_voltage",              unit: "V",  registers: [0x00, 0x08, 0x08], factor: 1000.0, length: 4, is_switch: false },
    SettingDef { name: "cell_request_charge_voltage",    unit: "V",  registers: [0x00, 0x09, 0x09], factor: 1000.0, length: 4, is_switch: false },
    SettingDef { name: "cell_request_float_voltage",     unit: "V",  registers: [0x00, 0x0A, 0x0A], factor: 1000.0, length: 4, is_switch: false },
    SettingDef { name: "power_off_voltage",              unit: "V",  registers: [0x00, 0x0B, 0x0B], factor: 1000.0, length: 4, is_switch: false },
    SettingDef { name: "balance_starting_voltage",       unit: "V",  registers: [0x00, 0x26, 0x22], factor: 1000.0, length: 4, is_switch: false },
    // Current settings
    SettingDef { name: "max_charge_current",             unit: "A",  registers: [0x00, 0x0C, 0x0C], factor: 1000.0, length: 4, is_switch: false },
    SettingDef { name: "max_discharge_current",          unit: "A",  registers: [0x00, 0x0F, 0x0F], factor: 1000.0, length: 4, is_switch: false },
    SettingDef { name: "max_balance_current",            unit: "A",  registers: [0x00, 0x13, 0x13], factor: 1000.0, length: 4, is_switch: false },
    // Timing settings
    SettingDef { name: "charge_ocp_delay",               unit: "s",  registers: [0x00, 0x0D, 0x0D], factor: 1.0,    length: 4, is_switch: false },
    SettingDef { name: "charge_ocp_recovery",            unit: "s",  registers: [0x00, 0x0E, 0x0E], factor: 1.0,    length: 4, is_switch: false },
    SettingDef { name: "discharge_ocp_delay",            unit: "s",  registers: [0x00, 0x10, 0x10], factor: 1.0,    length: 4, is_switch: false },
    SettingDef { name: "discharge_ocp_recovery",         unit: "s",  registers: [0x00, 0x11, 0x11], factor: 1.0,    length: 4, is_switch: false },
    SettingDef { name: "scp_recovery",                   unit: "s",  registers: [0x00, 0x12, 0x12], factor: 1.0,    length: 4, is_switch: false },
    SettingDef { name: "scp_delay",                      unit: "",   registers: [0x00, 0x25, 0x21], factor: 1.0,    length: 4, is_switch: false },
    // Temperature settings
    SettingDef { name: "charge_otp",                     unit: "°C", registers: [0x00, 0x14, 0x14], factor: 10.0,   length: 4, is_switch: false },
    SettingDef { name: "charge_otp_recovery",            unit: "°C", registers: [0x00, 0x15, 0x15], factor: 10.0,   length: 4, is_switch: false },
    SettingDef { name: "discharge_otp",                  unit: "°C", registers: [0x00, 0x16, 0x16], factor: 10.0,   length: 4, is_switch: false },
    SettingDef { name: "discharge_otp_recovery",         unit: "°C", registers: [0x00, 0x17, 0x17], factor: 10.0,   length: 4, is_switch: false },
    SettingDef { name: "charge_utp",                     unit: "°C", registers: [0x00, 0x18, 0x18], factor: 10.0,   length: 4, is_switch: false },
    SettingDef { name: "charge_utp_recovery",            unit: "°C", registers: [0x00, 0x19, 0x19], factor: 10.0,   length: 4, is_switch: false },
    SettingDef { name: "power_tube_otp",                 unit: "°C", registers: [0x00, 0x1A, 0x1A], factor: 10.0,   length: 4, is_switch: false },
    SettingDef { name: "power_tube_otp_recovery",        unit: "°C", registers: [0x00, 0x1B, 0x1B], factor: 10.0,   length: 4, is_switch: false },
    // JK02_32S only temperature settings
    SettingDef { name: "discharge_utp",                  unit: "°C", registers: [0x00, 0x00, 0x3A], factor: 1.0,    length: 1, is_switch: false },
    SettingDef { name: "discharge_utp_recovery",         unit: "°C", registers: [0x00, 0x00, 0x3B], factor: 1.0,    length: 1, is_switch: false },
    SettingDef { name: "heating_start_temperature",      unit: "°C", registers: [0x00, 0x00, 0x37], factor: 1.0,    length: 1, is_switch: false },
    SettingDef { name: "heating_stop_temperature",       unit: "°C", registers: [0x00, 0x00, 0x38], factor: 1.0,    length: 1, is_switch: false },
    // Cell / capacity config
    SettingDef { name: "cell_count",                     unit: "",   registers: [0x00, 0x1C, 0x1C], factor: 1.0,    length: 4, is_switch: false },
    SettingDef { name: "total_battery_capacity",         unit: "Ah", registers: [0x00, 0x20, 0x20], factor: 1000.0, length: 4, is_switch: false },
    // Calibration
    SettingDef { name: "voltage_calibration",            unit: "V",  registers: [0x00, 0x21, 0x64], factor: 1000.0, length: 4, is_switch: false },
    SettingDef { name: "current_calibration",            unit: "A",  registers: [0x00, 0x24, 0x67], factor: 1000.0, length: 4, is_switch: false },
    // JK02_32S only extra timing
    SettingDef { name: "discharge_precharge_time",       unit: "s",  registers: [0x00, 0x00, 0x25], factor: 1.0,    length: 4, is_switch: false },
    SettingDef { name: "cell_request_charge_voltage_time", unit: "h", registers: [0x00, 0x00, 0xB3], factor: 10.0, length: 1, is_switch: false },
    SettingDef { name: "cell_request_float_voltage_time",  unit: "h", registers: [0x00, 0x00, 0xB4], factor: 10.0, length: 1, is_switch: false },
    SettingDef { name: "re_bulk_soc",                    unit: "%",  registers: [0x00, 0x00, 0xB7], factor: 1.0,    length: 1, is_switch: false },
    // Switches
    SettingDef { name: "charging",                       unit: "",   registers: [0x00, 0x1D, 0x1D], factor: 1.0,    length: 4, is_switch: true },
    SettingDef { name: "discharging",                    unit: "",   registers: [0x00, 0x1E, 0x1E], factor: 1.0,    length: 4, is_switch: true },
    SettingDef { name: "balancer",                       unit: "",   registers: [0x6C, 0x1F, 0x1F], factor: 1.0,    length: 4, is_switch: true },
    // JK02_32S only switches
    SettingDef { name: "emergency",                      unit: "",   registers: [0x00, 0x00, 0x6B], factor: 1.0,    length: 4, is_switch: true },
    SettingDef { name: "heating",                        unit: "",   registers: [0x00, 0x00, 0x27], factor: 1.0,    length: 4, is_switch: true },
    SettingDef { name: "disable_temperature_sensors",    unit: "",   registers: [0x00, 0x00, 0x28], factor: 1.0,    length: 4, is_switch: true },
    SettingDef { name: "display_always_on",              unit: "",   registers: [0x00, 0x00, 0x2B], factor: 1.0,    length: 4, is_switch: true },
    SettingDef { name: "smart_sleep",                    unit: "",   registers: [0x00, 0x00, 0x2D], factor: 1.0,    length: 4, is_switch: true },
    SettingDef { name: "disable_pcl_module",             unit: "",   registers: [0x00, 0x00, 0x2E], factor: 1.0,    length: 4, is_switch: true },
    SettingDef { name: "timed_stored_data",              unit: "",   registers: [0x00, 0x00, 0x2F], factor: 1.0,    length: 4, is_switch: true },
    SettingDef { name: "charging_float_mode",            unit: "",   registers: [0x00, 0x00, 0x30], factor: 1.0,    length: 4, is_switch: true },
];

/// Get the register address for a given setting name and protocol version.
/// Returns None if the setting is not supported for this protocol version.
pub fn get_setting_register(name: &str, version: ProtocolVersion) -> Option<u8> {
    let idx = match version {
        ProtocolVersion::Jk04 => 0,
        ProtocolVersion::Jk02_24S => 1,
        ProtocolVersion::Jk02_32S => 2,
    };
    SETTINGS.iter().find(|s| s.name == name).map(|s| s.registers[idx]).filter(|&r| r != 0)
}

/// Look up a setting definition by name.
pub fn get_setting_def(name: &str) -> Option<&'static SettingDef> {
    SETTINGS.iter().find(|s| s.name == name)
}

/// Look up a setting definition by register address for a given protocol version.
pub fn get_setting_by_register(register: u8, version: ProtocolVersion) -> Option<&'static SettingDef> {
    let idx = match version {
        ProtocolVersion::Jk04 => 0,
        ProtocolVersion::Jk02_24S => 1,
        ProtocolVersion::Jk02_32S => 2,
    };
    SETTINGS.iter().find(|s| s.registers[idx] == register)
}

/// Parse JK02 settings frame (frame type 0x01).
/// Matches ESPHome `decode_jk02_settings_()` offsets.
fn parse_jk02_settings(version: ProtocolVersion, data: &[u8]) -> JkSettings {
    let mut s = JkSettings::new();
    s.raw_frame = data[..data.len().min(300)].to_vec();

    // Byte offsets from ESPHome decode_jk02_settings_():
    // 6:   Smart sleep voltage (4 bytes, *0.001 V)
    s.smart_sleep_voltage = get_32bit(data, 6) as f32 * 0.001;
    // 10:  Cell UVP
    s.cell_uvp = get_32bit(data, 10) as f32 * 0.001;
    // 14:  Cell UVPR
    s.cell_uvpr = get_32bit(data, 14) as f32 * 0.001;
    // 18:  Cell OVP
    s.cell_ovp = get_32bit(data, 18) as f32 * 0.001;
    // 22:  Cell OVPR
    s.cell_ovpr = get_32bit(data, 22) as f32 * 0.001;
    // 26:  Balance trigger voltage
    s.balance_trigger_voltage = get_32bit(data, 26) as f32 * 0.001;
    // 30:  SOC 100% voltage
    s.cell_soc100_voltage = get_32bit(data, 30) as f32 * 0.001;
    // 34:  SOC 0% voltage
    s.cell_soc0_voltage = get_32bit(data, 34) as f32 * 0.001;
    // 38:  Requested charge voltage (RCV)
    s.cell_request_charge_voltage = get_32bit(data, 38) as f32 * 0.001;
    // 42:  Requested float voltage (RFV)
    s.cell_request_float_voltage = get_32bit(data, 42) as f32 * 0.001;
    // 46:  Power off voltage
    s.power_off_voltage = get_32bit(data, 46) as f32 * 0.001;
    // 50:  Max charge current
    s.max_charge_current = get_32bit(data, 50) as f32 * 0.001;
    // 54:  Charge OCP delay
    s.charge_ocp_delay = get_32bit(data, 54) as f32;
    // 58:  Charge OCP recovery time
    s.charge_ocp_recovery = get_32bit(data, 58) as f32;
    // 62:  Max discharge current
    s.max_discharge_current = get_32bit(data, 62) as f32 * 0.001;
    // 66:  Discharge OCP delay
    s.discharge_ocp_delay = get_32bit(data, 66) as f32;
    // 70:  Discharge OCP recovery time
    s.discharge_ocp_recovery = get_32bit(data, 70) as f32;
    // 74:  SCP recovery time
    s.scp_recovery = get_32bit(data, 74) as f32;
    // 78:  Max balance current
    s.max_balance_current = get_32bit(data, 78) as f32 * 0.001;
    // 82:  Charge OTP (*0.1 °C)
    s.charge_otp = get_32bit(data, 82) as f32 * 0.1;
    // 86:  Charge OTP recovery
    s.charge_otp_recovery = get_32bit(data, 86) as f32 * 0.1;
    // 90:  Discharge OTP
    s.discharge_otp = get_32bit(data, 90) as f32 * 0.1;
    // 94:  Discharge OTP recovery
    s.discharge_otp_recovery = get_32bit(data, 94) as f32 * 0.1;
    // 98:  Charge UTP (signed, *0.1 °C)
    s.charge_utp = get_32bit(data, 98) as i32 as f32 * 0.1;
    // 102: Charge UTP recovery (signed, *0.1 °C)
    s.charge_utp_recovery = get_32bit(data, 102) as i32 as f32 * 0.1;
    // 106: MOSFET OTP (signed, *0.1 °C)
    s.power_tube_otp = get_32bit(data, 106) as i32 as f32 * 0.1;
    // 110: MOSFET OTP recovery (signed, *0.1 °C)
    s.power_tube_otp_recovery = get_32bit(data, 110) as i32 as f32 * 0.1;
    // 114: Cell count (single byte)
    s.cell_count = data.get(114).copied().unwrap_or(0);
    // 118: Charge switch
    s.charging_switch = data.get(118).is_some_and(|&b| b != 0);
    // 122: Discharge switch
    s.discharging_switch = data.get(122).is_some_and(|&b| b != 0);
    // 126: Balancer switch
    s.balancer_switch = data.get(126).is_some_and(|&b| b != 0);
    // 130: Nominal battery capacity (*0.001 Ah)
    s.total_battery_capacity = get_32bit(data, 130) as f32 * 0.001;
    // 134: SCP delay (JK02_24S: μs, JK02_32S: μs)
    s.scp_delay = get_32bit(data, 134) as f32;
    // 138: Start balance voltage (*0.001 V)
    s.balance_starting_voltage = get_32bit(data, 138) as f32 * 0.001;

    if version == ProtocolVersion::Jk02_24S {
        // 142-153: Unknown (4 x 4 bytes)
        // 158-253: Connection wire resistances 1-24 (*0.001 Ohm)
        for i in 0..24 {
            s.wire_resistance[i] = get_32bit(data, 158 + i * 4) as f32 * 0.001;
        }
    } else {
        // JK02_32S
        // 142-269: Connection wire resistances 1-32 (*0.001 Ohm)
        for i in 0..32 {
            s.wire_resistance[i] = get_32bit(data, 142 + i * 4) as f32 * 0.001;
        }
        // 270: Device address
        // 274: Precharge time (1 byte)
        s.discharge_precharge_time = data.get(274).copied().unwrap_or(0);

        // 282-283: New controls bitmask
        let ctrl_lo = data.get(282).copied().unwrap_or(0);
        let ctrl_hi = data.get(283).copied().unwrap_or(0);
        s.heating_switch = (ctrl_lo & 0x01) != 0;
        s.disable_temp_sensors = (ctrl_lo & 0x02) != 0;
        // bit 2: GPS Heartbeat
        // bit 3: Port switch (RS485 vs CAN)
        s.display_always_on = (ctrl_lo & 0x10) != 0;
        // bit 5: Special charger
        s.smart_sleep_switch = (ctrl_lo & 0x40) != 0;
        s.disable_pcl_module = (ctrl_lo & 0x80) != 0;
        s.timed_stored_data = (ctrl_hi & 0x01) != 0;
        s.charging_float_mode = (ctrl_hi & 0x02) != 0;

        // 284: Heating start temperature (i8)
        s.heating_start_temperature = data.get(284).map_or(0.0, |&b| b as i8 as f32);
        // 285: Heating stop temperature (i8)
        s.heating_stop_temperature = data.get(285).map_or(0.0, |&b| b as i8 as f32);

        // 296: Discharge UTP (i8)
        s.discharge_utp = data.get(296).map_or(0.0, |&b| b as i8 as f32);
        // 297: Discharge UTP recovery (i8)
        s.discharge_utp_recovery = data.get(297).map_or(0.0, |&b| b as i8 as f32);

        // Voltage calibration at offset derived from register 0x64 mapping
        // (not at a fixed settings frame offset; this is a write-only register)
    }

    s
}

/// Parse JK04 settings frame (frame type 0x01).
/// JK04 settings use IEEE 754 floats for many values.
fn parse_jk04_settings(data: &[u8]) -> JkSettings {
    let mut s = JkSettings::new();
    s.raw_frame = data[..data.len().min(300)].to_vec();

    // 34: Cell count (1 byte)
    s.cell_count = data.get(34).copied().unwrap_or(0);

    // 38: Power off voltage (IEEE 754 float)
    s.power_off_voltage = ieee_float(get_32bit(data, 38));

    // 98: Start balance voltage (IEEE 754 float)
    s.balance_starting_voltage = ieee_float(get_32bit(data, 98));

    // 106: Trigger delta voltage (IEEE 754 float)
    s.balance_trigger_voltage = ieee_float(get_32bit(data, 106));

    // 110: Max balance current (IEEE 754 float)
    s.max_balance_current = ieee_float(get_32bit(data, 110));

    // 114: Balancer switch (1 byte)
    s.balancer_switch = data.get(114).is_some_and(|&b| b != 0);

    s
}

/// Build a 20-byte write-register frame to send to the BMS.
/// Matches ESPHome `write_register()`:
///   AA 55 90 EB [register] [length] [value_lo..hi] [00 x9] [CRC]
///
/// The header bytes are in the **write** order (AA 55 90 EB),
/// which is the reverse of the read/response header (55 AA EB 90).
pub fn build_write_frame(register: u8, value: u32, length: u8) -> [u8; 20] {
    let mut frame = [0u8; 20];
    frame[0] = 0xAA;
    frame[1] = 0x55;
    frame[2] = 0x90;
    frame[3] = 0xEB;
    frame[4] = register;
    frame[5] = length;
    frame[6] = value as u8;
    frame[7] = (value >> 8) as u8;
    frame[8] = (value >> 16) as u8;
    frame[9] = (value >> 24) as u8;
    // bytes 10-18 are zero (already)
    frame[19] = crc(&frame, 19);
    frame
}

/// Build a write frame for a named setting with a human-readable value.
/// Returns the 20-byte frame, or None if the setting is not supported
/// for this protocol version.
pub fn build_setting_write_frame(name: &str, value: &str, version: ProtocolVersion) -> Option<[u8; 20]> {
    let def = get_setting_def(name)?;

    let idx = match version {
        ProtocolVersion::Jk04 => 0,
        ProtocolVersion::Jk02_24S => 1,
        ProtocolVersion::Jk02_32S => 2,
    };

    let register = def.registers[idx];
    if register == 0 {
        return None; // Not supported for this protocol version
    }

    let raw_value: u32 = if def.is_switch {
        // Parse boolean
        let b = value.parse::<bool>().unwrap_or_else(|_| {
            // Also accept 1/0, on/off, true/false
            match value.to_lowercase().as_str() {
                "1" | "on" | "true" | "yes" => true,
                "0" | "off" | "false" | "no" => false,
                _ => false,
            }
        });
        if b { 1 } else { 0 }
    } else {
        // Parse numeric: multiply by factor to get raw integer
        let v: f32 = value.parse().ok()?;
        (v * def.factor) as i32 as u32
    };

    Some(build_write_frame(register, raw_value, def.length))
}

/// Command to request settings frame from the BMS.
/// This is the same as the cell info command (0x96) — the BMS responds
/// with a settings frame (type 0x01) when it has settings data to send.
/// In practice, the BMS sends settings frames automatically in response
/// to the cell info request, interleaved with cell info frames.
pub fn get_settings_command() -> [u8; 20] {
    // Same format as cell info request; the BMS may respond with frame type 0x01
    [0xaa, 0x55, 0x90, 0xeb, 0x96, 0x00, 0x00, 0x00, 0x00, 0x00,
     0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10]
}

// ===========================================================================
// CAN Protocol Support
// ===========================================================================

/// CAN frame sizes for JK BMS
/// JK BMS CAN protocol uses 8-byte frames (standard CAN MTU)
pub const CAN_FRAME_SIZE: usize = 8;

/// JK BMS CAN command header
/// CAN frames start with command byte, followed by data
pub const CAN_CMD_INFO: u8 = 0x97;
pub const CAN_CMD_CELL_INFO: u8 = 0x96;
pub const CAN_CMD_WRITE_REG: u8 = 0x05;

/// Build a CAN command frame for JK BMS.
/// CAN frames are 8 bytes: [cmd][data...][padding]
pub fn build_can_command(cmd: u8, data: &[u8]) -> [u8; 8] {
    let mut frame = [0u8; 8];
    frame[0] = cmd;
    let len = data.len().min(7);
    frame[1..1 + len].copy_from_slice(&data[..len]);
    frame
}

/// Get info command for CAN (8 bytes)
pub fn get_can_info_command() -> [u8; 8] {
    build_can_command(CAN_CMD_INFO, &[])
}

/// Get cell info command for CAN (8 bytes)
pub fn get_can_cell_info_command() -> [u8; 8] {
    build_can_command(CAN_CMD_CELL_INFO, &[])
}

/// Build a CAN write register command
/// Format: [0x05][register][length][value_bytes...][padding]
pub fn build_can_write_frame(register: u8, value: u32, length: u8) -> [u8; 8] {
    let mut frame = [0u8; 8];
    frame[0] = CAN_CMD_WRITE_REG;
    frame[1] = register;
    frame[2] = length;
    
    // Value is sent as little-endian bytes
    let value_bytes = value.to_le_bytes();
    let copy_len = (length.min(5) as usize).min(5); // Max 5 bytes for value (8 - 3 header bytes)
    frame[3..3 + copy_len].copy_from_slice(&value_bytes[..copy_len]);
    
    frame
}

/// Build a CAN setting write frame
pub fn build_can_setting_write_frame(name: &str, value: &str, version: ProtocolVersion) -> Option<[u8; 8]> {
    let def = get_setting_def(name)?;
    
    let idx = match version {
        ProtocolVersion::Jk04 => 0,
        ProtocolVersion::Jk02_24S => 1,
        ProtocolVersion::Jk02_32S => 2,
    };
    
    let register = def.registers[idx];
    if register == 0 {
        return None;
    }
    
    let raw_value: u32 = if def.is_switch {
        let b = value.parse::<bool>().unwrap_or_else(|_| {
            match value.to_lowercase().as_str() {
                "1" | "on" | "true" | "yes" => true,
                "0" | "off" | "false" | "no" => false,
                _ => false,
            }
        });
        if b { 1 } else { 0 }
    } else {
        let v: f32 = value.parse().ok()?;
        (v * def.factor) as i32 as u32
    };
    
    Some(build_can_write_frame(register, raw_value, def.length))
}
