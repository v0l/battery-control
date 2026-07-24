//! SOK `0xEE`-command wire protocol (older 12V packs) — pure, no I/O.
//!
//! These batteries speak a small command protocol over BLE (service
//! `FFE0`, notify `FFE1`, write `FFE2`). A request is six bytes:
//! ```text
//!   EE <cmd> 00 00 00 <crc8>
//! ```
//! where `crc8` is CRC-8/MAXIM (Dallas) over the five preceding bytes. The
//! device answers with one or more frames, each tagged by a 2-byte big-endian
//! header (`0xCCF0` info, `0xCCF2` temperature, `0xCCF3` capacity, `0xCCF4`
//! cells). Ported from `IAmTheMitchell/sok-ble`.

/// Read info (current, cycles, SOC) → answers `0xCCF0` and `0xCCF2`.
pub const CMD_INFO: u8 = 0xC1;
/// Read detail (capacity, cells) → answers `0xCCF3` and `0xCCF4`.
pub const CMD_DETAIL: u8 = 0xC2;

/// Response headers (first two bytes, big-endian).
pub const HDR_INFO: u16 = 0xCCF0;
pub const HDR_TEMP: u16 = 0xCCF2;
pub const HDR_CAP: u16 = 0xCCF3;
pub const HDR_CELLS: u16 = 0xCCF4;

/// CRC-8/MAXIM (Dallas): poly 0x31 reflected (0x8C), init 0, no final xor.
pub fn crc8(data: &[u8]) -> u8 {
    let mut crc: u8 = 0;
    for &b in data {
        crc ^= b;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0x8C } else { crc >> 1 };
        }
    }
    crc
}

/// Build a six-byte command frame with its CRC.
pub fn command(cmd: u8) -> [u8; 6] {
    let mut frame = [0xEE, cmd, 0x00, 0x00, 0x00, 0x00];
    frame[5] = crc8(&frame[..5]);
    frame
}

/// The 2-byte big-endian response header, if the frame is long enough.
pub fn header(frame: &[u8]) -> Option<u16> {
    (frame.len() >= 2).then(|| ((frame[0] as u16) << 8) | frame[1] as u16)
}

fn u16_le(d: &[u8], i: usize) -> u16 {
    (d[i] as u16) | ((d[i + 1] as u16) << 8)
}
fn i16_le(d: &[u8], i: usize) -> i16 {
    u16_le(d, i) as i16
}
fn i24_le(d: &[u8], i: usize) -> i32 {
    let v = (d[i] as i32) | ((d[i + 1] as i32) << 8) | ((d[i + 2] as i32) << 16);
    if v & 0x0080_0000 != 0 {
        v - 0x0100_0000
    } else {
        v
    }
}
fn u24_be(d: &[u8], i: usize) -> u32 {
    ((d[i] as u32) << 16) | ((d[i + 1] as u32) << 8) | d[i + 2] as u32
}

/// Info frame (`0xCCF0`): current (A), cycles, SOC (%).
pub fn parse_info(buf: &[u8]) -> Option<(f32, u16, u16)> {
    if buf.len() < 20 {
        return None;
    }
    let current = i24_le(buf, 5) as f32 / 1000.0;
    let cycles = u16_le(buf, 14);
    let soc = u16_le(buf, 16);
    Some((current, cycles, soc))
}

/// Temperature frame (`0xCCF2`): °C.
pub fn parse_temp(buf: &[u8]) -> Option<f32> {
    (buf.len() >= 20).then(|| i16_le(buf, 5) as f32)
}

/// Capacity frame (`0xCCF3`): rated capacity in Ah.
pub fn parse_capacity(buf: &[u8]) -> Option<f32> {
    (buf.len() >= 20).then(|| u24_be(buf, 5) as f32 / 128.0)
}

/// Cells frame (`0xCCF4`): four cell voltages (V), each tagged by its index.
pub fn parse_cells(buf: &[u8]) -> Option<[f32; 4]> {
    if buf.len() < 20 {
        return None;
    }
    let mut cells = [0.0f32; 4];
    for x in 0..4 {
        let idx = buf[2 + x * 4] as usize;
        if (1..=4).contains(&idx) {
            cells[idx - 1] = u16_le(buf, 3 + x * 4) as f32 / 1000.0;
        }
    }
    Some(cells)
}

use crate::data::SokData;

/// Assemble a [`SokData`] from the four collected response frames.
pub fn from_frames(info: &[u8], temp: &[u8], capacity: &[u8], cells: &[u8]) -> Option<SokData> {
    let (current, cycles, soc) = parse_info(info)?;
    let temperature = parse_temp(temp)?;
    let capacity = parse_capacity(capacity)?;
    let cells = parse_cells(cells)?;
    let voltage = cells.iter().sum::<f32>() / cells.len() as f32 * 4.0;
    Some(SokData {
        voltage,
        current,
        power: voltage * current,
        soc,
        temperature,
        temps: vec![temperature],
        capacity,
        remaining: None,
        cycles: Some(cycles),
        cells: cells.to_vec(),
        model: None,
        serial: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc8_maxim_check_vector() {
        // CRC-8/MAXIM("123456789") == 0xA1
        assert_eq!(crc8(b"123456789"), 0xA1);
    }

    #[test]
    fn command_frame_shape() {
        let f = command(CMD_INFO);
        assert_eq!(&f[..5], &[0xEE, 0xC1, 0x00, 0x00, 0x00]);
        assert_eq!(f[5], crc8(&f[..5]));
    }

    #[test]
    fn parse_info_fields() {
        let mut b = vec![0u8; 20];
        b[0] = 0xCC;
        b[1] = 0xF0;
        // current +1.500 A → 1500 = 0xDC 0x05 0x00 (LE i24)
        b[5] = 0xDC;
        b[6] = 0x05;
        b[7] = 0x00;
        b[14..16].copy_from_slice(&42u16.to_le_bytes()); // cycles
        b[16..18].copy_from_slice(&87u16.to_le_bytes()); // soc
        let (current, cycles, soc) = parse_info(&b).unwrap();
        assert!((current - 1.5).abs() < 1e-3);
        assert_eq!(cycles, 42);
        assert_eq!(soc, 87);
        assert_eq!(header(&b), Some(HDR_INFO));
    }

    #[test]
    fn parse_info_negative_current() {
        let mut b = vec![0u8; 20];
        // -2.000 A → -2000 = 0x1000000 - 2000 = 0xFFF830 LE bytes 30 F8 FF
        b[5] = 0x30;
        b[6] = 0xF8;
        b[7] = 0xFF;
        let (current, _, _) = parse_info(&b).unwrap();
        assert!((current + 2.0).abs() < 1e-3);
    }

    #[test]
    fn parse_temp_capacity() {
        let mut t = vec![0u8; 20];
        t[5..7].copy_from_slice(&25i16.to_le_bytes());
        assert_eq!(parse_temp(&t), Some(25.0));

        let mut c = vec![0u8; 20];
        // capacity 100 Ah → 100*128 = 12800 = 0x003200 BE bytes 00 32 00
        c[5] = 0x00;
        c[6] = 0x32;
        c[7] = 0x00;
        assert_eq!(parse_capacity(&c), Some(100.0));
    }

    #[test]
    fn parse_cells_by_index() {
        let mut b = vec![0u8; 20];
        // four blocks of [_, idx, v_lo, v_hi] starting at offset 2
        for (x, (idx, mv)) in [(1u8, 3300u16), (2, 3301), (3, 3299), (4, 3302)]
            .into_iter()
            .enumerate()
        {
            b[2 + x * 4] = idx;
            b[3 + x * 4..5 + x * 4].copy_from_slice(&mv.to_le_bytes());
        }
        let cells = parse_cells(&b).unwrap();
        assert!((cells[0] - 3.300).abs() < 1e-4);
        assert!((cells[3] - 3.302).abs() < 1e-4);

        let data = from_frames(
            &{ let mut i = vec![0u8; 20]; i[16] = 50; i },
            &{ let mut t = vec![0u8; 20]; t[5] = 20; t },
            &{ let mut c = vec![0u8; 20]; c[6] = 0x32; c },
            &b,
        )
        .unwrap();
        assert_eq!(data.soc, 50);
        assert!((data.voltage - 3.3005 * 4.0).abs() < 1e-2);
    }
}
