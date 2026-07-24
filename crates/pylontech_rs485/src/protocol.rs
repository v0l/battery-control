//! Pylontech RS485 **console** protocol (low-voltage US2000/US3000 family) —
//! pure, no I/O. Ported from `Frankkkkk/python-pylontech`.
//!
//! ASCII-hex frames: `~ VER ADR CID1 CID2 LENGTH INFO CHKSUM \r`, where every
//! field is hex text. This complements the Pylontech **CAN** decoder (it exposes
//! per-cell/per-module detail the inverter CAN frames don't).

pub const SOI: u8 = b'~';
pub const EOI: u8 = b'\r';
pub const CID1_BATTERY: u8 = 0x46;
/// Read analog/telemetry values.
pub const CID2_ANALOG: u8 = 0x42;

/// 16-bit two's-complement checksum over the ASCII frame body.
pub fn checksum(ascii: &[u8]) -> u16 {
    let sum: u32 = ascii.iter().map(|&b| b as u32).sum();
    ((0x1_0000 - (sum & 0xFFFF)) & 0xFFFF) as u16
}

/// The LENGTH field: 12-bit info length with a 4-bit checksum in the top nibble.
pub fn info_length(info_ascii_len: usize) -> u16 {
    let n = info_ascii_len as u16;
    if n == 0 {
        return 0;
    }
    let nib = (n & 0xf) + ((n >> 4) & 0xf) + ((n >> 8) & 0xf);
    let lchk = (0b1111 - (nib % 16) + 1) & 0xf;
    (lchk << 12) | n
}

fn hex_nibble(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'A'..=b'F' => Some(c - b'A' + 10),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    }
}

fn hex_decode(ascii: &[u8]) -> Option<Vec<u8>> {
    if ascii.len() % 2 != 0 {
        return None;
    }
    ascii
        .chunks_exact(2)
        .map(|c| Some((hex_nibble(c[0])? << 4) | hex_nibble(c[1])?))
        .collect()
}

/// Build a request frame (`~…\r`) for `cid2` addressed to `address`.
pub fn encode(address: u8, cid2: u8, info: &[u8]) -> Vec<u8> {
    let header = format!(
        "{:02X}{:02X}{:02X}{:02X}{:04X}",
        0x20,
        address,
        CID1_BATTERY,
        cid2,
        info_length(info.len())
    );
    let mut body = header.into_bytes();
    body.extend_from_slice(info);
    let chk = checksum(&body);
    let mut frame = vec![SOI];
    frame.extend_from_slice(&body);
    frame.extend_from_slice(format!("{chk:04X}").as_bytes());
    frame.push(EOI);
    frame
}

/// A decoded response: the command byte and the (hex-decoded) INFO payload.
#[derive(Debug, Clone)]
pub struct Response {
    pub cid2: u8,
    pub info: Vec<u8>,
}

/// Validate and decode a raw `~…\r` line.
pub fn decode(raw: &[u8]) -> Result<Response, &'static str> {
    let raw = raw.strip_suffix(&[EOI]).unwrap_or(raw);
    if raw.first() != Some(&SOI) {
        return Err("missing SOI");
    }
    let body = &raw[1..];
    if body.len() < 12 + 4 {
        return Err("frame too short");
    }
    let (frame_ascii, chk_ascii) = body.split_at(body.len() - 4);
    let want = u16::from_str_radix(std::str::from_utf8(chk_ascii).map_err(|_| "bad chksum")?, 16)
        .map_err(|_| "bad chksum")?;
    if checksum(frame_ascii) != want {
        return Err("checksum mismatch");
    }
    let decoded = hex_decode(frame_ascii).ok_or("bad hex")?;
    if decoded.len() < 6 {
        return Err("truncated header");
    }
    Ok(Response {
        cid2: decoded[3],
        info: decoded[6..].to_vec(),
    })
}

// --- analog (0x42) telemetry ---

fn i16be(d: &[u8], i: usize) -> i16 {
    ((d[i] as i16) << 8) | d[i + 1] as i16
}
fn u16be(d: &[u8], i: usize) -> u16 {
    ((d[i] as u16) << 8) | d[i + 1] as u16
}
fn u24be(d: &[u8], i: usize) -> u32 {
    ((d[i] as u32) << 16) | ((d[i + 1] as u32) << 8) | d[i + 2] as u32
}
fn to_celsius(raw: i16) -> f32 {
    (raw as f32 - 2731.0) / 10.0 // Kelvin×10
}

/// One battery module in the chain.
#[derive(Debug, Clone, Default)]
pub struct Module {
    pub cells: Vec<f32>,   // V
    pub temps: Vec<f32>,   // °C (avg BMS temp + grouped cell temps)
    pub current: f32,      // A
    pub voltage: f32,      // V
    pub remaining_ah: f32,
    pub total_ah: f32,
    pub cycles: u16,
}

/// A decoded analog snapshot (all modules on the bus).
#[derive(Debug, Clone, Default)]
pub struct PylontechData {
    pub modules: Vec<Module>,
}

impl PylontechData {
    pub fn voltage(&self) -> f32 {
        if self.modules.is_empty() {
            return 0.0;
        }
        self.modules.iter().map(|m| m.voltage).sum::<f32>() / self.modules.len() as f32
    }
    pub fn current(&self) -> f32 {
        self.modules.iter().map(|m| m.current).sum()
    }
    pub fn power(&self) -> f32 {
        self.modules.iter().map(|m| m.current * m.voltage).sum()
    }
    pub fn remaining_ah(&self) -> f32 {
        self.modules.iter().map(|m| m.remaining_ah).sum()
    }
    pub fn total_ah(&self) -> f32 {
        self.modules.iter().map(|m| m.total_ah).sum()
    }
    pub fn soc(&self) -> f32 {
        let t = self.total_ah();
        if t > 0.0 {
            self.remaining_ah() / t * 100.0
        } else {
            0.0
        }
    }
}

/// Parse the INFO payload of an analog (0x42) response. `info` is the
/// hex-decoded payload including the leading info flag byte.
pub fn parse_analog(info: &[u8]) -> Result<PylontechData, &'static str> {
    if info.len() < 2 {
        return Err("analog payload too short");
    }
    let mut c = 1; // skip info flag
    let num_modules = info[c] as usize;
    c += 1;
    let mut modules = Vec::with_capacity(num_modules);

    let need = |c: usize, n: usize| -> Result<(), &'static str> {
        if c + n > info.len() {
            Err("analog payload truncated")
        } else {
            Ok(())
        }
    };

    for _ in 0..num_modules {
        need(c, 1)?;
        let ncells = info[c] as usize;
        c += 1;
        need(c, 2 * ncells)?;
        let cells = (0..ncells)
            .map(|i| i16be(info, c + 2 * i) as f32 / 1000.0)
            .collect();
        c += 2 * ncells;

        need(c, 1)?;
        let ntemps = info[c] as usize;
        c += 1;
        need(c, 2 * ntemps)?;
        let temps = (0..ntemps)
            .map(|i| to_celsius(i16be(info, c + 2 * i)))
            .collect();
        c += 2 * ntemps;

        need(c, 2 + 2 + 2 + 1 + 2 + 2)?;
        let current = i16be(info, c) as f32 / 10.0;
        let voltage = u16be(info, c + 2) as f32 / 1000.0;
        let rem1 = u16be(info, c + 4) as f32 / 1000.0;
        let udi = info[c + 6];
        let tot1 = u16be(info, c + 7) as f32 / 1000.0;
        let cycles = u16be(info, c + 9);
        c += 11;

        let (remaining_ah, total_ah) = if udi > 2 {
            need(c, 6)?;
            let rem2 = u24be(info, c) as f32 / 1000.0;
            let tot2 = u24be(info, c + 3) as f32 / 1000.0;
            c += 6;
            (rem2, tot2)
        } else {
            (rem1, tot1)
        };

        modules.push(Module {
            cells,
            temps,
            current,
            voltage,
            remaining_ah,
            total_ah,
            cycles,
        });
    }
    Ok(PylontechData { modules })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real capture: US2000 3-module analog response (python-pylontech tests).
    const FRAME: &[u8] = b"~20024600914211030F0CE70CE80CE60CE70CE80CE80CE80CE60CE50CE60CE80CE70CEA0CE50CE6050B910B870B870B870B87FFE6C18982DC02C350001F0F0CE20CE60CE60CE10CE50CE70CE60CE30CE20CE50CE30CE90CE70CE90CE9050B910B870B870B870B87FFE7C17082DC02C350001F0F0CE20CE50CE50CE20CE30CE30CE40CE50CE60CE60CE30CE40CE40CE60CE6050B910B7D0B7D0B7D0B7DFFE5C16082DC02C350001FB476\r";

    #[test]
    fn checksum_and_lenid() {
        // Known request: get analog values, address 2, info "FF".
        let f = encode(2, CID2_ANALOG, b"FF");
        assert_eq!(f[0], SOI);
        assert_eq!(*f.last().unwrap(), EOI);
        // round-trips through decode (checksum valid)
        assert!(decode(&f).is_ok());
    }

    #[test]
    fn decode_real_frame() {
        let r = decode(FRAME).expect("decode");
        assert_eq!(r.cid2, 0x00); // response OK
        let d = parse_analog(&r.info).expect("parse");
        assert_eq!(d.modules.len(), 3);

        let m = &d.modules[0];
        assert_eq!(m.cells.len(), 15);
        assert!((m.cells[0] - 3.303).abs() < 1e-3); // 0x0CE7 = 3303 mV
        assert!((m.current + 2.6).abs() < 1e-2); // -2.6 A
        assert!((m.voltage - 49.545).abs() < 1e-2);
        assert_eq!(m.cycles, 31);
        assert_eq!(m.temps.len(), 5);
        assert!((m.temps[0] - 23.0).abs() < 1e-2); // avg BMS temp
        assert!((m.remaining_ah - 33.5).abs() < 1e-1);
        assert!((m.total_ah - 50.0).abs() < 1e-1);

        // Aggregate across the 3 modules.
        assert!((d.current() + 7.8).abs() < 0.2); // ~ -2.6*3
        assert!((d.soc() - 67.0).abs() < 1.0);
    }
}
