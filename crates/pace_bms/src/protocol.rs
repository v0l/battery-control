//! PACE-BMS `PACE_MODBUS` register map (RS485, Modbus RTU function 0x03).
//! Ported from `syssi/esphome-pace-bms`. Offsets index the verified response
//! frame (`unit, func, byte_count, data…`), so register `r` is at byte `3 + r*2`.

use modbus_lite::{be_i16, be_u16};

/// One read covers registers 0..=36 (current, voltage, SOC, capacity, flags,
/// 16 cells, temps).
pub const READ_START: u16 = 0;
pub const READ_COUNT: u16 = 37;

fn reg_u16(frame: &[u8], r: u16) -> u16 {
    be_u16(frame, 3 + r as usize * 2)
}
fn reg_i16(frame: &[u8], r: u16) -> i16 {
    be_i16(frame, 3 + r as usize * 2)
}

/// Sentinel used by PACE for an unpopulated/invalid signed field.
const INVALID_I16: i16 = i16::MIN; // 0x8000

/// A decoded pack snapshot.
#[derive(Debug, Clone, Default)]
pub struct PaceData {
    pub voltage: f32,      // V
    pub current: f32,      // A (+ charge / − discharge)
    pub power: f32,        // W
    pub soc: u16,          // %
    pub soh: u16,          // %
    pub remaining_ah: f32, // Ah
    pub full_ah: f32,      // Ah
    pub design_ah: f32,    // Ah
    pub cycles: u16,
    pub cells: Vec<f32>,             // V, valid cells only
    pub temps: Vec<f32>,             // °C, battery probes
    pub mosfet_temp: Option<f32>,    // °C
    pub environment_temp: Option<f32>, // °C
    pub charging: bool,
    pub discharging: bool,
    pub balancing: bool,
    pub warning_flags: u16,
    pub protection_flags: u16,
    pub status_flags: u16,
}

impl PaceData {
    /// Human-readable alarms from the warning + protection bitfields.
    pub fn alarms(&self) -> Vec<String> {
        let mut v = Vec::new();
        if self.warning_flags != 0 {
            v.push(format!("warning flags {:#06x}", self.warning_flags));
        }
        if self.protection_flags != 0 {
            v.push(format!("protection flags {:#06x}", self.protection_flags));
        }
        v
    }
}

/// Decode a verified PACE_MODBUS response (registers 0..=36).
pub fn decode(frame: &[u8]) -> PaceData {
    let voltage = reg_u16(frame, 1) as f32 * 0.01;
    let current = reg_i16(frame, 0) as f32 * 0.01;

    // 16 possible cells at registers 15..=30; unpopulated read as 0 / 0xFFFF.
    let cells: Vec<f32> = (15..=30)
        .map(|r| reg_u16(frame, r))
        .filter(|&mv| mv != 0 && mv != 0xFFFF)
        .map(|mv| mv as f32 * 0.001)
        .collect();

    // Battery temps registers 31..=34. An exact 0 / 0x8000 marks an
    // unpopulated sensor (real probes read tenths of a degree, e.g. 250 =
    // 25.0 °C), so those are dropped rather than shown as phantom 0 °C.
    let temps: Vec<f32> = (31..=34)
        .map(|r| reg_i16(frame, r))
        .filter(|&t| t != INVALID_I16 && t != 0)
        .map(|t| t as f32 * 0.1)
        .collect();

    let opt_temp = |r: u16| {
        let t = reg_i16(frame, r);
        (t != INVALID_I16).then_some(t as f32 * 0.1)
    };

    let status = reg_u16(frame, 11);
    PaceData {
        voltage,
        current,
        power: voltage * current,
        soc: reg_u16(frame, 2),
        soh: reg_u16(frame, 3),
        remaining_ah: reg_u16(frame, 4) as f32 * 0.01,
        full_ah: reg_u16(frame, 5) as f32 * 0.01,
        design_ah: reg_u16(frame, 6) as f32 * 0.01,
        cycles: reg_u16(frame, 7),
        cells,
        temps,
        mosfet_temp: opt_temp(35),
        environment_temp: opt_temp(36),
        // Status/Fault flag (reg 11): bit 8 charging, bit 9 discharging.
        charging: status & 0x0100 != 0,
        discharging: status & 0x0200 != 0,
        balancing: reg_u16(frame, 12) != 0,
        warning_flags: reg_u16(frame, 9),
        protection_flags: reg_u16(frame, 10),
        status_flags: status,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use modbus_lite::crc16;

    /// Build a verified-shaped response covering registers 0..count.
    fn frame(regs: &[(u16, u16)], count: u16) -> Vec<u8> {
        let mut data = vec![0u8; count as usize * 2];
        for &(r, v) in regs {
            let off = r as usize * 2;
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
    fn decode_pack() {
        let f = frame(
            &[
                (0, 150),            // current +1.50 A
                (1, 1329),           // 13.29 V
                (2, 87),             // soc
                (3, 99),             // soh
                (4, 5000),           // remaining 50.00 Ah
                (5, 10000),          // full 100.00 Ah
                (7, 42),             // cycles
                (11, 0x0100),        // status: charging
                (15, 3320),
                (16, 3321),
                (17, 3319),
                (18, 3322),          // 4 cells; 19..30 stay 0 → filtered
                (31, 250),           // temp 25.0 °C
                (32, 251),
                (35, 300),           // mosfet 30.0 °C
                (36, 0x8000),        // environment invalid
            ],
            READ_COUNT,
        );
        let body = modbus_lite::verify(&f).unwrap();
        let d = decode(body);
        assert!((d.voltage - 13.29).abs() < 1e-2);
        assert!((d.current - 1.50).abs() < 1e-2);
        assert_eq!(d.soc, 87);
        assert_eq!(d.soh, 99);
        assert!((d.remaining_ah - 50.0).abs() < 1e-2);
        assert!((d.full_ah - 100.0).abs() < 1e-2);
        assert_eq!(d.cycles, 42);
        assert_eq!(d.cells.len(), 4);
        assert!((d.cells[0] - 3.320).abs() < 1e-3);
        assert_eq!(d.temps.len(), 2);
        assert!((d.mosfet_temp.unwrap() - 30.0).abs() < 1e-2);
        assert_eq!(d.environment_temp, None);
        assert!(d.charging && !d.discharging);
    }
}
