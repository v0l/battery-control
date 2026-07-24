//! Minimal Modbus-RTU-over-BLE helpers (function 0x03 reads), shared by the
//! Renogy register parsers. CRC-16/MODBUS on the wire (little-endian).

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

/// Build a "read holding registers" request (CRC appended little-endian).
pub fn build_read(unit: u8, start: u16, words: u16) -> [u8; 8] {
    let mut f = [
        unit,
        0x03,
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

/// Total expected response length once enough is buffered to tell.
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
/// stripped (i.e. `unit, func, byte_count, data...`). Renogy parsers index into
/// this directly, so data begins at offset 3.
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
    if frame[1] != 0x03 {
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
        // read 17 words from 5000 (0x1388) at unit 0xFF
        let f = build_read(0xFF, 5000, 17);
        assert_eq!(&f[..6], &[0xFF, 0x03, 0x13, 0x88, 0x00, 0x11]);
        let crc = crc16(&f[..6]);
        assert_eq!([f[6], f[7]], [crc as u8, (crc >> 8) as u8]);
    }

    #[test]
    fn verify_roundtrip() {
        let mut frame = vec![0x30u8, 0x03, 0x04, 0x00, 0x04, 0x00, 0x21];
        let crc = crc16(&frame);
        frame.push(crc as u8);
        frame.push((crc >> 8) as u8);
        let body = verify(&frame).unwrap();
        assert_eq!(be_u16(body, 3), 4); // cell count
        assert_eq!(be_u16(body, 5), 0x21);
    }
}
