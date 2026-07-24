//! A tiny protobuf wire-format walker — enough to pull a few telemetry fields
//! without generating the (very large, device-specific) EcoFlow `.proto` tree.
//!
//! Field mappings were read off the reference capture in `ef-ble-reverse`; they
//! are best-effort and want validation on real hardware.

/// Read a base-128 varint at `*pos`, advancing it. Returns `None` on overrun.
fn read_varint(buf: &[u8], pos: &mut usize) -> Option<u64> {
    let mut result = 0u64;
    let mut shift = 0;
    while *pos < buf.len() {
        let byte = buf[*pos];
        *pos += 1;
        result |= ((byte & 0x7f) as u64) << shift;
        if byte & 0x80 == 0 {
            return Some(result);
        }
        shift += 7;
        if shift >= 64 {
            return None;
        }
    }
    None
}

/// Skip a field of the given wire type, advancing `*pos`.
fn skip(buf: &[u8], pos: &mut usize, wire: u64) -> Option<()> {
    match wire {
        0 => {
            read_varint(buf, pos)?;
        }
        1 => *pos += 8,
        5 => *pos += 4,
        2 => {
            let len = read_varint(buf, pos)? as usize;
            *pos += len;
        }
        _ => return None,
    }
    if *pos > buf.len() {
        None
    } else {
        Some(())
    }
}

/// Find the first length-delimited (wire type 2) field with the given number.
pub fn find_bytes(buf: &[u8], field: u64) -> Option<&[u8]> {
    let mut pos = 0;
    while pos < buf.len() {
        let tag = read_varint(buf, &mut pos)?;
        let (num, wire) = (tag >> 3, tag & 7);
        if wire == 2 {
            let len = read_varint(buf, &mut pos)? as usize;
            let end = pos + len;
            if end > buf.len() {
                return None;
            }
            if num == field {
                return Some(&buf[pos..end]);
            }
            pos = end;
        } else {
            skip(buf, &mut pos, wire)?;
        }
    }
    None
}

/// Find the first varint (wire type 0) field with the given number.
pub fn find_varint(buf: &[u8], field: u64) -> Option<u64> {
    let mut pos = 0;
    while pos < buf.len() {
        let tag = read_varint(buf, &mut pos)?;
        let (num, wire) = (tag >> 3, tag & 7);
        if wire == 0 {
            let v = read_varint(buf, &mut pos)?;
            if num == field {
                return Some(v);
            }
        } else {
            skip(buf, &mut pos, wire)?;
        }
    }
    None
}

/// Find the first fixed32 (wire type 5) field with the given number, as f32.
pub fn find_f32(buf: &[u8], field: u64) -> Option<f32> {
    let mut pos = 0;
    while pos < buf.len() {
        let tag = read_varint(buf, &mut pos)?;
        let (num, wire) = (tag >> 3, tag & 7);
        if wire == 5 {
            if pos + 4 > buf.len() {
                return None;
            }
            let bytes: [u8; 4] = buf[pos..pos + 4].try_into().ok()?;
            pos += 4;
            if num == field {
                return Some(f32::from_le_bytes(bytes));
            }
        } else {
            skip(buf, &mut pos, wire)?;
        }
    }
    None
}

/// Decoded EcoFlow telemetry (best-effort, HD31 Smart Home Panel 2).
#[derive(Debug, Clone, Default)]
pub struct Telemetry {
    /// State of charge (%), from `backup_incre_info.cur_discharge_soc` or
    /// `backup_bat_per`.
    pub soc: Option<f64>,
}

/// Field numbers in `ProtoPushAndSet` / `backup_incre_info` from the reference
/// capture.
const F_BACKUP_INCRE_INFO: u64 = 80;
const F_BACKUP_BAT_PER: u64 = 3; // varint %
const F_CUR_DISCHARGE_SOC: u64 = 5; // fixed32 %

/// Best-effort decode of an HD31 push payload (cmd_set 0x0C, cmd_id 0x20/0x21).
pub fn decode_hd31(payload: &[u8]) -> Telemetry {
    let mut t = Telemetry::default();
    if let Some(backup) = find_bytes(payload, F_BACKUP_INCRE_INFO) {
        if let Some(soc) = find_f32(backup, F_CUR_DISCHARGE_SOC) {
            t.soc = Some(soc as f64);
        } else if let Some(pct) = find_varint(backup, F_BACKUP_BAT_PER) {
            t.soc = Some(pct as f64);
        }
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walks_nested_backup_info() {
        // backup_incre_info(field 80) { backup_bat_per(3)=46, cur_discharge_soc(5)=46.5f }
        let mut inner = vec![0x18, 0x2e]; // field 3 varint = 46
        inner.extend_from_slice(&[0x2d]); // field 5 fixed32
        inner.extend_from_slice(&46.5f32.to_le_bytes());

        // outer: tag for field 80, wire 2 => (80<<3)|2 = 642 => varint 0x82 0x05
        let mut msg = vec![0x82, 0x05, inner.len() as u8];
        msg.extend_from_slice(&inner);

        let t = decode_hd31(&msg);
        assert_eq!(t.soc, Some(46.5));
    }

    #[test]
    fn varint_and_bytes_lookup() {
        // field 1 varint = 300 (0xac 0x02), field 2 bytes = "hi"
        let msg = [0x08, 0xac, 0x02, 0x12, 0x02, b'h', b'i'];
        assert_eq!(find_varint(&msg, 1), Some(300));
        assert_eq!(find_bytes(&msg, 2), Some(&b"hi"[..]));
        assert_eq!(find_varint(&msg, 9), None);
    }
}
