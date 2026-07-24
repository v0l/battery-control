//! # battery_control
//!
//! One async trait — [`Battery`] — to monitor and control many different
//! batteries, BMSes and power stations through a single normalized data model.
//!
//! Each device family is a **feature-gated backend adapter** over an existing
//! crate or a native protocol decoder:
//!
//! | Feature | Backend | Class | Control |
//! |---------|---------|-------|---------|
//! | `anker` | Anker SOLIX (BLE) | power station | ports |
//! | `jk` | JK BMS (serial/BLE/CAN) | cell BMS | MOSFETs, settings |
//! | `daly` | Daly BMS (serial) | cell BMS | MOSFETs |
//! | `victron` | Victron (BLE Instant Readout) | monitor | read-only |
//! | `pylontech-can` | Pylontech CAN (EG4/SOK/…) | rack | read-only |
//!
//! Devices differ wildly, so every [`BatteryStatus`] field is optional and
//! control commands are gated by [`Capabilities`]; unsupported commands return
//! [`Error::Unsupported`].
//!
//! ## Adding a backend
//!
//! 1. Add the upstream crate as an optional dependency and a matching feature.
//! 2. Create `src/backends/<name>.rs` with a newtype adapter that implements
//!    [`Battery`]: map the device's telemetry into [`BatteryStatus`], advertise
//!    [`Capabilities`], and translate [`Command`]s in `execute`.
//! 3. Re-export it from `backends/mod.rs` behind the feature.
//!
//! Blocking (sync) upstreams should be driven with `tokio::task::spawn_blocking`
//! or `block_in_place` inside the async methods (see the `jk` adapter).

mod battery;
mod capabilities;
mod error;
mod types;

pub mod auth;
pub mod backends;
pub mod credentials;
pub mod discovery;

pub use auth::{AuthInput, AuthState};
pub use battery::{Battery, StatusStream};
pub use capabilities::Capabilities;
pub use credentials::{CredentialStore, FileStore};
pub use discovery::{discover, resolve, DeviceClass, Discovered, DiscoverOptions};
pub use error::{Error, Result};
pub use types::{
    BatteryStatus, CellInfo, Command, DeviceInfo, PortDirection, PortInfo, Reading, Sensor,
    Setting, SettingKind, SettingValue, StatusUpdate, Switch, SwitchId, UnknownId, Unit,
    TEMP_PREFIX,
};
