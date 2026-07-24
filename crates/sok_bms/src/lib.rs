//! SOK Bluetooth LiFePO4 battery protocols.
//!
//! Supports both SOK BLE generations, auto-detected at connect time:
//! - **EE** — older 12V packs speaking `0xEE` command frames (service `FFE0`),
//!   ported from `IAmTheMitchell/sok-ble`.
//! - **ABC** — the "ABC BMS" app: Modbus RTU over BLE (service `FFF0`), ported
//!   from `node-red-contrib/node-red-contrib-sok`.
//!
//! Mirrors the shape of `jk_bms`/`jbd_bms`: [`scan`], [`SokBms::connect_ble`],
//! [`SokBms::read`].
//!
//! ```no_run
//! # async fn run() -> sok_bms::Result<()> {
//! for dev in sok_bms::scan(4).await? {
//!     println!("{} ({:?}) variant={:?}", dev.id, dev.name, dev.variant);
//! }
//! let mut bms = sok_bms::SokBms::connect_ble("<peripheral-id>").await?;
//! let d = bms.read().await?;
//! println!("SOC {}% {:.2} V {:.1} Ah", d.soc, d.voltage, d.capacity);
//! # Ok(()) }
//! ```

pub mod abc;
pub mod bms;
pub mod data;
pub mod ee;
pub mod error;
pub mod transport;

pub use bms::SokBms;
pub use data::{SokData, Variant};
pub use error::{Error, Result};

#[cfg(feature = "bluetooth")]
pub use transport::{scan, BtDevice};
