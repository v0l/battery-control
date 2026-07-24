//! Seplos BMS V3 register map (RS485, Modbus RTU function 0x04 = read input
//! registers). Ported from `marcelrv/seplosBMSv3`. Two blocks are read:
//! PIA (`0x1000`×18, status) and PIB (`0x1100`×26, cells + temps). Offsets
//! index the verified frame (`unit, func, byte_count, data…`), so register `r`
//! is at byte `3 + r*2`. Temperatures are 0.1 K on the wire.

use modbus_lite::{be_i16, be_u16};

/// Pack Info A: basic status.
pub const PIA_START: u16 = 0x1000;
pub const PIA_COUNT: u16 = 0x12; // 18
/// Pack Info B: cell voltages + temperatures.
pub const PIB_START: u16 = 0x1100;
pub const PIB_COUNT: u16 = 0x1A; // 26

fn u16r(frame: &[u8], r: u16) -> u16 {
    be_u16(frame, 3 + r as usize * 2)
}
fn i16r(frame: &[u8], r: u16) -> i16 {
    be_i16(frame, 3 + r as usize * 2)
}
/// 0.1 K raw → °C.
fn temp_c(raw: u16) -> f32 {
    raw as f32 * 0.1 - 273.15
}

/// A decoded pack snapshot.
#[derive(Debug, Clone, Default)]
pub struct SeplosData {
    pub voltage: f32,      // V
    pub current: f32,      // A (+ charge / − discharge)
    pub power: f32,        // W
    pub remaining_ah: f32, // Ah
    pub total_ah: f32,     // Ah
    pub soc: f32,          // %
    pub soh: f32,          // %
    pub cycles: u16,
    pub cells: Vec<f32>,      // per-cell V
    pub cell_temps: Vec<f32>, // °C (cell temp sensors 1..4)
    pub ambient_temp: f32,    // °C
    pub power_temp: f32,      // °C
}

/// Decode a verified PIA block (registers `0x1000..`).
pub fn decode_pia(frame: &[u8], out: &mut SeplosData) {
    out.voltage = u16r(frame, 0) as f32 * 0.01;
    out.current = i16r(frame, 1) as f32 * 0.01;
    out.power = out.voltage * out.current;
    out.remaining_ah = u16r(frame, 2) as f32 * 0.01;
    out.total_ah = u16r(frame, 3) as f32 * 0.01;
    out.soc = u16r(frame, 5) as f32 * 0.1;
    out.soh = u16r(frame, 6) as f32 * 0.1;
    out.cycles = u16r(frame, 7);
}

/// Decode a verified PIB block (registers `0x1100..`).
pub fn decode_pib(frame: &[u8], out: &mut SeplosData) {
    // 16 cell voltages (mV). Cells above the pack's series count read 0.
    out.cells = (0..16)
        .map(|r| u16r(frame, r))
        .filter(|&mv| mv != 0)
        .map(|mv| mv as f32 * 0.001)
        .collect();
    // Cell temperature sensors 1..4 (the app-displayed ones) at regs 16..19.
    out.cell_temps = (16..20).map(|r| temp_c(u16r(frame, r))).collect();
    out.ambient_temp = temp_c(u16r(frame, 24));
    out.power_temp = temp_c(u16r(frame, 25));
}

#[cfg(test)]
mod tests {
    use super::*;
    use modbus_lite::crc16;

    /// Wrap register values into a verified-shaped 0x04 response frame.
    fn frame(regs: &[u16]) -> Vec<u8> {
        let mut f = vec![0x00u8, 0x04, (regs.len() * 2) as u8];
        for &v in regs {
            f.push((v >> 8) as u8);
            f.push(v as u8);
        }
        let crc = crc16(&f);
        f.push(crc as u8);
        f.push((crc >> 8) as u8);
        f
    }

    #[test]
    fn decode_real_pia() {
        // Captured from a real BMS16S200A (marcelrv/seplosBMSv3 README).
        let regs = [
            5236, 1301, 3800, 30400, 64, 125, 1000, 2, 3272, 2837, 3275, 3268, 2845, 2831, 0,
            180, 180, 1000,
        ];
        let f = frame(&regs);
        let body = modbus_lite::verify(&f).unwrap();
        let mut d = SeplosData::default();
        decode_pia(body, &mut d);
        assert!((d.voltage - 52.36).abs() < 1e-2);
        assert!((d.current - 13.01).abs() < 1e-2);
        assert!((d.remaining_ah - 38.0).abs() < 1e-2);
        assert!((d.total_ah - 304.0).abs() < 1e-2);
        assert!((d.soc - 12.5).abs() < 1e-2);
        assert!((d.soh - 100.0).abs() < 1e-2);
        assert_eq!(d.cycles, 2);
    }

    #[test]
    fn decode_real_pib() {
        let regs = [
            3273, 3273, 3268, 3272, 3274, 3274, 3275, 3275, 3274, 3273, 3273, 3270, 3271, 3271,
            3270, 3272, 2842, 2833, 2831, 2845, 2731, 2731, 2731, 2731, 2833, 2829,
        ];
        let f = frame(&regs);
        let body = modbus_lite::verify(&f).unwrap();
        let mut d = SeplosData::default();
        decode_pib(body, &mut d);
        assert_eq!(d.cells.len(), 16);
        assert!((d.cells[0] - 3.273).abs() < 1e-3);
        assert_eq!(d.cell_temps.len(), 4);
        assert!((d.cell_temps[0] - 11.05).abs() < 1e-2); // 2842 * 0.1 - 273.15
        assert!((d.ambient_temp - 10.15).abs() < 1e-2); // 2833
        assert!((d.power_temp - 9.75).abs() < 1e-2); // 2829
    }
}
