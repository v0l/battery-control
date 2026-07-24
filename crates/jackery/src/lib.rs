//! Jackery portable power station protocol — **local BLE only**, no cloud.
//!
//! The encryption key is derived from the BLE advertisement (no manual entry).
//! Most portable models use RC4 (implemented here); a couple of models and the
//! Box devices use AES-128-CBC (follow-up). Ported from `porcupin26/private_jack`.
//!
//! ```no_run
//! # async fn run() -> jackery::Result<()> {
//! for dev in jackery::scan(6).await? {
//!     println!("{} {} ({})", dev.id, dev.serial, jackery::model_name(dev.model));
//!     let mut j = jackery::Jackery::connect_ble(&dev.id, dev.key, dev.model, dev.serial).await?;
//!     let d = j.read().await?;
//!     println!("SOC {}%  in {} W  out {} W", d.rb, d.ip, d.op);
//! }
//! # Ok(()) }
//! ```

pub mod bms;
pub mod command;
pub mod crypto;
pub mod data;
pub mod error;
pub mod key;
pub mod transport;

pub use bms::Jackery;
pub use data::JackeryData;
pub use error::{Error, Result};
pub use key::AdvInfo;

#[cfg(feature = "bluetooth")]
pub use transport::{scan, BtDevice};

/// Human name for a Jackery model code (from `porcupin26/private_jack`).
pub fn model_name(code: u16) -> String {
    let name = match code {
        1 => "E3000Pro",
        2 => "E2000Plus",
        4 => "E300Plus",
        5 => "E1000Plus",
        6 => "E700Plus",
        7 => "E280Plus",
        8 => "E1000Pro2",
        9 => "E240",
        10 => "E600Plus",
        12 => "E2000Pro2",
        13 => "E5000Plus",
        14 => "E3000",
        15 => "E900",
        16 => "E1800",
        17 => "E1500Ultra",
        18 => "E1100Pro2",
        19 => "HP3000",
        20 => "HP3600",
        21 => "E1500V2",
        22 => "HP5000Plus",
        _ => return format!("Jackery (model {code})"),
    };
    name.to_string()
}
