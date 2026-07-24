//! JBD / Xiaoxiang / Overkill Solar wire protocol — pure, no I/O.
//!
//! Frame layout (both directions):
//! ```text
//!   request:   DD  A5  <reg> <len> <data...>  <chk_hi> <chk_lo>  77
//!   response:  DD <reg> <status> <len> <data...>  <chk_hi> <chk_lo>  77
//! ```
//! `A5` = read, `5A` = write. The checksum is the 16-bit two's-complement of the
//! sum of every byte from index 2 up to (but excluding) the checksum — i.e.
//! `0x10000 - sum`. For a read of register `0x03` that is `0x10000 - 0x03 =
//! 0xFFFD`, giving `DD A5 03 00 FF FD 77`.

pub const START: u8 = 0xDD;
pub const END: u8 = 0x77;
pub const CMD_READ: u8 = 0xA5;
pub const CMD_WRITE: u8 = 0x5A;

/// Register addresses.
pub const REG_BASIC: u8 = 0x03; // pack summary (voltage/current/soc/temps/...)
pub const REG_CELLS: u8 = 0x04; // per-cell millivolts
pub const REG_HWVER: u8 = 0x05; // hardware/name string
pub const REG_MOSFET: u8 = 0xE1; // charge/discharge FET control (write)

/// 16-bit two's-complement checksum over `payload` (the bytes at index 2..end).
pub fn checksum(payload: &[u8]) -> [u8; 2] {
    let sum = payload
        .iter()
        .fold(0u16, |acc, &b| acc.wrapping_add(b as u16));
    let c = sum.wrapping_neg(); // 0x10000 - sum
    [(c >> 8) as u8, c as u8]
}

/// Build a read request for `reg`, e.g. `read_reg(0x03)`.
pub fn read_reg(reg: u8) -> [u8; 7] {
    let payload = [reg, 0x00];
    let [hi, lo] = checksum(&payload);
    [START, CMD_READ, reg, 0x00, hi, lo, END]
}

/// Build a write request for `reg` carrying `data`.
pub fn write_reg(reg: u8, data: &[u8]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(2 + data.len());
    payload.push(reg);
    payload.push(data.len() as u8);
    payload.extend_from_slice(data);
    let [hi, lo] = checksum(&payload);
    let mut frame = Vec::with_capacity(payload.len() + 5);
    frame.push(START);
    frame.push(CMD_WRITE);
    frame.extend_from_slice(&payload);
    frame.push(hi);
    frame.push(lo);
    frame.push(END);
    frame
}

/// MOSFET control frame. `charge`/`discharge` are the desired ON states.
///
/// Register `0xE1` takes two bytes `[0x00, ctrl]` where bit0 set = charge FET
/// **off**, bit1 set = discharge FET **off**.
pub fn set_mosfet(charge: bool, discharge: bool) -> Vec<u8> {
    let mut ctrl = 0u8;
    if !charge {
        ctrl |= 0x01;
    }
    if !discharge {
        ctrl |= 0x02;
    }
    write_reg(REG_MOSFET, &[0x00, ctrl])
}

/// A decoded response frame.
#[derive(Debug, Clone)]
pub struct Response {
    pub register: u8,
    pub ok: bool,
    pub data: Vec<u8>,
}

/// Validate and decode a single complete frame (`DD .. 77`).
pub fn decode(frame: &[u8]) -> Result<Response, &'static str> {
    if frame.len() < 7 {
        return Err("frame too short");
    }
    if frame[0] != START || frame[frame.len() - 1] != END {
        return Err("bad start/end marker");
    }
    let register = frame[1];
    let status = frame[2];
    let len = frame[3] as usize;
    // DD reg status len <len bytes> chk_hi chk_lo 77
    if frame.len() != 4 + len + 3 {
        return Err("length mismatch");
    }
    let payload = &frame[2..4 + len]; // status + len + data
    let data = &frame[4..4 + len];
    let expected = checksum(payload);
    if [frame[4 + len], frame[4 + len + 1]] != expected {
        return Err("checksum mismatch");
    }
    Ok(Response {
        register,
        ok: status == 0x00,
        data: data.to_vec(),
    })
}

fn u16be(d: &[u8], i: usize) -> u16 {
    ((d[i] as u16) << 8) | d[i + 1] as u16
}
fn i16be(d: &[u8], i: usize) -> i16 {
    u16be(d, i) as i16
}

/// Decoded `REG_BASIC` (0x03) payload, in real units.
#[derive(Debug, Clone, Default)]
pub struct BasicInfo {
    pub voltage: f32,        // V
    pub current: f32,        // A (+ charge / − discharge)
    pub power: f32,          // W
    pub remaining_ah: f32,   // Ah
    pub full_ah: f32,        // Ah
    pub cycles: u16,
    pub soc: u8,             // %
    pub charging: bool,      // charge FET on
    pub discharging: bool,   // discharge FET on
    pub balancing: u32,      // bitmask of cells currently balancing
    pub protection: u16,     // raw protection bitmask
    pub sw_version: u8,
    pub cell_count: u8,
    pub temps: Vec<f32>,     // °C, one per NTC
}

impl BasicInfo {
    /// Protection/alarm bits decoded to human strings.
    pub fn alarms(&self) -> Vec<String> {
        protection_to_strings(self.protection)
    }
}

/// Parse a `REG_BASIC` (0x03) data payload.
pub fn parse_basic(d: &[u8]) -> Result<BasicInfo, &'static str> {
    if d.len() < 23 {
        return Err("basic info payload too short");
    }
    let voltage = u16be(d, 0) as f32 * 0.01;
    let current = i16be(d, 2) as f32 * 0.01;
    let ntc = d[22] as usize;
    if d.len() < 23 + 2 * ntc {
        return Err("basic info temps truncated");
    }
    let mut temps = Vec::with_capacity(ntc);
    for k in 0..ntc {
        let raw = u16be(d, 23 + 2 * k) as f32;
        temps.push((raw - 2731.0) / 10.0); // 0.1 K → °C
    }
    let mos = d[20];
    Ok(BasicInfo {
        voltage,
        current,
        power: voltage * current,
        remaining_ah: u16be(d, 4) as f32 * 0.01,
        full_ah: u16be(d, 6) as f32 * 0.01,
        cycles: u16be(d, 8),
        balancing: ((u16be(d, 14) as u32) << 16) | u16be(d, 12) as u32,
        protection: u16be(d, 16),
        sw_version: d[18],
        soc: d[19],
        charging: mos & 0x01 != 0,
        discharging: mos & 0x02 != 0,
        cell_count: d[21],
        temps,
    })
}

/// Parse a `REG_CELLS` (0x04) data payload into per-cell volts.
pub fn parse_cells(d: &[u8]) -> Vec<f32> {
    d.chunks_exact(2)
        .map(|c| (((c[0] as u16) << 8) | c[1] as u16) as f32 * 0.001)
        .collect()
}

const PROTECTION_BITS: [&str; 13] = [
    "cell overvoltage",
    "cell undervoltage",
    "pack overvoltage",
    "pack undervoltage",
    "charge over-temperature",
    "charge under-temperature",
    "discharge over-temperature",
    "discharge under-temperature",
    "charge overcurrent",
    "discharge overcurrent",
    "short circuit",
    "front-end IC error",
    "MOSFET software lock",
];

/// Decode a protection bitmask into human-readable strings.
pub fn protection_to_strings(bits: u16) -> Vec<String> {
    (0..PROTECTION_BITS.len())
        .filter(|&i| bits & (1 << i) != 0)
        .map(|i| PROTECTION_BITS[i].to_string())
        .collect()
}

/// Reassembles frames from BLE notifications, which fragment at the MTU.
#[derive(Default)]
pub struct FrameAssembler {
    buf: Vec<u8>,
}

impl FrameAssembler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed raw bytes from a notification.
    pub fn push(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    /// Pop the next complete, checksum-valid frame if one is buffered.
    pub fn next_frame(&mut self) -> Option<Vec<u8>> {
        loop {
            // Drop bytes until a start marker.
            match self.buf.iter().position(|&b| b == START) {
                Some(0) => {}
                Some(p) => drop(self.buf.drain(..p)),
                None => {
                    self.buf.clear();
                    return None;
                }
            }
            if self.buf.len() < 4 {
                return None; // need at least DD reg status len
            }
            let total = 4 + self.buf[3] as usize + 3;
            if self.buf.len() < total {
                return None; // wait for more bytes
            }
            if self.buf[total - 1] == END && decode(&self.buf[..total]).is_ok() {
                return Some(self.buf.drain(..total).collect());
            }
            // Bad frame: discard the stale start byte and resync.
            self.buf.remove(0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_request_frames() {
        assert_eq!(read_reg(REG_BASIC), [0xDD, 0xA5, 0x03, 0x00, 0xFF, 0xFD, 0x77]);
        assert_eq!(read_reg(REG_CELLS), [0xDD, 0xA5, 0x04, 0x00, 0xFF, 0xFC, 0x77]);
    }

    #[test]
    fn checksum_roundtrip() {
        // status+len+data all zero → checksum 0x0000
        assert_eq!(checksum(&[0x00, 0x00]), [0x00, 0x00]);
        assert_eq!(checksum(&[0x03, 0x00]), [0xFF, 0xFD]);
    }

    #[test]
    fn mosfet_frames() {
        // both on → ctrl 0x00
        assert_eq!(set_mosfet(true, true), vec![0xDD, 0x5A, 0xE1, 0x02, 0x00, 0x00, 0xFF, 0x1D, 0x77]);
        // charge off → ctrl 0x01
        assert_eq!(set_mosfet(false, true)[5], 0x01);
        // discharge off → ctrl 0x02
        assert_eq!(set_mosfet(true, false)[5], 0x02);
        // both off → ctrl 0x03
        assert_eq!(set_mosfet(false, false)[5], 0x03);
    }

    fn build_response(reg: u8, status: u8, data: &[u8]) -> Vec<u8> {
        let mut payload = vec![status, data.len() as u8];
        payload.extend_from_slice(data);
        let [hi, lo] = checksum(&payload);
        let mut f = vec![START, reg];
        f.extend_from_slice(&payload);
        f.push(hi);
        f.push(lo);
        f.push(END);
        f
    }

    #[test]
    fn decode_basic_info() {
        // 4-cell, 1 NTC pack: 13.20 V, +5.00 A, 50/100 Ah, 7 cycles, soc 50,
        // both FETs on, 4 cells, temp 25.0 °C (raw 2981 = 298.1 K).
        let mut d = vec![0u8; 25];
        d[0..2].copy_from_slice(&1320u16.to_be_bytes()); // 13.20 V
        d[2..4].copy_from_slice(&500i16.to_be_bytes()); // 5.00 A
        d[4..6].copy_from_slice(&5000u16.to_be_bytes()); // 50 Ah
        d[6..8].copy_from_slice(&10000u16.to_be_bytes()); // 100 Ah
        d[8..10].copy_from_slice(&7u16.to_be_bytes()); // cycles
        d[16..18].copy_from_slice(&0u16.to_be_bytes()); // protection
        d[18] = 0x20; // sw version
        d[19] = 50; // soc
        d[20] = 0x03; // both FETs on
        d[21] = 4; // cells
        d[22] = 1; // ntc
        d[23..25].copy_from_slice(&2981u16.to_be_bytes()); // 25.0 °C

        let frame = build_response(REG_BASIC, 0x00, &d);
        let r = decode(&frame).expect("decode");
        assert_eq!(r.register, REG_BASIC);
        assert!(r.ok);

        let b = parse_basic(&r.data).expect("parse");
        assert!((b.voltage - 13.20).abs() < 1e-3);
        assert!((b.current - 5.00).abs() < 1e-3);
        assert!((b.remaining_ah - 50.0).abs() < 1e-3);
        assert!((b.full_ah - 100.0).abs() < 1e-3);
        assert_eq!(b.cycles, 7);
        assert_eq!(b.soc, 50);
        assert!(b.charging && b.discharging);
        assert_eq!(b.cell_count, 4);
        assert_eq!(b.temps.len(), 1);
        assert!((b.temps[0] - 25.0).abs() < 1e-3);
    }

    #[test]
    fn decode_cells() {
        let mut d = Vec::new();
        for mv in [3300u16, 3301, 3299, 3302] {
            d.extend_from_slice(&mv.to_be_bytes());
        }
        let frame = build_response(REG_CELLS, 0x00, &d);
        let r = decode(&frame).expect("decode");
        let cells = parse_cells(&r.data);
        assert_eq!(cells.len(), 4);
        assert!((cells[0] - 3.300).abs() < 1e-4);
        assert!((cells[3] - 3.302).abs() < 1e-4);
    }

    #[test]
    fn assembler_reassembles_fragments() {
        let mut d = vec![0u8; 25];
        d[21] = 4;
        d[22] = 1;
        let frame = build_response(REG_BASIC, 0x00, &d);
        let mut asm = FrameAssembler::new();
        // split across three notifications
        asm.push(&frame[..3]);
        assert!(asm.next_frame().is_none());
        asm.push(&frame[3..10]);
        assert!(asm.next_frame().is_none());
        asm.push(&frame[10..]);
        let got = asm.next_frame().expect("frame");
        assert_eq!(got, frame);
    }

    #[test]
    fn assembler_skips_leading_garbage() {
        let mut d = vec![0u8; 25];
        d[21] = 4;
        d[22] = 1;
        let frame = build_response(REG_BASIC, 0x00, &d);
        let mut asm = FrameAssembler::new();
        asm.push(&[0x00, 0xFF, 0x12]); // junk before start
        asm.push(&frame);
        let got = asm.next_frame().expect("frame");
        assert_eq!(got, frame);
    }

    #[test]
    fn protection_strings() {
        let s = protection_to_strings(0b1 | 0b100); // cell OV + pack OV
        assert_eq!(s, vec!["cell overvoltage", "pack overvoltage"]);
    }
}
