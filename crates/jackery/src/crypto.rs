//! Jackery BLE encryption. Most portable models use **RC4**; a couple (and the
//! Box devices) use **AES-128-CBC**. Both wrap the plaintext with a `DFEC`/`DFED`
//! magic, a random suffix, and a CRC-16/MODBUS. Ported from `porcupin26/private_jack`.

/// RC4 keystream cipher (symmetric).
pub fn rc4(data: &[u8], key: &[u8]) -> Vec<u8> {
    let mut s: Vec<u8> = (0..=255).collect();
    let mut j = 0u8;
    for i in 0..256 {
        j = j
            .wrapping_add(s[i])
            .wrapping_add(key[i % key.len()]);
        s.swap(i, j as usize);
    }
    let (mut i, mut j) = (0u8, 0u8);
    let mut out = Vec::with_capacity(data.len());
    for &byte in data {
        i = i.wrapping_add(1);
        j = j.wrapping_add(s[i as usize]);
        s.swap(i as usize, j as usize);
        let k = s[(s[i as usize].wrapping_add(s[j as usize])) as usize];
        out.push(byte ^ k);
    }
    out
}

/// CRC-16/MODBUS over `data` (same as `modbus_lite`), appended little-endian on
/// the wire.
fn crc(data: &[u8]) -> [u8; 2] {
    let c = modbus_lite::crc16(data);
    [c as u8, (c >> 8) as u8]
}

const MAGIC_PORTABLE: [u8; 2] = [0xDF, 0xEC];

/// The RC4 cipher used by most portable Jackery models.
pub struct Rc4Cipher {
    key: Vec<u8>,
}

impl Rc4Cipher {
    pub fn new(key: Vec<u8>) -> Self {
        Self { key }
    }

    /// Encrypt an already-built command (including its `DFEC…` header).
    pub fn encrypt(&self, command: &[u8], security: u8) -> Vec<u8> {
        // plaintext = xor(command, sec) ‖ sec ‖ crc16(that)
        let mut plain: Vec<u8> = command.iter().map(|b| b ^ security).collect();
        plain.push(security);
        plain.extend_from_slice(&crc(&plain));
        rc4(&plain, &self.key)
    }

    /// Decrypt a response, returning the payload after the `DFEC` magic.
    pub fn decrypt(&self, enc: &[u8]) -> Option<Vec<u8>> {
        let dec = rc4(enc, &self.key);
        if dec.len() < 6 {
            return None;
        }
        let (body, crc_bytes) = dec.split_at(dec.len() - 2);
        if crc(body) != crc_bytes {
            return None;
        }
        let security = *body.last()?;
        let xored = &body[..body.len() - 1];
        let decoded: Vec<u8> = xored.iter().map(|b| b ^ security).collect();
        if decoded.len() < 2 || decoded[..2] != MAGIC_PORTABLE {
            return None;
        }
        Some(decoded[2..].to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rc4_matches_reference_vector() {
        // Classic RC4 test vector: key "Key", plaintext "Plaintext".
        let ct = rc4(b"Plaintext", b"Key");
        assert_eq!(ct, [0xBB, 0xF3, 0x16, 0xE8, 0xD9, 0x40, 0xAF, 0x0A, 0xD3]);
        // symmetric
        assert_eq!(rc4(&ct, b"Key"), b"Plaintext");
    }

    #[test]
    fn rc4_cipher_roundtrip() {
        let cipher = Rc4Cipher::new(b"a-22-byte-jackery-keyyy".to_vec());
        // A device-property query command: DFEC00 FC 03 00
        let command = [0xDF, 0xEC, 0x00, 0xFC, 0x03, 0x00];
        let enc = cipher.encrypt(&command, 0x5A);
        // The response uses the same wrapping; decrypt strips DFEC → 00 FC 03 00
        let payload = cipher.decrypt(&enc).expect("decrypt");
        assert_eq!(payload, [0x00, 0xFC, 0x03, 0x00]);
    }
}
