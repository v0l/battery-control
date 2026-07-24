//! Pylontech RS485 **console** protocol (low-voltage US2000/US3000 family).
//!
//! Reads the module chain over the Console (RS485/RS232) port, exposing
//! per-cell and per-module detail. Complements the Pylontech CAN decoder.
//! Ported from `Frankkkkk/python-pylontech`.
//!
//! ```no_run
//! # async fn run() -> pylontech_rs485::Result<()> {
//! let mut bms = pylontech_rs485::PylontechRs485::open_serial("/dev/ttyUSB0", 115200, 2).await?;
//! let d = bms.read().await?;
//! println!("{} modules, SOC {:.0}%", d.modules.len(), d.soc());
//! # Ok(()) }
//! ```

pub mod bms;
pub mod error;
pub mod protocol;
pub mod transport;

pub use bms::PylontechRs485;
pub use error::{Error, Result};
pub use protocol::{Module, PylontechData};
