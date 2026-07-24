//! JBD / Xiaoxiang / Overkill Solar / LLT BMS protocol.
//!
//! A small, dependency-light driver for the `0xDD`-framed protocol these
//! BMSes speak over BLE (service `FF00`) and UART. Mirrors the shape of
//! `jk_bms`/`anker_solix`: [`scan`], [`JbdBms::connect_ble`], [`JbdBms::read`],
//! [`JbdBms::set`].
//!
//! ```no_run
//! # async fn run() -> jbd_bms::Result<()> {
//! for dev in jbd_bms::scan(4).await? {
//!     println!("{} ({:?})", dev.id, dev.name);
//! }
//! let mut bms = jbd_bms::JbdBms::connect_ble("<peripheral-id>").await?;
//! let data = bms.read().await?;
//! println!("SOC {}% {:.2} V", data.basic.soc, data.basic.voltage);
//! # Ok(()) }
//! ```

pub mod bms;
pub mod error;
pub mod protocol;
pub mod transport;

pub use bms::{JbdBms, JbdData};
pub use error::{Error, Result};
pub use protocol::{
    parse_basic, parse_cells, protection_to_strings, BasicInfo, FrameAssembler, Response,
};

#[cfg(feature = "bluetooth")]
pub use transport::{scan, BtDevice};
