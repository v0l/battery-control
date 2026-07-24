//! The two CRCs EcoFlow uses on the wire.

/// CRC-16/ARC (poly 0x8005 reflected, init 0, xorout 0). Used on full packets.
pub fn crc16_arc(data: &[u8]) -> u16 {
    let mut crc: u16 = 0;
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

/// CRC-8/SMBUS (poly 0x07, init 0, no reflection). Used on packet headers.
pub fn crc8_smbus(data: &[u8]) -> u8 {
    let mut crc: u8 = 0;
    for &b in data {
        crc ^= b;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 {
                (crc << 1) ^ 0x07
            } else {
                crc << 1
            };
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_vectors() {
        assert_eq!(crc16_arc(b"123456789"), 0xBB3D);
        assert_eq!(crc8_smbus(b"123456789"), 0xF4);
    }
}
