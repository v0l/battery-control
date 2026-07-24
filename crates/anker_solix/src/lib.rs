//! `anker_solix` — a lightweight library for monitoring and controlling Anker
//! SOLIX portable power stations over Bluetooth LE, with no cloud or app.
//!
//! The heavy lifting lives in [`device::Device`]: it discovers a station,
//! performs the fixed ECDH handshake, and then exchanges AES-128-CBC encrypted
//! telemetry and control packets.
//!
//! ```no_run
//! use anker_solix::Device;
//! use std::time::Duration;
//!
//! # async fn demo() -> anker_solix::Result<()> {
//! let mut dev = Device::find_and_connect("C1000", 5).await?;
//! let t = dev.next_telemetry(Duration::from_secs(10)).await?;
//! println!("battery: {:?}%", t.battery_percentage);
//! dev.set_ac(true).await?;
//! # Ok(())
//! # }
//! ```

pub mod crypto;
pub mod device;
pub mod error;
pub mod model;
pub mod opcode;
pub mod protocol;

pub use device::{scan, Device, Discovered};
pub use error::{AnkerError, Result};
pub use model::{Brightness, Model, Port, PortStatus, Telemetry};
