//! Wire protocol for Anker SOLIX BLE: packet framing, TLV parameter parsing,
//! fragment reassembly and the fixed negotiation handshake constants.
//!
//! Packet layout (all multi-byte integers little-endian):
//!
//! ```text
//! +--------+----------+-----------+--------+-----------+----------+
//! | 0xFF09 | len (u16)| pattern 3B| cmd 2B | payload nB| xor csum |
//! +--------+----------+-----------+--------+-----------+----------+
//! ```
//!
//! `len` is the total packet length including the 2-byte header, the length
//! field itself and the trailing checksum. The checksum is the XOR of every
//! preceding byte.

use crate::crypto::checksum;
use crate::error::{AnkerError, Result};
use std::collections::BTreeMap;

// --- GATT UUIDs ---------------------------------------------------------------

/// Notifiable characteristic that streams telemetry / negotiation responses.
pub const UUID_TELEMETRY: &str = "8c850003-0302-41c5-b46e-cf057c562025";
/// Writable characteristic used to send commands / negotiation messages.
pub const UUID_COMMAND: &str = "8c850002-0302-41c5-b46e-cf057c562025";
/// Advertised service UUID used to identify Anker SOLIX / Prime devices.
pub const UUID_IDENTIFIER: &str = "0000ff09-0000-1000-8000-00805f9b34fb";

// --- Negotiation handshake ----------------------------------------------------
//
// These messages are entirely static because the client private key is fixed,
// so our ephemeral public key (and therefore every outbound frame) is constant.
// The device's public key arrives in the stage-5 response and is the only
// per-session variable.

pub const NEGOTIATION_COMMAND_0: &str = "ff0936000300010001a10442ad8c69a22462326463306231372d623735642d346162662d626136652d656337633939376332336537b9";
pub const NEGOTIATION_COMMAND_1: &str = "ff093d000300010003a10442ad8c69a22462326463306231372d623735642d346162662d626136652d656337633939376332336537a30120a40200f064";
pub const NEGOTIATION_COMMAND_2: &str = "ff0936000300010029a10442ad8c69a22462326463306231372d623735642d346162662d626136652d65633763393937633233653791";
pub const NEGOTIATION_COMMAND_3: &str = "ff0940000300010005a10443ad8c69a22462326463306231372d623735642d346162662d626136652d656337633939376332336537a30120a40200f0a50140fa";
pub const NEGOTIATION_COMMAND_4: &str = "ff094c000300010021a140060ea168f232aedb37fb2d120c49180329ac72ab5ec3eb8fd30a2f252dc5e151dabccd9b1dc1e288704ca760a0d8c918e5c94823a1f609a4bf07fb4c33ee219085";
pub const NEGOTIATION_COMMAND_5: &str = "ff095a000300014022580bc0532a53c739adf3da7b994a7b5f221bcc16bab6392c215cb4faaf41d9d58e2c81c016e474c78eed5569147cb74a1f22ca2b3fad2e209dbbcfbdaca352034a6c479f055f68581b5f1e22348809f526";

/// Base timestamp (little-endian u32) agreed during negotiation; commands add
/// the number of seconds elapsed since the handshake to defeat replay attacks.
pub const BASE_TIMESTAMP_LE: u32 = 0x698cad42; // bytes 42 ad 8c 69, read little-endian

/// Pattern used for outbound encrypted command packets.
pub const PATTERN_ENCRYPTED_TX: [u8; 3] = [0x03, 0x00, 0x0f];
/// Pattern that prefixes negotiation responses from the device.
pub const PATTERN_NEGOTIATION: [u8; 3] = [0x03, 0x00, 0x01];
/// Patterns that prefix session (telemetry / command reply) packets.
pub const PATTERN_SESSION_A: [u8; 3] = [0x03, 0x01, 0x0f];
pub const PATTERN_SESSION_B: [u8; 3] = [0x03, 0x01, 0x11];

/// Command codes that carry encrypted telemetry for most models.
pub const TELEMETRY_COMMANDS: &[[u8; 2]] = &[[0xc4, 0x02], [0x43, 0x00], [0xc4, 0x05]];

// --- Packet framing -----------------------------------------------------------

/// A decoded packet split into its pattern / command / payload sections.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    pub pattern: [u8; 3],
    pub cmd: [u8; 2],
    pub payload: Vec<u8>,
}

/// Build a framed packet (header, length, pattern, cmd, payload, checksum).
pub fn build_packet(pattern: [u8; 3], cmd: [u8; 2], payload: &[u8]) -> Vec<u8> {
    let length = 2 + 2 + 3 + 2 + payload.len() + 1;
    let mut pkt = Vec::with_capacity(length);
    pkt.extend_from_slice(&[0xff, 0x09]);
    pkt.extend_from_slice(&(length as u16).to_le_bytes());
    pkt.extend_from_slice(&pattern);
    pkt.extend_from_slice(&cmd);
    pkt.extend_from_slice(payload);
    pkt.push(checksum(&pkt));
    pkt
}

/// Validate framing and split a received packet into its sections.
pub fn split_packet(packet: &[u8]) -> Result<Packet> {
    if packet.len() < 8 {
        return Err(AnkerError::BadPacket(format!(
            "too short: {} bytes",
            packet.len()
        )));
    }
    if packet[0..2] != [0xff, 0x09] {
        return Err(AnkerError::BadPacket("bad header (expected ff09)".into()));
    }
    let encoded_len = u16::from_le_bytes([packet[2], packet[3]]) as usize;
    if encoded_len != packet.len() {
        return Err(AnkerError::BadPacket(format!(
            "length field {} != actual {}",
            encoded_len,
            packet.len()
        )));
    }
    let expected = checksum(&packet[..packet.len() - 1]);
    let actual = packet[packet.len() - 1];
    if expected != actual {
        return Err(AnkerError::BadPacket(format!(
            "checksum {actual:#04x} != computed {expected:#04x}"
        )));
    }

    let mut pattern = [0u8; 3];
    pattern.copy_from_slice(&packet[4..7]);
    let mut cmd = [0u8; 2];
    cmd.copy_from_slice(&packet[7..9]);
    let payload = packet[9..packet.len() - 1].to_vec();

    Ok(Packet {
        pattern,
        cmd,
        payload,
    })
}

// --- TLV parameter parsing ----------------------------------------------------

/// Parsed telemetry parameters keyed by their id byte (`a1`, `a2`, ... as hex).
pub type Params = BTreeMap<String, Vec<u8>>;

/// Parse a decrypted payload into `{param_id: bytes}`.
///
/// Parameters are encoded as `<id:1><len:1><data:len>`. A leading `0x00` byte
/// is sometimes present and stripped. Parsing is best-effort: a truncated
/// trailing parameter stops the loop but preserves everything decoded so far.
pub fn parse_params(payload: &[u8]) -> Params {
    let mut out = Params::new();
    let mut i = 0usize;

    if payload.first() == Some(&0x00) {
        i = 1;
    }

    while i < payload.len() {
        let id = format!("{:02x}", payload[i]);
        i += 1;
        if i >= payload.len() {
            out.insert(id, Vec::new());
            break;
        }
        let len = payload[i] as usize;
        i += 1;
        if i + len > payload.len() {
            // Truncated parameter; keep what we have and stop.
            break;
        }
        out.insert(id, payload[i..i + len].to_vec());
        i += len;
    }

    out
}

/// Read a little-endian integer from a parameter's bytes.
///
/// `begin` skips leading bytes (parameter values are usually `<type:1><data>`,
/// so callers pass `begin = 1`).
pub fn param_int(params: &Params, key: &str, begin: usize, signed: bool) -> Option<i64> {
    param_int_range(params, key, begin, None, signed)
}

/// Read a little-endian integer from `bytes[begin..end]` of a parameter.
///
/// Gen 2 telemetry packs several fields into one parameter, so callers slice a
/// specific byte range (`end = None` reads to the end).
pub fn param_int_range(
    params: &Params,
    key: &str,
    begin: usize,
    end: Option<usize>,
    signed: bool,
) -> Option<i64> {
    let bytes = params.get(key)?;
    let end = end.unwrap_or(bytes.len());
    if begin > bytes.len() || end > bytes.len() || begin > end {
        return None;
    }
    let slice = &bytes[begin..end];
    let mut val: i64 = 0;
    for (i, &b) in slice.iter().enumerate() {
        val |= (b as i64) << (8 * i);
    }
    if signed && !slice.is_empty() {
        let bits = 8 * slice.len();
        if bits < 64 && (val & (1 << (bits - 1))) != 0 {
            val -= 1 << bits;
        }
    }
    Some(val)
}

/// Read an ASCII string from a parameter's bytes (skipping `begin` leading bytes).
pub fn param_string(params: &Params, key: &str, begin: usize) -> Option<String> {
    param_string_range(params, key, begin, None)
}

/// Read an ASCII string from `bytes[begin..end]` of a parameter, trimming any
/// trailing NULs / whitespace (`end = None` reads to the end).
pub fn param_string_range(
    params: &Params,
    key: &str,
    begin: usize,
    end: Option<usize>,
) -> Option<String> {
    let bytes = params.get(key)?;
    let end = end.unwrap_or(bytes.len()).min(bytes.len());
    if begin > end {
        return None;
    }
    let s = String::from_utf8_lossy(&bytes[begin..end]);
    Some(s.trim_matches(|c: char| c == '\0' || c.is_whitespace()).to_string())
}

// --- Fragment reassembly ------------------------------------------------------

/// Reassembles multi-fragment telemetry payloads.
///
/// The first byte of an encrypted telemetry payload encodes fragmentation:
/// the high nibble is the 1-based fragment index and the low nibble the total
/// fragment count. Single-fragment payloads (`total <= 1`) just drop that byte.
#[derive(Default)]
pub struct Fragmenter {
    buffers: std::collections::HashMap<[u8; 2], BTreeMap<u8, Vec<u8>>>,
    totals: std::collections::HashMap<[u8; 2], u8>,
}

impl Fragmenter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one telemetry payload; returns the full reassembled body once all
    /// fragments for `cmd` have arrived, otherwise `None`.
    pub fn push(&mut self, cmd: [u8; 2], payload: &[u8]) -> Option<Vec<u8>> {
        if payload.is_empty() {
            return None;
        }
        let index = (payload[0] >> 4) & 0x0f;
        let total = payload[0] & 0x0f;
        let body = &payload[1..];

        if total <= 1 {
            return Some(body.to_vec());
        }

        let buf = self.buffers.entry(cmd).or_default();
        if index == 1 {
            buf.clear();
            self.totals.insert(cmd, total);
        }
        buf.insert(index, body.to_vec());

        if buf.len() as u8 >= total {
            let assembled: Vec<u8> = buf.values().flatten().copied().collect();
            self.buffers.remove(&cmd);
            self.totals.remove(&cmd);
            Some(assembled)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_then_split_roundtrips() {
        let pkt = build_packet([0x03, 0x00, 0x0f], [0xc4, 0x02], &[0xa1, 0x01, 0x21]);
        let parsed = split_packet(&pkt).unwrap();
        assert_eq!(parsed.pattern, [0x03, 0x00, 0x0f]);
        assert_eq!(parsed.cmd, [0xc4, 0x02]);
        assert_eq!(parsed.payload, vec![0xa1, 0x01, 0x21]);
    }

    #[test]
    fn split_rejects_bad_checksum() {
        let mut pkt = build_packet([0x03, 0x00, 0x0f], [0x40, 0x40], &[0x01]);
        *pkt.last_mut().unwrap() ^= 0xff;
        assert!(split_packet(&pkt).is_err());
    }

    #[test]
    fn negotiation_command_0_parses() {
        // Sanity check the static handshake constant is valid framing.
        let bytes = hex::decode(NEGOTIATION_COMMAND_0).unwrap();
        let p = split_packet(&bytes).unwrap();
        assert_eq!(p.pattern, PATTERN_NEGOTIATION);
    }

    #[test]
    fn parse_params_tlv() {
        // 00 (skip) a1 02 01 21  bb 01 01
        let payload = [0x00, 0xa1, 0x02, 0x01, 0x21, 0xbb, 0x01, 0x01];
        let p = parse_params(&payload);
        assert_eq!(p.get("a1").unwrap(), &vec![0x01, 0x21]);
        assert_eq!(p.get("bb").unwrap(), &vec![0x01]);
        // battery percentage style read: skip type byte
        assert_eq!(param_int(&p, "a1", 1, false), Some(0x21));
    }

    #[test]
    fn param_int_signed() {
        let mut p = Params::new();
        p.insert("bd".into(), vec![0x00, 0xf6, 0xff]); // type, then -10 LE i16
        assert_eq!(param_int(&p, "bd", 1, true), Some(-10));
    }

    #[test]
    fn fragmenter_reassembles() {
        let mut f = Fragmenter::new();
        // total=2: index1 then index2, high nibble=index low nibble=total
        assert_eq!(f.push([0xc4, 0x02], &[0x12, 0xaa, 0xbb]), None);
        assert_eq!(
            f.push([0xc4, 0x02], &[0x22, 0xcc, 0xdd]),
            Some(vec![0xaa, 0xbb, 0xcc, 0xdd])
        );
    }

    #[test]
    fn fragmenter_single() {
        let mut f = Fragmenter::new();
        assert_eq!(
            f.push([0xc4, 0x02], &[0x11, 0x01, 0x02]),
            Some(vec![0x01, 0x02])
        );
    }
}
