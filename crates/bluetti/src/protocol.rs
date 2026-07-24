//! Bluetti register map (Modbus RTU over BLE, function 0x03 read / 0x06 write).
//! Ported from `warhammerkid/bluetti_mqtt` (AC200M-family "page 0" layout,
//! shared by most plaintext models). Offsets index the verified frame
//! (`addr, func, byte_count, data…`), so register `R` in a block starting at
//! `start` is at byte `3 + (R - start) * 2`.

use modbus_lite::be_u16;

/// Core status block: device type, powers, battery %, AC/DC output state.
pub const CORE_START: u16 = 0x0A; // 10
pub const CORE_COUNT: u16 = 0x3C; // 60 → regs 10..69
/// Battery block: pack voltage and per-cell voltages.
pub const BATTERY_START: u16 = 0x5A; // 90
pub const BATTERY_COUNT: u16 = 0x20; // 32 → regs 90..121

/// Control registers (function 0x06 writes).
pub const CTRL_AC_OUTPUT: u16 = 3007;
pub const CTRL_DC_OUTPUT: u16 = 3008;

fn reg(frame: &[u8], start: u16, addr: u16) -> u16 {
    be_u16(frame, 3 + (addr - start) as usize * 2)
}

/// A decoded snapshot.
#[derive(Debug, Clone, Default)]
pub struct BluettiData {
    pub device_type: Option<String>,
    pub dc_input_power: u16,        // W
    pub ac_input_power: u16,        // W
    pub ac_output_power: u16,       // W
    pub dc_output_power: u16,       // W
    pub total_battery_percent: u16, // %
    pub ac_output_on: bool,
    pub dc_output_on: bool,
    pub total_battery_voltage: f32, // V
    pub pack_voltage: f32,          // V
    pub cells: Vec<f32>,            // per-cell V
}

impl BluettiData {
    pub fn input_power(&self) -> u16 {
        self.dc_input_power + self.ac_input_power
    }
    pub fn output_power(&self) -> u16 {
        self.ac_output_power + self.dc_output_power
    }
}

/// Decode the core status block (`CORE_START`).
pub fn decode_core(frame: &[u8], out: &mut BluettiData) {
    // device_type: 6 registers of ASCII at reg 10.
    let base = 3 + (10 - CORE_START) as usize * 2;
    if frame.len() >= base + 12 {
        let s = String::from_utf8_lossy(&frame[base..base + 12])
            .trim_matches(|c: char| c == '\0' || c.is_whitespace())
            .to_string();
        out.device_type = (!s.is_empty()).then_some(s);
    }
    let r = |a: u16| reg(frame, CORE_START, a);
    out.dc_input_power = r(36);
    out.ac_input_power = r(37);
    out.ac_output_power = r(38);
    out.dc_output_power = r(39);
    out.total_battery_percent = r(43);
    out.ac_output_on = r(48) != 0;
    out.dc_output_on = r(49) != 0;
}

/// Decode the battery block (`BATTERY_START`).
pub fn decode_battery(frame: &[u8], out: &mut BluettiData) {
    let r = |a: u16| reg(frame, BATTERY_START, a);
    out.total_battery_voltage = r(92) as f32 * 0.01; // scale 2
    out.pack_voltage = r(98) as f32 * 0.01;
    out.cells = (0..16)
        .map(|i| r(105 + i))
        .filter(|&v| v != 0)
        .map(|v| v as f32 * 0.01)
        .collect();
}

#[cfg(test)]
mod tests {
    use super::*;
    use modbus_lite::crc16;

    /// Build a verified-shaped response for a block starting at `start`.
    fn frame(start: u16, regs: &[(u16, u16)], count: u16) -> Vec<u8> {
        let mut data = vec![0u8; count as usize * 2];
        for &(a, v) in regs {
            let off = (a - start) as usize * 2;
            data[off] = (v >> 8) as u8;
            data[off + 1] = v as u8;
        }
        let mut f = vec![0x01u8, 0x03, (count as usize * 2) as u8];
        f.extend_from_slice(&data);
        let crc = crc16(&f);
        f.push(crc as u8);
        f.push((crc >> 8) as u8);
        f
    }

    #[test]
    fn decode_core_block() {
        // "AC200M" at reg 10 (6 words), powers, 83%, AC on / DC off.
        let mut regs = vec![
            (36, 120), // dc in
            (37, 0),   // ac in
            (38, 350), // ac out
            (39, 40),  // dc out
            (43, 83),  // soc
            (48, 1),   // ac on
            (49, 0),   // dc off
        ];
        for (i, b) in "AC200M".bytes().enumerate() {
            let a = 10 + (i / 2) as u16;
            let cur = regs.iter().find(|(x, _)| *x == a).map(|(_, v)| *v).unwrap_or(0);
            let v = if i % 2 == 0 {
                (b as u16) << 8 | (cur & 0xff)
            } else {
                (cur & 0xff00) | b as u16
            };
            regs.retain(|(x, _)| *x != a);
            regs.push((a, v));
        }
        let f = frame(CORE_START, &regs, CORE_COUNT);
        let body = modbus_lite::verify(&f).unwrap();
        let mut d = BluettiData::default();
        decode_core(body, &mut d);
        assert_eq!(d.device_type.as_deref(), Some("AC200M"));
        assert_eq!(d.total_battery_percent, 83);
        assert_eq!(d.ac_output_power, 350);
        assert_eq!(d.output_power(), 390);
        assert_eq!(d.input_power(), 120);
        assert!(d.ac_output_on && !d.dc_output_on);
    }

    #[test]
    fn decode_battery_block() {
        let mut regs = vec![(92, 5236), (98, 5236)];
        for i in 0..4 {
            regs.push((105 + i, 327 + i)); // ~3.27 V cells (scale 2)
        }
        let f = frame(BATTERY_START, &regs, BATTERY_COUNT);
        let body = modbus_lite::verify(&f).unwrap();
        let mut d = BluettiData::default();
        decode_battery(body, &mut d);
        assert!((d.total_battery_voltage - 52.36).abs() < 1e-2);
        assert_eq!(d.cells.len(), 4);
        assert!((d.cells[0] - 3.27).abs() < 1e-2);
    }
}
