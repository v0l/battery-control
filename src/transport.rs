//! Ready-to-use [`Transport`](crate::Transport) implementations.
//!
//! These let consumers talk to a JK BMS without writing their own link layer.
//! Each is behind a cargo feature:
//!
//! - [`BluetoothTransport`] — cross-platform BLE (`bluetooth` feature).
//! - [`SerialTransport`] — UART / USB-serial via `tokio-serial` (`serial` feature).
//! - [`CanTransport`] — Linux SocketCAN (`can` feature, Linux only).

#[cfg(feature = "bluetooth")]
mod bluetooth;
#[cfg(feature = "bluetooth")]
pub use bluetooth::{BluetoothTransport, BtDevice, scan};

#[cfg(feature = "serial")]
mod serial;
#[cfg(feature = "serial")]
pub use serial::SerialTransport;

// SocketCAN is Linux-only, so the `can` feature is a no-op on other targets.
#[cfg(all(feature = "can", target_os = "linux"))]
mod can;
#[cfg(all(feature = "can", target_os = "linux"))]
pub use can::CanTransport;
