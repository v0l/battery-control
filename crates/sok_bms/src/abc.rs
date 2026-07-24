//! ABC-BMS wire protocol (the "ABC BMS" app) — pure, no I/O.
//!
//! Modbus RTU over BLE (service `FFF0`, notify `FFF1`, write `FFF2`): slave
//! `0x01`, function `0x03` (read holding registers), CRC-16/MODBUS on the wire
//! (little-endian), big-endian 16-bit registers. Ported from
//! `node-red-contrib/node-red-contrib-sok`.

use crate::data::SokData;
use std::collections::HashMap;

pub const UNIT: u8 = 0x01;
pub const FUNC_READ: u8 = 0x03;

/// The telemetry register block: current/voltage/SOC/capacity/cells/temps/id.
pub const TELEMETRY_START: u16 = 0x0080;
pub const TELEMETRY_COUNT: u16 = 0x007a;

/// CRC-16/MODBUS (poly 0xA001 reflected, init 0xFFFF, no final xor).
pub fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &b in data {
        crc ^= b as u16;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xA001
            } else {
                crc >> 1
            };
        }
    }
    crc
}

/// Build a "read holding registers" request with the CRC appended little-endian.
pub fn build_read(start: u16, count: u16) -> [u8; 8] {
    let mut f = [
        UNIT,
        FUNC_READ,
        (start >> 8) as u8,
        start as u8,
        (count >> 8) as u8,
        count as u8,
        0,
        0,
    ];
    let crc = crc16(&f[..6]);
    f[6] = crc as u8; // low byte first
    f[7] = (crc >> 8) as u8;
    f
}

/// Total expected length of a response, once enough of it is buffered to tell.
/// Returns `None` while the byte-count byte is still missing.
pub fn response_len(buf: &[u8]) -> Option<usize> {
    if buf.len() < 2 {
        return None;
    }
    if buf[1] & 0x80 != 0 {
        return Some(5); // exception: unit, func|0x80, code, crc(2)
    }
    if buf.len() < 3 {
        return None;
    }
    Some(3 + buf[2] as usize + 2)
}

/// Verify CRC and decode a response frame into `address -> value`, where the
/// first register is at `start`.
pub fn parse_response(frame: &[u8], start: u16) -> Result<HashMap<u16, u16>, &'static str> {
    if frame.len() < 5 {
        return Err("frame too short");
    }
    let n = frame.len();
    let expected = crc16(&frame[..n - 2]);
    let got = (frame[n - 2] as u16) | ((frame[n - 1] as u16) << 8);
    if expected != got {
        return Err("crc mismatch");
    }
    if frame[1] & 0x80 != 0 {
        return Err("modbus exception");
    }
    if frame[1] != FUNC_READ {
        return Err("unexpected function code");
    }
    let byte_count = frame[2] as usize;
    let data = &frame[3..3 + byte_count.min(n.saturating_sub(5))];
    if data.len() % 2 != 0 {
        return Err("odd register byte count");
    }
    let mut regs = HashMap::new();
    for (i, chunk) in data.chunks_exact(2).enumerate() {
        let addr = start + i as u16;
        let val = ((chunk[0] as u16) << 8) | chunk[1] as u16;
        regs.insert(addr, val);
    }
    Ok(regs)
}

// --- register decoding helpers ---

fn u(regs: &HashMap<u16, u16>, addr: u16) -> Option<u16> {
    regs.get(&addr).copied().filter(|&v| v != 0xFFFF)
}
fn i(regs: &HashMap<u16, u16>, addr: u16) -> Option<i16> {
    regs.get(&addr).map(|&v| v as i16)
}
fn scaled_u(regs: &HashMap<u16, u16>, addr: u16, div: f32) -> Option<f32> {
    u(regs, addr).map(|v| v as f32 / div)
}
fn scaled_i(regs: &HashMap<u16, u16>, addr: u16, div: f32) -> Option<f32> {
    i(regs, addr).filter(|&v| v != -1).map(|v| v as f32 / div)
}

fn read_ascii(regs: &HashMap<u16, u16>, start: u16, max: u16) -> Option<String> {
    let mut bytes = Vec::new();
    for off in 0..max {
        match regs.get(&(start + off)) {
            Some(&v) if v != 0xFFFF => {
                bytes.push((v >> 8) as u8);
                bytes.push(v as u8);
            }
            _ => break,
        }
    }
    let text = String::from_utf8_lossy(&bytes)
        .replace('\0', "")
        .trim()
        .to_string();
    (!text.is_empty()).then_some(text)
}

/// Read a run of cell voltages (mV → V), stopping at the first invalid entry.
fn read_cells(regs: &HashMap<u16, u16>, start: u16, count: usize) -> Vec<f32> {
    let mut cells = Vec::new();
    for idx in 0..count {
        match regs.get(&(start + idx as u16)) {
            Some(&v) if v != 0xFFFF && v != 0x8000 && v >= 1000 => {
                cells.push(v as f32 / 1000.0);
            }
            _ => break,
        }
    }
    cells
}

/// Decode the telemetry register block into a [`SokData`].
pub fn decode_telemetry(regs: &HashMap<u16, u16>) -> SokData {
    let cell_count = u(regs, 0x0091).map(|v| (v as usize).min(32)).unwrap_or(4);
    let mut cells = read_cells(regs, 0x009b, cell_count);
    if cells.is_empty() {
        cells = read_cells(regs, 0x0092, cell_count);
    }

    let t1 = scaled_i(regs, 0x0095, 10.0);
    let temps: Vec<f32> = [0x0095u16, 0x0096, 0x0097, 0x0098]
        .iter()
        .filter_map(|&a| scaled_i(regs, a, 10.0))
        .collect();

    let voltage = scaled_u(regs, 0x0081, 100.0).unwrap_or_else(|| {
        if cells.is_empty() {
            0.0
        } else {
            cells.iter().sum::<f32>()
        }
    });
    let current = scaled_i(regs, 0x0080, 100.0).unwrap_or(0.0);

    SokData {
        voltage,
        current,
        power: voltage * current,
        soc: u(regs, 0x0082).unwrap_or(0),
        temperature: t1.unwrap_or(0.0),
        temps,
        capacity: scaled_u(regs, 0x0085, 100.0).unwrap_or(0.0),
        remaining: scaled_u(regs, 0x0084, 100.0),
        cycles: None,
        cells,
        model: read_ascii(regs, 0x00dc, 10),
        serial: read_ascii(regs, 0x00e6, 21),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc16_modbus_check_vector() {
        // CRC-16/MODBUS("123456789") == 0x4B37
        assert_eq!(crc16(b"123456789"), 0x4B37);
    }

    #[test]
    fn read_request_frame() {
        let f = build_read(TELEMETRY_START, TELEMETRY_COUNT);
        assert_eq!(&f[..6], &[0x01, 0x03, 0x00, 0x80, 0x00, 0x7a]);
        let crc = crc16(&f[..6]);
        assert_eq!(f[6], crc as u8);
        assert_eq!(f[7], (crc >> 8) as u8);
    }

    fn response(start: u16, regs: &[(u16, u16)]) -> Vec<u8> {
        // Build a contiguous register block covering min..=max of the addresses.
        let min = regs.iter().map(|(a, _)| *a).min().unwrap();
        let max = regs.iter().map(|(a, _)| *a).max().unwrap();
        assert_eq!(min, start);
        let count = (max - min + 1) as usize;
        let mut data = vec![0u8; count * 2];
        for &(a, v) in regs {
            let off = (a - start) as usize * 2;
            data[off] = (v >> 8) as u8;
            data[off + 1] = v as u8;
        }
        let mut frame = vec![UNIT, FUNC_READ, (count * 2) as u8];
        frame.extend_from_slice(&data);
        let crc = crc16(&frame);
        frame.push(crc as u8);
        frame.push((crc >> 8) as u8);
        frame
    }

    #[test]
    fn decode_synthetic_telemetry() {
        let regs = [
            (0x0080u16, 150u16), // current +1.50 A
            (0x0081, 1320),      // voltage 13.20 V
            (0x0082, 88),        // soc
            (0x0084, 8000),      // remaining 80.00 Ah
            (0x0085, 10000),     // full 100.00 Ah
            (0x0091, 4),         // cell count
            (0x0095, 250),       // t1 25.0 °C
            (0x0096, 251),
            (0x009b, 3300),
            (0x009c, 3301),
            (0x009d, 3299),
            (0x009e, 3302),
        ];
        let frame = response(0x0080, &regs);
        assert_eq!(response_len(&frame), Some(frame.len()));
        let map = parse_response(&frame, 0x0080).unwrap();
        let d = decode_telemetry(&map);
        assert!((d.voltage - 13.20).abs() < 1e-2);
        assert!((d.current - 1.50).abs() < 1e-2);
        assert_eq!(d.soc, 88);
        assert!((d.capacity - 100.0).abs() < 1e-2);
        assert!((d.remaining.unwrap() - 80.0).abs() < 1e-2);
        assert_eq!(d.cells.len(), 4);
        assert!((d.temperature - 25.0).abs() < 1e-2);
        // t1..t4 all present in the contiguous block (0x0097/0x0098 default 0.0).
        assert_eq!(d.temps.len(), 4);
        assert!((d.temps[1] - 25.1).abs() < 1e-2);
    }

    #[test]
    fn rejects_bad_crc() {
        let mut frame = response(0x0080, &[(0x0080, 1)]);
        let n = frame.len();
        frame[n - 1] ^= 0xFF;
        assert!(parse_response(&frame, 0x0080).is_err());
    }
}
