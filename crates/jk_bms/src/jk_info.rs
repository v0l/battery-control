use crate::pack::{MybmmPack, MAX_CELLS, MAX_TEMPS};

pub const INFO_MAX_STRINGS: usize = 32;
pub const INFO_MAX_TEMP_PROBES: usize = 6;

#[derive(Debug, Clone, Default)]
pub struct JkInfo {
    pub model: String,
    pub hwvers: String,
    pub swvers: String,
    pub uptime: u32,
    pub device: String,
    pub pin: String,
    pub num1: String,
    pub num2: String,
    pub pass: String,
    pub voltage: f32,
    pub current: f32,
    pub protectbits: u16,
    pub state: u16,
    pub strings: usize,
    pub cellvolt: [f32; MAX_CELLS],
    pub cellres: [f32; MAX_CELLS],
    pub cell_total: f32,
    pub cell_min: f32,
    pub cell_max: f32,
    pub cell_diff: f32,
    pub cell_avg: f32,
    pub probes: usize,
    pub temps: [f32; INFO_MAX_TEMP_PROBES],
}

impl JkInfo {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_pack(pack: &MybmmPack) -> Self {
        let mut info = Self::new();
        info.voltage = pack.voltage;
        info.current = pack.current;
        info.strings = pack.cells as usize;
        info.probes = pack.ntemps as usize;
        info.cellvolt[..MAX_CELLS].copy_from_slice(&pack.cellvolt[..MAX_CELLS]);
        for i in 0..INFO_MAX_TEMP_PROBES {
            if i < MAX_TEMPS {
                info.temps[i] = pack.temps[i];
            }
        }
        info.cell_total = pack.cellvolt[..pack.cells as usize].iter().sum();
        if pack.cells > 0 {
            let slice = &pack.cellvolt[..pack.cells as usize];
            info.cell_min = slice.iter().copied().fold(f32::INFINITY, f32::min);
            info.cell_max = slice.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            info.cell_diff = info.cell_max - info.cell_min;
            info.cell_avg = info.cell_total / pack.cells as f32;
        }
        info
    }
}

pub fn parse_info_strings(info: &mut JkInfo, data: &[u8]) {
    let mut i = 6;
    let mut strings = vec![];

    while i < 300 && data[i] != 0 {
        let start = i;
        while i < 300 && data[i] != 0 {
            i += 1;
        }
        if start < i {
            let s = String::from_utf8_lossy(&data[start..i]).to_string();
            strings.push(s);
        }
        while i < 300 && data[i] == 0 {
            i += 1;
        }
    }

    let mut idx = 0;
    if idx < strings.len() { info.model = strings[idx].clone(); idx += 1; }
    if idx < strings.len() { info.hwvers = strings[idx].clone(); idx += 1; }
    if idx < strings.len() { info.swvers = strings[idx].clone(); idx += 1; }
    if i + 8 <= data.len() {
        info.uptime = u32::from_le_bytes([data[i], data[i+1], data[i+2], data[i+3]]);
        i += 8;
    }
    while i < 300 && data[i] == 0 { i += 1; }
    if idx < strings.len() { info.device = strings[idx].clone(); idx += 1; }
    if idx < strings.len() { info.pin = strings[idx].clone(); idx += 1; }
    if idx < strings.len() { info.num1 = strings[idx].clone(); idx += 1; }
    if idx < strings.len() { info.num2 = strings[idx].clone(); idx += 1; }
    if idx < strings.len() { info.pass = strings[idx].clone(); }
}
