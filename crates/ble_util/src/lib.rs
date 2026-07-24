//! Shared BLE helpers for the battery backends: 16-bit UUID expansion and the
//! standard **Device Information Service** (`0x180A`) identity read.
//!
//! [`Identity`] and [`uuid16`] are always available (no BLE stack needed).
//! [`read_identity`] requires the `gatt` feature (pulls in `btleplug`).

use uuid::Uuid;

/// Expand a 16-bit Bluetooth SIG UUID alias into its full 128-bit form, e.g.
/// `uuid16(0xffe0)` → `0000ffe0-0000-1000-8000-00805f9b34fb`.
pub const fn uuid16(x: u16) -> Uuid {
    Uuid::from_u128(0x0000_0000_0000_1000_8000_0080_5f9b_34fb | ((x as u128) << 96))
}

/// Static device identity, mostly from the BLE Device Information Service
/// (`0x180A`), plus the advertised name.
#[derive(Debug, Clone, Default)]
pub struct Identity {
    /// The advertised BLE name — usually the pack's best human label (e.g.
    /// `"SOK-AA52810"`), distinct from the DIS `model` (often just the BLE
    /// module, e.g. `"BK-BLE-1.0"`).
    pub name: Option<String>,
    /// Manufacturer name (`0x2A29`).
    pub manufacturer: Option<String>,
    /// Model number (`0x2A24`).
    pub model: Option<String>,
    /// Serial number (`0x2A25`).
    pub serial: Option<String>,
    /// Firmware revision (`0x2A26`).
    pub firmware: Option<String>,
    /// Hardware revision (`0x2A27`).
    pub hardware: Option<String>,
}

#[cfg(feature = "gatt")]
mod gatt {
    use super::{uuid16, Identity};
    use btleplug::api::{Characteristic, Peripheral as _};
    use btleplug::platform::Peripheral;
    use std::collections::BTreeSet;
    use uuid::Uuid;

    const DIS_MANUFACTURER: Uuid = uuid16(0x2a29);
    const DIS_MODEL: Uuid = uuid16(0x2a24);
    const DIS_SERIAL: Uuid = uuid16(0x2a25);
    const DIS_FIRMWARE: Uuid = uuid16(0x2a26);
    const DIS_HARDWARE: Uuid = uuid16(0x2a27);

    async fn read_string(
        p: &Peripheral,
        chars: &BTreeSet<Characteristic>,
        uuid: Uuid,
    ) -> Option<String> {
        let c = chars.iter().find(|c| c.uuid == uuid)?;
        let bytes = p.read(c).await.ok()?;
        let txt = String::from_utf8_lossy(&bytes)
            .trim_matches(|c: char| c == '\0' || c.is_whitespace())
            .to_string();
        (!txt.is_empty()).then_some(txt)
    }

    /// Read the Device Information Service strings and advertised name from an
    /// already-connected peripheral whose services have been discovered.
    /// Missing/unreadable fields are simply left `None`.
    pub async fn read_identity(p: &Peripheral) -> Identity {
        let chars = p.characteristics();
        let name = p
            .properties()
            .await
            .ok()
            .flatten()
            .and_then(|pr| pr.local_name);
        Identity {
            name,
            manufacturer: read_string(p, &chars, DIS_MANUFACTURER).await,
            model: read_string(p, &chars, DIS_MODEL).await,
            serial: read_string(p, &chars, DIS_SERIAL).await,
            firmware: read_string(p, &chars, DIS_FIRMWARE).await,
            hardware: read_string(p, &chars, DIS_HARDWARE).await,
        }
    }
}

#[cfg(feature = "gatt")]
pub use gatt::read_identity;
