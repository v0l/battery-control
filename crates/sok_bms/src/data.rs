//! Unified snapshot produced by both SOK protocol variants.

/// Which on-the-wire protocol a device speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    /// Older 12V packs: `0xEE` command frames on service `FFE0`.
    Ee,
    /// ABC-BMS: Modbus RTU over BLE on service `FFF0` (the "ABC BMS" app).
    Abc,
}

impl Variant {
    pub fn as_str(&self) -> &'static str {
        match self {
            Variant::Ee => "ee",
            Variant::Abc => "abc",
        }
    }
}

/// A decoded pack snapshot. The `Ee` variant fills a subset (no per-probe temps,
/// model or serial); `Abc` fills everything it decodes.
#[derive(Debug, Clone, Default)]
pub struct SokData {
    pub voltage: f32,          // V
    pub current: f32,          // A (+ charge / − discharge)
    pub power: f32,            // W
    pub soc: u16,              // %
    pub temperature: f32,      // °C (primary probe)
    pub temps: Vec<f32>,       // all probes (Abc: cell1, cell2, mos, environment)
    pub capacity: f32,         // rated/full Ah
    pub remaining: Option<f32>, // remaining Ah
    pub cycles: Option<u16>,
    pub cells: Vec<f32>, // per-cell V
    pub model: Option<String>,
    pub serial: Option<String>,
}
