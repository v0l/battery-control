//! Minimal, transport-agnostic **Modbus-RTU** helpers for "read holding
//! registers" (function `0x03`) — the framing shared by the Modbus battery
//! backends (Renogy over BLE, PACE/Seplos over RS485). CRC-16/MODBUS on the
//! wire (little-endian); registers are big-endian 16-bit.

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

/// Build a Modbus read request for `func` (`0x03` holding / `0x04` input
/// registers), CRC appended little-endian.
pub fn build(unit: u8, func: u8, start: u16, words: u16) -> [u8; 8] {
    let mut f = [
        unit,
        func,
        (start >> 8) as u8,
        start as u8,
        (words >> 8) as u8,
        words as u8,
        0,
        0,
    ];
    let crc = crc16(&f[..6]);
    f[6] = crc as u8;
    f[7] = (crc >> 8) as u8;
    f
}

/// Read holding registers (function `0x03`).
pub fn build_read(unit: u8, start: u16, words: u16) -> [u8; 8] {
    build(unit, 0x03, start, words)
}

/// Read input registers (function `0x04`).
pub fn build_read_input(unit: u8, start: u16, words: u16) -> [u8; 8] {
    build(unit, 0x04, start, words)
}

/// Write a single holding register (function `0x06`). The response echoes the
/// request; a fixed 8 bytes (no byte-count), so read exactly 8 back.
pub fn build_write_single(unit: u8, addr: u16, value: u16) -> [u8; 8] {
    build(unit, 0x06, addr, value)
}

/// Total expected response length once enough of it is buffered to tell.
/// `None` while the byte-count byte is still missing.
pub fn response_len(buf: &[u8]) -> Option<usize> {
    if buf.len() < 2 {
        return None;
    }
    if buf[1] & 0x80 != 0 {
        return Some(5); // exception frame
    }
    if buf.len() < 3 {
        return None;
    }
    Some(3 + buf[2] as usize + 2)
}

/// Verify CRC + read function code, returning the frame with the trailing CRC
/// stripped (`unit, func, byte_count, data…`). Register data begins at offset 3.
pub fn verify(frame: &[u8]) -> Result<&[u8], &'static str> {
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
    if !matches!(frame[1], 0x03 | 0x04 | 0x06) {
        return Err("unexpected function code");
    }
    Ok(&frame[..n - 2])
}

pub fn be_u16(bs: &[u8], off: usize) -> u16 {
    if bs.len() < off + 2 {
        return 0;
    }
    ((bs[off] as u16) << 8) | bs[off + 1] as u16
}
pub fn be_i16(bs: &[u8], off: usize) -> i16 {
    be_u16(bs, off) as i16
}
pub fn be_u32(bs: &[u8], off: usize) -> u32 {
    if bs.len() < off + 4 {
        return 0;
    }
    ((bs[off] as u32) << 24)
        | ((bs[off + 1] as u32) << 16)
        | ((bs[off + 2] as u32) << 8)
        | bs[off + 3] as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc16_modbus_check_vector() {
        assert_eq!(crc16(b"123456789"), 0x4B37);
    }

    #[test]
    fn read_request() {
        let f = build_read(0x01, 0, 37);
        assert_eq!(&f[..6], &[0x01, 0x03, 0x00, 0x00, 0x00, 0x25]);
        let crc = crc16(&f[..6]);
        assert_eq!([f[6], f[7]], [crc as u8, (crc >> 8) as u8]);
    }

    #[test]
    fn verify_and_read() {
        let mut frame = vec![0x01u8, 0x03, 0x04, 0x01, 0x2c, 0x00, 0x64];
        let crc = crc16(&frame);
        frame.push(crc as u8);
        frame.push((crc >> 8) as u8);
        let body = verify(&frame).unwrap();
        assert_eq!(be_u16(body, 3), 300);
        assert_eq!(be_u16(body, 5), 100);
        assert_eq!(response_len(&frame), Some(frame.len()));
    }
}
