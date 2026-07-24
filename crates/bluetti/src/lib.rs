//! Bluetti power station protocol — **local BLE only**, no cloud, no account.
//!
//! Bluetti stations speak Modbus RTU over BLE (function 0x03 read / 0x06
//! write). This covers the plaintext models (AC200M/AC300/EB3A/EP500 family);
//! newer encrypted firmware is a planned follow-up. Ported from
//! `warhammerkid/bluetti_mqtt`.
//!
//! ```no_run
//! # async fn run() -> bluetti::Result<()> {
//! for dev in bluetti::scan(4).await? {
//!     println!("{} ({:?})", dev.id, dev.name);
//! }
//! let mut b = bluetti::Bluetti::connect_ble("<peripheral-id>").await?;
//! let d = b.read().await?;
//! println!("{}% in {} W out {} W", d.total_battery_percent, d.input_power(), d.output_power());
//! b.set_ac_output(true).await?;
//! # Ok(()) }
//! ```

pub mod bms;
pub mod error;
pub mod protocol;
pub mod transport;

pub use bms::Bluetti;
pub use error::{Error, Result};
pub use protocol::BluettiData;

#[cfg(feature = "bluetooth")]
pub use transport::{scan, BtDevice};
