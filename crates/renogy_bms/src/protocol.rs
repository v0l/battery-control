//! Renogy smart-battery register map. Ported from `cyrils/renogy-bt`
//! (`BatteryClient`). Modbus function 0x03; offsets are into the verified
//! response frame (`unit, func, byte_count, data…`), so data starts at byte 3.

use modbus_lite::{be_i16, be_u16, be_u32};

/// Default broadcast device id for a stand-alone battery. Hub/daisy-chained
/// packs use 48/49/50 — override via [`crate::RenogyBms::connect_ble_as`].
pub const DEFAULT_UNIT: u8 = 0xFF;

/// Register blocks to read (start register, word count).
pub const SECTIONS: [(u16, u16); 4] = [
    (5000, 17), // cell voltages
    (5017, 17), // cell temperatures
    (5042, 6),  // current / voltage / remaining / capacity
    (5122, 8),  // model string
];

/// A decoded battery snapshot.
#[derive(Debug, Clone, Default)]
pub struct RenogyData {
    pub voltage: f32,        // V
    pub current: f32,        // A (+ charge / − discharge)
    pub power: f32,          // W
    pub remaining_ah: f32,   // Ah
    pub capacity_ah: f32,    // Ah
    pub soc: f32,            // % (derived from remaining/capacity)
    pub cells: Vec<f32>,     // per-cell V
    pub temps: Vec<f32>,     // °C
    pub model: Option<String>,
}

impl RenogyData {
    pub fn recompute(&mut self) {
        self.power = self.voltage * self.current;
        self.soc = if self.capacity_ah > 0.0 {
            (self.remaining_ah / self.capacity_ah * 100.0).clamp(0.0, 100.0)
        } else {
            0.0
        };
    }
}

/// Cell voltages block (register 5000): count at byte 3, then u16 ×0.1 each.
pub fn parse_cells(frame: &[u8]) -> Vec<f32> {
    let count = (be_u16(frame, 3) as usize).min(32);
    (0..count)
        .map(|i| be_u16(frame, 5 + i * 2) as f32 * 0.1)
        .collect()
}

/// Cell temperature block (register 5017): count at byte 3, then i16 ×0.1 °C.
pub fn parse_temps(frame: &[u8]) -> Vec<f32> {
    let count = (be_u16(frame, 3) as usize).min(32);
    (0..count)
        .map(|i| be_i16(frame, 5 + i * 2) as f32 * 0.1)
        .collect()
}

/// Battery info block (register 5042).
pub fn parse_info(frame: &[u8]) -> (f32, f32, f32, f32) {
    let current = be_i16(frame, 3) as f32 * 0.01;
    let voltage = be_u16(frame, 5) as f32 * 0.1;
    let remaining = be_u32(frame, 7) as f32 * 0.001;
    let capacity = be_u32(frame, 11) as f32 * 0.001;
    (current, voltage, remaining, capacity)
}

/// Model string block (register 5122): 16 ASCII bytes at offset 3.
pub fn parse_model(frame: &[u8]) -> Option<String> {
    let end = (3 + 16).min(frame.len());
    if end <= 3 {
        return None;
    }
    let s = String::from_utf8_lossy(&frame[3..end])
        .trim_matches(|c: char| c == '\0' || c.is_whitespace())
        .to_string();
    (!s.is_empty()).then_some(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use modbus_lite::{build_read, crc16, verify};

    fn frame(unit: u8, data: &[u8]) -> Vec<u8> {
        let mut f = vec![unit, 0x03, data.len() as u8];
        f.extend_from_slice(data);
        let crc = crc16(&f);
        f.push(crc as u8);
        f.push((crc >> 8) as u8);
        f
    }

    #[test]
    fn cells_and_info() {
        // 4 cells @ 3.6 V (raw 36)
        let mut data = vec![0x00, 0x04]; // cell count 4
        for _ in 0..4 {
            data.extend_from_slice(&36u16.to_be_bytes());
        }
        let f = frame(0x30, &data);
        let body = verify(&f).unwrap();
        let cells = parse_cells(body);
        assert_eq!(cells.len(), 4);
        assert!((cells[0] - 3.6).abs() < 1e-3);

        // info: current +1.40 A (140), voltage 14.5 V (145), remaining 99941,
        // capacity 100000 (×0.001 → 99.941 / 100.0 Ah)
        let mut info = Vec::new();
        info.extend_from_slice(&140i16.to_be_bytes());
        info.extend_from_slice(&145u16.to_be_bytes());
        info.extend_from_slice(&99_941u32.to_be_bytes());
        info.extend_from_slice(&100_000u32.to_be_bytes());
        let fi = frame(0x30, &info);
        let (current, voltage, remaining, capacity) = parse_info(verify(&fi).unwrap());
        assert!((current - 1.40).abs() < 1e-2);
        assert!((voltage - 14.5).abs() < 1e-2);
        assert!((remaining - 99.941).abs() < 1e-2);
        assert!((capacity - 100.0).abs() < 1e-2);

        let mut d = RenogyData {
            current,
            voltage,
            remaining_ah: remaining,
            capacity_ah: capacity,
            cells,
            ..Default::default()
        };
        d.recompute();
        assert!((d.soc - 99.941).abs() < 0.1);
        assert!((d.power - 14.5 * 1.4).abs() < 1e-1);
    }

    #[test]
    fn model_string() {
        let mut data = b"RBT100LFP12S-G\0\0".to_vec();
        data.truncate(16);
        let f = frame(0x30, &data);
        assert_eq!(parse_model(verify(&f).unwrap()).as_deref(), Some("RBT100LFP12S-G"));
    }

    #[test]
    fn request_uses_default_unit() {
        let f = build_read(DEFAULT_UNIT, SECTIONS[0].0, SECTIONS[0].1);
        assert_eq!(f[0], 0xFF);
        assert_eq!(&f[2..6], &[0x13, 0x88, 0x00, 0x11]);
    }
}
