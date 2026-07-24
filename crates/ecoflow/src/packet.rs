//! EcoFlow packet framing. Two layers:
//! - [`Packet`]: the inner application packet (`0xAA` prefix, header CRC-8, whole
//!   CRC-16), which carries a src/dst/cmd_set/cmd_id and a (possibly XOR-masked)
//!   payload.
//! - [`EncPacket`]: the outer BLE frame (`0x5A5A` prefix) whose payload is the
//!   AES-CBC-encrypted inner packet.

use crate::crc::{crc16_arc, crc8_smbus};

/// An inner application packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    pub src: u8,
    pub dst: u8,
    pub cmd_set: u8,
    pub cmd_id: u8,
    pub payload: Vec<u8>,
    pub dsrc: u8,
    pub ddst: u8,
    pub version: u8,
    pub seq: [u8; 4],
    pub product_id: i32,
}

impl Packet {
    pub const PREFIX: u8 = 0xAA;

    pub fn new(src: u8, dst: u8, cmd_set: u8, cmd_id: u8, payload: Vec<u8>) -> Self {
        Self {
            src,
            dst,
            cmd_set,
            cmd_id,
            payload,
            dsrc: 1,
            ddst: 1,
            version: 3,
            seq: [0; 4],
            product_id: 0,
        }
    }

    fn product_byte(&self) -> u8 {
        if self.product_id >= 0 {
            0x0d
        } else {
            0x0c
        }
    }

    /// Serialize to the on-the-wire byte stream.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut data = vec![Self::PREFIX, self.version];
        data.extend_from_slice(&(self.payload.len() as u16).to_le_bytes());
        data.push(crc8_smbus(&data)); // header crc over [AA, ver, len_lo, len_hi]
        data.push(self.product_byte());
        data.extend_from_slice(&self.seq);
        data.extend_from_slice(&[0x00, 0x00]);
        data.extend_from_slice(&[self.src, self.dst, self.dsrc, self.ddst, self.cmd_set, self.cmd_id]);
        data.extend_from_slice(&self.payload);
        data.extend_from_slice(&crc16_arc(&data).to_le_bytes());
        data
    }

    /// Parse from a byte stream. `is_xor` (Y711/Delta Pro Ultra) un-masks the
    /// payload with the first sequence byte.
    pub fn from_bytes(data: &[u8], is_xor: bool) -> Option<Packet> {
        if data.len() < 20 || data[0] != Self::PREFIX {
            return None;
        }
        let version = data[1];
        let payload_length = u16::from_le_bytes([data[2], data[3]]) as usize;
        if version == 3 {
            let want = u16::from_le_bytes([data[data.len() - 2], data[data.len() - 1]]);
            if crc16_arc(&data[..data.len() - 2]) != want {
                return None;
            }
        }
        if crc8_smbus(&data[..4]) != data[4] {
            return None;
        }
        let seq: [u8; 4] = data[6..10].try_into().ok()?;
        let (src, dst, dsrc, ddst) = (data[12], data[13], data[14], data[15]);
        let (cmd_set, cmd_id) = (data[16], data[17]);

        let mut payload = Vec::new();
        if payload_length > 0 && 18 + payload_length <= data.len() {
            payload = data[18..18 + payload_length].to_vec();
            if is_xor && seq[0] != 0 {
                for byte in &mut payload {
                    *byte ^= seq[0];
                }
            }
            if version == 19 && payload.ends_with(&[0xbb, 0xbb]) {
                payload.truncate(payload.len() - 2);
            }
        }
        Some(Packet {
            src,
            dst,
            cmd_set,
            cmd_id,
            payload,
            dsrc,
            ddst,
            version,
            seq,
            product_id: 0,
        })
    }
}

/// Outer BLE frame types.
#[derive(Clone, Copy)]
pub enum FrameType {
    Command = 0x00,
    Protocol = 0x01,
}

/// Build an outer `0x5A5A` frame carrying `payload` (already encrypted, or
/// plaintext for the unencrypted handshake steps).
pub fn enc_packet(frame_type: FrameType, payload: &[u8]) -> Vec<u8> {
    let mut data = vec![0x5A, 0x5A, (frame_type as u8) << 4, 0x01];
    data.extend_from_slice(&((payload.len() + 2) as u16).to_le_bytes());
    data.extend_from_slice(payload);
    data.extend_from_slice(&crc16_arc(&data).to_le_bytes());
    data
}

/// Split a stream into complete outer-frame payloads (`0x5A5A …`), returning the
/// payloads and any trailing incomplete bytes to buffer for next time.
pub fn split_enc_frames(mut data: &[u8]) -> (Vec<Vec<u8>>, Vec<u8>) {
    let mut out = Vec::new();
    while data.len() >= 6 && data[0] == 0x5A && data[1] == 0x5A {
        let len = u16::from_le_bytes([data[4], data[5]]) as usize;
        let total = 6 + len;
        if total > data.len() {
            break;
        }
        let frame = &data[..total];
        // payload is between the 6-byte header and the trailing crc16
        let want = u16::from_le_bytes([frame[total - 2], frame[total - 1]]);
        if crc16_arc(&frame[..total - 2]) == want {
            out.push(frame[6..total - 2].to_vec());
        }
        data = &data[total..];
    }
    (out, data.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packet_roundtrip() {
        let p = Packet::new(0x21, 0x35, 0x35, 0x86, vec![1, 2, 3, 4]);
        let bytes = p.to_bytes();
        let back = Packet::from_bytes(&bytes, false).expect("parse");
        assert_eq!(back.src, 0x21);
        assert_eq!(back.cmd_set, 0x35);
        assert_eq!(back.cmd_id, 0x86);
        assert_eq!(back.payload, vec![1, 2, 3, 4]);
    }

    #[test]
    fn enc_frame_split() {
        let f1 = enc_packet(FrameType::Command, &[0xde, 0xad]);
        let f2 = enc_packet(FrameType::Protocol, &[0xbe, 0xef, 0x00]);
        let mut stream = f1.clone();
        stream.extend_from_slice(&f2);
        stream.extend_from_slice(&[0x5A, 0x5A, 0x00]); // partial trailer
        let (frames, rest) = split_enc_frames(&stream);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0], vec![0xde, 0xad]);
        assert_eq!(frames[1], vec![0xbe, 0xef, 0x00]);
        assert_eq!(rest, vec![0x5A, 0x5A, 0x00]);
    }
}
