//! Encryption-key derivation from the BLE advertisement (no manual key entry).
//! The device serial comes from the manufacturer id + data; the model/GUID come
//! from RC4-decrypted service data; the session key is `serial[-6:] ‖ guid ‖ salt`.

use crate::crypto::rc4;

const SALT_RC4: &[u8] = b"LYx*G!6u9#";
const SALT_KEY: &[u8] = b"6*SY1c5B9@";

/// Info recovered from a Jackery advertisement.
#[derive(Debug, Clone)]
pub struct AdvInfo {
    pub serial: String,
    pub guid: [u8; 6],
    pub model: u16,
    pub battery: u8,
}

impl AdvInfo {
    /// The derived encryption key material (RC4 uses all of it; AES the first 16).
    pub fn key(&self) -> Vec<u8> {
        let mut k = Vec::with_capacity(22);
        k.extend_from_slice(&self.serial.as_bytes()[self.serial.len() - 6..]);
        k.extend_from_slice(&self.guid);
        k.extend_from_slice(SALT_KEY);
        k
    }
}

/// Parse a Jackery advertisement into [`AdvInfo`]. `manufacturer_id` is the
/// 16-bit company id; `mfg_data` its bytes; `service_data` the 14-byte payload
/// advertised under service `0xBDEE`.
pub fn parse_advertisement(
    manufacturer_id: u16,
    mfg_data: &[u8],
    service_data: &[u8],
) -> Option<AdvInfo> {
    // Serial: one char from the (byte-swapped) manufacturer id + the mfg data.
    let id_hex = format!("{manufacturer_id:04x}");
    let swapped = format!("{}{}", &id_hex[2..4], &id_hex[0..2]);
    let sn_part1 = u8::from_str_radix(&swapped[2..4], 16).ok()? as char;
    let sn_part2 = std::str::from_utf8(mfg_data).ok()?;
    let serial = format!("{sn_part1}{sn_part2}");
    if serial.len() != 15 || service_data.len() != 14 {
        return None;
    }

    // RC4-decrypt the service data with a key from the serial.
    let mut rc4_key = Vec::new();
    rc4_key.extend_from_slice(&serial.as_bytes()[0..3]);
    rc4_key.extend_from_slice(&serial.as_bytes()[serial.len() - 5..]);
    rc4_key.extend_from_slice(SALT_RC4);
    let decrypted = rc4(service_data, &rc4_key); // 14 bytes

    let (data_part, crc_bytes) = decrypted.split_at(decrypted.len() - 2);
    let crc = modbus_lite::crc16(data_part);
    if [crc as u8, (crc >> 8) as u8] != crc_bytes {
        return None;
    }
    // data_part = payload(11) ‖ xor_key(1)
    let (payload, xor_key) = data_part.split_at(data_part.len() - 1);
    let xor = xor_key[0];
    let decoded: Vec<u8> = payload.iter().map(|b| b ^ xor).collect();
    if decoded.len() < 9 {
        return None;
    }
    let model = ((decoded[0] as u16) << 8) | decoded[1] as u16;
    let mut guid = [0u8; 6];
    guid.copy_from_slice(&decoded[2..8]);
    let battery = decoded[8];

    Some(AdvInfo {
        serial,
        guid,
        model,
        battery,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a synthetic advertisement and confirm it round-trips through the
    /// parser (self-consistency KAT for the key-derivation chain).
    #[test]
    fn advertisement_roundtrip() {
        // Serial "J1234567890ABCD" → sn_part1 'J' from id, rest from mfg data.
        let serial = "J1234567890ABCD";
        let mfg_data = b"1234567890ABCD"; // 14 chars
                                          // manufacturer id: id_hex="4a02" (swap→"024a": app_type 02, char 0x4a='J')
        let manufacturer_id: u16 = 0x4A02;

        let model: u16 = 9;
        let guid = [0x11u8, 0x22, 0x33, 0x44, 0x55, 0x66];
        let battery = 85u8;
        let reset = [0x00u8, 0x01];

        // decoded = model(2) ‖ guid(6) ‖ battery(1) ‖ reset(2) = 11 bytes
        let mut decoded = vec![(model >> 8) as u8, model as u8];
        decoded.extend_from_slice(&guid);
        decoded.push(battery);
        decoded.extend_from_slice(&reset);

        let xor = 0x3Cu8;
        let mut data_part: Vec<u8> = decoded.iter().map(|b| b ^ xor).collect();
        data_part.push(xor);
        let crc = modbus_lite::crc16(&data_part);
        let mut plain = data_part.clone();
        plain.extend_from_slice(&[crc as u8, (crc >> 8) as u8]); // 14 bytes

        // RC4-encrypt with the serial-derived key (RC4 is symmetric).
        let mut rc4_key = Vec::new();
        rc4_key.extend_from_slice(&serial.as_bytes()[0..3]);
        rc4_key.extend_from_slice(&serial.as_bytes()[serial.len() - 5..]);
        rc4_key.extend_from_slice(SALT_RC4);
        let service_data = rc4(&plain, &rc4_key);
        assert_eq!(service_data.len(), 14);

        let info = parse_advertisement(manufacturer_id, mfg_data, &service_data).expect("parse");
        assert_eq!(info.serial, serial);
        assert_eq!(info.model, model);
        assert_eq!(info.guid, guid);
        assert_eq!(info.battery, battery);

        // Key = serial[-6:] ‖ guid ‖ salt = 22 bytes
        let key = info.key();
        assert_eq!(key.len(), 22);
        assert_eq!(&key[0..6], b"90ABCD");
        assert_eq!(&key[6..12], &guid);
    }
}
