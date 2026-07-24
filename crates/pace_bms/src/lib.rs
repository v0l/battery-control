//! PACE-BMS `PACE_MODBUS` protocol (Modbus RTU over RS485).
//!
//! One backend for the many rebranded 48V rack batteries built on the PACE
//! chipset. Enable the RS485 protocol in PbmsTools (System Config → Inverter
//! protocol → RS485 Protocol), use the RS485 port next to the CAN port.
//! Mirrors the shape of the other backends: [`PaceBms::open_serial`],
//! [`PaceBms::read`]. Ported from `syssi/esphome-pace-bms`.
//!
//! ```no_run
//! # async fn run() -> pace_bms::Result<()> {
//! let mut bms = pace_bms::PaceBms::open_serial("/dev/ttyUSB0", 9600, 1).await?;
//! let d = bms.read().await?;
//! println!("SOC {}% {:.2} V", d.soc, d.voltage);
//! # Ok(()) }
//! ```

pub mod bms;
pub mod error;
pub mod protocol;
pub mod transport;

pub use bms::PaceBms;
pub use error::{Error, Result};
pub use protocol::PaceData;
