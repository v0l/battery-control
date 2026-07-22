//! Backend adapters. Each is gated behind its own cargo feature so you only
//! compile (and pull the transitive deps of) the devices you actually use.

#[cfg(feature = "anker")]
pub mod anker;
#[cfg(feature = "anker")]
pub use anker::AnkerBattery;

#[cfg(feature = "jk")]
pub mod jk;
#[cfg(feature = "jk")]
pub use jk::JkBattery;

#[cfg(feature = "daly")]
pub mod daly;
#[cfg(feature = "daly")]
pub use daly::DalyBattery;

#[cfg(feature = "victron")]
pub mod victron;
#[cfg(feature = "victron")]
pub use victron::VictronMonitor;

#[cfg(feature = "pylontech-can")]
pub mod pylontech;
#[cfg(feature = "pylontech-can")]
pub use pylontech::PylontechState;
#[cfg(feature = "can-socket")]
pub use pylontech::PylontechCan;
