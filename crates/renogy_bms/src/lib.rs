//! Renogy smart-battery protocol (Modbus RTU over BLE via BT-1/BT-2).
//!
//! Mirrors the shape of `jk_bms`/`sok_bms`: [`scan`], [`RenogyBms::connect_ble`],
//! [`RenogyBms::read`]. Ported from `cyrils/renogy-bt`.
//!
//! ```no_run
//! # async fn run() -> renogy_bms::Result<()> {
//! for dev in renogy_bms::scan(4).await? {
//!     println!("{} ({:?})", dev.id, dev.name);
//! }
//! let mut bms = renogy_bms::RenogyBms::connect_ble("<peripheral-id>").await?;
//! let d = bms.read().await?;
//! println!("SOC {:.0}% {:.2} V", d.soc, d.voltage);
//! # Ok(()) }
//! ```

pub mod bms;
pub mod error;
pub mod protocol;
pub mod transport;

pub use bms::RenogyBms;
pub use error::{Error, Result};
pub use protocol::RenogyData;

#[cfg(feature = "bluetooth")]
pub use transport::{scan, BtDevice};
