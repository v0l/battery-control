//! Backend adapters. Each is gated behind its own cargo feature so you only
//! compile (and pull the transitive deps of) the devices you actually use.

#[cfg(feature = "anker")]
pub mod anker;
#[cfg(feature = "anker")]
pub use anker::AnkerBattery;

#[cfg(any(feature = "jk-ble", feature = "jk-serial"))]
pub mod jk;
#[cfg(any(feature = "jk-ble", feature = "jk-serial"))]
pub use jk::JkBattery;

#[cfg(any(feature = "jbd-ble", feature = "jbd-serial"))]
pub mod jbd;
#[cfg(any(feature = "jbd-ble", feature = "jbd-serial"))]
pub use jbd::JbdBattery;

#[cfg(feature = "daly")]
pub mod daly;
#[cfg(feature = "daly")]
pub use daly::DalyBattery;

#[cfg(feature = "victron")]
pub mod victron;
#[cfg(feature = "victron")]
pub use victron::VictronMonitor;

#[cfg(feature = "vedirect")]
pub mod vedirect;
#[cfg(feature = "vedirect")]
pub use vedirect::VedirectMonitor;

#[cfg(feature = "pylontech-can")]
pub mod pylontech;
#[cfg(feature = "pylontech-can")]
pub use pylontech::PylontechState;
#[cfg(feature = "can-socket")]
pub use pylontech::PylontechCan;
