//! SOK Bluetooth LiFePO4 battery protocol.
//!
//! A small driver for SOK 12 V batteries, which speak a custom `0xEE`-command
//! protocol (CRC-8/MAXIM) over BLE (service `FFE0`). Mirrors the shape of
//! `jk_bms`/`jbd_bms`: [`scan`], [`SokBms::connect_ble`], [`SokBms::read`].
//! Ported from `IAmTheMitchell/sok-ble`.
//!
//! ```no_run
//! # async fn run() -> sok_bms::Result<()> {
//! for dev in sok_bms::scan(4).await? {
//!     println!("{} ({:?})", dev.id, dev.name);
//! }
//! let mut bms = sok_bms::SokBms::connect_ble("<peripheral-id>").await?;
//! let d = bms.read().await?;
//! println!("SOC {}% {:.2} V {:.1} Ah", d.soc, d.voltage, d.capacity);
//! # Ok(()) }
//! ```

pub mod bms;
pub mod error;
pub mod protocol;
pub mod transport;

pub use bms::SokBms;
pub use error::{Error, Result};
pub use protocol::{crc8, SokData};

#[cfg(feature = "bluetooth")]
pub use transport::{scan, BtDevice};
