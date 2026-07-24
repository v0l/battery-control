//! Seplos BMS V3 protocol (Modbus RTU over RS485, function 0x04).
//!
//! Reads the PIA (status) and PIB (cells/temps) input-register blocks from a
//! Seplos V3 rack pack. Mirrors the other backends: [`SeplosBms::open_serial`],
//! [`SeplosBms::read`]. Ported from `marcelrv/seplosBMSv3`.
//!
//! ```no_run
//! # async fn run() -> seplos_bms::Result<()> {
//! // BMSStudio "client 1" == Modbus address 0.
//! let mut bms = seplos_bms::SeplosBms::open_serial("/dev/ttyUSB0", 19200, 0).await?;
//! let d = bms.read().await?;
//! println!("SOC {:.1}% {:.2} V", d.soc, d.voltage);
//! # Ok(()) }
//! ```

pub mod bms;
pub mod error;
pub mod protocol;
pub mod transport;

pub use bms::SeplosBms;
pub use error::{Error, Result};
pub use protocol::SeplosData;
